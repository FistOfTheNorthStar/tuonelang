//! Unreachable block removal: drop basic blocks no edge reaches.
//!
//! A block that no control-flow edge from the entry can reach is dead: it
//! never executes, so deleting it cannot change what the program does. This
//! pass computes the set of blocks reachable from the entry (`bb0`) by
//! following every terminator edge, drops the rest, and **renumbers** the
//! survivors so the block vector stays dense (a [`BlockId`] is an index into
//! `Function::blocks`), rewriting every terminator target through the
//! renumbering.
//!
//! Constant folding can turn a `Branch`/`Switch` on a now-constant condition
//! into an unconditional edge in a later revision of these passes; even today
//! a program may lower with a block that becomes unreachable once earlier
//! passes run. Removing them shrinks the CFG the backends and the verifier's
//! data-flow pass must walk.
//!
//! This pass is purely structural: it moves no statement and rewrites no
//! rvalue, so a folded constant or a live block is untouched.

use std::collections::BTreeSet;

use tuo_types::TypeckResult;

use super::Pass;
use crate::mir::{BlockId, Function, Terminator};

/// The unreachable-block-removal pass.
pub(super) struct UnreachableBlocks;

impl Pass for UnreachableBlocks {
    fn name(&self) -> &'static str {
        "unreachable-blocks"
    }

    fn purpose(&self) -> &'static str {
        "Delete basic blocks unreachable from the entry block and renumber \
         the survivors densely, rewriting every terminator target — sound \
         because a block no edge reaches never executes."
    }

    fn preconditions(&self) -> &'static [&'static str] {
        &[
            "verified MIR",
            "block 0 is the entry (reachability starts there)",
        ]
    }

    fn run(&self, function: &mut Function, _types: &TypeckResult) -> bool {
        let reachable = reachable_blocks(function);
        if reachable.len() == function.blocks.len() {
            return false;
        }

        // Build old-index → new-index for the survivors, in ascending old
        // order so bb0 stays bb0 and the relative order is preserved.
        let survivors: Vec<usize> = (0..function.blocks.len())
            .filter(|index| reachable.contains(index))
            .collect();
        let mut remap = vec![None; function.blocks.len()];
        for (new_index, &old_index) in survivors.iter().enumerate() {
            remap[old_index] = Some(new_index);
        }

        // Compact the block vector to the survivors, then rewrite targets.
        let mut old_blocks = std::mem::take(&mut function.blocks);
        let mut kept = Vec::with_capacity(survivors.len());
        for old_index in survivors {
            let mut block = std::mem::replace(
                &mut old_blocks[old_index],
                // A placeholder the loop never revisits.
                crate::mir::BasicBlock {
                    statements: Vec::new(),
                    terminator: Terminator::Trap(crate::mir::Trap::Unreachable),
                },
            );
            remap_terminator(&mut block.terminator, &remap);
            kept.push(block);
        }
        function.blocks = kept;
        true
    }
}

/// The set of block indices reachable from the entry by following terminator
/// edges.
fn reachable_blocks(function: &Function) -> BTreeSet<usize> {
    let mut seen = BTreeSet::new();
    let mut stack = vec![0usize];
    while let Some(index) = stack.pop() {
        if !seen.insert(index) {
            continue;
        }
        if let Some(block) = function.blocks.get(index) {
            for target in terminator_targets(&block.terminator) {
                stack.push(target as usize);
            }
        }
    }
    seen
}

/// The block targets a terminator names.
fn terminator_targets(terminator: &Terminator) -> Vec<u32> {
    match terminator {
        Terminator::Return(_) | Terminator::Trap(_) => Vec::new(),
        Terminator::Goto(target) => vec![target.0],
        Terminator::Branch {
            then_block,
            else_block,
            ..
        } => vec![then_block.0, else_block.0],
        Terminator::Switch {
            arms, otherwise, ..
        } => arms
            .iter()
            .map(|(_, target)| target.0)
            .chain(std::iter::once(otherwise.0))
            .collect(),
        Terminator::Assert { target, .. } => vec![target.0],
    }
}

/// Rewrite a terminator's targets through the old→new block remapping. Every
/// target of a reachable block is itself reachable, so each lookup is `Some`.
fn remap_terminator(terminator: &mut Terminator, remap: &[Option<usize>]) {
    let map = |block: &mut BlockId| {
        if let Some(new_index) = remap.get(block.0 as usize).and_then(|entry| *entry) {
            block.0 = u32::try_from(new_index).unwrap_or(block.0);
        }
    };
    match terminator {
        Terminator::Return(_) | Terminator::Trap(_) => {}
        Terminator::Goto(target) => map(target),
        Terminator::Branch {
            then_block,
            else_block,
            ..
        } => {
            map(then_block);
            map(else_block);
        }
        Terminator::Switch {
            arms, otherwise, ..
        } => {
            for (_, target) in arms.iter_mut() {
                map(target);
            }
            map(otherwise);
        }
        Terminator::Assert { target, .. } => map(target),
    }
}

#[cfg(test)]
mod tests {
    use tuo_types::{Ty, TypeckResult};

    use super::{Pass, UnreachableBlocks};
    use crate::mir::{BasicBlock, BlockId, Const, Operand, Terminator, Trap};
    use crate::opt::tests_support::{func, span};

    fn ret(value: i128) -> Terminator {
        Terminator::Return(Operand::Const(Const::Int(value, tuo_types::IntKind::I64)))
    }

    #[test]
    fn removes_a_block_no_edge_reaches() {
        // bb0 returns; bb1 is dead; bb2 is dead. Only bb0 survives.
        let mut function = func(
            vec![],
            vec![],
            vec![
                BasicBlock {
                    statements: vec![],
                    terminator: ret(1),
                },
                BasicBlock {
                    statements: vec![],
                    terminator: ret(2),
                },
                BasicBlock {
                    statements: vec![],
                    terminator: Terminator::Trap(Trap::Unreachable),
                },
            ],
            Ty::int(),
        );
        let changed = UnreachableBlocks.run(&mut function, &TypeckResult::default());
        assert!(changed);
        assert_eq!(function.blocks.len(), 1);
    }

    #[test]
    fn renumbers_targets_of_survivors() {
        // bb0 -> bb2 (skipping dead bb1); after removal bb2 becomes bb1.
        let mut function = func(
            vec![],
            vec![],
            vec![
                BasicBlock {
                    statements: vec![],
                    terminator: Terminator::Goto(BlockId(2)),
                },
                BasicBlock {
                    statements: vec![],
                    terminator: ret(9),
                },
                BasicBlock {
                    statements: vec![],
                    terminator: ret(7),
                },
            ],
            Ty::int(),
        );
        let changed = UnreachableBlocks.run(&mut function, &TypeckResult::default());
        assert!(changed);
        assert_eq!(function.blocks.len(), 2);
        // bb0 now jumps to the renumbered bb1 (the old bb2).
        match &function.blocks[0].terminator {
            Terminator::Goto(target) => assert_eq!(target.0, 1),
            other => panic!("expected a goto, got {other:?}"),
        }
    }

    #[test]
    fn a_fully_reachable_function_is_unchanged() {
        let mut function = func(
            vec![],
            vec![],
            vec![BasicBlock {
                statements: vec![],
                terminator: ret(1),
            }],
            Ty::int(),
        );
        let before = function.blocks.len();
        let changed = UnreachableBlocks.run(&mut function, &TypeckResult::default());
        assert!(!changed);
        assert_eq!(function.blocks.len(), before);
        let _ = span();
    }
}
