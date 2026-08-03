//! The required validation pipeline every corpus candidate must pass.
//!
//! A candidate program is only admitted to a *correct*-programs corpus once it
//! clears the full, ordered gauntlet:
//!
//! ```text
//! format → parse → resolve → type check → ownership → MIR verify →
//!     specs/tests → native execution (where applicable)
//! ```
//!
//! The stages run in that order and short-circuit: the first *failed* required
//! stage stops the pipeline, and every later stage is recorded as
//! [`StageStatus::Skipped`]. This module drives the **real** compiler stages —
//! the canonical formatter ([`tuo_fmt`]), the front-end facade
//! ([`tuo_compiler::check_sources`]), MIR lowering + the mandatory verifier
//! ([`tuo_mir`]), and the reference spec runner ([`tuo_spec`]) — so a program's
//! validation record reflects what the compiler actually does, never a
//! re-implementation.
//!
//! Native execution is the one stage this crate cannot perform alone: producing
//! a runnable binary needs the `cc` link step, which lives in the CLI. It is
//! therefore a **host-injected seam** — a [`NativeExecutor`] the caller supplies
//! (the CLI wires in its linker; tests can wire a backend-only compile check).
//! Mirroring how the agent injects a `Formatter`, this keeps `tuo-corpus` at its
//! dependency layer and free of any concrete backend.

use tuo_compiler::check_sources;
use tuo_diagnostics::{Diagnostic, Namespace, Severity};
use tuo_source::{SourceId, SourceMap};
use tuo_spec::{Limits, RunOutcome, Selection};

use crate::features::{ProgramFacts, extract_facts};
use crate::metadata::{StageStatus, ValidationResults};

/// The entry function a native run starts at (the v0 entry ABI).
pub const ENTRY: &str = "main";

/// A host-provided way to compile and run a program natively.
///
/// The corpus pipeline validates a program's *semantics* through the real
/// front end, MIR verifier, and interpreter; **native execution** — compiling
/// through a concrete backend, linking with the runtime, and running the
/// produced binary — needs machinery (`cc`, a chosen backend) that lives above
/// this crate. A host implements this trait to plug that in.
///
/// Implementations must not panic: any failure to compile, link, or run is
/// reported as [`NativeRun::Failed`]. A program with no runnable entry is
/// [`NativeRun::NotApplicable`].
pub trait NativeExecutor {
    /// Compile the program formed by `sources` (drawn from `map`) to native
    /// code, link it, run it, and report the outcome.
    fn compile_and_run(&self, map: &SourceMap, sources: &[SourceId]) -> NativeRun;
}

/// The outcome of a native-execution attempt.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum NativeRun {
    /// The program compiled, linked, ran, and terminated normally with this
    /// exit status (the integer its entry returned).
    Ran {
        /// The process exit status.
        exit_status: i32,
    },
    /// Native execution does not apply to this program (no runnable entry).
    NotApplicable,
    /// Native execution was attempted and failed (a compile, link, or run
    /// error, or an abnormal termination such as a runtime trap).
    Failed {
        /// A short, human-readable reason.
        reason: String,
    },
}

/// The full result of validating one candidate.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ValidationReport {
    /// The per-stage pass/fail/skip record.
    pub results: ValidationResults,
    /// Every diagnostic the pipeline produced, in stage order.
    pub diagnostics: Vec<Diagnostic>,
    /// The canonical (formatted) text of the first source, if formatting
    /// succeeded — the representation a *correct* corpus entry is stored in.
    pub canonical_source: Option<String>,
    /// The facts extracted from the validated program (features and the
    /// declaration/spec counts feeding the complexity measure). Present once
    /// the front end accepted the program.
    pub facts: Option<ProgramFacts>,
    /// Whether the program has a runnable native entry (a function named
    /// `main`), independent of whether a [`NativeExecutor`] was supplied.
    pub has_native_entry: bool,
}

impl ValidationReport {
    /// Did every required stage that ran pass (nothing failed)?
    #[must_use]
    pub fn is_clean(&self) -> bool {
        !self.results.any_failed()
    }

    /// The highest-numbered diagnostic namespace present among *errors*, which
    /// identifies the stage a failing program failed at. `None` if there are no
    /// error diagnostics.
    #[must_use]
    pub fn first_error_namespace(&self) -> Option<Namespace> {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| d.code.namespace())
            .min()
    }
}

