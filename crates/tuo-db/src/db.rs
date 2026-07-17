//! The database: inputs, memoized queries, and the red-green engine.
//!
//! Everything in this module is TDG-owned. The engine internals (`Key`,
//! `Value`, `Memo`, revisions) are private; the public surface speaks
//! [`tuo_source`] identities, plain values, and [`QueryError`].

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::sync::Arc;

use tuo_source::{FileId, SourceId, SourceMap, SourceText, SourceTooLarge};

/// A revision of the database: bumped once per input change.
type Revision = u64;

/// Internal identity of one query instance (a query plus its arguments).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Key {
    /// Input: the current text snapshot of a file.
    SourceText(FileId),
    /// Input: the files that currently have text, in insertion order.
    FileList,
    /// Derived: number of lines in a file.
    LineCount(FileId),
    /// Derived: total number of lines across all files.
    TotalLineCount,
}

impl Key {
    const fn is_input(self) -> bool {
        matches!(self, Self::SourceText(_) | Self::FileList)
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SourceText(file) => write!(f, "source_text({file})"),
            Self::FileList => write!(f, "file_list()"),
            Self::LineCount(file) => write!(f, "line_count({file})"),
            Self::TotalLineCount => write!(f, "total_line_count()"),
        }
    }
}

/// Internal result value of a query. Cheap to clone.
#[derive(Clone, Debug)]
enum Value {
    Source(Arc<SourceText>),
    Files(Vec<FileId>),
    Count(u64),
}

impl PartialEq for Value {
    /// Semantic equality, used for early cutoff. Two source snapshots are
    /// equal when their **text** is equal — a re-set of identical text yields
    /// a new snapshot identity but does not invalidate downstream results.
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Source(a), Self::Source(b)) => a.text() == b.text(),
            (Self::Files(a), Self::Files(b)) => a == b,
            (Self::Count(a), Self::Count(b)) => a == b,
            _ => false,
        }
    }
}

/// A memoized query result with its red-green bookkeeping.
#[derive(Clone, Debug)]
struct Memo {
    value: Value,
    /// Revision at which the value last actually changed.
    changed_at: Revision,
    /// Revision at which the value was last confirmed up to date.
    verified_at: Revision,
    /// The queries this result read, in fetch order.
    deps: Vec<Key>,
}

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

/// The tuonelang compiler database: source inputs plus memoized, dependency-
/// tracked queries. See the crate docs for the query surface, the engine
/// design, and the purity requirements every query must obey.
///
/// Inputs are set through `&mut self` methods ([`Database::set_source_text`]);
/// queries take `&self`. The database is single-threaded today; the
/// cancellation flag is the seam where a host thread will later signal
/// long-running computations.
#[derive(Debug, Default)]
pub struct Database {
    /// File interning and immutable snapshot history.
    sources: SourceMap,
    /// Files that currently have text, in insertion order.
    files_with_text: Vec<FileId>,
    /// Current revision; bumped once per input change.
    revision: Revision,
    /// Revision at which each *input* last changed.
    input_changed: HashMap<Key, Revision>,
    /// Memoized derived-query results.
    memos: RefCell<HashMap<Key, Memo>>,
    /// Derived queries currently executing (cycle detection).
    active: RefCell<Vec<Key>>,
    /// Dependency-recording frames, one per executing/validating query.
    frames: RefCell<Vec<Vec<Key>>>,
    /// Instrumentation: derived-query executions, in order.
    log: RefCell<Vec<String>>,
    /// Cooperative cancellation flag.
    cancelled: Cell<bool>,
}

impl Database {
    /// An empty database.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    // ------------------------------------------------------------------
    // Inputs.
    // ------------------------------------------------------------------

    /// Intern a file name (see [`SourceMap::intern_file`]). Interning alone
    /// does not change any input; a file participates in queries once
    /// [`Database::set_source_text`] gives it text.
    pub fn intern_file(&mut self, name: &str) -> FileId {
        self.sources.intern_file(name)
    }

    /// The interned name of `file`.
    ///
    /// # Panics
    ///
    /// Panics if `file` was not produced by this database.
    #[must_use]
    pub fn file_name(&self, file: FileId) -> &str {
        self.sources.file_name(file)
    }

