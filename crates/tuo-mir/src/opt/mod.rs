//! The tuonelang MIR optimization framework.
//!
//! MIR is the single executable semantic representation, so an optimization
//! is only ever a **meaning-preserving rewrite** of one [`Program`] into
//! another: the interpreter's result on the original MIR is the reference,
//! and an optimized program that computes anything else is a bug, not a
//! faster program. Every pass here is deliberately conservative — it rewrites
//! only where it can prove the observable behavior (return value and the
//! *which*/*whether* of a deterministic trap) is unchanged, and declines
//! otherwise.
//!
//! # A pass is an isolated, self-describing rewrite
//!
//! Each pass is one [`Pass`] with:
//!
//! - a stable [`name`](Pass::name) (for measurement and debug output),
//! - a declared [`purpose`](Pass::purpose) — what it rewrites and why it is
//!   sound,
//! - declared [`preconditions`](Pass::preconditions) — the MIR properties it
//!   assumes (always at least "verified MIR", which the driver guarantees),
//! - a [`run`](Pass::run) that mutates a [`Function`] in place and reports
//!   whether it changed anything.
//!
//! Passes do not call each other and hold no shared state: the driver owns
//! sequencing. This keeps each pass independently testable (its own
//! before/after golden) and independently sound.
//!
//! # The driver guarantees verified MIR in and out
//!
//! [`optimize`] runs the pass pipeline over every function of a program that
//! has **already been verified** (the caller lowered it and ran the mandatory
//! [`crate::verify`] gate). After *each* pass it calls
//! [`crate::debug_assert_verified`], so a pass that corrupts the MIR is caught
//! immediately in debug/test builds — turning "a pass broke the IR" from a
//! silent miscompile into a loud, attributable panic naming the pass. The
//! output is therefore verified MIR a backend may consume directly.
//!
//! Optimization is **not** part of the reference semantics: the interpreter
//! runs *unoptimized* MIR, and the differential suites pin that the optimized
//! program a backend compiles still agrees with it. Optimization changes only
//! how fast the program is and how much code it takes, never what it does.
//!
//! # The v0 pass set
//!
//! Four small, well-understood passes, each owning one file:
//!
//! - [`const_fold`] — fold constant-operand arithmetic, comparisons, unary
//!   ops, and casts to a single constant, *except* where the operation would
//!   trap (folding away a trap would change behavior).
//! - [`copy_prop`] — forward a local that is a simple copy of another operand
//!   to that operand's readers, so the copy's storage can later be removed.
//! - [`unreachable_blocks`] — drop basic blocks no control-flow edge reaches.
//! - [`dead_locals`] — remove locals (and the statements that only write
//!   them) whose value is never read.
//!
//! Nothing speculative or globally interprocedural lives here yet: no
//! inlining, no loop transforms, no alias-based reasoning. The release
//! backend's own LLVM pipeline still performs downstream optimization on top
//! of whatever this produces; these passes exist to shrink the MIR the
//! backends start from and to demonstrate the framework, not to compete with
//! LLVM.

mod const_fold;
mod copy_prop;
mod dead_locals;
mod unreachable_blocks;

use tuo_types::TypeckResult;

use crate::mir::{Function, Program};
use crate::verify::debug_assert_verified;

/// One isolated, meaning-preserving MIR rewrite.
///
/// A pass is stateless: the driver constructs it once and runs it over each
/// function. Its [`run`](Pass::run) mutates the function in place and returns
/// whether it changed anything (so the driver can report and, in principle,
/// iterate to a fixed point).
pub trait Pass {
    /// The pass's stable identifier, used in measurement and debug output.
    fn name(&self) -> &'static str;

    /// What the pass rewrites and the argument for why the rewrite preserves
    /// the program's observable behavior.
    fn purpose(&self) -> &'static str;

    /// The MIR properties the pass assumes about its input. Every pass may
    /// assume verified MIR (the driver guarantees it); a pass lists here only
    /// what it additionally relies on.
    fn preconditions(&self) -> &'static [&'static str];

    /// Rewrite `function` in place. `types` is the program's type-check
    /// result, for the passes that consult it (e.g. `Copy`-ness). Returns
    /// `true` iff the function was modified.
    fn run(&self, function: &mut Function, types: &TypeckResult) -> bool;
}

/// The v0 optimization pipeline, in run order.
///
/// The order is deliberate: constant folding first exposes constants that
/// copy propagation can forward and dead blocks that become unreachable;
/// copy propagation then removes indirection so dead-local elimination can
/// see the now-unread locals; unreachable-block removal prunes the CFG; and
/// dead-local elimination runs last to sweep up locals nothing reads anymore.
#[must_use]
fn pipeline() -> Vec<Box<dyn Pass>> {
    vec![
        Box::new(const_fold::ConstFold),
        Box::new(copy_prop::CopyProp),
        Box::new(unreachable_blocks::UnreachableBlocks),
        Box::new(dead_locals::DeadLocals),
    ]
}

/// A per-pass record of how much a pass changed, for measurement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PassReport {
    /// The pass's [`Pass::name`].
    pub name: &'static str,
    /// Whether the pass modified any function of the program.
    pub changed: bool,
}

