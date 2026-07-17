//! Spans: a text range anchored to a specific source snapshot.

use std::fmt;

use crate::{SourceId, TextRange};

/// A [`TextRange`] anchored to one immutable source snapshot ([`SourceId`]).
///
/// A span is the unit of location the rest of the compiler passes around and
/// diagnostics render. Because the anchor is a *snapshot* identity — not a
/// file — a span can never silently be interpreted against a different
/// revision of the file than the one it was produced from.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Span {
    source: SourceId,
    range: TextRange,
}

impl Span {
    /// Anchor `range` to `source`.
    #[must_use]
    pub const fn new(source: SourceId, range: TextRange) -> Self {
        Self { source, range }
    }

    /// The source snapshot this span points into.
    #[must_use]
    pub const fn source(self) -> SourceId {
        self.source
    }

    /// The byte range within the source text.
    #[must_use]
    pub const fn range(self) -> TextRange {
        self.range
    }

    /// The smallest span covering both `self` and `other`, or `None` if they
    /// are anchored to different source snapshots (joining across sources is
    /// meaningless and must not silently succeed).
    #[must_use]
    pub fn try_join(self, other: Self) -> Option<Self> {
        (self.source == other.source).then(|| Self {
            source: self.source,
            range: self.range.cover(other.range),
        })
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.source, self.range)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ByteOffset;

    fn range(start: u32, end: u32) -> TextRange {
        TextRange::new(start, end).expect("test range must be forward")
    }

    #[test]
    fn join_within_one_source_covers_both() {
        let src = SourceId::from_raw(0);
        let joined = Span::new(src, range(1, 3))
            .try_join(Span::new(src, range(7, 9)))
            .expect("same source must join");
        assert_eq!(joined.range().start(), ByteOffset::new(1));
        assert_eq!(joined.range().end(), ByteOffset::new(9));
    }

    #[test]
    fn join_across_sources_is_refused() {
        let a = Span::new(SourceId::from_raw(0), range(0, 1));
        let b = Span::new(SourceId::from_raw(1), range(0, 1));
        assert_eq!(a.try_join(b), None);
    }
}
