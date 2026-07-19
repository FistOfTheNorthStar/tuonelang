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
//! lexing, parsing, the lossless CST, and typed AST views), which the CLI's
//! `tuo debug syntax` / `tuo debug ast` developer tools drive. Later stages
//! are still stubs, so no further orchestration exists yet.

/// Typed AST views over the CST (re-export of [`tuo_ast`]).
pub use tuo_ast as ast;
/// Structured diagnostics (re-export of [`tuo_diagnostics`]).
pub use tuo_diagnostics as diagnostics;
/// Lexing (re-export of [`tuo_lexer`]).
pub use tuo_lexer as lexer;
/// Parsing (re-export of [`tuo_parser`]).
pub use tuo_parser as parser;
/// Source files, spans, and source maps (re-export of [`tuo_source`]).
pub use tuo_source as source;
/// The lossless concrete syntax tree (re-export of [`tuo_syntax`]).
pub use tuo_syntax as syntax;
