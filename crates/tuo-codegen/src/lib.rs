//! Backend-agnostic code-generation interface for tuonelang.
//!
//! This crate will define the shared abstractions that concrete native
//! backends implement, keeping the MIR consumers uniform. It depends on
//! [`tuo_mir`] but not on any specific backend, so that Cranelift and LLVM
//! remain interchangeable behind a tuonelang-owned interface.
//!
//! No codegen interface is implemented yet; only the crate boundary is
//! established.
