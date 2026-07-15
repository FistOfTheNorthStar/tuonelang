//! Compiler facade for tuonelang.
//!
//! `tuo-compiler` is the single orchestration seam that will coordinate the
//! tuonelang pipeline end to end:
//!
//! ```text
//! source → lex → parse → resolve → type check → ownership → MIR
//! ```
//!
//! It exists so that the CLI, the language server ([`tuo_lsp`](../tuo_lsp)),
//! and the agent protocol ([`tuo_agent`](../tuo_agent)) drive the compiler
//! through one shared entry point instead of each re-wiring the stages. Its
//! dependencies deliberately stop at the [`tuo_codegen`] abstraction so the
//! facade never learns about a specific native backend, and it never depends
//! on CLI presentation.
//!
//! To keep this crate from becoming a dumping ground, it should hold only
//! orchestration and re-exports — pipeline *logic* lives in the individual
//! stage crates.
//!
//! No pipeline orchestration is implemented yet; only the crate boundary and
//! its intended dependencies are established.
