//! Cranelift backend for tuonelang.
//!
//! This crate will lower verified [`tuo_mir`] to native code via Cranelift,
//! implementing the backend-agnostic interface in [`tuo_codegen`]. Cranelift
//! itself will be wrapped behind tuonelang-owned abstractions rather than exposed
//! throughout the compiler.
//!
//! No backend is implemented yet; only the crate boundary is established.