/// How thorough a validation run should be.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Config {
    /// The execution limits for the spec runner's sandbox.
    pub limits: Limits,
}

/// Validate the program formed by `sources` (drawn from `map`) through the full
/// pipeline, optionally attempting native execution via `native`.
///
/// The stages short-circuit at the first required failure; later stages are
/// recorded as [`StageStatus::Skipped`]. Native execution runs only when the
/// semantic stages all passed (validating a broken program natively is
/// meaningless) and a [`NativeExecutor`] was supplied; otherwise it is skipped.
#[must_use]
pub fn validate(
    map: &SourceMap,
    sources: &[SourceId],
    config: Config,
    native: Option<&dyn NativeExecutor>,
) -> ValidationReport {
    let mut results = ValidationResults::pending();
    let mut diagnostics = Vec::new();

    // Stage 1: format. A corpus program has one canonical representation; a file
    // is well-formatted when the formatter leaves it unchanged (`fmt --check`).
    let canonical_source = format_stage(map, sources, &mut results);
    if results.format == StageStatus::Failed {
        return report(results, diagnostics, canonical_source, None, false);
    }

    // Stages 2–5: parse → resolve → type check → ownership, driven by the front
    // end. `check_sources` runs them all; we attribute failures to the right
    // stage by diagnostic namespace so each stage's status is honest.
    let check = check_sources(map, sources);
    diagnostics.extend(check.diagnostics.iter().cloned());
    attribute_front_end(&check.diagnostics, &mut results);
    if check.has_errors() {
        return report(results, diagnostics, canonical_source, None, false);
    }

    let facts = extract_facts(map, sources, &check.resolution);
    let has_native_entry = facts.has_native_entry;

    // Stage 6: MIR verify. Lower the accepted program and run the mandatory
    // verifier; a corpus program whose MIR does not verify is not admissible.
    results.mir_verify = mir_verify_stage(map, sources, &check);
    if results.mir_verify == StageStatus::Failed {
        return report(
            results,
            diagnostics,
            canonical_source,
            Some(facts),
            has_native_entry,
        );
    }

    // Stage 7: specs/tests. Run every colocated spec through the reference
    // interpreter; any failure fails the stage. A program with no specs passes
    // this stage vacuously (there is nothing to falsify).
    results.specs = specs_stage(map, sources, config.limits, &mut diagnostics);
    if results.specs == StageStatus::Failed {
        return report(
            results,
            diagnostics,
            canonical_source,
            Some(facts),
            has_native_entry,
        );
    }

    // Stage 8: native execution where applicable. Only meaningful once the
    // program is semantically valid; needs a host executor and a runnable entry.
    results.native_execution = native_stage(map, sources, has_native_entry, native);

    report(
        results,
        diagnostics,
        canonical_source,
        Some(facts),
        has_native_entry,
    )
}

/// Assemble a [`ValidationReport`].
fn report(
    results: ValidationResults,
    diagnostics: Vec<Diagnostic>,
    canonical_source: Option<String>,
    facts: Option<ProgramFacts>,
    has_native_entry: bool,
) -> ValidationReport {
    ValidationReport {
        results,
        diagnostics,
        canonical_source,
        facts,
        has_native_entry,
    }
}

/// Run the format stage. Returns the canonical text of the first source when
/// formatting is well-defined (the formatter's self-check passed), regardless of
/// whether the file was already canonical.
fn format_stage(
    map: &SourceMap,
    sources: &[SourceId],
    results: &mut ValidationResults,
) -> Option<String> {
    let mut first_canonical = None;
    let mut all_canonical = true;
    let mut all_safe = true;
    for (index, &id) in sources.iter().enumerate() {
        let outcome = tuo_fmt::format_source(map.source(id));
        if !outcome.safe {
            // The formatter could not verify meaning preservation on this input:
            // its output is untrustworthy, so the file is not canonically
            // formattable and fails the stage.
            all_safe = false;
        }
        if outcome.changed {
            all_canonical = false;
        }
        if index == 0 {
            first_canonical = Some(outcome.text);
        }
    }
    results.format = if all_safe && all_canonical {
        StageStatus::Passed
    } else {
        StageStatus::Failed
    };
    first_canonical
}

