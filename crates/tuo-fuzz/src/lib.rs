//! The TDG whole-compiler fuzzing harness.
//!
//! Prompt 37 asks for fuzz coverage of the *entire* pipeline — lexer, parser,
//! syntax-tree operations, formatter, AST lowering, type checker, ownership
//! checker, HIR → MIR lowering, MIR verifier, and MIR interpreter — governed by
//! a set of load-bearing invariants, with regression fixtures added automatically
//! for every discovered bug. This crate is the shared engine that makes that
//! coverage honest and non-duplicative.
//!
//! # One invariant, two drivers
//!
//! Each stage's contract is written **once**, as a `check_*` function in
//! [`stages`]. Two drivers exercise it:
//!
//! - **`cargo fuzz`** targets (nightly, coverage-guided) under each stage's
//!   `fuzz/` directory call the checker on libFuzzer-supplied bytes. These are
//!   *not* workspace members — cargo-fuzz manages them and they need a nightly
//!   toolchain — so they never run in ordinary CI.
//! - a **stable robustness sweep** ([`tests/sweep.rs`](../tests/sweep.rs)) calls
//!   the same checkers over the fixed-seed [`corpus`], so the invariants are
//!   enforced on every `cargo test` with no nightly toolchain.
//!
//! Because both drivers call the identical checker, the coverage-guided fuzzer
//! and the deterministic sweep can never disagree about what "correct" means.
//!
//! # The invariants
//!
//! - **arbitrary source input must not crash the compiler** — every stage entry
//!   point up through MIR lowering is total by construction (malformed input
//!   becomes error tokens, recovery islands, poison nodes, or diagnostics); the
//!   checkers assert reaching the end of each stage without a panic, plus each
//!   stage's structural contract.
//! - **formatting must be idempotent** and **valid formatted source must remain
//!   parseable** — [`stages::check_fmt`].
//! - **verified MIR must not trigger interpreter structural panics** —
//!   [`stages::check_interp`] only runs MIR the interpreter's verify gate
//!   accepted, and rejects any `TrapKind::Internal`.
//! - **differential execution must agree across engines where defined** — this
//!   is already a mandatory CI gate over the *accepted-program* generator in
//!   `tuo-cli/tests/differential.rs` (interpreter vs Cranelift vs LLVM). This
//!   crate does not re-implement the native build; it targets the complementary
//!   goal — *robustness on malformed input* — and defers cross-engine agreement
//!   to that existing suite rather than asserting a weaker copy.
//!
//! # Regression fixtures, automatically
//!
//! [`regression`] turns a discovered crash into a committed obligation: [`record`]
//! writes the exact crashing input to `regressions/<stage>/` (content-addressed,
//! idempotent), and [`replay_all`] re-runs every committed fixture through its
//! stage checker on each `cargo test`, so a fixed bug can never silently return.
//!
//! # Not a stage, and never advertised as one
//!
//! This crate is tooling *around* the compiler, not a pipeline stage: it depends
//! only on crates below it in the layering and adds no edge into the pipeline.
//! It exposes no CLI subcommand — fuzzing is a developer/CI activity driven by
//! `cargo test` (the sweep) and `cargo fuzz` (coverage), never a promise the
//! `tuo` binary makes to a user.

pub mod corpus;
pub mod driver;
pub mod regression;
pub mod stages;

pub use corpus::{Flavor, Rng, input};
pub use driver::guarded;
pub use regression::{Fixture, checker_for, load_all, record, replay_all};
pub use stages::{
    NamedChecker, all_checkers, check_ast_lowering, check_fmt, check_front_end, check_interp,
    check_lexer, check_mir, check_parser, check_syntax,
};
