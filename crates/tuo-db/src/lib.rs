//! Query-based compiler database for tuonelang.
//!
//! This crate is the **incremental query boundary** of the compiler: the one
//! computation layer that the CLI, LSP, and agent protocol all drive, so
//! tuonelang semantics are computed once and reused across tools. All compiler
//! state lives behind [`Database`]; tools set *inputs* (source text) and ask
//! *queries*, and the engine memoizes, validates, and recomputes minimally.
//!
//! # The query surface (architecture)
//!
//! The public architecture expresses the pipeline as queries over the
//! database. The intended surface is:
//!
//! ```text
//! source_text(file)          → the current source snapshot         [implemented]
//! lex(file)                  → tokens                              [awaits tuo-lexer]
//! parse(file)                → syntax tree                         [awaits tuo-parser]
//! lower_ast(file)            → AST → HIR lowering                  [awaits tuo-hir]
//! module_items(module)       → the items a module declares         [awaits tuo-hir]
//! resolve_name(scope, name)  → what a name refers to               [awaits tuo-resolve]
//! type_of(item)              → an item's type                      [awaits tuo-types]
//! mir_of(function)           → a function's MIR                    [awaits tuo-mir]
//! specs_for(function)        → the specs colocated with a function [awaits tuo-spec]
//! ```
//!
//! Per the project rule that no surface advertises behavior the compiler
//! cannot perform, only the queries whose backing stages exist are
//! implemented today: the `source_text` input plus two small real derived
//! queries over it (`line_count`, `total_line_count`) that exercise the
//! incremental engine end to end. Stage queries slot in as their crates gain
//! functionality, replacing nothing.
//!
//! # Incremental engine
//!
//! The engine is a TDG-owned salsa-style red-green implementation
//! (`Database` in [`db`]): a global revision counter, per-query memos with
//! `changed_at`/`verified_at` stamps and recorded dependencies, dependency
//! revalidation before recomputation, and **early cutoff** — a recomputation
//! that produces an equal value keeps its old `changed_at`, so downstream
//! queries are not invalidated. A third-party engine (Salsa) could replace
//! this implementation behind the same boundary, but no engine-specific type
//! (Salsa's or this one's — keys, memos, revisions) ever appears in the
//! public API: the boundary speaks [`tuo_source`] identities, plain values,
//! and [`QueryError`].
//!
//! # Query purity requirements
//!
//! Every derived query, present and future, must obey these rules — they are
//! what make memoization, invalidation, and (later) persistence sound:
//!
//! 1. **Deterministic:** equal inputs produce an equal result, across runs
//!    and machines. No randomness, no wall-clock time, no iteration over
//!    nondeterministically ordered containers.
//! 2. **Closed over the database:** a query may read *only* (a) its
//!    arguments and (b) other queries/inputs fetched through the database,
//!    so every read is tracked as a dependency. No file-system or network
//!    I/O, no environment variables, no global mutable state.
//! 3. **Side-effect free:** a query computes a value and nothing else. It
//!    must not mutate anything observable — the engine may cache a result
//!    forever, recompute it any number of times, or skip it entirely.
//! 4. **Meaningful equality:** result types must implement equality that
//!    reflects semantic sameness, because early cutoff compares old and new
//!    values to decide whether dependents stay valid.
//! 5. **Abandonable:** a query may be cancelled between dependency fetches
//!    (see [`QueryError::Cancelled`]). Because of rules 2–3 this is always
//!    safe: an abandoned query has changed nothing, and completed
//!    sub-results remain valid memos.
//! 6. **Total:** for well-formed arguments a query returns a value or a
//!    [`QueryError`]; it must not panic on malformed *source* (that is what
//!    diagnostics are for — errors are values here).

mod db;

pub use db::{Database, QueryError};
