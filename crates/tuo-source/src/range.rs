//! Half-open byte ranges over source text.

use std::error::Error;
use std::fmt;

use crate::ByteOffset;

/// A half-open byte range `[start, end)` in some source text.
///
/// `TextRange` is pure geometry: it does not know which source it refers to
/// (that is [`Span`](crate::Span)) and does not validate against any text
/// (that is [`SourceText::slice`](crate::SourceText::slice)). Its single
/// invariant, enforced at construction, is `start <= end` — a backwards range
/// cannot be represented, so it cannot silently propagate.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TextRange {
    start: ByteOffset,
    end: ByteOffset,
}

/// Error: attempted to construct a backwards [`TextRange`] (`start > end`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct InvalidRange {
    /// The offered start offset.
    pub start: ByteOffset,
    /// The offered end offset, which was less than `start`.
    pub end: ByteOffset,
}

impl fmt::Display for InvalidRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid text range: start {} is past end {}",
            self.start, self.end
        )
    }
}

impl Error for InvalidRange {}

impl TextRange {
    /// Construct the range `[start, end)`, rejecting backwards ranges.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidRange`] if `start > end`.
    pub fn new(
        start: impl Into<ByteOffset>,
        end: impl Into<ByteOffset>,
    ) -> Result<Self, InvalidRange> {
        let (start, end) = (start.into(), end.into());
        if start > end {
            return Err(InvalidRange { start, end });
        }
        Ok(Self { start, end })
    }

    /// The empty range `[offset, offset)`.
    #[must_use]
    pub fn empty(offset: ByteOffset) -> Self {
        Self {
            start: offset,
            end: offset,
        }
    }

    /// The range starting at `start` and covering `len` bytes.
    ///
    /// # Panics
    ///
    /// Panics if the end would overflow the 32-bit offset space.
    #[must_use]
    pub fn at(start: ByteOffset, len: u32) -> Self {
        Self {
            start,
            end: start + len,
        }
    }

    /// The inclusive start offset.
    #[must_use]
    pub const fn start(self) -> ByteOffset {
        self.start
    }

    /// The exclusive end offset.
    #[must_use]
    pub const fn end(self) -> ByteOffset {
        self.end
    }

    /// Length in bytes.
    #[must_use]
    pub fn len(self) -> u32 {
        self.end - self.start
    }

    /// Whether the range covers zero bytes.
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    /// Whether `offset` lies inside the half-open range.
    #[must_use]
    pub fn contains(self, offset: ByteOffset) -> bool {
        self.start <= offset && offset < self.end
    }

    /// Whether `other` lies entirely within `self`.
    #[must_use]
    pub fn contains_range(self, other: Self) -> bool {
        self.start <= other.start && other.end <= self.end
    }

    /// The smallest range covering both `self` and `other`.
    #[must_use]
    pub fn cover(self, other: Self) -> Self {
        Self {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }

    /// The overlap of `self` and `other`, or `None` if they are disjoint.
    /// Touching ranges overlap in an empty range at the shared boundary.
    #[must_use]
    pub fn intersect(self, other: Self) -> Option<Self> {
        let start = self.start.max(other.start);
        let end = self.end.min(other.end);
        (start <= end).then_some(Self { start, end })
    }
}

impl fmt::Display for TextRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..{}", self.start, self.end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(start: u32, end: u32) -> TextRange {
        TextRange::new(start, end).expect("test range must be forward")
    }

    #[test]
    fn backwards_range_is_rejected() {
        let err = TextRange::new(5u32, 4u32).expect_err("must reject");
        assert_eq!(err.start, ByteOffset::new(5));
        assert_eq!(err.end, ByteOffset::new(4));
    }

    #[test]
    fn empty_range_is_allowed_and_contains_nothing() {
        let r = range(3, 3);
        assert!(r.is_empty());
        assert_eq!(r.len(), 0);
        assert!(!r.contains(ByteOffset::new(3)));
    }

    #[test]
    fn containment_is_half_open() {
        let r = range(2, 5);
        assert!(r.contains(ByteOffset::new(2)));
        assert!(r.contains(ByteOffset::new(4)));
        assert!(!r.contains(ByteOffset::new(5)));
    }

    #[test]
    fn cover_and_intersect() {
        let a = range(0, 4);
        let b = range(2, 8);
        assert_eq!(a.cover(b), range(0, 8));
        assert_eq!(a.intersect(b), Some(range(2, 4)));
        assert_eq!(range(0, 1).intersect(range(2, 3)), None);
        // Touching ranges meet in an empty intersection.
        assert_eq!(range(0, 2).intersect(range(2, 3)), Some(range(2, 2)));
    }

    #[test]
    fn contains_range_includes_boundaries() {
        let outer = range(1, 9);
        assert!(outer.contains_range(range(1, 9)));
        assert!(outer.contains_range(range(3, 3)));
        assert!(!outer.contains_range(range(0, 2)));
    }
}
