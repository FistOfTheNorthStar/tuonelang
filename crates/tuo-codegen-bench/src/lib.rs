//! The TDG code-generation evaluation harness.
//!
//! This crate is the *complete* LLM evaluation harness for tuonelang: it
//! benchmarks a code-generation model by **driving the real compiler** over
//! everything the model produces, so every reliability metric it reports is
//! earned by an actual compile rather than asserted. It complements — and does
//! not replace — [`tuo_bench`]'s version-controlled metric *schema* (Prompt 2):
//! `tuo-bench` defines the shapes and does not run the pipeline; this crate runs
//! the pipeline.
//!
//! # Pluggable models, no model embedded
//!
//! A model is reached through a [`ModelAdapter`] the host implements — an LLM
//! behind an API, a local runner, or a deterministic generator. **No LLM
//! provider is embedded here.** The adapter turns a [`Prompt`] (and, on repair
//! turns, the compiler's diagnostics) into tuonelang source; the harness compiles
//! that source and scores it. This is the same injection seam pattern the corpus
//! (`NativeExecutor`) and agent (`Formatter`) use.
//!
//! # Metrics (all compiler-derived)
//!
//! For each task [`run_task`] measures, and [`BenchmarkSummary`] aggregates:
//! **Parse@1**, **Check@1**, **SpecPass@1**, **TestPass@1**, **Repair@1**, repair
//! cycles, generated tokens, wall-clock feedback latency, **invented symbol
//! count** (undefined-name diagnostics), and **unrelated edit rate** (repairs
//! that touched code the compiler had not flagged).
//!
//! # Provenance, kept in full
//!
//! A [`BenchmarkRun`] keeps everything needed to trust and reproduce a result:
//! the exact **prompts** (per [`TurnRecord`]), the **model configuration**
//! ([`ModelConfig`]), the **compiler and language versions**, the model's
//! **outputs** (every turn's generated source), and the compiler's **results**
//! (every turn's evaluation). Nothing is summarized away in the raw run.
//!
//! # Tasks are never changed silently
//!
//! Benchmark tasks are version-controlled [`BenchTask`]s pinned by a content
//! [`digest`](BenchTask::digest); a [`TaskSet`] verifies every pin on load
//! ([`TaskSet::verify_digests`]), so a task cannot be edited without the change
//! being loud. Tasks may carry comparable [`SyntaxVariant`]s so a language-design
//! decision can be evaluated empirically across spellings.
//!
//! # Reports
//!
//! Both a machine-readable report ([`BenchmarkSummary::to_json`], versioned by
//! [`SCHEMA_VERSION`]) and a human-readable one ([`render_human`]) are produced
//! from the same summary, so they never disagree.

mod evaluate;
mod harness;
mod model;
mod report;
mod task;

pub use evaluate::{Evaluation, evaluate, evaluate_tests};
pub use harness::{
    BenchmarkRun, RunConfig, TaskRun, TurnRecord, compiler_version, language_version, rescore,
    run_task,
};
pub use model::{ConfigEntry, Generation, GenerationError, ModelAdapter, ModelConfig, Prompt};
pub use report::{BenchmarkSummary, render_human};
pub use task::{BenchTask, DigestMismatch, PinnedTask, SyntaxVariant, TaskSet};

/// The serialization/schema version for this harness's machine-readable outputs
/// (task sets, benchmark runs, summaries). Bump only on a breaking change to an
/// output shape. Independent of [`tuo_bench::SCHEMA_VERSION`].
pub const SCHEMA_VERSION: u32 = 1;
