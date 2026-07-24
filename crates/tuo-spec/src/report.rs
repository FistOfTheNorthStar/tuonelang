//! The structured result of executing specs.
//!
//! A run produces a [`SpecReport`]: one [`SpecRun`] per lowered spec, each
//! carrying its identity, per-assertion [`AssertionResult`]s, and a measured
//! [`duration`](SpecRun::duration). A failing assertion records everything a
//! developer needs to diagnose it — the clause, its source text, the expected
//! and actual values (when the assertion is an equality comparison), the
//! source span, and the TDG call trace where the interpreter provided one.

use std::time::Duration;

use tuo_mir::AssertionKind;
use tuo_resolve::SymbolId;
use tuo_source::Span;

/// The outcome of executing a selection of specs.
#[derive(Clone, Debug, Default)]
pub struct SpecReport {
    /// One entry per lowered spec, in source order.
    pub runs: Vec<SpecRun>,
    /// Specs the v0 lowering declined (a clause outside the executable
    /// subset), reported rather than silently dropped.
    pub skipped: Vec<SkippedSpec>,
}

impl SpecReport {
    /// Did every executed assertion pass?
    #[must_use]
    pub fn passed(&self) -> bool {
        self.runs.iter().all(SpecRun::passed)
    }

    /// The number of specs that ran (excludes skipped specs).
    #[must_use]
    pub fn ran(&self) -> usize {
        self.runs.len()
    }

    /// The number of specs with at least one failing or errored assertion.
    #[must_use]
    pub fn failures(&self) -> usize {
        self.runs.iter().filter(|run| !run.passed()).count()
    }

    /// The total measured execution time across every spec.
    #[must_use]
    pub fn total_duration(&self) -> Duration {
        self.runs.iter().map(|run| run.duration).sum()
    }
}

/// One executed spec: its identity, per-assertion results, and timing.
#[derive(Clone, Debug)]
pub struct SpecRun {
    /// The spec's stable identity (names may repeat — ADR-0002).
    pub symbol: SymbolId,
    /// The spec's name as written (identifier or string form).
    pub name: String,
    /// The targeted function, for identifier-named specs; `None` for
    /// free-standing (string-named) specs.
    pub target: Option<SymbolId>,
    /// The whole `spec { … }` block's span.
    pub span: Span,
    /// One result per `then` / `assert` clause, in source order.
    pub assertions: Vec<AssertionResult>,
    /// The wall-clock time spent executing this spec's assertions. The
    /// runner does **not** promise any particular latency; this is the
    /// measured figure so callers can report actual performance.
    pub duration: Duration,
}

impl SpecRun {
    /// Did every assertion in this spec pass?
    #[must_use]
    pub fn passed(&self) -> bool {
        self.assertions
            .iter()
            .all(|assertion| matches!(assertion.outcome, Outcome::Passed))
    }
}

/// The result of one `then` / `assert` clause.
#[derive(Clone, Debug)]
pub struct AssertionResult {
    /// Whether the clause was written `then` or `assert`.
    pub kind: AssertionKind,
    /// The clause expression's source text.
    pub source: String,
    /// The clause expression's span.
    pub span: Span,
    /// What happened.
    pub outcome: Outcome,
}

/// What executing one assertion produced.
#[derive(Clone, Debug)]
pub enum Outcome {
    /// The assertion evaluated to `true`.
    Passed,
    /// The assertion evaluated to `false`.
    Failed(Failure),
    /// Executing the assertion aborted (a language trap or a resource
    /// limit) before it produced a boolean.
    Errored(TrapReport),
}

/// A failed assertion's diagnostic detail.
#[derive(Clone, Debug)]
pub struct Failure {
    /// The expected value's rendering (the right-hand side of an `==`
    /// comparison), or `None` when the assertion is not an equality
    /// comparison the runner could decompose.
    pub expected: Option<String>,
    /// The actual value's rendering (the left-hand side of an `==`
    /// comparison), or `None` as above.
    pub actual: Option<String>,
}

/// A trap or resource abort that stopped an assertion mid-execution.
#[derive(Clone, Debug)]
pub struct TrapReport {
    /// The trap's stable label (e.g. `"integer_overflow"`, `"out_of_fuel"`).
    pub label: String,
    /// A human-readable explanation.
    pub message: String,
    /// The span the abort is attributed to.
    pub span: Span,
    /// The TDG call trace at the point of abort, innermost frame first
    /// (each entry is a `(function, span)` pair). Empty when the interpreter
    /// provided no backtrace.
    pub trace: Vec<TraceFrame>,
}

/// One frame of a trap's call trace.
#[derive(Clone, Debug)]
pub struct TraceFrame {
    /// The executing function's name.
    pub function: String,
    /// The span within it.
    pub span: Span,
}

/// A spec the v0 lowering could not turn into executable MIR.
#[derive(Clone, Debug)]
pub struct SkippedSpec {
    /// The spec's symbol.
    pub symbol: SymbolId,
    /// The spec's name as written.
    pub name: String,
    /// Why it was not lowered.
    pub reason: String,
    /// The spec block's span.
    pub span: Span,
}
