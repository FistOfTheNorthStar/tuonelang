//! Mid-level intermediate representation (MIR) for tuonelang.
//!
//! MIR is intended to become the *single* executable semantic representation
//! of tuonelang. The interpreter and every native backend consume the same verified
//! MIR, which prevents tuonelang semantics from being reimplemented independently in
//! multiple backends.
//!
//! For that reason this crate must remain backend-agnostic: no Cranelift-,
//! LLVM-, or target-specific types may appear here.
//!
//! No MIR is implemented yet; only the crate boundary is established.
