//! The performance laboratory: reproducible compiler and runtime benchmarks that
//! drive the **real** compiler and report only what they measured.
//!
//! Prompt 38 asks for a benchmark *repository* that can prove its claims — cold
//! and incremental compiler benchmarks, runtime benchmarks, and cross-language
//! comparisons — recording the hardware, OS, compiler versions, exact commands,
//! and source, and *never publishing an unsupported "blazing fast" claim*. This
//! module is the harness core; the committed data lives under top-level
//! `benchmarks/`, and the CLI wires in the two host seams the crate cannot
//! provide itself.
//!
//! # The shape of an honest benchmark
//!
//! Three principles run through every submodule:
//!
//! - **Measure the real compiler.** [`compiler`] drives
//!   [`check_sources`](tuo_compiler::check_sources) and
//!   [`IncrementalSession`](tuo_compiler::IncrementalSession) directly — the same
//!   code paths the CLI uses. [`runtime`] runs compiled programs through a
//!   host-injected [`NativeRunner`](runtime::NativeRunner) (the CLI's real
//!   Cranelift+`cc` builder). Nothing is simulated.
//! - **Report only what exists.** tuonelang v0 runs the scalar, control-flow
//!   core and nothing else, so [`runtime`]'s allocation/collections/string/
//!   networking workloads are tagged [`Unsupported`](runtime::Support::Unsupported)
//!   with the exact reason and emit **no number**. A comparison exists only where
//!   a peer language shares the workload's semantics ([`compare`]).
//! - **No claim without a number.** [`compare::Verdict`] reaches `Measured` only
//!   when both languages actually ran under recorded toolchains; otherwise it is
//!   `Skipped` with the reason. [`report::render_human`] carries no superlative
//!   and no aggregate verdict — it prints measurements, unmeasured workloads with
//!   their reasons, and nothing else.
//!
//! # Host seams
//!
//! Two capabilities this crate deliberately does not have — turning source into a
//! running native binary, and running a foreign compiler — are traits the host
//! implements: [`runtime::NativeRunner`] and [`compare::ComparisonRunner`]. This
//! mirrors the corpus pipeline's `NativeExecutor` and the codegen benchmark's
//! `ModelAdapter`: the crate names no backend and no external toolchain, so it
//! stays layering-clean and testable entirely offline with fakes.

pub mod compare;
pub mod compiler;
pub mod env;
pub mod report;
pub mod runtime;
pub mod timing;

pub use env::Environment;
pub use report::{LabReport, render_human};
