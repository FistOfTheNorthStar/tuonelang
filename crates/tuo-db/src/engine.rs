//! The generic red-green incremental engine.
//!
//! This module owns the reusable salsa-style machinery — a global revision
//! counter, per-query memos with `changed_at`/`verified_at` stamps and recorded
//! dependencies, dependency revalidation before recomputation, **early cutoff**,
//! cycle detection, cooperative cancellation, and an execution log — with **no**
//! knowledge of any compiler stage. It knows only opaque [`QueryKey`]s and
//! opaque, host-comparable [`StoredValue`]s.
//!
//! The engine is deliberately value-agnostic so a host above the dependency
//! layering (`tuo-compiler`, which may depend on every stage) can register
//! arbitrary derived queries — parse, resolve, type-check, MIR, spec dependency
//! graphs — while `tuo-db` itself stays at its layer, depending only on
//! [`tuo_source`] and [`tuo_diagnostics`]. The in-crate [`Database`](crate::Database)
//! is one such host, a thin typed facade built directly on this engine.
//!
//! # How a host drives it
//!
//! A host implements [`QueryHost`]: given a [`QueryKey`] and a [`QueryCtx`], it
//! computes that query's value, reading every dependency through the context so
//! the edge is recorded. The host owns its inputs' actual values (source text,
//! parse trees, …) and tells the engine, via [`QueryEngine::bump_input`], only
//! *when* an input changed — the engine never stores an input's value, so it
//! stays value-agnostic.
//!
//! To run a query the host calls [`QueryEngine::query`], passing itself. The
//! engine memoizes, revalidates dependencies, applies early cutoff, and calls
//! back into `host.compute(key, ctx)` only when a (re)computation is actually
//! required. Because the host is passed in on every call (never captured in a
//! memo), a query is freely **re-runnable** — the engine may recompute a stale
//! dependency during another query's revalidation, which is what lets early
//! cutoff propagate through the whole graph.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use crate::QueryError;

/// A revision of the engine: bumped once per input change.
pub(crate) type Revision = u64;

/// An opaque identity for one query instance (a query kind plus its argument).
///
/// The host assigns a distinct `kind` to each query it registers and encodes
/// the query's argument — a [`FileId`](tuo_source::FileId) raw index, a symbol
/// index, or `0` for a nullary query — in `arg`. The engine never interprets
/// either field; it only compares and hashes the pair, so equal `(kind, arg)`
/// pairs denote the same query and share one memo.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct QueryKey {
    kind: u32,
    arg: u64,
}

impl QueryKey {
    /// A key for query `kind` applied to argument `arg`. Use `0` for `arg` when
    /// the query takes no argument.
    #[must_use]
    pub const fn new(kind: u32, arg: u64) -> Self {
        Self { kind, arg }
    }

    /// The query kind tag.
    #[must_use]
    pub const fn kind(self) -> u32 {
        self.kind
    }

    /// The query argument.
    #[must_use]
    pub const fn arg(self) -> u64 {
        self.arg
    }
}

impl fmt::Display for QueryKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "query(kind={}, arg={})", self.kind, self.arg)
    }
}

/// A memoized value stored by the engine.
///
/// The engine holds every result as an `Arc<dyn StoredValue>` so it can
/// memoize values of any host type. The one operation it needs is **semantic
/// equality**, which powers early cutoff: after recomputing a query, the engine
/// asks whether the new value equals the old one, and if so keeps the old
/// `changed_at` so dependents are not invalidated. The host's own `PartialEq`
/// therefore decides sameness (purity rule 4 in the crate docs).
///
/// Wrap any `T: Any + PartialEq + Debug` with [`stored`] to get one for free.
pub trait StoredValue: Any + fmt::Debug {
    /// Compare against another stored value for semantic equality. Values of
    /// different concrete types are never equal.
    fn dyn_eq(&self, other: &dyn StoredValue) -> bool;

    /// Upcast to `Any` so a host can downcast a fetched value to its type.
    fn as_any(&self) -> &dyn Any;
}

/// A wrapper making any `T: Any + PartialEq + Debug` a [`StoredValue`].
#[derive(Debug)]
struct Stored<T>(T);

impl<T: Any + PartialEq + fmt::Debug> StoredValue for Stored<T> {
    fn dyn_eq(&self, other: &dyn StoredValue) -> bool {
        other
            .as_any()
            .downcast_ref::<T>()
            .is_some_and(|other| &self.0 == other)
    }

