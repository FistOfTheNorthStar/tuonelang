//! The spec runner: front end → MIR spec lowering → sandboxed execution.
//!
//! [`run`] drives one program snapshot end to end. It runs the front-end
//! stages directly (parse → resolve → type-check → ownership-check, the same
//! sequence the `tuo check` facade runs); refuses to execute anything if the
//! program has front-end errors (a broken spec does not run — ADR-0002);
//! lowers every spec to verified MIR ([`tuo_mir::lower_specs`]); builds one
//! [`Interpreter`](tuo_mir_interp::Interpreter) over the whole program (whose
//! constructor enforces the mandatory verification gate); and executes each
//! selected spec's assertions under the configured [`Limits`].
//!
//! Every assertion runs in the interpreter's deterministic sandbox with
//! instruction fuel, a recursion-depth ceiling, and a memory (live-value)
//! budget — no host filesystem, network, clock, or randomness is reachable,
//! because MIR has no instruction that could reach them. Execution is timed
//! (per spec) so callers can report actual latency; the runner promises no
//! particular speed.

use std::time::Instant;

use tuo_ast::Ast;
use tuo_diagnostics::{Diagnostic, Severity};
use tuo_mir::{SpecAssertion, SpecMir};
use tuo_mir_interp::{Interpreter, RuntimeError};
use tuo_resolve::Resolution;
use tuo_source::{SourceId, SourceMap, Span};

pub use tuo_mir_interp::Limits;

use crate::report::{
    AssertionResult, Failure, Outcome, SkippedSpec, SpecReport, SpecRun, TraceFrame, TrapReport,
};

/// Which specs to execute.
#[derive(Clone, Debug, Default)]
pub enum Selection {
    /// Every spec in the program.
    #[default]
    All,
    /// Only specs attached to the function named `target`, plus free-standing
    /// specs whose written name equals `target`.
    Target(String),
}

/// The outcome of attempting a spec run.
#[derive(Clone, Debug)]
pub enum RunOutcome {
    /// The front end rejected the program; no spec executed. The diagnostics
    /// are the front-end errors (and warnings) in stage order.
    NotChecked(Vec<Diagnostic>),
    /// The program checked; these are the results of the executed specs.
    Ran(SpecReport),
}

/// Run the specs of the program formed by `sources` (drawn from `map`),
/// selecting with `selection` and bounding each execution with `limits`.
///
/// Returns [`RunOutcome::NotChecked`] with the front-end diagnostics if the
/// program does not check, and [`RunOutcome::Ran`] with a [`SpecReport`]
/// otherwise. Never panics: interpreter aborts become structured
/// [`Outcome::Errored`] results.
#[must_use]
pub fn run(
    map: &SourceMap,
    sources: &[SourceId],
    selection: &Selection,
    limits: Limits,
) -> RunOutcome {
    // The spec runner is a MIR consumer, like a backend: it drives the
    // front-end stages directly rather than through the orchestration facade
    // (which sits above it). A program with front-end errors does not run
    // (ADR-0002).
    let parses: Vec<tuo_parser::ParseResult> = sources
        .iter()
        .map(|&id| tuo_parser::parse(map.source(id)))
        .collect();
    let mut diagnostics: Vec<Diagnostic> = parses
        .iter()
        .flat_map(tuo_parser::ParseResult::all_diagnostics)
        .collect();
    let asts: Vec<Ast<'_>> = parses
        .iter()
        .zip(sources)
        .map(|(parse, &id)| Ast::new(&parse.tree, map.source(id).text()))
        .collect();
    let resolution = tuo_resolve::resolve(&asts);
    let types = tuo_types::check(&asts, &resolution);
    let ownership = tuo_ownership::check(&asts, &resolution, &types);
    diagnostics.extend_from_slice(resolution.diagnostics());
    diagnostics.extend_from_slice(types.diagnostics());
    diagnostics.extend_from_slice(ownership.diagnostics());
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
    {
        return RunOutcome::NotChecked(diagnostics);
    }

    let lowered_hir = tuo_hir::lower(&asts, &resolution);
    let specs = tuo_mir::lower_specs(&lowered_hir, &resolution, &types);

    // The interpreter constructor is the mandatory verification gate. Lowering
    // is expected to emit verifiable MIR; if it somehow did not, surface that
    // as diagnostics rather than executing malformed MIR. Tracing stays off —
    // failure reporting uses the trap's own backtrace, not the full trace.
    let interpreter = match Interpreter::new(&specs.program, &types)
        .map(|interpreter| interpreter.with_limits(limits))
    {
        Ok(interpreter) => interpreter,
        Err(problems) => return RunOutcome::NotChecked(problems),
    };

    let mut report = SpecReport::default();
    for skipped in &specs.skipped {
        report.skipped.push(SkippedSpec {
            symbol: skipped.symbol,
            name: skipped.name.clone(),
            reason: skipped.reason.clone(),
            span: skipped.span,
        });
    }
    for spec in &specs.specs {
        if !selects(selection, spec, &resolution) {
            continue;
        }
        report.runs.push(run_one(&interpreter, spec, map));
    }
    RunOutcome::Ran(report)
}