    /// Set (or replace) the source text of `file`, creating a new immutable
    /// snapshot at the file's next revision and invalidating dependent
    /// queries. Returns the new snapshot's identity.
    ///
    /// # Errors
    ///
    /// Returns [`SourceTooLarge`] if the text exceeds the offset space; the
    /// database is unchanged in that case.
    pub fn set_source_text(
        &mut self,
        file: FileId,
        text: impl Into<Arc<str>>,
    ) -> Result<SourceId, SourceTooLarge> {
        let first_text = self.sources.latest(file).is_none();
        let snapshot = self.sources.add_source(file, text)?;
        self.revision += 1;
        self.input_changed
            .insert(Key::SourceText(file), self.revision);
        if first_text {
            self.files_with_text.push(file);
            self.input_changed.insert(Key::FileList, self.revision);
        }
        Ok(snapshot)
    }

    // ------------------------------------------------------------------
    // Queries.
    // ------------------------------------------------------------------

    /// Query: the current source snapshot of `file`.
    ///
    /// # Errors
    ///
    /// [`QueryError::NoText`] if the file has no text; [`QueryError::Cancelled`]
    /// if cancellation was requested.
    pub fn source_text(&self, file: FileId) -> Result<Arc<SourceText>, QueryError> {
        match self.fetch(Key::SourceText(file))? {
            (Value::Source(text), _) => Ok(text),
            _ => unreachable!("source_text produces a Source value"),
        }
    }

    /// Query: the number of lines in `file` (as defined by
    /// [`SourceText::line_count`]).
    ///
    /// # Errors
    ///
    /// [`QueryError::NoText`] if the file has no text; [`QueryError::Cancelled`]
    /// if cancellation was requested.
    pub fn line_count(&self, file: FileId) -> Result<u64, QueryError> {
        match self.fetch(Key::LineCount(file))? {
            (Value::Count(count), _) => Ok(count),
            _ => unreachable!("line_count produces a Count value"),
        }
    }

    /// Query: the total number of lines across every file with text.
    ///
    /// # Errors
    ///
    /// [`QueryError::Cancelled`] if cancellation was requested.
    pub fn total_line_count(&self) -> Result<u64, QueryError> {
        match self.fetch(Key::TotalLineCount)? {
            (Value::Count(count), _) => Ok(count),
            _ => unreachable!("total_line_count produces a Count value"),
        }
    }

    // ------------------------------------------------------------------
    // Cancellation.
    // ------------------------------------------------------------------

    /// Request cooperative cancellation: every query entered from now on
    /// returns [`QueryError::Cancelled`] until [`Database::clear_cancellation`].
    /// Abandoning a computation never corrupts the database — no memo is
    /// written for an unfinished query, and finished sub-results stay valid.
    pub fn request_cancellation(&self) {
        self.cancelled.set(true);
    }

    /// Clear a previously requested cancellation.
    pub fn clear_cancellation(&self) {
        self.cancelled.set(false);
    }

    // ------------------------------------------------------------------
    // Instrumentation.
    // ------------------------------------------------------------------

    /// Instrumentation: descriptions of every derived-query **execution**
    /// (not cache hit or revalidation), in order, since the last
    /// [`Database::clear_executions`]. For tests and tooling; the format is
    /// human-oriented and not a stable API.
    #[must_use]
    pub fn executions(&self) -> Vec<String> {
        self.log.borrow().clone()
    }

    /// Instrumentation: forget the recorded executions.
    pub fn clear_executions(&self) {
        self.log.borrow_mut().clear();
    }

    // ------------------------------------------------------------------
    // Engine.
    // ------------------------------------------------------------------

