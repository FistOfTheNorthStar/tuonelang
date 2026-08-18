//! Mid-level intermediate representation (MIR) for tuonelang.
//!
//! MIR is the **single executable semantic representation** of
//! tuonelang: the reference interpreter and every native backend
//! (Cranelift, LLVM) consume the same MIR, so the language's semantics
//! are defined once — by the instruction documentation in [`mir`] — and
//! never re-derived per backend. This crate stays backend-agnostic: no
//! Cranelift-, LLVM-, or target-specific types appear here.
//!
//! The IR is a typed control-flow graph: functions of typed locals and
//! basic blocks, with constants, direct calls, returns, conditional
//! branches, multi-way switches, arithmetic and comparisons (trapping
//! integer overflow, Constitution §24), struct/enum construction, field
//! access, enum discrimination, the ownership model's memory operations
//! (moves, borrow-mode call arguments, explicit drops), and explicit
//! traps and checks ([`mir::Terminator::Assert`], [`mir::Terminator::Trap`]).
//! Every instruction's semantics are fully defined on its type — there
//! is no undefined behavior in MIR.
//!
//! [`lower`] translates the typed HIR of a **front-end-clean** program
//! (parse, resolve, type check, and ownership check all passed) into
//! MIR; [`render`] is the development pretty-printer behind
//! `tuo debug mir` and the golden tests — deterministic output, not a
//! stable protocol.
//!
//! # Optimization
//!
//! [`optimize`] runs a small, conservative pass pipeline ([`opt`]) that
//! rewrites MIR into equivalent, smaller MIR — constant folding, copy
//! propagation, unreachable-block removal, and dead-local elimination.
//! Optimization is **not** part of the reference semantics: the interpreter
//! executes *unoptimized* MIR, and every pass is a meaning-preserving
//! rewrite that leaves the program's observable behavior (its return value
//! and which/whether it traps) unchanged. Each pass re-verifies its output
//! (see [`debug_assert_verified`]), so the optimized MIR a backend consumes
//! is verified MIR. It is applied on the native path (`tuo build`/`tuo run`)
//! and inspectable with `tuo debug mir --opt`.
//!
//! # Verification
//!
//! MIR is only meaningful when it is well-formed, so [`verify`] is a
//! **mandatory** gate: it walks a [`Program`] and returns structured
//! diagnostics (`Mxxxx`) for every malformed construct — dangling block
//! targets, out-of-range locals, illegal projections, type mismatches,
//! use of an undefined or moved-out value, and the ownership invariants
//! lowering must preserve. The verifier is total (it never panics). Every
//! consumer — the interpreter and both backends — must reject MIR that
//! does not verify, and the verifier runs after lowering here and after
//! every future optimization pass in debug/test configurations.
//!
//! # v0 lowering limits
//!
//! Constructs outside the v0 subset are never mis-lowered: the whole
//! containing function is recorded as [`mir::Skipped`] with a reason.
//! The current limits, each lifted by the feature that owns it:
//!
//! - **Method calls** and **function-typed values / indirect calls**
//!   (pending the trait system; the type checker already poisons the
//!   former).
//! - **`const` references** are lowered only for literal (or negated
//!   literal) initializers.
//! - **Destructuring `let`/`for` patterns** (only names and `_` bind
//!   there today).
//! - **Or-patterns that bind**, and **match guards on arms binding
//!   non-`Copy` values** (bindings would need to be undone when the
//!   guard fails).
//! - **Iterating arrays of non-`Copy` elements** (element moves out of
//!   loops need drop flags the model forbids).
//! - Defensively, a body whose initialization states could disagree at a
//!   merge point is skipped rather than guessed — the ownership checker
//!   proves this cannot happen for accepted programs.

mod lower;
mod mir;
pub mod opt;
mod print;
pub mod spec;
mod verify;

pub use lower::{lower, lower_specs};
pub use mir::{
    AggregateKind, Arg, BasicBlock, BinOp, BlockId, CastKind, Const, EffectOp, Function, LocalDecl,
    LocalId, Operand, PassMode, Place, Program, Projection, Rvalue, Skipped, Statement, StrOp,
    Terminator, Trap, UnOp,
};
pub use opt::{OptReport, PassReport, optimize, pass_descriptions};
pub use print::render;
pub use spec::{AssertionKind, Comparison, SkippedSpec, SpecAssertion, SpecMir, SpecProgram};
pub use verify::{debug_assert_verified, is_well_formed, verify};
