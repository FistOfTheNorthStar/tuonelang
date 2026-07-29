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
//! Implemented so far: the front-end re-exports below (source management,
//! lexing, parsing, the lossless CST, typed AST views, name resolution,
//! HIR lowering, type checking, and MIR lowering), plus the first
//! orchestration seam — [`check_sources`], the parse → resolve →
//! type-check → ownership-check pipeline behind `tuo check`. The CLI's
//! `tuo debug syntax` / `tuo debug ast` / `tuo debug hir` /
//! `tuo debug mir` developer tools drive the re-exports directly. Later
//! stages are still stubs, so no further orchestration exists yet.

mod check;
mod incremental;

pub use check::{CheckResult, check_sources};
pub use incremental::IncrementalSession;

/// Typed AST views over the CST (re-export of [`tuo_ast`]).
pub use tuo_ast as ast;
/// The incremental query database (re-export of [`tuo_db`]).
pub use tuo_db as db;
/// Structured diagnostics (re-export of [`tuo_diagnostics`]).
pub use tuo_diagnostics as diagnostics;
/// The desugared high-level IR (re-export of [`tuo_hir`]).
pub use tuo_hir as hir;
/// Lexing (re-export of [`tuo_lexer`]).
pub use tuo_lexer as lexer;
/// The mid-level IR and its HIR lowering (re-export of [`tuo_mir`]).
pub use tuo_mir as mir;
/// Parsing (re-export of [`tuo_parser`]).
pub use tuo_parser as parser;
/// Name resolution (re-export of [`tuo_resolve`]).
pub use tuo_resolve as resolve;
/// Source files, spans, and source maps (re-export of [`tuo_source`]).
pub use tuo_source as source;
/// The lossless concrete syntax tree (re-export of [`tuo_syntax`]).
pub use tuo_syntax as syntax;
/// Type checking and inference (re-export of [`tuo_types`]).
pub use tuo_types as types;
