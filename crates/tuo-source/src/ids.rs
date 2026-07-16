//! Strong identity types: files, source snapshots, and revisions.

use std::fmt;

/// Identity of a source **file** (a path/name), stable across edits.
///
/// A `FileId` never changes when the file's contents change; the contents are
/// identified by [`SourceId`] snapshots. Allocate `FileId`s through
/// [`SourceMap::intern_file`](crate::SourceMap::intern_file).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct FileId(u32);

impl FileId {
    /// Construct from a raw index. Intended for the source map and for
    /// serialization boundaries; ordinary code receives `FileId`s from a
    /// [`SourceMap`](crate::SourceMap).
    #[must_use]
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    /// The raw index.
    #[must_use]
    pub const fn as_raw(self) -> u32 {
        self.0
    }
}

impl fmt::Display for FileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "file#{}", self.0)
    }
}

/// Identity of one **immutable snapshot** of a file's text.
///
/// Every edit to a file produces a fresh `SourceId` (at the file's next
/// [`Revision`]); a `SourceId` therefore always denotes exactly one text,
/// forever. Spans carry a `SourceId`, so a span can never accidentally be
/// resolved against a different revision than the one it was produced from —
/// this is the anchor later incremental compilation relies on.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct SourceId(u32);

impl SourceId {
    /// Construct from a raw index. Intended for the source map and for
    /// serialization boundaries; ordinary code receives `SourceId`s from a
    /// [`SourceMap`](crate::SourceMap).
    #[must_use]
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    /// The raw index.
    #[must_use]
    pub const fn as_raw(self) -> u32 {
        self.0
    }
}

impl fmt::Display for SourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "source#{}", self.0)
    }
}

/// Monotonic revision counter for the snapshots of a **single file**.
///
/// The first snapshot of a file is [`Revision::INITIAL`]; each subsequent
/// snapshot gets the next revision. Revisions of different files are not
/// comparable in any meaningful way.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Revision(u32);

impl Revision {
    /// The revision of a file's first snapshot.
    pub const INITIAL: Self = Self(0);

    /// The revision immediately after `self`.
    ///
    /// # Panics
    ///
    /// Panics on revision-counter overflow (after 2³² − 1 edits of one file),
    /// rather than silently reusing an identity.
    #[must_use]
    pub fn next(self) -> Self {
        Self(
            self.0
                .checked_add(1)
                .expect("revision counter overflowed u32"),
        )
    }

    /// The raw counter value.
    #[must_use]
    pub const fn as_raw(self) -> u32 {
        self.0
    }
}

impl fmt::Display for Revision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "rev{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revisions_are_ordered_and_advance() {
        let r0 = Revision::INITIAL;
        let r1 = r0.next();
        assert!(r0 < r1);
        assert_eq!(r1.as_raw(), 1);
    }

    #[test]
    fn ids_round_trip_raw_values() {
        assert_eq!(FileId::from_raw(7).as_raw(), 7);
        assert_eq!(SourceId::from_raw(9).as_raw(), 9);
    }
}