/// The measurement summary of one [`optimize`] run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OptReport {
    /// One entry per pass, in run order.
    pub passes: Vec<PassReport>,
}

impl OptReport {
    /// Whether any pass changed the program.
    #[must_use]
    pub fn changed(&self) -> bool {
        self.passes.iter().any(|pass| pass.changed)
    }
}

/// The maximum number of times the pipeline is re-run to a fixed point.
///
/// The pipeline is re-run while any pass reports a change, because one pass
/// exposes work for another (const-fold produces the constants copy-prop
/// forwards; copy-prop makes locals dead-locals reclaims; a folded branch can
/// make blocks unreachable). Each pass is monotone — it only removes work or
/// replaces a computation with a constant — so the process terminates; this
/// cap is a defensive backstop against a bug that oscillates, never a limit a
/// correct pipeline reaches.
const MAX_ROUNDS: usize = 8;

/// Run the v0 optimization pipeline over `program` in place to a fixed point,
/// returning a per-pass [`OptReport`] accumulated across rounds.
///
/// # Preconditions
///
/// `program` must already be **verified** (`crate::verify` returned no
/// diagnostics). The driver re-verifies after every pass in debug/test builds
/// via [`crate::debug_assert_verified`]; the returned program is verified MIR
/// a backend may consume.
///
/// # Panics
///
/// In debug/test builds, panics (naming the pass) if a pass produces MIR that
/// does not verify — that is always a compiler bug, never user-facing.
pub fn optimize(program: &mut Program, types: &TypeckResult) -> OptReport {
    let passes = pipeline();
    // Accumulate one report row per pass: `changed` is true if the pass
    // changed the program in *any* round.
    let mut totals: Vec<PassReport> = passes
        .iter()
        .map(|pass| PassReport {
            name: pass.name(),
            changed: false,
        })
        .collect();

    for _round in 0..MAX_ROUNDS {
        let mut round_changed = false;
        for (index, pass) in passes.iter().enumerate() {
            let mut changed = false;
            for function in &mut program.functions {
                changed |= pass.run(function, types);
            }
            // A pass must hand back verifiable MIR; catch a corrupting pass at
            // the seam, attributed by name, before any later pass or backend
            // sees it.
            debug_assert_verified(pass.name(), program, types);
            totals[index].changed |= changed;
            round_changed |= changed;
        }
        if !round_changed {
            break;
        }
    }

    OptReport { passes: totals }
}

/// The declared purpose and preconditions of every pass in the pipeline, for
/// documentation and the framework self-check test. The order matches
/// [`pipeline`].
#[must_use]
pub fn pass_descriptions() -> Vec<(&'static str, &'static str, &'static [&'static str])> {
    pipeline()
        .iter()
        .map(|pass| (pass.name(), pass.purpose(), pass.preconditions()))
        .collect()
}

/// Shared builders for the per-pass unit tests: a minimal function skeleton
/// the individual pass tests populate and corrupt. Test-only.
#[cfg(test)]
pub(super) mod tests_support {
    use tuo_resolve::SymbolId;
    use tuo_source::{SourceId, Span, TextRange};
    use tuo_types::Ty;

    use crate::mir::{BasicBlock, Function, LocalDecl, PassMode};

    /// A throwaway one-byte span for synthesized MIR.
    #[must_use]
    pub(crate) fn span() -> Span {
        Span::new(
            SourceId::from_raw(0),
            TextRange::new(0u32, 1u32).expect("forward range"),
        )
    }

    /// A local of the given type with no name.
    #[must_use]
    pub(crate) fn local(ty: Ty) -> LocalDecl {
        LocalDecl {
            ty,
            name: None,
            span: span(),
        }
    }

    /// A single-function program body from parts (symbol 0, name `f`).
    #[must_use]
    pub(crate) fn func(
        params: Vec<PassMode>,
        locals: Vec<LocalDecl>,
        blocks: Vec<BasicBlock>,
        ret: Ty,
    ) -> Function {
        Function {
            symbol: SymbolId::from_raw(0),
            name: "f".to_owned(),
            params,
            locals,
            blocks,
            ret,
            span: span(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{optimize, pass_descriptions, pipeline};
    use crate::mir::Program;
    use tuo_types::TypeckResult;

    #[test]
    fn every_pass_declares_a_purpose_and_preconditions() {
        for (name, purpose, preconditions) in pass_descriptions() {
            assert!(!name.is_empty(), "a pass has an empty name");
            assert!(
                purpose.len() > 20,
                "pass `{name}` has a too-terse purpose: {purpose:?}"
            );
            // "verified MIR" is implicit; each pass still lists at least it, so
            // the precondition contract is never silently empty.
            assert!(
                !preconditions.is_empty(),
                "pass `{name}` declares no preconditions"
            );
        }
    }

    #[test]
    fn pass_names_are_unique() {
        let names: Vec<&str> = pipeline().iter().map(|pass| pass.name()).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "duplicate pass name: {names:?}");
    }

    #[test]
    fn optimizing_an_empty_program_is_a_clean_no_op() {
        let mut program = Program::default();
        let report = optimize(&mut program, &TypeckResult::default());
        assert!(!report.changed());
        // Every pass still ran and reported.
        assert_eq!(report.passes.len(), pipeline().len());
    }
}
