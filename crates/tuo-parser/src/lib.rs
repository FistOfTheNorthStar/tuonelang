//! Parser for tuonelang.
//!
//! This crate will consume the token stream from [`tuo_lexer`] and produce the
//! lossless syntax tree ([`tuo_syntax`]) and abstract syntax tree
//! ([`tuo_ast`]). It must never depend on type checking, MIR, or any code
//! generation backend (neither Cranelift nor LLVM).
//!
//! No parsing logic is implemented yet; only the crate boundary is
//! established. There is intentionally no fake `parse` function that reports
//! success regardless of input.
