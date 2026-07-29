//! The database: inputs, memoized queries, and the red-green engine facade.
//!
//! Everything in this module is TDG-owned. The reusable engine lives in
//! [`crate::engine`]; this [`Database`] is a thin **typed facade** over it — the
//! in-crate worked example of a host implementing [`QueryHost`]. It exposes the
//! source input plus two small derived queries (`line_count`,
//! `total_line_count`) that exercise memoization, precise invalidation, early
//! cutoff, and cancellation end to end. Real compiler-stage queries live in a
//! host that may depend on the stage crates (`tuo-compiler`), never here: the
//! public surface speaks [`tuo_source`] identities, plain values, and
//! [`QueryError`].

use std::sync::Arc;

use tuo_source::{FileId, SourceId, SourceMap, SourceText, SourceTooLarge};

use crate::QueryError;
use crate::engine::{Computed, QueryCtx, QueryEngine, QueryHost, QueryKey, downcast, stored};

/// Query-kind tags for this facade's queries (see [`QueryKey`]).
mod kind {
    /// Input: the current text snapshot of a file (arg = `FileId` raw).
    pub(super) const SOURCE_TEXT: u32 = 0;
    /// Input: the files that currently have text (arg = 0).
    pub(super) const FILE_LIST: u32 = 1;
    /// Derived: number of lines in a file (arg = `FileId` raw).
    pub(super) const LINE_COUNT: u32 = 2;
    /// Derived: total number of lines across all files (arg = 0).
    pub(super) const TOTAL_LINE_COUNT: u32 = 3;
}

/// A line count stored as a derived-query result.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct CountValue(u64);

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
    /// The generic incremental engine backing every query.
    engine: QueryEngine,
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
        self.engine.bump_input(source_key(file));
        if first_text {
            self.files_with_text.push(file);
            self.engine.bump_input(file_list_key());
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
        if self.engine.is_cancelled() {
            return Err(QueryError::Cancelled);
        }
        self.source_input(file)
    }

    /// Query: the number of lines in `file` (as defined by
    /// [`SourceText::line_count`]).
    ///
    /// # Errors
    ///
    /// [`QueryError::NoText`] if the file has no text; [`QueryError::Cancelled`]
    /// if cancellation was requested.
    pub fn line_count(&self, file: FileId) -> Result<u64, QueryError> {
        let value = self.engine.query(line_count_key(file), self)?;
        Ok(downcast::<CountValue>(&value)
            .expect("line_count stores a count")
            .0)
    }

    /// Query: the total number of lines across every file with text.
    ///
    /// # Errors
    ///
    /// [`QueryError::Cancelled`] if cancellation was requested.
    pub fn total_line_count(&self) -> Result<u64, QueryError> {
        let value = self.engine.query(total_line_count_key(), self)?;
        Ok(downcast::<CountValue>(&value)
            .expect("total stores a count")
            .0)
    }

    // ------------------------------------------------------------------
    // Cancellation.
    // ------------------------------------------------------------------

    /// Request cooperative cancellation: every query entered from now on
    /// returns [`QueryError::Cancelled`] until [`Database::clear_cancellation`].
    /// Abandoning a computation never corrupts the database — no memo is written
    /// for an unfinished query, and finished sub-results stay valid.
    pub fn request_cancellation(&self) {
        self.engine.request_cancellation();
    }

    /// Clear a previously requested cancellation.
    pub fn clear_cancellation(&self) {
        self.engine.clear_cancellation();
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
        self.engine.executions()
    }

    /// Instrumentation: forget the recorded executions.
    pub fn clear_executions(&self) {
        self.engine.clear_executions();
    }

    // ------------------------------------------------------------------
    // Facade helpers.
    // ------------------------------------------------------------------

    /// Read the source snapshot input for `file` directly.
    fn source_input(&self, file: FileId) -> Result<Arc<SourceText>, QueryError> {
        match self.sources.latest(file) {
            Some(snapshot) => Ok(Arc::clone(self.sources.source(snapshot))),
            None => Err(QueryError::NoText { file }),
        }
    }
}

impl QueryHost for Database {
    fn compute(&self, key: QueryKey, ctx: &QueryCtx<'_>) -> Result<Computed, QueryError> {
        match key.kind() {
            kind::LINE_COUNT => {
                let file = FileId::from_raw(u32::try_from(key.arg()).expect("file id fits"));
                let _ = ctx.depend_on_input(source_key(file))?;
                let text = self.source_input(file)?;
                Ok(Computed::new(
                    stored(CountValue(u64::from(text.line_count()))),
                    format!("line_count(file#{})", file.as_raw()),
                ))
            }
            kind::TOTAL_LINE_COUNT => {
                let _ = ctx.depend_on_input(file_list_key())?;
                let mut total = 0u64;
                for &file in &self.files_with_text {
                    let value = ctx.query(line_count_key(file))?;
                    total += downcast::<CountValue>(&value)
                        .expect("line_count stores a count")
                        .0;
                }
                Ok(Computed::new(
                    stored(CountValue(total)),
                    "total_line_count()".to_owned(),
                ))
            }
            other => unreachable!("Database has no derived query of kind {other}"),
        }
    }
}

// ----------------------------------------------------------------------
// Key constructors.
// ----------------------------------------------------------------------

fn source_key(file: FileId) -> QueryKey {
    QueryKey::new(kind::SOURCE_TEXT, u64::from(file.as_raw()))
}

fn file_list_key() -> QueryKey {
    QueryKey::new(kind::FILE_LIST, 0)
}

fn line_count_key(file: FileId) -> QueryKey {
    QueryKey::new(kind::LINE_COUNT, u64::from(file.as_raw()))
}

fn total_line_count_key() -> QueryKey {
    QueryKey::new(kind::TOTAL_LINE_COUNT, 0)
}
