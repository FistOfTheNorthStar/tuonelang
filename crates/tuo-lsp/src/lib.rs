//! Language server for tuonelang.
//!
//! The LSP must reuse the shared compiler semantic engine rather than
//! reimplementing analysis. It therefore depends on the query database
//! ([`tuo_db`]) and the compiler facade ([`tuo_compiler`]).
//!
//! No language server is implemented yet; only the crate boundary is
//! established.
