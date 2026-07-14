//! Reference interpreter over tuonelang MIR.
//!
//! This crate will execute [`tuo_mir`] directly, serving as the reference
//! semantics and the substrate for executable specifications. It consumes the
//! same MIR as the native backends.
//!
//! No interpreter is implemented yet; only the crate boundary is established.
