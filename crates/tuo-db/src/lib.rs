//! Query-based compiler database for tuonelang.
//!
//! This crate will host the incremental computation layer that ties the
//! compiler together and that the CLI, LSP, and agent protocol all share, so
//! that tuonelang semantics are computed once and reused across tools. It depends
//! only on the lowest layers ([`tuo_source`], [`tuo_diagnostics`]).
//!
//! No query engine is implemented yet; only the crate boundary is established.