    fn as_any(&self) -> &dyn Any {
        &self.0
    }
}

/// Box a host value into an [`Arc<dyn StoredValue>`] for the engine to memoize.
///
/// The `T` must implement `PartialEq` (for early cutoff) and `Debug` (for
/// instrumentation), and be `'static`.
#[must_use]
pub fn stored<T: Any + PartialEq + fmt::Debug>(value: T) -> Arc<dyn StoredValue> {
    Arc::new(Stored(value))
}

/// Downcast a fetched stored value to `&T`, or `None` if it holds another type.
///
/// A host that always stores type `T` under a given query kind can `expect`
/// this; a `None` means the host mixed value types under one kind, which is a
/// host bug.
#[must_use]
pub fn downcast<T: Any>(value: &Arc<dyn StoredValue>) -> Option<&T> {
    value.as_any().downcast_ref::<T>()
}

/// The computed outcome of a query: its value and a human label for the
/// execution log.
pub struct Computed {
    /// The query's result value.
    pub value: Arc<dyn StoredValue>,
    /// A stable, human-oriented label (e.g. `"line_count(file#0)"`).
    pub label: String,
}

impl Computed {
    /// A computed result with `value` and `label`.
    #[must_use]
    pub fn new(value: Arc<dyn StoredValue>, label: impl Into<String>) -> Self {
        Self {
            value,
            label: label.into(),
        }
    }
}

/// A host that knows how to compute each of its queries.
///
/// The engine calls [`QueryHost::compute`] with a [`QueryKey`] and a
/// [`QueryCtx`]; the host dispatches on `key.kind()` / `key.arg()`, reads its
/// dependencies through the context (recording edges), and returns the value
/// plus a label. `compute` must obey the crate's purity rules — deterministic,
/// closed over the database, side-effect free — because the engine may call it
/// any number of times or not at all.
pub trait QueryHost {
    /// Compute the query identified by `key`, reading dependencies through
    /// `ctx`.
    ///
    /// # Errors
    ///
    /// Returns [`QueryError::Cancelled`] if cancellation is observed while
    /// fetching a dependency, or any domain error the query defines.
    fn compute(&self, key: QueryKey, ctx: &QueryCtx<'_>) -> Result<Computed, QueryError>;
}

/// A memoized query result with its red-green bookkeeping.
#[derive(Clone)]
struct Memo {
    value: Arc<dyn StoredValue>,
    /// Revision at which the value last actually changed.
    changed_at: Revision,
    /// Revision at which the value was last confirmed up to date.
    verified_at: Revision,
    /// The queries and inputs this result read, in fetch order.
    deps: Vec<QueryKey>,
}

/// The reusable incremental engine. All engine state lives here; the host owns
/// its inputs' actual values and only tells the engine when they change.
#[derive(Default)]
pub struct QueryEngine {
    /// Current revision; bumped once per input change.
    revision: Revision,
    /// Revision at which each *input* key last changed.
    input_changed: HashMap<QueryKey, Revision>,
    /// Memoized derived-query results.
    memos: RefCell<HashMap<QueryKey, Memo>>,
    /// Derived queries currently executing (cycle detection).
    active: RefCell<Vec<QueryKey>>,
    /// Dependency-recording frames, one per executing/validating query.
    frames: RefCell<Vec<Vec<QueryKey>>>,
    /// Instrumentation: labels of derived-query executions, in order.
    log: RefCell<Vec<String>>,
    /// Cooperative cancellation flag.
    cancelled: Cell<bool>,
}

impl fmt::Debug for QueryEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QueryEngine")
            .field("revision", &self.revision)
            .field("inputs", &self.input_changed.len())
            .field("memos", &self.memos.borrow().len())
            .finish_non_exhaustive()
    }
}

/// The context handed to a query while it computes. Every dependency the query
/// reads must go through this so the edge is recorded (purity rule 2). It
/// carries the engine and the host so a query can fetch derived dependencies,
/// which the engine may in turn recompute through the same host.
pub struct QueryCtx<'a> {
    engine: &'a QueryEngine,
    host: &'a dyn QueryHost,
}

