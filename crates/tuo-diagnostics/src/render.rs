//! Plain-text human rendering of diagnostics.
//!
//! This module is a rendering back end over the canonical [`Diagnostic`]
//! model; it consumes the data read-only and returns a `String` (it never
//! prints). The output is deliberately deterministic plain text — no ANSI
//! colors, no terminal-width dependence — so it is stable under golden tests
//! and readable in logs.
//!
//! A dedicated rendering library (e.g. `ariadne`) could replace the layout
//! logic here later, since it handles labelled and multiline spans well; it
//! would slot in behind this function without its types ever appearing in the
//! canonical model.
//!
//! Layout, following the familiar rustc shape:
//!
//! ```text
//! error[T0301]: mismatched types
//!  --> main.tuo:2:18
//!   |
//! 2 |     let x: Int = "hi";
//!   |                  ^^^^ expected `Int`, found `String`
//!  --> main.tuo:1:5
//!   |
//! 1 |     let y = x;
//!   |         - declared here
//!   = expected: `Int`
//!   = actual: `String`
//!   = note: `Int` and `String` never coerce
//!   = help: convert the string explicitly
//!   = suggestion (probable): parse the string
//!       - replace main.tuo:2:18..2:22 with `"hi".parse()`
//! ```
//!
//! Columns are counted in Unicode scalar values (matching
//! [`tuo_source::LineCol`]); rendering does not attempt display-width
//! correction for wide glyphs. Spans that do not resolve against the provided
//! [`SourceMap`] are reported explicitly (never silently dropped) as a
//! `--> <unresolved span …>` line with the canonical byte offsets.

use std::fmt::Write as _;

use tuo_source::{SourceMap, SourceText, Span};

use crate::{Diagnostic, Edit};

/// Render one diagnostic as deterministic plain text, ending with a newline.
#[must_use]
pub fn render(diagnostic: &Diagnostic, sources: &SourceMap) -> String {
    let mut out = String::new();

    // Header.
    let _ = writeln!(
        out,
        "{}[{}]: {}",
        diagnostic.severity, diagnostic.code, diagnostic.message
    );

    // Gutter width: fits the largest 1-based line number we will print.
    let gutter = gutter_width(diagnostic, sources);

    // Primary span, then secondaries, each as its own excerpt block.
    excerpt(
        &mut out,
        diagnostic.primary_span,
        '^',
        diagnostic.primary_label.as_deref().unwrap_or(""),
        gutter,
        sources,
    );
    for label in &diagnostic.secondary_labels {
        excerpt(&mut out, label.span, '-', &label.message, gutter, sources);
    }

    // Trailing facts.
    if !diagnostic.expected.is_empty() {
        let _ = writeln!(out, "  = expected: {}", join_values(&diagnostic.expected));
    }
    if !diagnostic.actual.is_empty() {
        let _ = writeln!(out, "  = actual: {}", join_values(&diagnostic.actual));
    }
    for note in &diagnostic.notes {
        let _ = writeln!(out, "  = note: {note}");
    }
    if let Some(help) = &diagnostic.help {
        let _ = writeln!(out, "  = help: {help}");
    }
    for suggestion in &diagnostic.suggestions {
        let _ = writeln!(
            out,
            "  = suggestion ({}): {}",
            suggestion.confidence, suggestion.message
        );
        for edit in &suggestion.edits {
            let _ = writeln!(out, "      - {}", render_edit(edit, sources));
        }
    }

    out
}

