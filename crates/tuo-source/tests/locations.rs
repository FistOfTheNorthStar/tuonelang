//! Integration tests for `tuo-source`: line/column mapping, CRLF vs LF,
//! multibyte UTF-8, empty files, missing final newlines, and span boundaries.

use std::sync::Arc;

use tuo_source::{
    ByteOffset, FileId, LineCol, LocateError, Revision, SourceId, SourceMap, SourceText, Span,
    TextRange,
};

fn text(s: &str) -> SourceText {
    SourceText::new(
        SourceId::from_raw(0),
        FileId::from_raw(0),
        Revision::INITIAL,
        Arc::from(s),
    )
    .expect("test text must fit the offset space")
}

fn range(start: u32, end: u32) -> TextRange {
    TextRange::new(start, end).expect("test range must be forward")
}

fn lc(line: u32, column: u32) -> LineCol {
    LineCol { line, column }
}

// --- ASCII ------------------------------------------------------------

#[test]
fn ascii_line_col_mapping() {
    // offsets: a=0 b=1 \n=2 | c=3 d=4 \n=5 | e=6
    let t = text("ab\ncd\ne");
    assert_eq!(t.line_count(), 3);
    assert_eq!(t.line_col(ByteOffset::new(0)), Ok(lc(0, 0)));
    assert_eq!(t.line_col(ByteOffset::new(1)), Ok(lc(0, 1)));
    // The `\n` itself belongs to the line it terminates.
    assert_eq!(t.line_col(ByteOffset::new(2)), Ok(lc(0, 2)));
    assert_eq!(t.line_col(ByteOffset::new(3)), Ok(lc(1, 0)));
    assert_eq!(t.line_col(ByteOffset::new(6)), Ok(lc(2, 0)));
    // End-of-text is a valid position (where the cursor goes after `e`).
    assert_eq!(t.line_col(ByteOffset::new(7)), Ok(lc(2, 1)));
}

#[test]
fn ascii_line_ranges_and_slices() {
    let t = text("ab\ncd\ne");
    assert_eq!(t.slice(t.line_range(0).expect("line 0")), Ok("ab"));
    assert_eq!(t.slice(t.line_range(1).expect("line 1")), Ok("cd"));
    assert_eq!(t.slice(t.line_range(2).expect("line 2")), Ok("e"));
    assert_eq!(
        t.line_range(3),
        Err(LocateError::LineOutOfBounds {
            line: 3,
            line_count: 3
        })
    );
}

// --- Multibyte UTF-8 --------------------------------------------------

#[test]
fn multibyte_columns_count_scalars_not_bytes() {
    // "ä" = 2 bytes, "€" = 3 bytes, "𝄞" = 4 bytes.
    // Bytes:  ä: 0..2, €: 2..5, 𝄞: 5..9, x: 9..10
    let t = text("ä€𝄞x");
    assert_eq!(t.line_col(ByteOffset::new(0)), Ok(lc(0, 0)));
    assert_eq!(t.line_col(ByteOffset::new(2)), Ok(lc(0, 1)));
    assert_eq!(t.line_col(ByteOffset::new(5)), Ok(lc(0, 2)));
    assert_eq!(t.line_col(ByteOffset::new(9)), Ok(lc(0, 3)));
    assert_eq!(t.line_col(ByteOffset::new(10)), Ok(lc(0, 4)));
}

#[test]
fn offsets_inside_multibyte_sequences_are_rejected() {
    let t = text("ä€𝄞x");
    for bad in [1u32, 3, 4, 6, 7, 8] {
        assert_eq!(
            t.line_col(ByteOffset::new(bad)),
            Err(LocateError::NotCharBoundary {
                offset: ByteOffset::new(bad)
            }),
            "offset {bad} splits a scalar and must not map",
        );
    }
}

#[test]
fn multibyte_lines_map_and_slice_correctly() {
    // Bytes: "päivää" = p ä(2) i v ä(2) ä(2) = 9 bytes, \n at 9,
    //        "yö" = y ö(2) at 10..13.
    let t = text("päivää\nyö");
    assert_eq!(t.line_count(), 2);
    assert_eq!(t.slice(t.line_range(0).expect("line 0")), Ok("päivää"));
    assert_eq!(t.slice(t.line_range(1).expect("line 1")), Ok("yö"));
    assert_eq!(t.line_col(ByteOffset::new(10)), Ok(lc(1, 0)));
    assert_eq!(t.line_col(ByteOffset::new(13)), Ok(lc(1, 2)));
}

// --- Empty files -------------------------------------------------------

#[test]
fn empty_file_has_one_empty_line_and_only_offset_zero() {
    let t = text("");
    assert!(t.is_empty());
    assert_eq!(t.line_count(), 1);
    assert_eq!(t.full_range(), range(0, 0));
    assert_eq!(t.line_col(ByteOffset::ZERO), Ok(lc(0, 0)));
    assert_eq!(t.slice(t.line_range(0).expect("line 0")), Ok(""));
    assert_eq!(
        t.line_col(ByteOffset::new(1)),
        Err(LocateError::OutOfBounds {
            offset: ByteOffset::new(1),
            len: 0
        })
    );
}

// --- CRLF vs LF --------------------------------------------------------

