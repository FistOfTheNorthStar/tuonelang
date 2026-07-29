//! The presentation edge: converting between the compiler's UTF-8 byte
//! positions and the Language Server Protocol's UTF-16 line/character
//! positions.
//!
//! Two encodings meet here and nowhere else. Internally the compiler locates
//! everything with [`ByteOffset`]s and [`Span`]s over UTF-8 text; the LSP wire
//! format ([`Position`], [`Range`]) is zero-based lines with **UTF-16 code
//! unit** columns. [`tuo_source::SourceText::line_col`] already maps a byte
//! offset to a zero-based line and a *Unicode scalar* column — correct for the
//! line but not for the column, which LSP wants in UTF-16 units. So this module
//! computes the column itself by counting UTF-16 code units along the line, and
//! provides the reverse (position → byte offset) that the source layer does not
//! expose.
//!
//! Every conversion is total and lossless for valid inputs and clamps
//! out-of-range inputs to the nearest valid position rather than failing — an
//! editor can send a cursor one past the end of a line, and a stale position
//! must map somewhere sane rather than drop the request.

use tuo_source::{ByteOffset, SourceMap, SourceText, Span, TextRange};

use crate::wire::{Position, Range};

/// Map a byte `offset` within `text` to an LSP [`Position`] (zero-based line,
/// UTF-16 column). An offset past the end of the text clamps to the end.
#[must_use]
pub fn position_of_offset(text: &SourceText, offset: ByteOffset) -> Position {
    let clamped = offset.as_u32().min(text.len());
    // The line index (and its start) come from the source layer's own line
    // index; only the column needs re-deriving in UTF-16. A clamped offset is
    // always in bounds, so `line_col` succeeds; treat any residual error
    // (a non-char-boundary offset an editor should never send) as line 0.
    let line = text
        .line_col(ByteOffset::new(clamped))
        .map_or(0, |line_col| line_col.line);
    let line_start = text
        .line_range(line)
        .map_or(clamped, |range| range.start().as_u32());
    let source = text.text();
    // Count UTF-16 code units from the line start up to the offset.
    let column: u32 = source
        .get(line_start as usize..clamped as usize)
        .unwrap_or("")
        .chars()
        .map(|c| c.len_utf16() as u32)
        .sum();
    Position::new(line, column)
}

/// Map an LSP [`Position`] to a byte offset within `text`, clamping a position
/// past the end of its line (or the text) to the nearest valid boundary.
#[must_use]
pub fn offset_of_position(text: &SourceText, position: Position) -> ByteOffset {
    let source = text.text();
    // Resolve the line's content range; a line index past the end clamps to the
    // final line's end.
    let Ok(line_range) = text.line_range(position.line) else {
        return ByteOffset::new(text.len());
    };
    let line_start = line_range.start().as_u32() as usize;
    let line_end = line_range.end().as_u32() as usize;
    let line = source.get(line_start..line_end).unwrap_or("");

    // Walk the line counting UTF-16 code units until we reach the requested
    // character, then take that byte position.
    let mut utf16 = 0u32;
    for (byte_idx, ch) in line.char_indices() {
        if utf16 >= position.character {
            return ByteOffset::new(line_start as u32 + byte_idx as u32);
        }
        utf16 += ch.len_utf16() as u32;
    }
    // The character is at or past the end of the line's content.
    ByteOffset::new(line_end as u32)
}

/// Map a [`Span`] to an LSP [`Range`] using the map the span is anchored to.
/// Returns `None` if the span's source snapshot is not in `map`.
#[must_use]
pub fn range_of_span(map: &SourceMap, span: Span) -> Option<Range> {
    let text = map.get_source(span.source())?;
    Some(range_within(text, span.range()))
}

/// Map a byte [`TextRange`] within `text` to an LSP [`Range`].
#[must_use]
pub fn range_within(text: &SourceText, range: TextRange) -> Range {
    Range::new(
        position_of_offset(text, range.start()),
        position_of_offset(text, range.end()),
    )
}

/// The file name of a span's source snapshot, or `None` if unknown.
#[must_use]
pub fn uri_of_span(map: &SourceMap, span: Span) -> Option<String> {
    let text = map.get_source(span.source())?;
    Some(map.file_name(text.file()).to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(text: &str) -> (SourceMap, tuo_source::SourceId) {
        let mut map = SourceMap::new();
        let file = map.intern_file("t.tuo");
        let id = map.add_source(file, text).expect("fits");
        (map, id)
    }

    #[test]
    fn offset_and_position_round_trip_on_ascii() {
        let (map, id) = source("fn a()\nlet b = 1\n");
        let text = map.source(id);
        // Walk every char boundary and confirm offset → position → offset.
        for (byte, _) in text.text().char_indices() {
            let offset = ByteOffset::new(byte as u32);
            let position = position_of_offset(text, offset);
            assert_eq!(
                offset_of_position(text, position),
                offset,
                "round trip at byte {byte}"
            );
        }
    }

    #[test]
    fn columns_are_counted_in_utf16_code_units() {
        // `𝔸` is one scalar but two UTF-16 code units; a following `x` must be
        // at character 2, not 1.
        let (map, id) = source("𝔸x\n");
        let text = map.source(id);
        let x_byte = text.text().find('x').unwrap() as u32;
        let position = position_of_offset(text, ByteOffset::new(x_byte));
        assert_eq!(position, Position::new(0, 2), "𝔸 spans two UTF-16 units");
        // And the reverse maps character 2 back to the byte of `x`.
        assert_eq!(
            offset_of_position(text, Position::new(0, 2)),
            ByteOffset::new(x_byte)
        );
    }

    #[test]
    fn positions_are_zero_based_across_lines() {
        let (map, id) = source("a\nbb\nccc\n");
        let text = map.source(id);
        let second_c = text.text().rfind('c').unwrap() as u32;
        // The last `c` is on line 2, character 2.
        assert_eq!(
            position_of_offset(text, ByteOffset::new(second_c)),
            Position::new(2, 2)
        );
    }

    #[test]
    fn out_of_range_positions_clamp() {
        let (map, id) = source("ab\n");
        let text = map.source(id);
        // A character past the line's end clamps to the line's content end.
        let offset = offset_of_position(text, Position::new(0, 99));
        assert_eq!(offset, ByteOffset::new(2));
        // A line past the end clamps to the text's end.
        let past = offset_of_position(text, Position::new(50, 0));
        assert_eq!(past, ByteOffset::new(text.len()));
    }
}
