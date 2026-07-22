//! Ownership and memory-safety analysis for tuonelang.
//!
//! [`check`] enforces the v0 ownership model frozen in
//! `specification/ownership.md` (adopted by ADR-0003): moves and derived
//! `Copy`, the `in`/`mut`/`take` parameter modes, per-argument-list borrow
//! conflicts, partial moves, conservative joins at branches and loop back
//! edges, and statically known drop points ("no hidden drop flags"). There
//! is **no user-written lifetime syntax**: a borrow lives exactly as long
//! as the call that takes it, so the whole analysis is per-function and
//! flow-sensitive over places (bindings, parameters, and their field
//! paths).
//!
//! The checker runs after type checking and consumes its results (§16): a
//! body containing a type error is not ownership-checked, and expressions
//! the type checker poisons (method calls, pending the trait system) never
//! produce ownership noise. Diagnostics use the reserved `O0001`–`O0009`
//! codes of `specification/ownership.md` §15; each names the place, the
//! earlier action that produced the state, and — where one exists — the
//! local fix. The executable counterpart of the specification is the
//! fixture corpus in `tests/ownership/fixtures/` (`ok/` must produce zero
//! ownership diagnostics; each `err/` case exactly its annotated code),
//! which `tests/fixtures.rs` enforces.
//!
//! # Known conservative rejections
//!
//! Deliberate simplifications, frozen by ADR-0003 — each has a local
//! rewrite and may be revisited by a future ADR:
//!
//! - **No drop flags (`O0008`)**: a place moved on some paths but not
//!   others is rejected at its drop point even if never used again.
//! - **No move-out of `mut` parameters (`O0003`)**, even when
//!   reinitialized before returning.
//! - **Loop-carried moves (`O0002`)**: a place moved in a loop body must
//!   be reinitialized before the back edge; the responsible move is
//!   reported even when only later iterations would observe it.
//! - **A deferred `let` may not be initialized inside a loop body**
//!   (`O0004`): the initialization could run once per iteration.
//! - **Field assignment does not initialize a deferred binding**: a
//!   deferred `let`/`var` must first be assigned as a whole.
//! - **Index expressions are not places (`O0007`)**: nothing moves out of
//!   or is assigned through `items[i]`; only `Copy` elements are read out
//!   by value.
//! - **Generic values are move-only**: `T` is treated as non-`Copy` inside
//!   a generic body (checking is pre-monomorphization).
//!
//! Two constructs are deliberately *not* analyzed deeper than reads, and
//! accept accordingly: method-call receivers/arguments (method signatures
//! arrive with the trait system; the type checker poisons these calls the
//! same way) and calls through function-typed values (v0 function types do
//! not carry parameter modes). Both surfaces are unreachable in meaningful
//! v0 programs today.

mod check;
mod env;
mod place;

use tuo_ast::Ast;
use tuo_diagnostics::Diagnostic;
use tuo_resolve::Resolution;
use tuo_types::TypeckResult;

pub use place::Place;

/// Is `ty` `Copy` under the v0 model (`specification/ownership.md` §2)?
///
/// `Copy`-ness is derived structurally, never user-written: scalars, `()`,
/// `Char`, `Str`, and ranges are `Copy`; tuples, `Option`/`Result`, and
/// user aggregates are `Copy` iff every component is; `String`, `Array`,
/// the memory wrappers, and generic parameters never are. This crate owns
/// the definition; MIR lowering consumes it to decide copies versus moves
/// and which drops to emit.
#[must_use]
pub fn is_copy(types: &TypeckResult, ty: &tuo_types::Ty) -> bool {
    env::TypeEnv::new(types).is_copy(ty)
}

/// Everything ownership checking produced for one program snapshot.
#[derive(Debug, Default)]
pub struct OwnershipResult {
    diagnostics: Vec<Diagnostic>,
}

impl OwnershipResult {
    /// Ownership diagnostics (`Oxxxx` codes), in discovery order per body.
    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
}

/// Ownership-check `files` against `resolution` and `types` (produced by
/// [`tuo_resolve::resolve`] and [`tuo_types::check`] over the same parse).
#[must_use]
pub fn check(files: &[Ast<'_>], resolution: &Resolution, types: &TypeckResult) -> OwnershipResult {
    OwnershipResult {
        diagnostics: check::run(files, resolution, types),
    }
}
