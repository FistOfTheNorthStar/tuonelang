//! Structured runtime outcomes: traps, resource-limit aborts, and the
//! call-stack backtrace attached to each.
//!
//! A running MIR program either returns a value or **aborts**. An abort is
//! never a panic and never a compile diagnostic — it is a structured
//! [`RuntimeError`] the caller can inspect: a [`TrapKind`] naming *why*, the
//! source [`Span`] where it happened, and a [`Frame`] backtrace of the call
//! stack at the point of abort (innermost first). This mirrors the language's
//! trap model (Constitution §24): deterministic aborts with no unwinding and
//! no destructor execution.

use tuo_source::Span;

/// Why a MIR program aborted.
///
/// The first group are *language* traps — outcomes the MIR semantics define
/// (Constitution §24). The second group are *resource* aborts imposed by the
/// interpreter's sandbox ([`crate::Limits`]); they are not part of the
/// language, but they are deterministic given the same limits. The final
/// variant guards the interpreter against MIR that should never have reached
/// it (a verifier bug or an unverified program): it is an internal error, not
/// a language outcome.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TrapKind {
    /// Integer arithmetic overflowed its type (add/sub/mul, negation of the
    /// minimum, or `MIN / -1`). Two's complement, trapping (§24).
    IntegerOverflow,
    /// Integer division or remainder by zero.
    DivisionByZero,
    /// An array index was out of bounds (a failed bounds-check assert, or an
    /// out-of-range `Index` projection).
    IndexOutOfBounds,
    /// Control reached a point the front end proved unreachable — a
    /// [`tuo_mir::Trap::Unreachable`] or a failed exhaustiveness assumption.
    Unreachable,
    /// The instruction-fuel budget was exhausted.
    OutOfFuel,
    /// The call stack exceeded the configured recursion depth.
    RecursionLimit,
    /// The live-value budget (a proxy for memory) was exceeded.
    MemoryBudget,
    /// The interpreter was handed MIR it could not execute: unverified MIR,
    /// or a construct outside the deterministic v0 subset it supports. This
    /// is an interpreter/compiler bug, surfaced structurally rather than as a
    /// panic. The string names the specific violation.
    Internal(String),
}

impl TrapKind {
    /// A stable, machine-facing label for the trap cause.
    #[must_use]
    pub fn label(&self) -> &str {
        match self {
            Self::IntegerOverflow => "integer_overflow",
            Self::DivisionByZero => "division_by_zero",
            Self::IndexOutOfBounds => "index_out_of_bounds",
            Self::Unreachable => "unreachable",
            Self::OutOfFuel => "out_of_fuel",
            Self::RecursionLimit => "recursion_limit",
            Self::MemoryBudget => "memory_budget",
            Self::Internal(_) => "internal",
        }
    }

    /// Whether this abort is a **language** trap (a defined MIR outcome), as
    /// opposed to a sandbox resource limit or an internal error.
    #[must_use]
    pub fn is_language_trap(&self) -> bool {
        matches!(
            self,
            Self::IntegerOverflow
                | Self::DivisionByZero
                | Self::IndexOutOfBounds
                | Self::Unreachable
        )
    }
}

/// One frame of the call-stack backtrace: which function, and where in it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Frame {
    /// The function's name (for display; the symbol is its identity).
    pub function: String,
    /// The span of the instruction the frame was executing.
    pub span: Span,
}

/// A structured runtime abort: why, where, and the call stack at that point.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RuntimeError {
    /// The cause of the abort.
    pub kind: TrapKind,
    /// A human-readable explanation.
    pub message: String,
    /// The source span the abort is attributed to.
    pub span: Span,
    /// The call stack, innermost frame first.
    pub backtrace: Vec<Frame>,
}

impl RuntimeError {
    /// The trap's stable label (see [`TrapKind::label`]).
    #[must_use]
    pub fn label(&self) -> &str {
        self.kind.label()
    }
}
