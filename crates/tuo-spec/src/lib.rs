//! Executable specification infrastructure for tuonelang.
//!
//! A distinguishing tuonelang goal is colocated, executable specifications.
//! Their language semantics are formalized in **ADR-0002** and implemented in
//! the front end:
//!
//! - parsing (`tuo-parser`), attachment, and dependency discovery
//!   ([`tuo_resolve::Resolution::specs_for`],
//!   [`tuo_resolve::Resolution::target_of`],
//!   [`tuo_resolve::Resolution::dependencies_of`]);
//! - type checking of spec bodies (`tuo-types`), all driven by `tuo check`.
//!
//! This crate will own the *execution* side — the spec runner that follows a
//! spec's dependencies, lowers them, and drives the body through the MIR
//! interpreter ([`tuo_mir_interp`]). Specs do not execute until that
//! interpreter exists; only the crate boundary is established here.