    /// Fetch a query result, memoizing and revalidating as needed. Returns
    /// the value and the revision at which it last changed.
    fn fetch(&self, key: Key) -> Result<(Value, Revision), QueryError> {
        if self.cancelled.get() {
            return Err(QueryError::Cancelled);
        }
        // The caller (if any) depends on `key`.
        if let Some(frame) = self.frames.borrow_mut().last_mut() {
            frame.push(key);
        }

        if key.is_input() {
            let value = self.read_input(key)?;
            let changed_at = self.input_changed.get(&key).copied().unwrap_or(0);
            return Ok((value, changed_at));
        }

        assert!(
            !self.active.borrow().contains(&key),
            "query cycle detected at {key}",
        );

        // Green path: an existing memo may be reusable.
        let existing = self.memos.borrow().get(&key).cloned();
        if let Some(memo) = &existing {
            if memo.verified_at == self.revision {
                return Ok((memo.value.clone(), memo.changed_at));
            }
            if self.deps_unchanged_since(&memo.deps, memo.verified_at)? {
                let mut memos = self.memos.borrow_mut();
                let memo = memos.get_mut(&key).expect("memo exists on green path");
                memo.verified_at = self.revision;
                return Ok((memo.value.clone(), memo.changed_at));
            }
        }

        // Red path: (re)compute, recording dependencies.
        self.active.borrow_mut().push(key);
        self.frames.borrow_mut().push(Vec::new());
        let result = self.compute(key);
        let deps = self.frames.borrow_mut().pop().expect("own frame present");
        self.active.borrow_mut().pop();
        let value = result?;
        self.log.borrow_mut().push(key.to_string());

        // Early cutoff: an equal value keeps its old changed_at, so
        // dependents of this query are not invalidated.
        let changed_at = match &existing {
            Some(memo) if memo.value == value => memo.changed_at,
            _ => self.revision,
        };
        self.memos.borrow_mut().insert(
            key,
            Memo {
                value: value.clone(),
                changed_at,
                verified_at: self.revision,
                deps,
            },
        );
        Ok((value, changed_at))
    }

    /// Whether every dependency's value is unchanged since `verified_at`.
    /// Fetches happen inside a throwaway frame so revalidation does not leak
    /// transitive dependencies into the caller's frame.
    fn deps_unchanged_since(
        &self,
        deps: &[Key],
        verified_at: Revision,
    ) -> Result<bool, QueryError> {
        self.frames.borrow_mut().push(Vec::new());
        let mut unchanged = true;
        let mut outcome = Ok(());
        for &dep in deps {
            match self.fetch(dep) {
                Ok((_, dep_changed_at)) => {
                    if dep_changed_at > verified_at {
                        unchanged = false;
                        break;
                    }
                }
                Err(error) => {
                    outcome = Err(error);
                    break;
                }
            }
        }
        self.frames.borrow_mut().pop();
        outcome.map(|()| unchanged)
    }

    /// Read an input's current value.
    fn read_input(&self, key: Key) -> Result<Value, QueryError> {
        match key {
            Key::SourceText(file) => match self.sources.latest(file) {
                Some(snapshot) => Ok(Value::Source(Arc::clone(self.sources.source(snapshot)))),
                None => Err(QueryError::NoText { file }),
            },
            Key::FileList => Ok(Value::Files(self.files_with_text.clone())),
            Key::LineCount(_) | Key::TotalLineCount => {
                unreachable!("{key} is a derived query, not an input")
            }
        }
    }

    /// Execute a derived query. Every read goes through [`Database::fetch`]
    /// so it is recorded as a dependency (purity rule 2).
    fn compute(&self, key: Key) -> Result<Value, QueryError> {
        match key {
            Key::LineCount(file) => {
                let (value, _) = self.fetch(Key::SourceText(file))?;
                let Value::Source(text) = value else {
                    unreachable!("source_text produces a Source value")
                };
                Ok(Value::Count(u64::from(text.line_count())))
            }
            Key::TotalLineCount => {
                let (value, _) = self.fetch(Key::FileList)?;
                let Value::Files(files) = value else {
                    unreachable!("file_list produces a Files value")
                };
                let mut total = 0u64;
                for file in files {
                    let (value, _) = self.fetch(Key::LineCount(file))?;
                    let Value::Count(count) = value else {
                        unreachable!("line_count produces a Count value")
                    };
                    total += count;
                }
                Ok(Value::Count(total))
            }
            Key::SourceText(_) | Key::FileList => {
                unreachable!("{key} is an input, not a derived query")
            }
        }
    }
}