/// Attribute front-end error diagnostics to the parse / resolve / type-check /
/// ownership stages by namespace, and mark the stages that ran.
fn attribute_front_end(diagnostics: &[Diagnostic], results: &mut ValidationResults) {
    // Which stages produced an error?
    let mut parse_err = false;
    let mut resolve_err = false;
    let mut type_err = false;
    let mut ownership_err = false;
    for d in diagnostics {
        if d.severity != Severity::Error {
            continue;
        }
        match d.code.namespace() {
            Namespace::Lexical | Namespace::Parser => parse_err = true,
            Namespace::Resolution => resolve_err = true,
            Namespace::Type => type_err = true,
            Namespace::Ownership => ownership_err = true,
            // MIR/spec/codegen diagnostics are not produced by the front end.
            Namespace::Mir | Namespace::Specification | Namespace::Codegen => {}
        }
    }

    // The stages run in order and short-circuit at the first failure. A later
    // stage's status is `Skipped` once an earlier one failed, because the
    // compiler does not trust its output on a broken program.
    results.parse = status(parse_err);
    if parse_err {
        return;
    }
    results.resolve = status(resolve_err);
    if resolve_err {
        return;
    }
    results.type_check = status(type_err);
    if type_err {
        return;
    }
    results.ownership = status(ownership_err);
}

/// `Failed` if the flag is set, else `Passed`.
fn status(failed: bool) -> StageStatus {
    if failed {
        StageStatus::Failed
    } else {
        StageStatus::Passed
    }
}

/// Lower the accepted program to MIR and run the mandatory verifier.
fn mir_verify_stage(
    map: &SourceMap,
    sources: &[SourceId],
    check: &tuo_compiler::CheckResult,
) -> StageStatus {
    use tuo_compiler::ast::Ast;
    let parses: Vec<_> = sources
        .iter()
        .map(|&id| tuo_compiler::parser::parse(map.source(id)))
        .collect();
    let asts: Vec<Ast<'_>> = parses
        .iter()
        .zip(sources)
        .map(|(parse, &id)| Ast::new(&parse.tree, map.source(id).text()))
        .collect();
    let hir = tuo_compiler::hir::lower(&asts, &check.resolution);
    let program = tuo_compiler::mir::lower(&hir, &check.resolution, &check.types);
    if tuo_compiler::mir::verify(&program, &check.types).is_empty() {
        StageStatus::Passed
    } else {
        StageStatus::Failed
    }
}

/// Run every colocated spec; fail the stage on any spec failure. A front-end
/// re-check inside the runner should not fire (the program already checked), but
/// if it somehow does, that is a failure. Spec failures are surfaced as
/// specification diagnostics appended to the report.
fn specs_stage(
    map: &SourceMap,
    sources: &[SourceId],
    limits: Limits,
    diagnostics: &mut Vec<Diagnostic>,
) -> StageStatus {
    match tuo_spec::run(map, sources, &Selection::All, limits) {
        RunOutcome::NotChecked(mut problems) => {
            // The program checked a moment ago; a NotChecked here is unexpected.
            diagnostics.append(&mut problems);
            StageStatus::Failed
        }
        RunOutcome::Ran(report) => {
            if report.passed() {
                StageStatus::Passed
            } else {
                StageStatus::Failed
            }
        }
    }
}

/// Attempt native execution when it applies and an executor was supplied.
fn native_stage(
    map: &SourceMap,
    sources: &[SourceId],
    has_native_entry: bool,
    native: Option<&dyn NativeExecutor>,
) -> StageStatus {
    if !has_native_entry {
        // No runnable entry: native execution does not apply to this program.
        return StageStatus::Skipped;
    }
    let Some(executor) = native else {
        // No host executor was supplied; we cannot honestly claim native
        // execution passed, so it is skipped rather than assumed.
        return StageStatus::Skipped;
    };
    match executor.compile_and_run(map, sources) {
        NativeRun::Ran { .. } => StageStatus::Passed,
        NativeRun::NotApplicable => StageStatus::Skipped,
        NativeRun::Failed { .. } => StageStatus::Failed,
    }
}