/// Does `selection` include `spec`?
fn selects(selection: &Selection, spec: &SpecMir, resolution: &Resolution) -> bool {
    match selection {
        Selection::All => true,
        Selection::Target(target) => {
            // An identifier-named spec matches when its target function's name
            // matches; a string-named spec matches on its own written name.
            let by_target = spec
                .target
                .is_some_and(|function| resolution.symbol(function).name == *target);
            by_target || spec.name == *target
        }
    }
}

/// Execute one spec's assertions, timing the whole spec.
fn run_one(interpreter: &Interpreter<'_>, spec: &SpecMir, map: &SourceMap) -> SpecRun {
    let started = Instant::now();
    let assertions = spec
        .assertions
        .iter()
        .map(|assertion| run_assertion(interpreter, assertion, map))
        .collect();
    let duration = started.elapsed();
    SpecRun {
        symbol: spec.symbol,
        name: spec.name.clone(),
        target: spec.target,
        span: spec.span,
        assertions,
        duration,
    }
}

/// Execute one `then` / `assert` clause.
fn run_assertion(
    interpreter: &Interpreter<'_>,
    assertion: &SpecAssertion,
    map: &SourceMap,
) -> AssertionResult {
    let source = slice(map, assertion.span);
    let outcome = match interpreter.run(&assertion.condition, Vec::new()) {
        Ok(value) => {
            if value.as_bool() == Some(true) {
                Outcome::Passed
            } else {
                Outcome::Failed(explain(interpreter, assertion))
            }
        }
        Err(error) => Outcome::Errored(trap_report(&error)),
    };
    AssertionResult {
        kind: assertion.kind,
        source,
        span: assertion.span,
        outcome,
    }
}

/// Build the failure detail for a `false` assertion: for an `==` comparison,
/// evaluate both operands to report actual and expected; otherwise leave them
/// unset (the assertion is simply `false`).
fn explain(interpreter: &Interpreter<'_>, assertion: &SpecAssertion) -> Failure {
    let Some(comparison) = &assertion.comparison else {
        return Failure {
            expected: None,
            actual: None,
        };
    };
    // Operand evaluation can itself trap; a trap there means we cannot render
    // that side, so we leave it unset rather than inventing a value.
    let actual = interpreter
        .run(&comparison.actual, Vec::new())
        .ok()
        .map(|value| value.render());
    let expected = interpreter
        .run(&comparison.expected, Vec::new())
        .ok()
        .map(|value| value.render());
    Failure { expected, actual }
}

/// Translate an interpreter abort into a reportable trap. The spans travel
/// raw in the report; a caller with the [`SourceMap`] renders them.
fn trap_report(error: &RuntimeError) -> TrapReport {
    TrapReport {
        label: error.label().to_owned(),
        message: error.message.clone(),
        span: error.span,
        trace: error
            .backtrace
            .iter()
            .map(|frame| TraceFrame {
                function: frame.function.clone(),
                span: frame.span,
            })
            .collect(),
    }
}

/// The source text a span covers, or an empty string if it cannot be located
/// (a defensive fallback; spec spans always resolve for a checked program).
fn slice(map: &SourceMap, span: Span) -> String {
    let Some(text) = map.get_source(span.source()) else {
        return String::new();
    };
    let source = text.text();
    let range = span.range();
    let (start, end) = (range.start().as_usize(), range.end().as_usize());
    source.get(start..end).unwrap_or_default().to_owned()
}
