//! Name resolution for tuonelang.
//!
//! This crate will resolve names and paths over the [`tuo_hir`]. It is one of
//! two semantic passes that consume HIR (the other being type checking) and
//! must not depend on CLI presentation or code generation.
//!
//! No resolution logic is implemented yet; only the crate boundary is
//! established.
