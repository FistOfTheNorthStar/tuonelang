//! Source positions on the wire, and their conversion to compiler spans.
//!
//! The compiler's native position is a UTF-8 [`ByteOffset`](tuo_source::ByteOffset).
//! An LLM agent, though, reasons in the terms it already has — a byte offset it
//! computed, or a `line:column` it read from a diagnostic. So the wire
//! [`Position`] carries **both**: a request may anchor by `offset` *or* by
//! `line`+`column` (offset wins when both are present), and every position the
//! server *returns* is fully populated (all three), so a client never has to
//! convert. Lines and columns are **one-based** to match how the compiler's
//! human diagnostics and `tuo`'s `file:line:col` render — the form an LLM has
//! seen in the tool's own output.
//!
//! This is the single place wire positions meet compiler offsets; keeping the
//! translation here mirrors the LSP's `convert` seam.

use serde::{Deserialize, Serialize};
use tuo_source::{ByteOffset, SourceText, Span};

/// A position in a document, over-specified so it round-trips without loss.
///
/// On the way **in**, a client may set just `offset`, or just `line`+`column`;
/// [`Position::to_offset`] resolves either. On the way **out**, the server sets
/// all three so the client can use whichever it prefers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct Position {
    /// The zero-based UTF-8 byte offset. Authoritative when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u32>,
    /// The one-based line (as in `file:line:col`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// The one-based column, counted in Unicode scalar values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
}

impl Position {
    /// Resolve this wire position to a byte offset in `text`.
    ///
    /// `offset` is authoritative when set; otherwise `line`+`column` (one-based)
    /// are converted. A position past the end of the text clamps to its end, and
    /// a missing/short line or column clamps within the line — the server never
    /// fails a request over an out-of-range position, it resolves the nearest
    /// valid one.
    #[must_use]
    pub fn to_offset(self, text: &SourceText) -> ByteOffset {
        let len = u32::try_from(text.text().len()).unwrap_or(u32::MAX);
        if let Some(offset) = self.offset {
            return ByteOffset::new(offset.min(len));
        }
        let line = self.line.unwrap_or(1).max(1);
        let column = self.column.unwrap_or(1).max(1);
        // Walk to the start of the requested line.
        let source = text.text();
        let mut line_start = 0usize;
        let mut current = 1u32;
        while current < line {
            match source[line_start..].find('\n') {
                Some(rel) => {
                    line_start += rel + 1;
                    current += 1;
                }
                // Fewer lines than requested: clamp to the last line's content.
                None => return ByteOffset::new(len),
            }
        }
        // Advance `column - 1` Unicode scalars into the line, stopping at its
        // newline or the end of the text.
        let mut offset = line_start;
        let remaining = column - 1;
        for (stepped, (idx, ch)) in source[line_start..].char_indices().enumerate() {
            if ch == '\n' || u32::try_from(stepped).unwrap_or(u32::MAX) >= remaining {
                offset = line_start + idx;
                return ByteOffset::new(u32::try_from(offset).unwrap_or(len));
            }
            offset = line_start + idx + ch.len_utf8();
        }
        ByteOffset::new(u32::try_from(offset).unwrap_or(len).min(len))
    }

    /// Build a fully-populated wire position for `offset` in `text` (all of
    /// `offset`, `line`, `column` set). Used for every position the server
    /// returns.
    ///
    /// The compiler's [`LineCol`](tuo_source::LineCol) is zero-based; the wire
    /// is **one-based** (matching `file:line:col`), so both are shifted by one.
    #[must_use]
    pub fn of_offset(text: &SourceText, offset: ByteOffset) -> Self {
        let line_col = text.line_col(offset).ok();
        Self {
            offset: Some(offset.as_u32()),
            line: line_col.map(|lc| lc.line + 1),
            column: line_col.map(|lc| lc.column + 1),
        }
    }
}

/// A half-open source range `[start, end)`, echoing the compiler's span model.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct Range {
    /// The inclusive start position.
    pub start: Position,
    /// The exclusive end position.
    pub end: Position,
}

impl Range {
    /// The wire range for `span`'s byte range within `text`.
    #[must_use]
    pub fn of_span(text: &SourceText, span: Span) -> Self {
        Self {
            start: Position::of_offset(text, span.range().start()),
            end: Position::of_offset(text, span.range().end()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tuo_source::SourceMap;

    fn text(src: &str) -> (SourceMap, tuo_source::SourceId) {
        let mut map = SourceMap::new();
        let file = map.intern_file("t.tuo");
        let id = map.add_source(file, src).expect("fits");
        (map, id)
    }

    #[test]
    fn offset_is_authoritative() {
        let (map, id) = text("abc\ndef");
        let pos = Position {
            offset: Some(5),
            line: Some(99),
            column: Some(99),
        };
        assert_eq!(pos.to_offset(map.source(id)).as_u32(), 5);
    }

    #[test]
    fn line_column_is_one_based() {
        let (map, id) = text("abc\ndef");
        // line 2, column 1 → offset of 'd' == 4.
        let pos = Position {
            offset: None,
            line: Some(2),
            column: Some(1),
        };
        assert_eq!(pos.to_offset(map.source(id)).as_u32(), 4);
    }

    #[test]
    fn out_of_range_clamps() {
        let (map, id) = text("abc");
        let pos = Position {
            offset: Some(999),
            line: None,
            column: None,
        };
        assert_eq!(pos.to_offset(map.source(id)).as_u32(), 3);
    }

    #[test]
    fn of_offset_populates_all_three() {
        let (map, id) = text("abc\ndef");
        let pos = Position::of_offset(map.source(id), ByteOffset::new(4));
        assert_eq!(pos.offset, Some(4));
        assert_eq!(pos.line, Some(2));
        assert_eq!(pos.column, Some(1));
    }

    #[test]
    fn columns_count_unicode_scalars() {
        // "𝔸b" — the astral char is 4 UTF-8 bytes; column 2 is 'b'.
        let (map, id) = text("𝔸b");
        let pos = Position {
            offset: None,
            line: Some(1),
            column: Some(2),
        };
        assert_eq!(pos.to_offset(map.source(id)).as_u32(), 4);
    }
}
