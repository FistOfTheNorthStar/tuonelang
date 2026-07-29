//! Language server for tuonelang, built on the shared compiler semantic engine.
//!
//! `tuo-lsp` answers Language Server Protocol requests **without reimplementing
//! any compiler stage**. It drives the same incremental query engine the CLI
//! and the agent protocol use — [`tuo_compiler::IncrementalSession`] — and each
//! feature is a thin projection of a query that engine already computes:
//! diagnostics, name resolution, type checking, and the colocated-spec
//! dependency graph. Parsing, resolution, and type inference happen once, in the
//! shared session; the LSP only *reads* their results and translates compiler
//! [`Span`](tuo_source::Span)s into LSP ranges.
//!
//! # Architecture
//!
//! ```text
//! editor ─ LSP request ─▶ Analysis ─▶ IncrementalSession (shared queries)
//!                           │              │
//!                           │              ├─ tuo_resolve::Resolution
//!                           │              ├─ tuo_types::TypeckResult
//!                           │              └─ diagnostics (with spans)
//!                           ▼
//!                        wire types  ◀── convert (UTF-8 span ⇄ UTF-16 range)
//! ```
//!
//! - [`Analysis`] is the request surface: one method per feature, each a
//!   read-only query over the session. It owns the session and the open-document
//!   set, and holds no analysis state of its own.
//! - [`wire`] is the LSP JSON vocabulary (positions, ranges, hovers, edits,
//!   symbols, completions, semantic tokens, code actions), [`serde`]-serializable
//!   and carrying no compiler types.
//! - [`convert`] is the sole boundary between the compiler's UTF-8 byte
//!   positions and the protocol's UTF-16 line/character positions.
//!
//! # Supported features
//!
//! Diagnostics, hover, go-to-definition, find-references, rename, document
//! symbols, completion, signature help, semantic tokens, quick-fix code actions
//! (only for compiler-authored, machine-applicable suggestions), and navigation
//! both ways between a function and its colocated specs — every one delegating
//! to an existing query in the table on [`Analysis`].
//!
//! # Transport
//!
//! This crate is the *analysis core*, not a wire server: [`Analysis`] answers
//! requests as plain typed values so it is exhaustively testable in-process. A
//! JSON-RPC/stdio transport that owns an [`Analysis`] and marshals
//! `initialize`/`textDocument/*` traffic is a thin future addition; the semantic
//! answers it would serve live here.

pub mod analysis;
pub mod convert;
pub mod wire;

pub use analysis::Analysis;
