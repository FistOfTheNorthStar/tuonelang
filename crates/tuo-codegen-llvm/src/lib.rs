//! LLVM backend for tuonelang.
//!
//! This crate will lower verified [`tuo_mir`] to native code via LLVM,
//! implementing the backend-agnostic interface in [`tuo_codegen`]. LLVM will
//! be wrapped behind tuonelang-owned abstractions.
//!
//! No backend is implemented yet; only the crate boundary is established.
