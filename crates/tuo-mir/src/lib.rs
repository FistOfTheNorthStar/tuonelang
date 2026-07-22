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
//! is no undefined behavior in MIR, and no optimization is performed.
//!
//! [`lower`] translates the typed HIR of a **front-end-clean** program
//! (parse, resolve, type check, and ownership check all passed) into
//! MIR; [`render`] is the development pretty-printer behind
//! `tuo debug mir` and the golden tests — deterministic output, not a
//! stable protocol.
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
mod print;

pub use lower::lower;
pub use mir::{
    AggregateKind, Arg, BasicBlock, BinOp, BlockId, CastKind, Const, Function, LocalDecl, LocalId,
    Operand, PassMode, Place, Program, Projection, Rvalue, Skipped, Statement, Terminator, Trap,
    UnOp,
};
pub use print::render;
