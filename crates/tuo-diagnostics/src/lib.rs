//! Structured diagnostics for the tuonelang compiler.
//!
//! tuonelang treats diagnostics as first-class, machine-readable data — a core goal
//! of the project. This crate will define the diagnostic model (severity,
//! codes, labels, suggestions) and rendering, referencing [`tuo_source`] for
//! locations. It must not depend on parsing or later semantic stages.
//!
//! No diagnostic types are implemented yet; only the crate boundary and its
//! dependency on `tuo-source` are established.
