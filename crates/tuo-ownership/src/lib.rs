//! Ownership and memory-safety analysis for tuonelang.
//!
//! Memory safety without a garbage collector is a core tuonelang goal. This
//! crate will enforce the v0 ownership model — moves, derived `Copy`, the
//! `in`/`mut`/`take` borrow modes, partial moves, conservative joins, and
//! statically known drop points — using type information from [`tuo_types`].
//!
//! The model is **frozen before implementation**: the normative rules live
//! in `specification/ownership.md` (adopted by ADR-0003), and their
//! executable counterpart is the fixture corpus in
//! `tests/ownership/fixtures/` (`ok/` must compile, `err/` must fail with
//! the annotated `O0001`–`O0009` diagnostic). The checker implemented here
//! must match that agreed pair exactly; the corpus is its acceptance suite.
//!
//! No ownership analysis is implemented yet; only the crate boundary and
//! the corpus guard (`tests/fixtures.rs`) exist.
