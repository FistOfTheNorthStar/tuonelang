//! Research harness for tuonelang: the performance laboratory, tokenizer
//! measurement, and LLM benchmarking.
//!
//! This crate is the *core* of three research tools whose front-ends live under
//! `benchmarks/`, `tools/tokenizer-lab`, and `benchmarks/llm`:
//!
//! - The **performance laboratory** ([`lab`]) runs reproducible compiler and
//!   runtime benchmarks against the *real* compiler, records the full run
//!   environment, and — following the project's honesty rule — reports only what
//!   it measured, tagging every workload the v0 core cannot express with the
//!   exact reason instead of a fabricated number. It compares against a peer
//!   language only where the semantics are equivalent, and only when a real
//!   number backs the comparison.
//!
//! - The **tokenizer lab** ([`tokenizer`], [`measure`], [`fixtures`]) measures
//!   how candidate tuonelang syntax tokenizes under *multiple* tokenizers. Its
//!   entire reason for existing is to keep tuonelang's syntax from being
//!   designed around the quirks of a single tokenizer. Token count is one input
//!   to syntax decisions, deliberately **not** the deciding factor (see
//!   [`fixtures`] and the tool's README).
//!
//! - The **LLM benchmark schema** ([`llm`]) is the version-controlled,
//!   machine-readable shape for recording code-generation reliability metrics
//!   (Parse@1, Check@1, SpecPass@1, Repair@1, average repair cycles, generated
//!   tokens, hallucinated-symbol count). This crate defines and validates the
//!   schema; it does not run models.
//!
//! # Determinism
//!
//! Everything here is deterministic and offline. The built-in tokenizer
//! adapters ([`tokenizer::adapters`]) are self-contained — they embed no
//! external vocabulary files and make no network calls — so measurements are
//! reproducible in CI. Real production tokenizers (e.g. tiktoken/Claude BPE) are
//! added later as additional [`tokenizer::Tokenizer`] implementations behind the
//! same trait, without changing the measurement engine or the schema.

pub mod fixtures;
pub mod lab;
pub mod llm;
pub mod measure;
pub mod tokenizer;

/// The serialization format version for machine-readable harness outputs.
///
/// Committed result files record this so that a consumer can detect a schema
/// change. Bump it only on a breaking change to an output shape.
pub const SCHEMA_VERSION: u32 = 1;
