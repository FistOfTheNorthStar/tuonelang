//! Simple copy propagation: forward a local that is a plain copy of a
//! constant or another local to that local's readers.
//!
//! When a statement is `_x = copy _y` or `_x = const c` (a [`Rvalue::Use`] of
//! a [`Copy`] place or a constant), a later read of `_x` can read the source
//! directly — the value is the same. Forwarding the source turns the copy's
//! destination into a candidate for [dead-local elimination](super::dead_locals),
//! which is where the actual code shrinks; this pass just removes the
//! indirection.
//!
//! # Scope: intra-block, conservatively invalidated
//!
//! This is the *simple* form the prompt asks for, not a global value-numbering
//! pass. Propagation is confined to a single basic block and a running table
//! of "local ⇒ known copy source" that is invalidated the instant anything
//! could make the fact stale:
//!
//! - a write to `_x` (it is no longer that copy),
//! - a write to the source `_y` (the forwarded value would change),
//! - any borrow of `_x` or `_y` (`in`/`mut` args may read or mutate through
//!   the place, so its value is no longer statically known),
//! - a `Call` or `Drop` touching either place, and
//! - the block boundary (a fact never crosses into another block; a value
//!   flowing in from a predecessor is not tracked here).
//!
//! Only `Copy` sources are forwarded: a non-`Copy` place cannot be read twice
//! without a move, so `_x = copy _y` never names one, and constants are always
//! safe. Because the source place is re-read verbatim, a forwarded read has
//! exactly the value the copied read had — the rewrite is meaning-preserving,
//! and re-verification confirms it did not resurrect a moved or uninitialized
//! local.

use std::collections::HashMap;

use tuo_types::TypeckResult;

use super::Pass;
use crate::mir::{Arg, Function, Operand, Rvalue, Statement, Terminator};

/// The simple copy-propagation pass.
pub(super) struct CopyProp;

impl Pass for CopyProp {
    fn name(&self) -> &'static str {
        "copy-prop"
    }

    fn purpose(&self) -> &'static str {
        "Forward a local defined as a plain copy of a constant or another \
         local to that local's later readers within the same block, removing \
         the indirection so the copy can become dead — invalidating the fact \
         on any write, borrow, call, or drop that could make it stale."
    }

    fn preconditions(&self) -> &'static [&'static str] {
        &[
            "verified MIR",
            "copy sources are `Copy` places (a non-Copy value would be moved, \
             not copied), so re-reading the source is always sound",
        ]
    }

    fn run(&self, function: &mut Function, _types: &TypeckResult) -> bool {
        let mut changed = false;
        for block in &mut function.blocks {
            // Known facts: destination local ⇒ the operand it is a copy of.
            let mut known: HashMap<u32, Operand> = HashMap::new();
            for statement in &mut block.statements {
                // First, forward known copies into this statement's *reads*.
                changed |= rewrite_statement_reads(statement, &known);
                // Then update the fact table for this statement's effects.
                update_known(statement, &mut known);
            }
            changed |= rewrite_terminator_reads(&mut block.terminator, &known);
        }
        changed
    }
}

/// Replace `Copy(place)`/`Move(place)` operand reads of a tracked local with
/// its known source, in a statement's read positions (never its destination).
fn rewrite_statement_reads(statement: &mut Statement, known: &HashMap<u32, Operand>) -> bool {
    match statement {
        Statement::Assign { rvalue, .. } => rewrite_rvalue(rvalue, known),
        Statement::Call { args, .. } => {
            let mut changed = false;
            for arg in args {
                if let Arg::Value(operand) = arg {
                    changed |= rewrite_operand(operand, known);
                }
            }
            changed
        }
        // A drop consumes its place; forwarding a copy into it would change
        // *what* is dropped, so leave drops alone.
        Statement::Drop { .. } => false,
    }
}

fn rewrite_terminator_reads(terminator: &mut Terminator, known: &HashMap<u32, Operand>) -> bool {
    match terminator {
        Terminator::Return(operand)
        | Terminator::Branch { cond: operand, .. }
        | Terminator::Assert { cond: operand, .. }
        | Terminator::Switch { discr: operand, .. } => rewrite_operand(operand, known),
        Terminator::Goto(_) | Terminator::Trap(_) => false,
    }
}

fn rewrite_rvalue(rvalue: &mut Rvalue, known: &HashMap<u32, Operand>) -> bool {
    match rvalue {
        Rvalue::Use(operand) | Rvalue::Unary { operand, .. } | Rvalue::Cast { operand, .. } => {
            rewrite_operand(operand, known)
        }
        Rvalue::Binary { lhs, rhs, .. } => {
            let mut changed = rewrite_operand(lhs, known);
            changed |= rewrite_operand(rhs, known);
            changed
        }
        Rvalue::Aggregate { fields, .. } => {
            let mut changed = false;
            for field in fields {
                changed |= rewrite_operand(field, known);
            }
            changed
        }
        // Discriminant/Len read a place, not an operand — a copy source is an
        // operand, so there is nothing to forward here.
        Rvalue::Discriminant(_) | Rvalue::Len(_) => false,
    }
}

