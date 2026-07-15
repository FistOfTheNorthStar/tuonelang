//! Lossless syntax representation for tuonelang.
//!
//! This crate will define the concrete, lossless syntax tree (retaining
//! trivia such as whitespace and comments) that enables high-fidelity
//! formatting and IDE tooling. It sits between the lexer and the abstract
//! syntax tree and must not depend on code generation or semantic analysis.
//!
//! No syntax tree is implemented yet; only the crate boundary is established.