#[test]
fn crlf_and_lf_map_to_the_same_line_structure() {
    // offsets (crlf): a=0 b=1 \r=2 \n=3 | c=4 d=5 \r=6 \n=7 | e=8
    let crlf = text("ab\r\ncd\r\ne");
    let lf = text("ab\ncd\ne");
    assert_eq!(crlf.line_count(), 3);
    assert_eq!(lf.line_count(), 3);

    // Line content excludes the terminator — no stray `\r` under CRLF.
    for line in 0..3 {
        let c = crlf
            .slice(crlf.line_range(line).expect("line"))
            .expect("slice");
        let l = lf.slice(lf.line_range(line).expect("line")).expect("slice");
        assert_eq!(c, l, "line {line} content must agree across CRLF/LF");
    }

    // Positions on the next line agree in line/column despite the extra byte.
    assert_eq!(crlf.line_col(ByteOffset::new(4)), Ok(lc(1, 0)));
    assert_eq!(lf.line_col(ByteOffset::new(3)), Ok(lc(1, 0)));

    // The `\r` of a CRLF pair still belongs to the line it terminates.
    assert_eq!(crlf.line_col(ByteOffset::new(2)), Ok(lc(0, 2)));
}

#[test]
fn lone_carriage_return_does_not_start_a_line() {
    // A bare `\r` (no following `\n`) is not a tuonelang line terminator; it
    // is just a character on the line. (The lexer rejects it — §1 — but the
    // source layer must still locate it faithfully for the diagnostic.)
    let t = text("a\rb");
    assert_eq!(t.line_count(), 1);
    assert_eq!(t.line_col(ByteOffset::new(2)), Ok(lc(0, 2)));
    assert_eq!(t.slice(t.line_range(0).expect("line 0")), Ok("a\rb"));
}

#[test]
fn crlf_file_ending_with_terminator_has_trailing_empty_line() {
    let t = text("ab\r\n");
    assert_eq!(t.line_count(), 2);
    assert_eq!(t.slice(t.line_range(0).expect("line 0")), Ok("ab"));
    assert_eq!(t.slice(t.line_range(1).expect("line 1")), Ok(""));
}

// --- Final line without newline -----------------------------------------

#[test]
fn final_line_without_newline_runs_to_end_of_text() {
    let t = text("one\ntwo");
    assert_eq!(t.line_count(), 2);
    let last = t.line_range(1).expect("line 1");
    assert_eq!(last, range(4, 7));
    assert_eq!(t.slice(last), Ok("two"));
    // End-of-text maps onto the final line.
    assert_eq!(t.line_col(ByteOffset::new(7)), Ok(lc(1, 3)));
}

#[test]
fn trailing_newline_creates_a_final_empty_line() {
    let t = text("one\n");
    assert_eq!(t.line_count(), 2);
    assert_eq!(t.slice(t.line_range(1).expect("line 1")), Ok(""));
    assert_eq!(t.line_col(ByteOffset::new(4)), Ok(lc(1, 0)));
}

// --- Span boundaries -----------------------------------------------------

#[test]
fn span_at_start_end_and_full_extent() {
    let t = text("ab\ncd");
    let src = t.source();

    // Empty span at the very start.
    let start = Span::new(src, TextRange::empty(ByteOffset::ZERO));
    assert_eq!(t.slice(start.range()), Ok(""));

    // Empty span at the very end (offset == len) is valid.
    let end = Span::new(src, TextRange::empty(ByteOffset::new(5)));
    assert_eq!(t.slice(end.range()), Ok(""));

    // Full-extent span covers everything, across the newline.
    let full = Span::new(src, t.full_range());
    assert_eq!(t.slice(full.range()), Ok("ab\ncd"));
}

#[test]
fn out_of_bounds_and_backwards_ranges_cannot_slice() {
    let t = text("abc");
    assert!(TextRange::new(2u32, 1u32).is_err());
    assert_eq!(
        t.slice(range(0, 4)),
        Err(LocateError::OutOfBounds {
            offset: ByteOffset::new(4),
            len: 3
        })
    );
}

#[test]
fn slicing_through_a_multibyte_boundary_is_rejected() {
    let t = text("aä"); // ä occupies bytes 1..3
    assert_eq!(
        t.slice(range(0, 2)),
        Err(LocateError::NotCharBoundary {
            offset: ByteOffset::new(2)
        })
    );
    assert_eq!(t.slice(range(1, 3)), Ok("ä"));
}

#[test]
fn spans_stay_anchored_across_revisions() {
    let mut map = SourceMap::new();
    let file = map.intern_file("main.tuo");
    let v0 = map.add_source(file, "let x = 1;").expect("fits");
    let span = Span::new(v0, range(4, 5));
    assert_eq!(map.source(v0).slice(span.range()), Ok("x"));

    // Editing the file produces a new snapshot; the old span still resolves
    // against the snapshot it was created from, byte for byte.
    let v1 = map.add_source(file, "let renamed = 1;").expect("fits");
    assert_ne!(v0, v1);
    assert_eq!(map.source(v0).slice(span.range()), Ok("x"));

    // And spans from different snapshots refuse to join.
    let other = Span::new(v1, range(4, 11));
    assert_eq!(span.try_join(other), None);
}
