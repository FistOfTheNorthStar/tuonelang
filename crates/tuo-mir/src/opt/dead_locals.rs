//! Dead local elimination: remove locals whose value is never read.
//!
//! A local that is written but never read contributes nothing to the
//! program's result. This pass removes such locals in two steps, each
//! meaning-preserving:
//!
//! 1. **Dead store removal.** An `Assign` whose destination local is read
//!    nowhere in the function, and whose rvalue **cannot trap or otherwise
//!    have an observable effect**, is deleted — its value was going nowhere.
//!    A trapping rvalue (integer arithmetic that could overflow, a division,
//!    a `MIN` negation) is *kept* even when its result is unread, because the
//!    trap itself is observable. A `Call` is never removed: it may diverge,
//!    trap, or mutate a `mut`-borrowed argument, all observable regardless of
//!    whether its return value is used.
//! 2. **Dead local removal.** After dead stores are gone, any local that is
//!    now referenced nowhere — not by a parameter slot, not by any place,
//!    operand, argument, or index — is deleted and the remaining locals are
//!    renumbered densely (a [`LocalId`] is an index into `Function::locals`).
//!    Parameters (locals `0..params.len()`) are always retained: the calling
//!    convention fixes their slots even if the body ignores them.
//!
//! The two steps compose: removing a dead store can make its destination
//! local unreferenced, which step 2 then reclaims. Copy propagation
//! (running earlier) is what usually makes a copy's source or destination
//! dead in the first place.

use std::collections::BTreeSet;

use tuo_types::TypeckResult;

use super::Pass;
use crate::mir::{Arg, Function, LocalId, Operand, Place, Rvalue, Statement, Terminator};

/// The dead-local-elimination pass.
pub(super) struct DeadLocals;

impl Pass for DeadLocals {
    fn name(&self) -> &'static str {
        "dead-locals"
    }

    fn purpose(&self) -> &'static str {
        "Delete side-effect-free assignments whose destination local is never \
         read, then remove and densely renumber locals that end up referenced \
         nowhere — keeping parameters and any trapping or calling statement, \
         whose effects are observable regardless of use."
    }

    fn preconditions(&self) -> &'static [&'static str] {
        &[
            "verified MIR",
            "locals 0..params.len() are the parameters and are never removed",
        ]
    }

    fn run(&self, function: &mut Function, _types: &TypeckResult) -> bool {
        let mut changed = remove_dead_stores(function);
        changed |= remove_unreferenced_locals(function);
        changed
    }
}

/// Delete `Assign` statements whose destination is a whole (unprojected)
/// local that is read nowhere and whose rvalue cannot trap. Returns whether
/// anything was removed.
fn remove_dead_stores(function: &mut Function) -> bool {
    let read = read_locals(function);
    let mut changed = false;
    for block in &mut function.blocks {
        let before = block.statements.len();
        block.statements.retain(|statement| match statement {
            Statement::Assign { place, rvalue } => {
                let dead = place.projection.is_empty()
                    && !read.contains(&place.local.0)
                    && !rvalue_can_trap(rvalue);
                !dead
            }
            // Calls and drops have effects beyond their destination.
            Statement::Call { .. } | Statement::Drop { .. } => true,
        });
        changed |= block.statements.len() != before;
    }
    changed
}

/// Remove locals referenced nowhere (after dead stores are gone) and renumber
/// the survivors densely. Parameters are always kept. Returns whether the
/// local set changed.
fn remove_unreferenced_locals(function: &mut Function) -> bool {
    let referenced = referenced_locals(function);
    let param_count = u32::try_from(function.params.len()).unwrap_or(u32::MAX);

    // A local survives if it is a parameter or is referenced somewhere.
    let survives = |index: u32| index < param_count || referenced.contains(&index);
    let local_count = u32::try_from(function.locals.len()).unwrap_or(u32::MAX);
    if (0..local_count).all(survives) {
        return false;
    }

    // Build old→new local numbering over the survivors, in ascending order so
    // parameter slots keep their indices.
    let survivors: Vec<u32> = (0..local_count).filter(|&index| survives(index)).collect();
    let mut remap = vec![None; function.locals.len()];
    for (new_index, &old_index) in survivors.iter().enumerate() {
        remap[old_index as usize] = u32::try_from(new_index).ok();
    }

    // Compact the local declarations.
    let mut old_locals = std::mem::take(&mut function.locals);
    let mut kept = Vec::with_capacity(survivors.len());
    for &old_index in &survivors {
        kept.push(std::mem::replace(
            &mut old_locals[old_index as usize],
            // Placeholder never revisited.
            crate::mir::LocalDecl {
                ty: tuo_types::Ty::Unit,
                name: None,
                span: function.span,
            },
        ));
    }
    function.locals = kept;

    // Rewrite every LocalId in the body through the remapping.
    for block in &mut function.blocks {
        for statement in &mut block.statements {
            remap_statement(statement, &remap);
        }
        remap_terminator(&mut block.terminator, &remap);
    }
    true
}

