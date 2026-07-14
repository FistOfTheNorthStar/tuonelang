//! Abstract syntax tree (AST) for tuonelang.
//!
//! This crate will define the AST produced by the parser and consumed when
//! lowering to HIR. It depends only on foundational crates and carries no
//! semantic (type or ownership) information.
//!
//! No AST node types are implemented yet; only the crate boundary is
//! established.