/// Render several diagnostics separated by blank lines.
#[must_use]
pub fn render_all(diagnostics: &[Diagnostic], sources: &SourceMap) -> String {
    diagnostics
        .iter()
        .map(|d| render(d, sources))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The 1-based start line of a span, when it resolves.
fn start_line(span: Span, sources: &SourceMap) -> Option<u32> {
    let text = sources.get_source(span.source())?;
    let lc = text.line_col(span.range().start()).ok()?;
    Some(lc.line + 1)
}

fn gutter_width(diagnostic: &Diagnostic, sources: &SourceMap) -> usize {
    let max_line = std::iter::once(diagnostic.primary_span)
        .chain(diagnostic.secondary_labels.iter().map(|l| l.span))
        .filter_map(|span| start_line(span, sources))
        .max()
        .unwrap_or(1);
    max_line.to_string().len()
}

/// Append one excerpt block for `span`, underlined with `mark`.
fn excerpt(
    out: &mut String,
    span: Span,
    mark: char,
    message: &str,
    gutter: usize,
    sources: &SourceMap,
) {
    let Some(text) = sources.get_source(span.source()) else {
        let _ = writeln!(
            out,
            " --> <unresolved span: unknown {} @ {}>",
            span.source(),
            span.range()
        );
        return;
    };
    let (Ok(start), Ok(end)) = (
        text.line_col(span.range().start()),
        text.line_col(span.range().end()),
    ) else {
        let _ = writeln!(
            out,
            " --> <unresolved span: {} @ {} is out of bounds>",
            span.source(),
            span.range()
        );
        return;
    };

    let file = sources.file_name(text.file());
    let _ = writeln!(out, " --> {file}:{}:{}", start.line + 1, start.column + 1);
    let _ = writeln!(out, "{:>gutter$} |", "");

    let line_text = line_content(text, start.line);
    let _ = writeln!(out, "{:>gutter$} | {line_text}", start.line + 1);

    // Underline: from the start column to the end column on the start line
    // (or to the end of the line for multiline spans, marked with `...`).
    let (width, continues) = if end.line == start.line {
        ((end.column - start.column) as usize, false)
    } else {
        let line_len = line_text.chars().count();
        (line_len.saturating_sub(start.column as usize), true)
    };
    let underline = if width == 0 {
        mark.to_string() // empty span: one mark at the insertion point
    } else {
        mark.to_string().repeat(width)
    };
    let mut marker_line = format!(
        "{:>gutter$} | {:pad$}{underline}{}",
        "",
        "",
        if continues { "..." } else { "" },
        pad = start.column as usize,
    );
    if !message.is_empty() {
        let _ = write!(marker_line, " {message}");
    }
    let _ = writeln!(out, "{marker_line}");
}

/// The content of a 0-based line, without its terminator. Infallible for
/// lines returned by a successful `line_col`.
fn line_content(text: &SourceText, line: u32) -> String {
    text.line_range(line)
        .ok()
        .and_then(|range| text.slice(range).ok())
        .unwrap_or_default()
        .to_owned()
}

fn join_values(values: &[crate::StructuredValue]) -> String {
    values
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Render one suggested edit as a human line.
fn render_edit(edit: &Edit, sources: &SourceMap) -> String {
    let point = edit.span.range().is_empty();
    let location = match sources.get_source(edit.span.source()) {
        Some(text) => {
            let file = sources.file_name(text.file());
            match (
                text.line_col(edit.span.range().start()),
                text.line_col(edit.span.range().end()),
            ) {
                // An insertion happens at a single point; a replacement or
                // deletion covers a start..end extent.
                (Ok(s), _) if point => format!("{file}:{}:{}", s.line + 1, s.column + 1),
                (Ok(s), Ok(e)) => format!(
                    "{file}:{}:{}..{}:{}",
                    s.line + 1,
                    s.column + 1,
                    e.line + 1,
                    e.column + 1
                ),
                _ => format!("{file} @ {} (out of bounds)", edit.span.range()),
            }
        }
        None => format!("<unknown {}> @ {}", edit.span.source(), edit.span.range()),
    };
    if point {
        format!("insert `{}` at {location}", edit.replacement)
    } else if edit.replacement.is_empty() {
        format!("delete {location}")
    } else {
        format!("replace {location} with `{}`", edit.replacement)
    }
}
