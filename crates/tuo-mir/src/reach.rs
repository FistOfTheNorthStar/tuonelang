//! Whole-program reachability: drop the functions an entry point can never
//! call before a backend sees the program.
//!
//! A native build lowers every function it is handed, so a function the
//! program never calls can still refuse the build — the standard library's
//! `std::collections::pair` returns a generic struct the backends do not lay
//! out yet, and a program that only calls `std::collections::sum` would be
//! refused for it. The interpreter has no such problem: it executes only what
//! is called. Pruning to the entry's call graph makes the native path agree.
//!
//! A function is reachable when a root reaches it through a direct call or by
//! naming it as a function value ([`Const::Fn`] — the value may be called
//! indirectly anywhere it flows, so naming it is enough). Removing an
//! unreachable function changes nothing observable: no execution can enter it.
//!
//! The roots are the entry *and* every function the host says to keep. The
//! CLI keeps everything the user wrote, so a user's own function outside the
//! native subset is still refused whether or not `main` calls it (the
//! long-standing contract, pinned by the refusal suites), and only the
//! library's unused functions are dropped.

use std::collections::HashSet;

use tuo_resolve::SymbolId;

use crate::mir::{
    Arg, BasicBlock, Callee, Const, Function, Operand, Program, Rvalue, Statement, Terminator,
};

/// Keep only the functions reachable from the first function named `entry` or
/// from any function `keep` accepts, returning how many were removed. With no
/// entry the program is left untouched, so the backend still reports its own
/// missing-entry error.
pub fn retain_reachable(
    program: &mut Program,
    entry: &str,
    keep: impl Fn(&Function) -> bool,
) -> usize {
    let Some(root) = program.functions.iter().find(|f| f.name == entry) else {
        return 0;
    };
    let mut reachable: HashSet<SymbolId> = HashSet::new();
    let mut work = vec![root.symbol];
    work.extend(
        program
            .functions
            .iter()
            .filter(|function| keep(function))
            .map(|function| function.symbol),
    );
    while let Some(symbol) = work.pop() {
        if !reachable.insert(symbol) {
            continue;
        }
        for function in program.functions.iter().filter(|f| f.symbol == symbol) {
            for block in &function.blocks {
                referenced_in(block, &mut work);
            }
        }
    }
    let before = program.functions.len();
    program
        .functions
        .retain(|function| reachable.contains(&function.symbol));
    before - program.functions.len()
}

/// Push every function symbol `block` calls directly or names as a value.
fn referenced_in(block: &BasicBlock, out: &mut Vec<SymbolId>) {
    for statement in &block.statements {
        match statement {
            Statement::Assign { rvalue, .. } => rvalue_refs(rvalue, out),
            Statement::Call { callee, args, .. } => {
                match callee {
                    Callee::Direct(symbol) => out.push(*symbol),
                    Callee::Indirect(operand) => operand_ref(operand, out),
                }
                args_refs(args, out);
            }
            Statement::Effect { args, .. } => args_refs(args, out),
            Statement::HeapMutate { args, .. } => {
                for operand in args {
                    operand_ref(operand, out);
                }
            }
            Statement::Drop { .. } => {}
        }
    }
    match &block.terminator {
        Terminator::Return(operand)
        | Terminator::Branch { cond: operand, .. }
        | Terminator::Switch { discr: operand, .. }
        | Terminator::Assert { cond: operand, .. } => operand_ref(operand, out),
        Terminator::Goto(_) | Terminator::Trap(_) => {}
    }
}

fn rvalue_refs(rvalue: &Rvalue, out: &mut Vec<SymbolId>) {
    match rvalue {
        Rvalue::Use(operand) | Rvalue::Unary { operand, .. } | Rvalue::Cast { operand, .. } => {
            operand_ref(operand, out)
        }
        Rvalue::Binary { lhs, rhs, .. } => {
            operand_ref(lhs, out);
            operand_ref(rhs, out);
        }
        Rvalue::Aggregate {
            fields: operands, ..
        }
        | Rvalue::StrOp { args: operands, .. }
        | Rvalue::HeapOp { args: operands, .. } => {
            for operand in operands {
                operand_ref(operand, out);
            }
        }
        Rvalue::Discriminant(_) | Rvalue::Len(_) => {}
    }
}

fn args_refs(args: &[Arg], out: &mut Vec<SymbolId>) {
    for arg in args {
        if let Arg::Value(operand) = arg {
            operand_ref(operand, out);
        }
    }
}

fn operand_ref(operand: &Operand, out: &mut Vec<SymbolId>) {
    if let Operand::Const(Const::Fn(symbol)) = operand {
        out.push(*symbol);
    }
}
