//! Reference interpreter over tuonelang MIR.
//!
//! This crate executes [`tuo_mir`] directly and is the language's **initial
//! semantic reference implementation**: the meaning of a tuonelang program is,
//! by definition, what this interpreter computes for its MIR. The native
//! backends (Cranelift, LLVM) consume the same MIR and must agree with the
//! interpreter instruction for instruction; where they diverge, the backend
//! is wrong.
//!
//! # What it executes
//!
//! It interprets **verified** MIR — never AST, never source syntax. The
//! mandatory [`tuo_mir::verify`] gate runs in [`Interpreter::new`], and
//! unverified MIR is refused with the verifier's structured diagnostics, not
//! executed. Within verified MIR, v0 supports the deterministic core the
//! language already lowers: arithmetic and comparisons (with trapping integer
//! overflow, Constitution §24), branches and multi-way switches, direct calls
//! and recursion, borrow-mode arguments, struct/enum/`Option`/`Result`
//! construction and projection, arrays with bounds-checked indexing, and
//! explicit drops.
//!
//! # Determinism and safety
//!
//! Execution is **deterministic**: the same program under the same [`Limits`]
//! yields the same result, the same trap, and the same [`Trace`] every run.
//! The interpreter is a **sandbox** — it has no host filesystem, network,
//! process, clock, or randomness access, because MIR has no instruction that
//! could reach them. Three configurable ceilings bound a run so an untrusted
//! or non-terminating program cannot exhaust the host: instruction
//! [`fuel`](Limits::fuel), [`recursion depth`](Limits::max_call_depth), and a
//! [`live-value budget`](Limits::max_live_values). Exceeding any of them, like
//! any language trap, produces a structured [`RuntimeError`] rather than a
//! panic.
//!
//! # Outcomes
//!
//! A run either returns a [`Value`] or aborts with a [`RuntimeError`] carrying
//! a [`TrapKind`], a source [`span`](tuo_source::Span), and a call-stack
//! backtrace. A structured [`Trace`] of the whole execution is available on
//! request ([`Interpreter::run_traced`]).
//!
//! ```
//! # // Doc-level overview only; see the conformance suite for runnable cases.
//! use tuo_mir_interp::{Interpreter, Limits};
//! ```

mod config;
mod interp;
mod trace;
mod trap;
mod value;

pub use config::Limits;
pub use interp::{Interpreter, RunResult};
pub use trace::{Trace, TraceEvent};
pub use trap::{Frame, RuntimeError, TrapKind};
pub use value::Value;