impl QueryCtx<'_> {
    /// Fetch a derived dependency, recording the edge and returning its value.
    ///
    /// # Errors
    ///
    /// Propagates [`QueryError::Cancelled`] and any error the dependency's
    /// computation returns.
    pub fn query(&self, key: QueryKey) -> Result<Arc<dyn StoredValue>, QueryError> {
        self.engine.fetch(key, self.host).map(|(value, _)| value)
    }

    /// Record a dependency on an input key and return the revision at which it
    /// last changed. The host reads the input's *value* itself; the engine only
    /// tracks the edge and the change revision.
    ///
    /// # Errors
    ///
    /// [`QueryError::Cancelled`] if cancellation was requested.
    pub fn depend_on_input(&self, key: QueryKey) -> Result<Revision, QueryError> {
        self.engine.fetch_input_dep(key)
    }
}

impl QueryEngine {
    /// A fresh engine at revision 0.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The current revision.
    #[must_use]
    pub fn revision(&self) -> Revision {
        self.revision
    }

    /// Mark an input key as changed: bump the global revision and record that
    /// this input last changed at the new revision. The host must have already
    /// stored the input's new value before calling this.
    pub fn bump_input(&mut self, key: QueryKey) {
        self.revision += 1;
        self.input_changed.insert(key, self.revision);
    }

    /// Whether an input key has ever been set (has a recorded change revision).
    #[must_use]
    pub fn input_is_set(&self, key: QueryKey) -> bool {
        self.input_changed.contains_key(&key)
    }

    // ------------------------------------------------------------------
    // Cancellation.
    // ------------------------------------------------------------------

    /// Request cooperative cancellation. Abandoning a computation never
    /// corrupts the engine — no memo is written for an unfinished query, and
    /// finished sub-results stay valid.
    pub fn request_cancellation(&self) {
        self.cancelled.set(true);
    }

    /// Clear a previously requested cancellation.
    pub fn clear_cancellation(&self) {
        self.cancelled.set(false);
    }

