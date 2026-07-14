//! Source-text infrastructure for the tuonelang compiler.
//!
//! This crate will own the representation of source files, byte offsets,
//! spans, and source maps that the rest of the compiler references when
//! reporting locations. It is the foundation of the pipeline and must not
//! depend on any later stage (lexer, parser, diagnostics, or codegen).
//!
//! No source-handling functionality is implemented yet; only the crate
//! boundary is established.