/// Forward a tracked copy into one operand. Only whole-local (`unprojected`)
/// `Copy`/`Move` reads are forwarded; a projected read (`copy _x.0`) is left
/// alone (the fact tracks the whole local's value, not a field).
fn rewrite_operand(operand: &mut Operand, known: &HashMap<u32, Operand>) -> bool {
    let (Operand::Copy(place) | Operand::Move(place)) = operand else {
        return false;
    };
    if !place.projection.is_empty() {
        return false;
    }
    let Some(source) = known.get(&place.local.0) else {
        return false;
    };
    // Forward: the operand becomes the source (a Const, or a Copy of the
    // source place). A `Move` of a tracked local can only occur for a `Copy`
    // type in verified MIR (moving a Copy is a verifier error), so replacing
    // it with the source Copy/Const is sound.
    *operand = source.clone();
    true
}

/// Update the known-copy table for a statement's effects: record a new copy
/// fact for a simple `Use` assignment, and invalidate every fact the
/// statement could make stale.
fn update_known(statement: &Statement, known: &mut HashMap<u32, Operand>) {
    // Invalidate first (a self-assignment `_x = copy _x` would otherwise
    // record a fact about a local it just clobbered).
    invalidate(statement, known);

    if let Statement::Assign { place, rvalue } = statement
        && place.projection.is_empty()
        && let Rvalue::Use(source) = rvalue
        && let Some(operand) = copyable_source(source)
    {
        // `_x = copy _y` / `_x = const c`: record `_x ⇒ source`. Skip the
        // degenerate `_x = copy _x`.
        if source_local(&operand) != Some(place.local.0) {
            known.insert(place.local.0, operand);
        }
    }
}

/// A source operand worth tracking: a constant, or a `Copy` of a whole local.
/// A `Move` source is not tracked (the source is de-initialized after, so it
/// cannot be re-read).
fn copyable_source(operand: &Operand) -> Option<Operand> {
    match operand {
        Operand::Const(_) => Some(operand.clone()),
        Operand::Copy(place) if place.projection.is_empty() => Some(operand.clone()),
        Operand::Copy(_) | Operand::Move(_) => None,
    }
}

/// The source local of a tracked operand (for the degenerate-self check and
/// for invalidation), or `None` for a constant.
fn source_local(operand: &Operand) -> Option<u32> {
    match operand {
        Operand::Copy(place) | Operand::Move(place) => Some(place.local.0),
        Operand::Const(_) => None,
    }
}

/// Invalidate any fact a statement could make stale: a write to a local drops
/// facts *about* it and facts whose *source* is it; a borrow or drop of a
/// local does the same (the place may be read or mutated through the borrow,
/// or destroyed).
fn invalidate(statement: &Statement, known: &mut HashMap<u32, Operand>) {
    let mut touched: Vec<u32> = Vec::new();
    match statement {
        Statement::Assign { place, .. } => {
            touched.push(place.local.0);
        }
        Statement::Call { dest, args, .. } => {
            touched.push(dest.local.0);
            for arg in args {
                match arg {
                    Arg::Borrow(place) | Arg::BorrowMut(place) => touched.push(place.local.0),
                    Arg::Value(_) => {}
                }
            }
        }
        Statement::Drop { place } => touched.push(place.local.0),
    }
    for local in touched {
        drop_facts_touching(known, local);
    }
}

/// Remove every fact whose destination or source is `local`.
fn drop_facts_touching(known: &mut HashMap<u32, Operand>, local: u32) {
    known.remove(&local);
    known.retain(|_dest, source| source_local(source) != Some(local));
}

#[cfg(test)]
mod tests {
    use tuo_types::{IntKind, Ty, TypeckResult};

    use super::{CopyProp, Pass};
    use crate::mir::{BasicBlock, Const, LocalId, Operand, Place, Rvalue, Statement, Terminator};
    use crate::opt::tests_support::{func, local};

    fn int(value: i128) -> Operand {
        Operand::Const(Const::Int(value, IntKind::I64))
    }

