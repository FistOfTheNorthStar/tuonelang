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
//! database. The intended stage surface is:
//!
//! ```text
//! source_text(file)          → the current source snapshot
//! parse(file)                → syntax tree
//! module_interface(file)     → a file's signature-level summary
//! resolve()                  → name/path resolution
//! type_of(function)          → a function's type-check result
//! mir_of(function)           → a function's MIR
//! spec_dependencies(spec)    → a spec's semantic dependency closure
//! ```
//!
//! These are **not** implemented in this crate, and cannot be: `tuo-db` sits at
//! its dependency layer with visibility of only [`tuo_source`] and
//! `tuo-diagnostics`, while every backing stage crate (`tuo-parser`,
//! `tuo-resolve`, `tuo-types`, `tuo-mir`, `tuo-spec`) is layered strictly above
//! it. So this crate provides the *engine*, not the stage wiring: the reusable,
//! stage-agnostic [`QueryEngine`] (module [`engine`]), driven by any host that
//! implements [`QueryHost`]. The real stage queries are registered by
//! `tuo-compiler`'s incremental session, which does depend on every stage. In
//! this crate the [`Database`] facade is the worked example — the `source_text`
//! input plus two small derived queries (`line_count`, `total_line_count`) that
//! exercise memoization, invalidation, early cutoff, and cancellation end to
//! end.
//!
//! # Incremental engine
//!
//! The engine ([`QueryEngine`]) is a TDG-owned salsa-style red-green
//! implementation: a global revision counter, per-query memos with
//! `changed_at`/`verified_at` stamps and recorded dependencies, dependency
//! revalidation before recomputation, and **early cutoff** — a recomputation
//! that produces an equal value keeps its old `changed_at`, so downstream
//! queries are not invalidated. It is generic: it stores results as opaque
//! [`StoredValue`]s (equality, hence early cutoff, is decided by the host's own
//! `PartialEq`) keyed by opaque [`QueryKey`]s, and drives recomputation through
//! the host's [`QueryHost::compute`]. A third-party engine (Salsa) could replace
//! this implementation behind the same boundary, but no engine-specific type
//! (Salsa's or this one's — keys, memos, revisions) ever appears in a host's
//! public API: the boundary speaks [`tuo_source`] identities, plain values, and
//! [`QueryError`].
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

use std::error::Error;
use std::fmt;

use tuo_source::FileId;

mod db;
pub mod engine;

pub use db::Database;
pub use engine::{
    Computed, QueryCtx, QueryEngine, QueryHost, QueryKey, StoredValue, downcast, stored,
};

/// Error returned by queries.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum QueryError {
    /// The computation was abandoned because cancellation was requested.
    /// Nothing was corrupted: completed sub-results remain valid, and the
    /// query can simply be asked again after [`Database::clear_cancellation`].
    Cancelled,
    /// The file has no source text set.
    NoText {
        /// The file that has no text.
        file: FileId,
    },
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => f.write_str("query cancelled"),
            Self::NoText { file } => write!(f, "no source text set for {file}"),
        }
    }
}

impl Error for QueryError {}
