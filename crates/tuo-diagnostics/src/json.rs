//! JSON serialization of diagnostics, with an explicit schema version.
//!
//! This module is a rendering back end over the canonical [`Diagnostic`]
//! model: it builds `serde_json::Value`s by hand so the wire format is
//! spelled out here explicitly, versioned by [`SCHEMA_VERSION`], and cannot
//! silently drift when the Rust types evolve. `serde_json` is an
//! implementation detail; its types are not part of the canonical model.
//!
//! # Schema (version 1)
//!
//! ```json
//! {
//!   "schema_version": 1,
//!   "diagnostics": [
//!     {
//!       "code": "T0301",
//!       "severity": "error",
//!       "message": "mismatched types",
//!       "primary_span": <span>,
//!       "primary_label": "expected `Int`" | null,
//!       "secondary_labels": [{"span": <span>, "message": "…"}],
//!       "notes": ["…"],
//!       "help": "…" | null,
//!       "expected": [<value>],
//!       "actual": [<value>],
//!       "suggestions": [
//!         {
//!           "message": "…",
//!           "confidence": "machine-applicable" | "probable" | "speculative",
//!           "edits": [{"span": <span>, "replacement": "…"}]
//!         }
//!       ]
//!     }
//!   ]
//! }
//! ```
//!
//! where `<span>` is
//!
//! ```json
//! {
//!   "source": 0,
//!   "file": "main.tuo",
//!   "start": 4, "end": 6,                      // byte offsets (canonical)
//!   "start_line_col": {"line": 1, "column": 5}, // 1-based, presentation
//!   "end_line_col": {"line": 1, "column": 7}    // null if unlocatable
//! }
//! ```
//!
//! and `<value>` is `{"kind": "type"|"token"|"name"|"count"|"text",
//! "value": …}` (`value` is a JSON number for `count`, a string otherwise).
//!
//! Byte offsets are the canonical positions; the line/column fields are
//! derived presentation data and are `null` (never fabricated) when a span
//! does not resolve against its source — for example when the `SourceId` is
//! unknown to the provided [`SourceMap`].

use serde_json::{Value, json};
use tuo_source::{SourceMap, Span};

use crate::{Diagnostic, StructuredValue, Suggestion};

/// The version of the JSON schema emitted by this module. Bumped on any
/// backwards-incompatible change to the layout documented above.
pub const SCHEMA_VERSION: u32 = 1;

/// Serialize a batch of diagnostics as the versioned envelope object.
#[must_use]
pub fn to_json(diagnostics: &[Diagnostic], sources: &SourceMap) -> Value {
    json!({
        "schema_version": SCHEMA_VERSION,
        "diagnostics": diagnostics
            .iter()
            .map(|d| diagnostic_to_json(d, sources))
            .collect::<Vec<_>>(),
    })
}

/// Serialize a batch of diagnostics as a compact JSON string.
///
/// # Panics
///
/// Never panics in practice: the value built by [`to_json`] contains no
/// non-string map keys or other unserializable data.
#[must_use]
pub fn to_json_string(diagnostics: &[Diagnostic], sources: &SourceMap) -> String {
    serde_json::to_string(&to_json(diagnostics, sources))
        .expect("diagnostic JSON value is always serializable")
}

/// Serialize one diagnostic (without the envelope).
#[must_use]
pub fn diagnostic_to_json(diagnostic: &Diagnostic, sources: &SourceMap) -> Value {
    json!({
        "code": diagnostic.code.to_string(),
        "severity": diagnostic.severity.name(),
        "message": diagnostic.message,
        "primary_span": span_to_json(diagnostic.primary_span, sources),
        "primary_label": diagnostic.primary_label,
        "secondary_labels": diagnostic
            .secondary_labels
            .iter()
            .map(|l| json!({
                "span": span_to_json(l.span, sources),
                "message": l.message,
            }))
            .collect::<Vec<_>>(),
        "notes": diagnostic.notes,
        "help": diagnostic.help,
        "expected": diagnostic.expected.iter().map(value_to_json).collect::<Vec<_>>(),
        "actual": diagnostic.actual.iter().map(value_to_json).collect::<Vec<_>>(),
        "suggestions": diagnostic
            .suggestions
            .iter()
            .map(|s| suggestion_to_json(s, sources))
            .collect::<Vec<_>>(),
    })
}

fn suggestion_to_json(suggestion: &Suggestion, sources: &SourceMap) -> Value {
    json!({
        "message": suggestion.message,
        "confidence": suggestion.confidence.name(),
        "edits": suggestion
            .edits
            .iter()
            .map(|e| json!({
                "span": span_to_json(e.span, sources),
                "replacement": e.replacement,
            }))
            .collect::<Vec<_>>(),
    })
}

fn value_to_json(value: &StructuredValue) -> Value {
    let rendered = match value {
        StructuredValue::Type(s)
        | StructuredValue::Token(s)
        | StructuredValue::Name(s)
        | StructuredValue::Text(s) => json!(s),
        StructuredValue::Count(n) => json!(n),
    };
    json!({ "kind": value.kind(), "value": rendered })
}

/// Serialize a [`Span`] as the versioned span object: canonical byte offsets
/// plus derived line/column (null when the span does not resolve against
/// `sources`). This is the one source-range representation the JSON schema
/// uses everywhere, so protocol layers built on top of diagnostics attach
/// ranges through it rather than inventing their own shape.
#[must_use]
pub fn span_to_json(span: Span, sources: &SourceMap) -> Value {
    let (file, start_lc, end_lc) = match sources.get_source(span.source()) {
        Some(text) => (
            json!(sources.file_name(text.file())),
            line_col_to_json(text.line_col(span.range().start()).ok()),
            line_col_to_json(text.line_col(span.range().end()).ok()),
        ),
        // The span's snapshot is unknown to this map: keep the canonical
        // offsets, leave every derived field null rather than guessing.
        None => (Value::Null, Value::Null, Value::Null),
    };
    json!({
        "source": span.source().as_raw(),
        "file": file,
        "start": span.range().start().as_u32(),
        "end": span.range().end().as_u32(),
        "start_line_col": start_lc,
        "end_line_col": end_lc,
    })
}

fn line_col_to_json(lc: Option<tuo_source::LineCol>) -> Value {
    match lc {
        Some(lc) => json!({ "line": lc.line + 1, "column": lc.column + 1 }),
        None => Value::Null,
    }
}
