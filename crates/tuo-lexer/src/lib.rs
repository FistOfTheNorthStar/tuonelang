//! Lexer for tuonelang.
//!
//! This crate will turn source text into a token stream. It depends only on
//! [`tuo_source`] and [`tuo_diagnostics`] and must never depend on parsing,
//! name resolution, or type checking.
//!
//! No tokenizer is implemented yet; only the crate boundary is established.
//! In particular, there is intentionally no fake `tokenize` function.