/// Whether an rvalue can trap or otherwise have an effect that must survive
/// even when its result is unread. Only integer arithmetic that overflows,
/// division/remainder, and `MIN` negation can trap; everything else is a pure
/// value computation.
fn rvalue_can_trap(rvalue: &Rvalue) -> bool {
    use crate::mir::{BinOp, UnOp};
    match rvalue {
        Rvalue::Binary { op, .. } => matches!(
            op,
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem
        ),
        // Integer `Neg` traps on the minimum; bool `Not` cannot. Without the
        // operand type here we conservatively treat `Neg` as trapping.
        Rvalue::Unary { op, .. } => matches!(op, UnOp::Neg),
        // Casts never trap; `Use`/`Aggregate`/`Discriminant`/`Len` are pure.
        Rvalue::Use(_)
        | Rvalue::Cast { .. }
        | Rvalue::Aggregate { .. }
        | Rvalue::Discriminant(_)
        | Rvalue::Len(_) => false,
    }
}

/// The set of locals *read* anywhere in the function: as the source of a
/// `Copy`/`Move`, a borrow argument, an index, a discriminant/len place, a
/// drop, or a terminator operand. A projected write (`place.field = ...`)
/// also *reads* its root (the root must exist to project into it).
fn read_locals(function: &Function) -> BTreeSet<u32> {
    let mut read = BTreeSet::new();
    let mut note = |local: LocalId| {
        read.insert(local.0);
    };
    for block in &function.blocks {
        for statement in &block.statements {
            match statement {
                Statement::Assign { place, rvalue } => {
                    read_place_roots(place, /*is_write=*/ true, &mut note);
                    read_rvalue(rvalue, &mut note);
                }
                Statement::Call { dest, args, .. } => {
                    read_place_roots(dest, /*is_write=*/ true, &mut note);
                    for arg in args {
                        read_arg(arg, &mut note);
                    }
                }
                Statement::Drop { place } => read_place_roots(place, false, &mut note),
            }
        }
        read_terminator(&block.terminator, &mut note);
    }
    read
}

/// Every local *referenced* anywhere (read or written, including as a
/// destination root and as an index/projection local). Used to decide which
/// local declarations can be dropped entirely.
fn referenced_locals(function: &Function) -> BTreeSet<u32> {
    let mut referenced = read_locals(function);
    // Also count destinations and borrow-mut roots, which `read_locals` marks
    // via `is_write` for projected places but not for whole-local writes.
    for block in &function.blocks {
        for statement in &block.statements {
            match statement {
                Statement::Assign { place, .. } => {
                    referenced.insert(place.local.0);
                }
                Statement::Call { dest, .. } => {
                    referenced.insert(dest.local.0);
                }
                Statement::Drop { .. } => {}
            }
        }
    }
    referenced
}

fn read_rvalue(rvalue: &Rvalue, note: &mut impl FnMut(LocalId)) {
    match rvalue {
        Rvalue::Use(operand) | Rvalue::Unary { operand, .. } | Rvalue::Cast { operand, .. } => {
            read_operand(operand, note);
        }
        Rvalue::Binary { lhs, rhs, .. } => {
            read_operand(lhs, note);
            read_operand(rhs, note);
        }
        Rvalue::Aggregate { fields, .. } => {
            for field in fields {
                read_operand(field, note);
            }
        }
        Rvalue::Discriminant(place) | Rvalue::Len(place) => read_place_roots(place, false, note),
    }
}

fn read_operand(operand: &Operand, note: &mut impl FnMut(LocalId)) {
    match operand {
        Operand::Copy(place) | Operand::Move(place) => read_place_roots(place, false, note),
        Operand::Const(_) => {}
    }
}

