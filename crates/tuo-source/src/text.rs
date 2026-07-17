//! Immutable source-text snapshots with line/column mapping.

use std::error::Error;
use std::fmt;
use std::sync::Arc;

use crate::{ByteOffset, FileId, Revision, SourceId, TextRange};

/// One immutable snapshot of a file's text, with a precomputed line index.
///
/// The text is any valid Unicode (`str` guarantees UTF-8); no tuonelang
/// lexical policy is applied here. The snapshot is never mutated — an edit
/// produces a *new* `SourceText` under a new [`SourceId`] and [`Revision`].
/// Both the text (`Arc<str>`) and the snapshot itself (handed out as
/// `Arc<SourceText>` by [`SourceMap`](crate::SourceMap)) are cheap to clone
/// and share across threads.
#[derive(Clone, Debug)]
pub struct SourceText {
    source: SourceId,
    file: FileId,
    revision: Revision,
    text: Arc<str>,
    /// Byte offset of the first byte of every line: `line_starts[0] == 0`,
    /// and one entry after every `\n`. (A `\r` is only treated as part of a
    /// line terminator when a `\n` follows it; a lone `\r` does not start a
    /// new line.)
    line_starts: Vec<ByteOffset>,
}

/// Error: a source text exceeded the 32-bit byte-offset space (4 GiB) and was
/// rejected rather than truncated.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SourceTooLarge {
    /// The offered text's length in bytes.
    pub len: usize,
}

impl fmt::Display for SourceTooLarge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "source text of {} bytes exceeds the 32-bit offset space",
            self.len
        )
    }
}

impl Error for SourceTooLarge {}

/// Error: an offset, range, or line could not be located within a
/// [`SourceText`]. Locating never silently clamps or wraps.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LocateError {
    /// The offset lies past the end of the text.
    OutOfBounds {
        /// The offending offset.
        offset: ByteOffset,
        /// The text length in bytes.
        len: u32,
    },
    /// The offset points into the middle of a multibyte UTF-8 sequence.
    NotCharBoundary {
        /// The offending offset.
        offset: ByteOffset,
    },
    /// The line index is not a line of this text.
    LineOutOfBounds {
        /// The offending zero-based line index.
        line: u32,
        /// The number of lines in the text.
        line_count: u32,
    },
}

impl fmt::Display for LocateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfBounds { offset, len } => {
                write!(f, "offset {offset} is out of bounds (text is {len} bytes)")
            }
            Self::NotCharBoundary { offset } => {
                write!(f, "offset {offset} is not a UTF-8 character boundary")
            }
            Self::LineOutOfBounds { line, line_count } => {
                write!(
                    f,
                    "line {line} is out of bounds (text has {line_count} lines)"
                )
            }
        }
    }
}

impl Error for LocateError {}

/// A **presentation-only** line/column position.
///
/// Both fields are zero-based; [`Display`](fmt::Display) renders the
/// conventional one-based `line:column`. `column` counts Unicode scalar
/// values from the line start — not bytes, and not display width. Internal
/// compiler positions are always [`ByteOffset`]s; convert to `LineCol` only
/// at the presentation edge.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct LineCol {
    /// Zero-based line index.
    pub line: u32,
    /// Zero-based column, in Unicode scalar values from the line start.
    pub column: u32,
}

impl fmt::Display for LineCol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line + 1, self.column + 1)
    }
}

