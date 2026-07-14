//! Type checking and inference for tuonelang.
//!
//! This crate will implement tuonelang's static type system over the [`tuo_hir`],
//! using resolved names from [`tuo_resolve`]. It feeds ownership checking and,
//! ultimately, MIR construction.
//!
//! No type system is implemented yet; only the crate boundary is established.