    /// Whether cooperative cancellation is currently requested. A host reading
    /// an input directly (outside a query) checks this so input reads honor the
    /// same cancellation contract as derived queries.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.get()
    }

    // ------------------------------------------------------------------
    // Instrumentation.
    // ------------------------------------------------------------------

    /// Labels of every derived-query **execution** (not cache hit or
    /// revalidation), in order, since the last [`QueryEngine::clear_executions`].
    #[must_use]
    pub fn executions(&self) -> Vec<String> {
        self.log.borrow().clone()
    }

    /// Forget the recorded executions.
    pub fn clear_executions(&self) {
        self.log.borrow_mut().clear();
    }

    // ------------------------------------------------------------------
    // Query entry point.
    // ------------------------------------------------------------------

    /// Compute (or reuse) the query identified by `key`, driving `host` to
    /// (re)compute it when necessary. Returns the memoized value.
    ///
    /// # Errors
    ///
    /// [`QueryError::Cancelled`] if cancellation was requested, or any error the
    /// host's computation returns.
    pub fn query(
        &self,
        key: QueryKey,
        host: &dyn QueryHost,
    ) -> Result<Arc<dyn StoredValue>, QueryError> {
        self.fetch(key, host).map(|(value, _)| value)
    }

    // ------------------------------------------------------------------
    // Engine internals.
    // ------------------------------------------------------------------

    /// Record that the current query (if any) depends on `key` and return the
    /// revision at which that input last changed.
    fn fetch_input_dep(&self, key: QueryKey) -> Result<Revision, QueryError> {
        if self.cancelled.get() {
            return Err(QueryError::Cancelled);
        }
        if let Some(frame) = self.frames.borrow_mut().last_mut() {
            frame.push(key);
        }
        Ok(self.input_changed.get(&key).copied().unwrap_or(0))
    }

    /// Fetch a derived query result, memoizing and revalidating as needed.
    /// Returns the value and the revision at which it last changed.
    fn fetch(
        &self,
        key: QueryKey,
        host: &dyn QueryHost,
    ) -> Result<(Arc<dyn StoredValue>, Revision), QueryError> {
        if self.cancelled.get() {
            return Err(QueryError::Cancelled);
        }
        // The caller (if any) depends on `key`.
        if let Some(frame) = self.frames.borrow_mut().last_mut() {
            frame.push(key);
        }

        assert!(
            !self.active.borrow().contains(&key),
            "query cycle detected at {key}",
        );

        // Green path: an existing memo may be reusable.
        let existing = self.memos.borrow().get(&key).cloned();
        if let Some(memo) = &existing {
            if memo.verified_at == self.revision {
                return Ok((Arc::clone(&memo.value), memo.changed_at));
            }
            if self.deps_unchanged_since(&memo.deps, memo.verified_at, host)? {
                let mut memos = self.memos.borrow_mut();
                let memo = memos.get_mut(&key).expect("memo exists on green path");
                memo.verified_at = self.revision;
                return Ok((Arc::clone(&memo.value), memo.changed_at));
            }
        }

        // Red path: (re)compute, recording dependencies.
        self.recompute(key, host, existing.as_ref())
    }

    /// Recompute `key` by driving `host`, recording its dependencies, logging
    /// the execution, and applying early cutoff against `existing` (the prior
    /// memo, if any). Installs the fresh memo and returns its value and
    /// `changed_at`.
    fn recompute(
        &self,
        key: QueryKey,
        host: &dyn QueryHost,
        existing: Option<&Memo>,
    ) -> Result<(Arc<dyn StoredValue>, Revision), QueryError> {
        self.active.borrow_mut().push(key);
        self.frames.borrow_mut().push(Vec::new());
        let ctx = QueryCtx { engine: self, host };
        let result = host.compute(key, &ctx);
        let deps = self.frames.borrow_mut().pop().expect("own frame present");
        self.active.borrow_mut().pop();
        let computed = result?;

        self.log.borrow_mut().push(computed.label);

        // Early cutoff: an equal value keeps its old changed_at, so dependents
        // of this query are not invalidated.
        let changed_at = match existing {
            Some(memo) if memo.value.dyn_eq(computed.value.as_ref()) => memo.changed_at,
            _ => self.revision,
        };
        self.memos.borrow_mut().insert(
            key,
            Memo {
                value: Arc::clone(&computed.value),
                changed_at,
                verified_at: self.revision,
                deps,
            },
        );
        Ok((computed.value, changed_at))
    }

    /// Whether every dependency's value is unchanged since `verified_at`.
    ///
    /// Each derived dependency is revalidated (recursively, recomputing it
    /// through the host if any of *its* dependencies changed), so early cutoff
    /// propagates: a dependency that recomputes to an equal value keeps its old
    /// `changed_at` and does not force the caller to recompute. Input
    /// dependencies are checked against their recorded change revision.
    fn deps_unchanged_since(
        &self,
        deps: &[QueryKey],
        verified_at: Revision,
        host: &dyn QueryHost,
    ) -> Result<bool, QueryError> {
        self.frames.borrow_mut().push(Vec::new());
        let mut unchanged = true;
        let mut outcome = Ok(());
        for &dep in deps {
            let dep_changed_at = if self.is_memoized(dep) {
                match self.revalidate_memoized(dep, host) {
                    Ok(changed_at) => changed_at,
                    Err(error) => {
                        outcome = Err(error);
                        break;
                    }
                }
            } else {
                // An input dependency: its recorded change revision.
                self.input_changed.get(&dep).copied().unwrap_or(0)
            };
            if dep_changed_at > verified_at {
                unchanged = false;
                break;
            }
        }
        self.frames.borrow_mut().pop();
        outcome.map(|()| unchanged)
    }

    /// Whether a key currently has a memo (i.e. it is a derived query the engine
    /// has computed at least once).
    fn is_memoized(&self, key: QueryKey) -> bool {
        self.memos.borrow().contains_key(&key)
    }

    /// Revalidate an already-memoized derived query and return its `changed_at`.
    /// Recurses through the memo's own dependencies; on a clean revalidation the
    /// memo's `verified_at` is advanced. If a transitive dependency changed, the
    /// query is recomputed through the host so early cutoff can compare the
    /// fresh value against the old one — the returned `changed_at` reflects that
    /// comparison, not merely the current revision.
    fn revalidate_memoized(
        &self,
        key: QueryKey,
        host: &dyn QueryHost,
    ) -> Result<Revision, QueryError> {
        if self.cancelled.get() {
            return Err(QueryError::Cancelled);
        }
        let memo = self.memos.borrow().get(&key).cloned();
        let Some(memo) = memo else {
            return Ok(self.input_changed.get(&key).copied().unwrap_or(0));
        };
        if memo.verified_at == self.revision {
            return Ok(memo.changed_at);
        }
        if self.deps_unchanged_since(&memo.deps, memo.verified_at, host)? {
            let mut memos = self.memos.borrow_mut();
            if let Some(memo) = memos.get_mut(&key) {
                memo.verified_at = self.revision;
                return Ok(memo.changed_at);
            }
        }
        // A dependency changed: recompute through the host (permitted, since
        // this key is not on the active stack), and let early cutoff decide the
        // fresh `changed_at`.
        let (_, changed_at) = self.recompute(key, host, Some(&memo))?;
        Ok(changed_at)
    }
}