impl SourceText {
    /// Build a snapshot from its identity and text, computing the line index.
    ///
    /// # Errors
    ///
    /// Returns [`SourceTooLarge`] if the text does not fit the 32-bit offset
    /// space.
    pub fn new(
        source: SourceId,
        file: FileId,
        revision: Revision,
        text: Arc<str>,
    ) -> Result<Self, SourceTooLarge> {
        if u32::try_from(text.len()).is_err() {
            return Err(SourceTooLarge { len: text.len() });
        }
        let mut line_starts = vec![ByteOffset::ZERO];
        for (i, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                // `i` fits u32: text.len() was checked above. `+ 1` cannot
                // overflow because a `\n` at the last valid u32 offset is
                // impossible (the length check would have failed first).
                #[allow(clippy::cast_possible_truncation)]
                line_starts.push(ByteOffset::new(i as u32 + 1));
            }
        }
        Ok(Self {
            source,
            file,
            revision,
            text,
            line_starts,
        })
    }

    /// The snapshot identity spans anchor to.
    #[must_use]
    pub const fn source(&self) -> SourceId {
        self.source
    }

    /// The file this snapshot belongs to.
    #[must_use]
    pub const fn file(&self) -> FileId {
        self.file
    }

    /// This snapshot's revision within its file.
    #[must_use]
    pub const fn revision(&self) -> Revision {
        self.revision
    }

    /// The full text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// A cheap shared handle to the full text.
    #[must_use]
    pub fn shared_text(&self) -> Arc<str> {
        Arc::clone(&self.text)
    }

    /// Length of the text in bytes.
    #[must_use]
    pub fn len(&self) -> u32 {
        // Guaranteed to fit by the constructor's length check.
        #[allow(clippy::cast_possible_truncation)]
        {
            self.text.len() as u32
        }
    }

    /// Whether the text is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// The range covering the entire text.
    #[must_use]
    pub fn full_range(&self) -> TextRange {
        TextRange::at(ByteOffset::ZERO, self.len())
    }

    /// Number of lines. Every text has at least one line; each `\n` starts
    /// another, so `"a\n"` has two lines (the second empty).
    #[must_use]
    pub fn line_count(&self) -> u32 {
        // Bounded by the number of bytes + 1, which fits u32.
        #[allow(clippy::cast_possible_truncation)]
        {
            self.line_starts.len() as u32
        }
    }

    /// Validate that `offset` is inside the text (end inclusive) and on a
    /// UTF-8 character boundary.
    ///
    /// # Errors
    ///
    /// Returns [`LocateError::OutOfBounds`] or [`LocateError::NotCharBoundary`].
    pub fn check_offset(&self, offset: ByteOffset) -> Result<(), LocateError> {
        if offset.as_u32() > self.len() {
            return Err(LocateError::OutOfBounds {
                offset,
                len: self.len(),
            });
        }
        if !self.text.is_char_boundary(offset.as_usize()) {
            return Err(LocateError::NotCharBoundary { offset });
        }
        Ok(())
    }

    /// Map a byte offset to a presentation [`LineCol`].
    ///
    /// The end-of-text offset is valid and maps to the position just past the
    /// last character. An offset pointing at the `\r` of a `\r\n` terminator
    /// maps to the column after the line's visible content.
    ///
    /// # Errors
    ///
    /// Returns [`LocateError`] for offsets past the end of the text or inside
    /// a multibyte UTF-8 sequence — invalid positions never map silently.
    pub fn line_col(&self, offset: ByteOffset) -> Result<LineCol, LocateError> {
        self.check_offset(offset)?;
        let line_idx = self.line_starts.partition_point(|&start| start <= offset) - 1;
        let line_start = self.line_starts[line_idx];
        let column = self.text[line_start.as_usize()..offset.as_usize()]
            .chars()
            .count();
        // Both fit u32: bounded by the u32 text length.
        #[allow(clippy::cast_possible_truncation)]
        Ok(LineCol {
            line: line_idx as u32,
            column: column as u32,
        })
    }

    /// The range of a line's **content**, excluding its terminator (`\n` or
    /// `\r\n` — both are handled; this is what makes CRLF sources render
    /// correctly without a stray `\r` in the excerpt).
    ///
    /// # Errors
    ///
    /// Returns [`LocateError::LineOutOfBounds`] for a line index past the
    /// last line.
    pub fn line_range(&self, line: u32) -> Result<TextRange, LocateError> {
        let Some(&start) = self.line_starts.get(line as usize) else {
            return Err(LocateError::LineOutOfBounds {
                line,
                line_count: self.line_count(),
            });
        };
        let end = match self.line_starts.get(line as usize + 1) {
            // The next line starts after this line's `\n`; also strip a
            // preceding `\r` so CRLF content ends before the terminator.
            Some(&next_start) => {
                let mut end = next_start.as_u32() - 1;
                if end > start.as_u32() && self.text.as_bytes()[end as usize - 1] == b'\r' {
                    end -= 1;
                }
                ByteOffset::new(end)
            }
            // Final line without a newline: runs to the end of the text.
            None => ByteOffset::new(self.len()),
        };
        Ok(TextRange::new(start, end).expect("line starts precede line ends"))
    }

    /// Slice the text at `range`.
    ///
    /// # Errors
    ///
    /// Returns [`LocateError`] if either endpoint is past the end of the text
    /// or not on a UTF-8 character boundary — an invalid range never yields a
    /// (wrong) string.
    pub fn slice(&self, range: TextRange) -> Result<&str, LocateError> {
        self.check_offset(range.start())?;
        self.check_offset(range.end())?;
        Ok(&self.text[range.start().as_usize()..range.end().as_usize()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> SourceText {
        SourceText::new(
            SourceId::from_raw(0),
            FileId::from_raw(0),
            Revision::INITIAL,
            Arc::from(s),
        )
        .expect("test text must fit the offset space")
    }

    #[test]
    fn empty_text_has_one_empty_line() {
        let t = text("");
        assert!(t.is_empty());
        assert_eq!(t.line_count(), 1);
        assert_eq!(
            t.line_col(ByteOffset::ZERO),
            Ok(LineCol { line: 0, column: 0 })
        );
        let line = t.line_range(0).expect("line 0 must exist");
        assert_eq!(t.slice(line), Ok(""));
    }

    #[test]
    fn sharing_text_is_cheap_and_identical() {
        let t = text("fn main() {}");
        let a = t.shared_text();
        let b = t.shared_text();
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(&*a, t.text());
    }
}
