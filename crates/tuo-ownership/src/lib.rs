//! Ownership and memory-safety analysis for tuonelang.
//!
//! Memory safety without a garbage collector is a core tuonelang goal. This crate
//! will enforce the ownership rules using type information from [`tuo_types`].
//!
//! No ownership analysis is implemented yet; only the crate boundary is
//! established.