    #[test]
    fn forwards_a_constant_into_a_later_read() {
        // _1 = const 5; return copy _1  ==>  return const 5 (after forward).
        let mut function = func(
            vec![],
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
        let changed = CopyProp.run(&mut function, &TypeckResult::default());
        assert!(changed);
        match &function.blocks[0].terminator {
            Terminator::Return(Operand::Const(Const::Int(5, _))) => {}
            other => panic!("expected return const 5, got {other:?}"),
        }
    }

    #[test]
    fn forwards_a_copy_into_a_binary_operand() {
        // _2 = copy _0 (a param); _3 = add(copy _2, const 1)
        // ==> _3 = add(copy _0, const 1).
        let mut function = func(
            vec![crate::mir::PassMode::Value],
            vec![local(Ty::int()), local(Ty::int()), local(Ty::int())],
            vec![BasicBlock {
                statements: vec![
                    Statement::Assign {
                        place: Place::local(LocalId(1)),
                        rvalue: Rvalue::Use(Operand::Copy(Place::local(LocalId(0)))),
                    },
                    Statement::Assign {
                        place: Place::local(LocalId(2)),
                        rvalue: Rvalue::Binary {
                            op: crate::mir::BinOp::Add,
                            lhs: Operand::Copy(Place::local(LocalId(1))),
                            rhs: int(1),
                        },
                    },
                ],
                terminator: Terminator::Return(Operand::Copy(Place::local(LocalId(2)))),
            }],
            Ty::int(),
        );
        let changed = CopyProp.run(&mut function, &TypeckResult::default());
        assert!(changed);
        match &function.blocks[0].statements[1] {
            Statement::Assign {
                rvalue: Rvalue::Binary { lhs, .. },
                ..
            } => match lhs {
                Operand::Copy(place) => assert_eq!(place.local.0, 0, "forwarded to the param"),
                other => panic!("expected copy _0, got {other:?}"),
            },
            other => panic!("expected a binary assign, got {other:?}"),
        }
    }

    #[test]
    fn a_reassigned_source_invalidates_the_fact() {
        // _1 = const 5; _1 = const 9; return copy _1 ==> return const 9.
        let mut function = func(
            vec![],
            vec![local(Ty::int()), local(Ty::int())],
            vec![BasicBlock {
                statements: vec![
                    Statement::Assign {
                        place: Place::local(LocalId(1)),
                        rvalue: Rvalue::Use(int(5)),
                    },
                    Statement::Assign {
                        place: Place::local(LocalId(1)),
                        rvalue: Rvalue::Use(int(9)),
                    },
                ],
                terminator: Terminator::Return(Operand::Copy(Place::local(LocalId(1)))),
            }],
            Ty::int(),
        );
        CopyProp.run(&mut function, &TypeckResult::default());
        match &function.blocks[0].terminator {
            Terminator::Return(Operand::Const(Const::Int(9, _))) => {}
            other => panic!("expected return const 9, got {other:?}"),
        }
    }

    #[test]
    fn a_borrow_of_the_source_invalidates_forwarding() {
        // _1 = copy _0; call g(mut _1); return copy _1.
        // The mut borrow may change _1, so the return must NOT be forwarded
        // to _0 — it stays `copy _1`.
        let mut function = func(
            vec![crate::mir::PassMode::Value],
            vec![local(Ty::int()), local(Ty::int()), local(Ty::Unit)],
            vec![BasicBlock {
                statements: vec![
                    Statement::Assign {
                        place: Place::local(LocalId(1)),
                        rvalue: Rvalue::Use(Operand::Copy(Place::local(LocalId(0)))),
                    },
                    Statement::Call {
                        dest: Place::local(LocalId(2)),
                        callee: tuo_resolve::SymbolId::from_raw(1),
                        args: vec![crate::mir::Arg::BorrowMut(Place::local(LocalId(1)))],
                    },
                ],
                terminator: Terminator::Return(Operand::Copy(Place::local(LocalId(1)))),
            }],
            Ty::int(),
        );
        CopyProp.run(&mut function, &TypeckResult::default());
        match &function.blocks[0].terminator {
            Terminator::Return(Operand::Copy(place)) => {
                assert_eq!(place.local.0, 1, "must still read _1, not the stale _0");
            }
            other => panic!("expected return copy _1, got {other:?}"),
        }
    }

    #[test]
    fn facts_do_not_cross_block_boundaries() {
        // bb0: _1 = const 5; goto bb1. bb1: return copy _1.
        // The fact from bb0 must not forward into bb1 (simple = intra-block).
        let mut function = func(
            vec![],
            vec![local(Ty::int()), local(Ty::int())],
            vec![
                BasicBlock {
                    statements: vec![Statement::Assign {
                        place: Place::local(LocalId(1)),
                        rvalue: Rvalue::Use(int(5)),
                    }],
                    terminator: Terminator::Goto(crate::mir::BlockId(1)),
                },
                BasicBlock {
                    statements: vec![],
                    terminator: Terminator::Return(Operand::Copy(Place::local(LocalId(1)))),
                },
            ],
            Ty::int(),
        );
        CopyProp.run(&mut function, &TypeckResult::default());
        match &function.blocks[1].terminator {
            Terminator::Return(Operand::Copy(place)) => assert_eq!(place.local.0, 1),
            other => panic!("expected return copy _1 (not forwarded), got {other:?}"),
        }
    }
}
