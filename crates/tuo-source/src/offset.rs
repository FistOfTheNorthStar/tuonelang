//! Byte offsets into source text.

use std::fmt;
use std::ops::{Add, AddAssign, Sub};

/// A position in source text, measured in **UTF-8 bytes** from the start of
/// the text.
///
/// Byte offsets are the only position representation used internally by the
/// compiler; line/column values exist purely for presentation (see
/// [`LineCol`](crate::LineCol)). Offsets are 32-bit, capping a single source
/// text at 4 GiB — [`SourceText::new`](crate::SourceText::new) rejects larger
/// inputs rather than truncating.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct ByteOffset(u32);

impl ByteOffset {
    /// Offset zero: the start of the text.
    pub const ZERO: Self = Self(0);

    /// Construct from a raw byte count.
    #[must_use]
    pub const fn new(raw: u32) -> Self {
        Self(raw)
    }

    /// The raw byte count.
    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self.0
    }

    /// The raw byte count as a `usize`, for indexing.
    #[must_use]
    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }

    /// Checked conversion from a `usize` byte count; `None` if it exceeds the
    /// 32-bit offset space.
    #[must_use]
    pub fn try_from_usize(raw: usize) -> Option<Self> {
        u32::try_from(raw).ok().map(Self)
    }

    /// Checked addition; `None` on overflow of the offset space.
    #[must_use]
    pub fn checked_add(self, bytes: u32) -> Option<Self> {
        self.0.checked_add(bytes).map(Self)
    }
}

impl Add<u32> for ByteOffset {
    type Output = Self;

    /// Advance the offset by `bytes`.
    ///
    /// # Panics
    ///
    /// Panics on overflow of the 32-bit offset space (use
    /// [`ByteOffset::checked_add`] to handle it).
    fn add(self, bytes: u32) -> Self {
        self.checked_add(bytes).expect("byte offset overflowed u32")
    }
}

impl AddAssign<u32> for ByteOffset {
    fn add_assign(&mut self, bytes: u32) {
        *self = *self + bytes;
    }
}

impl Sub for ByteOffset {
    type Output = u32;

    /// The distance in bytes between two offsets.
    ///
    /// # Panics
    ///
    /// Panics if `rhs` is past `self`; a negative distance is always a logic
    /// error, never a value to compute with.
    fn sub(self, rhs: Self) -> u32 {
        self.0
            .checked_sub(rhs.0)
            .expect("subtracted a byte offset past the left operand")
    }
}

impl From<u32> for ByteOffset {
    fn from(raw: u32) -> Self {
        Self(raw)
    }
}

impl fmt::Display for ByteOffset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic_and_ordering() {
        let a = ByteOffset::new(3);
        let b = a + 4;
        assert_eq!(b.as_u32(), 7);
        assert_eq!(b - a, 4);
        assert!(a < b);
    }

    #[test]
    fn checked_add_reports_overflow() {
        assert_eq!(ByteOffset::new(u32::MAX).checked_add(1), None);
    }

    #[test]
    #[should_panic(expected = "past the left operand")]
    fn negative_distance_panics() {
        let _ = ByteOffset::new(1) - ByteOffset::new(2);
    }
}