fn read_arg(arg: &Arg, note: &mut impl FnMut(LocalId)) {
    match arg {
        Arg::Value(operand) => read_operand(operand, note),
        Arg::Borrow(place) | Arg::BorrowMut(place) => read_place_roots(place, false, note),
    }
}

/// Note the roots a place reads: its index locals always, and its root local
/// unless this is a whole-local *write* (a bare destination is defined, not
/// read). A projected place reads its root either way.
fn read_place_roots(place: &Place, is_write: bool, note: &mut impl FnMut(LocalId)) {
    if !is_write || !place.projection.is_empty() {
        note(place.local);
    }
    for projection in &place.projection {
        if let crate::mir::Projection::Index(index) = projection {
            note(*index);
        }
    }
}

fn read_terminator(terminator: &Terminator, note: &mut impl FnMut(LocalId)) {
    match terminator {
        Terminator::Return(operand)
        | Terminator::Branch { cond: operand, .. }
        | Terminator::Assert { cond: operand, .. }
        | Terminator::Switch { discr: operand, .. } => read_operand(operand, note),
        Terminator::Goto(_) | Terminator::Trap(_) => {}
    }
}

// --- local renumbering ------------------------------------------------------

fn remap_local(local: &mut LocalId, remap: &[Option<u32>]) {
    if let Some(new_index) = remap.get(local.0 as usize).and_then(|entry| *entry) {
        local.0 = new_index;
    }
}

fn remap_place(place: &mut Place, remap: &[Option<u32>]) {
    remap_local(&mut place.local, remap);
    for projection in &mut place.projection {
        if let crate::mir::Projection::Index(index) = projection {
            remap_local(index, remap);
        }
    }
}

fn remap_operand(operand: &mut Operand, remap: &[Option<u32>]) {
    match operand {
        Operand::Copy(place) | Operand::Move(place) => remap_place(place, remap),
        Operand::Const(_) => {}
    }
}

fn remap_rvalue(rvalue: &mut Rvalue, remap: &[Option<u32>]) {
    match rvalue {
        Rvalue::Use(operand) | Rvalue::Unary { operand, .. } | Rvalue::Cast { operand, .. } => {
            remap_operand(operand, remap);
        }
        Rvalue::Binary { lhs, rhs, .. } => {
            remap_operand(lhs, remap);
            remap_operand(rhs, remap);
        }
        Rvalue::Aggregate { fields, .. } => {
            for field in fields {
                remap_operand(field, remap);
            }
        }
        Rvalue::Discriminant(place) | Rvalue::Len(place) => remap_place(place, remap),
    }
}

fn remap_statement(statement: &mut Statement, remap: &[Option<u32>]) {
    match statement {
        Statement::Assign { place, rvalue } => {
            remap_place(place, remap);
            remap_rvalue(rvalue, remap);
        }
        Statement::Call { dest, args, .. } => {
            remap_place(dest, remap);
            for arg in args {
                match arg {
                    Arg::Value(operand) => remap_operand(operand, remap),
                    Arg::Borrow(place) | Arg::BorrowMut(place) => remap_place(place, remap),
                }
            }
        }
        Statement::Drop { place } => remap_place(place, remap),
    }
}

