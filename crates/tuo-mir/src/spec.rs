//! Lowered MIR for colocated `spec` blocks (ADR-0002).
//!
//! `tuo check` parses, resolves, and type-checks specs but never executes
//! them. Execution is the spec runner's job, and it runs **verified MIR** —
//! so specs, like every other body, must be lowered. A spec body is not a
//! function, though: it is a sequence of `given` / `when` / `then` / `assert`
//! clauses. [`lower_specs`](crate::lower_specs) turns each spec into a
//! [`SpecMir`] whose assertions are ordinary synthetic MIR functions in the
//! same [`Program`](crate::Program) — so the interpreter and the mandatory
//! verifier need no special cases, and every native backend could execute a
//! spec the same way.
//!
//! Each `then` / `assert` clause becomes one synthetic **condition function**
//! that replays the spec's setup (every clause up to and including the
//! assertion) and returns the assertion's `Bool` value. When the assertion is
//! a `lhs == rhs` comparison, two more synthetic functions are emitted — one
//! returning `lhs` (the *actual* value) and one returning `rhs` (the
//! *expected* value) — so a failing assertion can report concrete operands,
//! not just "expected true, got false". Everything is a normal function call
//! and a normal return; there is no new instruction.

use tuo_resolve::SymbolId;
use tuo_source::Span;

/// Every spec lowered from one program snapshot, plus the specs the v0
/// subset declined to lower.
#[derive(Clone, Debug, Default)]
pub struct SpecProgram {
    /// The MIR to execute: the program's ordinary functions (the specs'
    /// dependencies) **and** the synthetic assertion functions the specs
    /// lower to. This is what the interpreter is built over and what the
    /// verifier gates.
    pub program: crate::Program,
    /// One entry per spec that lowered, in source order.
    pub specs: Vec<SpecMir>,
    /// Specs that could not be lowered (a clause outside the v0 subset),
    /// with the reason — never silently dropped.
    pub skipped: Vec<SkippedSpec>,
}

/// One lowered spec: its identity and the assertions to run.
#[derive(Clone, Debug)]
pub struct SpecMir {
    /// The spec's own symbol (its stable identity; names may repeat —
    /// ADR-0002).
    pub symbol: SymbolId,
    /// The spec's name as written (identifier or string form).
    pub name: String,
    /// The targeted function, for identifier-named specs that resolved;
    /// `None` for free-standing (string-named) specs.
    pub target: Option<SymbolId>,
    /// The whole `spec { … }` block's span.
    pub span: Span,
    /// The assertions to execute, in source order. A spec with no `then` /
    /// `assert` clause has none (it still lowers, proving its setup runs).
    pub assertions: Vec<SpecAssertion>,
}

/// One `then` / `assert` clause, lowered to runnable functions.
#[derive(Clone, Debug)]
pub struct SpecAssertion {
    /// Which clause keyword introduced it.
    pub kind: AssertionKind,
    /// The clause expression's span (what a failure points at, and what the
    /// runner slices to recover the assertion's source text).
    pub span: Span,
    /// The name of the synthetic function that returns this assertion's
    /// `Bool` value. Run it: `true` passes, `false` fails.
    pub condition: String,
    /// When the assertion is `lhs == rhs`, the names of the two synthetic
    /// functions returning the *actual* (`lhs`) and *expected* (`rhs`)
    /// operands, for richer failure reporting. `None` for any other
    /// assertion shape.
    pub comparison: Option<Comparison>,
}

/// The names of the two synthetic functions that evaluate the sides of a
/// `lhs == rhs` assertion.
#[derive(Clone, Debug)]
pub struct Comparison {
    /// The function returning the left-hand side (the *actual* value).
    pub actual: String,
    /// The function returning the right-hand side (the *expected* value).
    pub expected: String,
}

/// Whether a clause was written `then` or `assert` (they execute
/// identically; the distinction is preserved for reporting).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AssertionKind {
    /// A `then expr;` clause.
    Then,
    /// An `assert expr;` clause.
    Assert,
}

impl AssertionKind {
    /// The clause keyword, for diagnostics.
    #[must_use]
    pub const fn keyword(self) -> &'static str {
        match self {
            Self::Then => "then",
            Self::Assert => "assert",
        }
    }
}

/// A spec the v0 lowering declined, with the reason.
#[derive(Clone, Debug)]
pub struct SkippedSpec {
    /// The spec's symbol.
    pub symbol: SymbolId,
    /// The spec's name as written.
    pub name: String,
    /// Why it was not lowered (a clause outside the v0 subset).
    pub reason: String,
    /// The spec block's span.
    pub span: Span,
}
