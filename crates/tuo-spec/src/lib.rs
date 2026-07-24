//! Executable specification infrastructure for tuonelang.
//!
//! A distinguishing tuonelang goal is colocated, executable specifications.
//! Their language semantics are formalized in **ADR-0002** and implemented in
//! the front end:
//!
//! - parsing (`tuo-parser`), attachment, and dependency discovery
//!   ([`tuo_resolve::Resolution::specs_for`],
//!   [`tuo_resolve::Resolution::target_of`],
//!   [`tuo_resolve::Resolution::dependencies_of`]);
//! - type checking of spec bodies (`tuo-types`), all driven by `tuo check`.
//!
//! This crate owns the **execution** side. `tuo check` parses, resolves, and
//! type-checks specs but never runs them; the spec runner here lowers each
//! spec to verified MIR ([`tuo_mir::lower_specs`]) and drives its assertions
//! through the reference interpreter ([`tuo_mir_interp`]).
//!
//! # What running a spec means
//!
//! Every `then` / `assert` clause lowers to a synthetic MIR function that
//! replays the spec's `given` / `when` setup and returns the assertion's
//! `Bool` value. The runner executes that function in the interpreter's
//! deterministic sandbox and, when an equality assertion fails, evaluates each
//! operand to report the concrete *actual* and *expected* values. A failure
//! records the spec's identity, the assertion, its expected and actual values,
//! the source span, and the TDG call trace where the interpreter provided one.
//!
//! # Safety and determinism
//!
//! Each execution is bounded by configurable instruction [`Limits::fuel`], a
//! [`recursion depth`](Limits::max_call_depth), and a [`memory
//! budget`](Limits::max_live_values), and runs with **no** host filesystem,
//! network, clock, or randomness access (MIR reaches none of them). The runner
//! is total: an interpreter abort becomes a structured
//! [`Outcome::Errored`](report::Outcome::Errored) result, never a panic.
//!
//! # Performance
//!
//! The runner makes **no** latency promise. It times each spec's execution
//! ([`SpecRun::duration`](report::SpecRun::duration),
//! [`SpecReport::total_duration`](report::SpecReport::total_duration)) so
//! callers can measure and report actual performance.

pub mod report;
mod runner;

pub use report::{
    AssertionResult, Failure, Outcome, SkippedSpec, SpecReport, SpecRun, TraceFrame, TrapReport,
};
pub use runner::{Limits, RunOutcome, Selection, run};