fn remap_terminator(terminator: &mut Terminator, remap: &[Option<u32>]) {
    match terminator {
        Terminator::Return(operand)
        | Terminator::Branch { cond: operand, .. }
        | Terminator::Assert { cond: operand, .. }
        | Terminator::Switch { discr: operand, .. } => remap_operand(operand, remap),
        Terminator::Goto(_) | Terminator::Trap(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use tuo_types::{IntKind, Ty, TypeckResult};

    use super::{DeadLocals, Pass};
    use crate::mir::LocalId;
    use crate::mir::{BasicBlock, BinOp, Const, Operand, Place, Rvalue, Statement, Terminator};
    use crate::opt::tests_support::{func, local};

    fn int(value: i128) -> Operand {
        Operand::Const(Const::Int(value, IntKind::I64))
    }

    // Fixtures use `_0` as a value parameter so it is a retained slot; this
    // keeps the local counts a test asserts about exactly the locals under
    // test (a stray unreferenced local would itself be reclaimed).

    #[test]
    fn removes_a_dead_pure_store_and_its_local() {
        // param _0; _1 = const 1 (never read); return const 0. _1 is dead.
        let mut function = func(
            vec![crate::mir::PassMode::Value],
            vec![local(Ty::int()), local(Ty::int())],
            vec![BasicBlock {
                statements: vec![Statement::Assign {
                    place: Place::local(LocalId(1)),
                    rvalue: Rvalue::Use(int(1)),
                }],
                terminator: Terminator::Return(int(0)),
            }],
            Ty::int(),
        );
        let changed = DeadLocals.run(&mut function, &TypeckResult::default());
        assert!(changed);
        assert!(
            function.blocks[0].statements.is_empty(),
            "dead store removed"
        );
        // Only the parameter slot _0 remains; the dead local _1 is gone.
        assert_eq!(function.locals.len(), 1, "dead local _1 removed");
    }

    #[test]
    fn keeps_a_dead_but_trapping_store() {
        // param _0; _1 = add(const 1, const 2) — result unread, but the add
        // can overflow and trap, so the store must not be removed. _1 is then
        // referenced only by that surviving store, so it is retained too.
        let mut function = func(
            vec![crate::mir::PassMode::Value],
            vec![local(Ty::int()), local(Ty::int())],
            vec![BasicBlock {
                statements: vec![Statement::Assign {
                    place: Place::local(LocalId(1)),
                    rvalue: Rvalue::Binary {
                        op: BinOp::Add,
                        lhs: int(1),
                        rhs: int(2),
                    },
                }],
                terminator: Terminator::Return(int(0)),
            }],
            Ty::int(),
        );
        let changed = DeadLocals.run(&mut function, &TypeckResult::default());
        assert!(!changed, "a trapping store must be preserved");
        assert_eq!(function.blocks[0].statements.len(), 1);
        assert_eq!(function.locals.len(), 2);
    }

    #[test]
    fn keeps_a_read_local() {
        // param _0; _1 = const 5; return copy _1. _1 is read, so it stays.
        let mut function = func(
            vec![crate::mir::PassMode::Value],
            vec![local(Ty::int()), local(Ty::int())],
            vec![BasicBlock {
                statements: vec![Statement::Assign {
                    place: Place::local(LocalId(1)),
                    rvalue: Rvalue::Use(int(5)),
                }],
                terminator: Terminator::Return(Operand::Copy(Place::local(LocalId(1)))),
            }],
            Ty::int(),
        );
        let changed = DeadLocals.run(&mut function, &TypeckResult::default());
        assert!(!changed);
        assert_eq!(function.locals.len(), 2);
    }

    #[test]
    fn renumbers_reads_after_removing_an_earlier_local() {
        // param _0; _1 = const 1 (dead); _2 = const 7; return copy _2.
        // After removal _2 becomes _1 (the param slot _0 keeps its index).
        let mut function = func(
            vec![crate::mir::PassMode::Value],
            vec![local(Ty::int()), local(Ty::int()), local(Ty::int())],
            vec![BasicBlock {
                statements: vec![
                    Statement::Assign {
                        place: Place::local(LocalId(1)),
                        rvalue: Rvalue::Use(int(1)),
                    },
                    Statement::Assign {
                        place: Place::local(LocalId(2)),
                        rvalue: Rvalue::Use(int(7)),
                    },
                ],
                terminator: Terminator::Return(Operand::Copy(Place::local(LocalId(2)))),
            }],
            Ty::int(),
        );
        let changed = DeadLocals.run(&mut function, &TypeckResult::default());
        assert!(changed);
        // _0 (param) and the renumbered old-_2 remain; dead _1 removed.
        assert_eq!(function.locals.len(), 2, "one dead local removed");
        // The surviving store now writes _1, and the return reads _1.
        match &function.blocks[0].terminator {
            Terminator::Return(Operand::Copy(place)) => assert_eq!(place.local.0, 1),
            other => panic!("expected return copy _1, got {other:?}"),
        }
    }

    #[test]
    fn never_removes_a_parameter_even_if_unused() {
        // One param, unused; body returns a constant. The param slot stays.
        let mut function = func(
            vec![crate::mir::PassMode::Value],
            vec![local(Ty::int())],
            vec![BasicBlock {
                statements: vec![],
                terminator: Terminator::Return(int(0)),
            }],
            Ty::int(),
        );
        let changed = DeadLocals.run(&mut function, &TypeckResult::default());
        assert!(!changed);
        assert_eq!(function.locals.len(), 1, "the parameter slot is retained");
    }
}
