//! Schema tests for the JSON output: the envelope carries an explicit
//! schema version, every documented key is present with the documented
//! type, and derived (presentation) fields are null — never fabricated —
//! when a span cannot be resolved.

use serde_json::Value;
use tuo_diagnostics::{
    Confidence, Diagnostic, DiagnosticCode, Edit, Namespace, StructuredValue, json,
};
use tuo_source::{SourceId, SourceMap, Span, TextRange};

fn span(source: SourceId, start: u32, end: u32) -> Span {
    Span::new(
        source,
        TextRange::new(start, end).expect("test range must be forward"),
    )
}

/// A representative diagnostic exercising every field of the schema.
fn fixture() -> (SourceMap, Diagnostic) {
    let mut map = SourceMap::new();
    let file = map.intern_file("main.tuo");
    let src = map
        .add_source(
            file,
            "fn main() -> Int {\n    let x: Int = \"hi\";\n    x\n}\n",
        )
        .expect("fixture fits");
    let diag = Diagnostic::error(
        DiagnosticCode::new(Namespace::Type, 301),
        "mismatched types",
        span(src, 36, 40),
    )
    .with_primary_label("expected `Int`, found `String`")
    .with_secondary_label(span(src, 30, 33), "declared `Int` here")
    .with_expected(StructuredValue::Type("Int".into()))
    .with_actual(StructuredValue::Type("String".into()))
    .with_note("`Int` and `String` never coerce into each other")
    .with_help("convert the string explicitly")
    .with_suggestion(
        "change the binding's type",
        vec![Edit {
            span: span(src, 30, 33),
            replacement: "String".into(),
        }],
        Confidence::Probable,
    );
    (map, diag)
}

fn keys(object: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = object
        .as_object()
        .expect("must be a JSON object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    keys
}

#[test]
fn envelope_carries_explicit_schema_version() {
    let (map, diag) = fixture();
    let value = json::to_json(&[diag], &map);
    assert_eq!(keys(&value), ["diagnostics", "schema_version"]);
    assert_eq!(value["schema_version"], Value::from(json::SCHEMA_VERSION));
    assert_eq!(value["schema_version"], Value::from(1u32));
    assert_eq!(
        value["diagnostics"].as_array().map(Vec::len),
        Some(1),
        "one diagnostic in, one out"
    );
}

#[test]
fn output_is_valid_json_and_round_trips() {
    let (map, diag) = fixture();
    let diags = [diag];
    let text = json::to_json_string(&diags, &map);
    let reparsed: Value = serde_json::from_str(&text).expect("output must parse as JSON");
    assert_eq!(reparsed, json::to_json(&diags, &map));
}

#[test]
fn diagnostic_object_has_exactly_the_documented_keys() {
    let (map, diag) = fixture();
    let value = json::diagnostic_to_json(&diag, &map);
    assert_eq!(
        keys(&value),
        [
            "actual",
            "code",
            "expected",
            "help",
            "message",
            "notes",
            "primary_label",
            "primary_span",
            "secondary_labels",
            "severity",
            "suggestions",
        ]
    );
    assert_eq!(value["code"], Value::from("T0301"));
    assert_eq!(value["severity"], Value::from("error"));
    assert_eq!(value["message"], Value::from("mismatched types"));
    assert_eq!(value["notes"].as_array().map(Vec::len), Some(1));
    assert_eq!(value["help"], Value::from("convert the string explicitly"));
}

#[test]
fn spans_carry_canonical_offsets_and_derived_line_cols() {
    let (map, diag) = fixture();
    let value = json::diagnostic_to_json(&diag, &map);
    let span = &value["primary_span"];
    assert_eq!(
        keys(span),
        [
            "end",
            "end_line_col",
            "file",
            "source",
            "start",
            "start_line_col"
        ]
    );
    assert_eq!(span["file"], Value::from("main.tuo"));
    assert_eq!(span["start"], Value::from(36));
    assert_eq!(span["end"], Value::from(40));
    // 1-based presentation coordinates: `"hi"` sits on line 2.
    assert_eq!(span["start_line_col"]["line"], Value::from(2));
    assert_eq!(span["start_line_col"]["column"], Value::from(18));
    assert_eq!(span["end_line_col"]["column"], Value::from(22));
}

#[test]
fn structured_values_are_kind_tagged() {
    let (map, diag) = fixture();
    let value = json::diagnostic_to_json(&diag, &map);
    assert_eq!(
        value["expected"][0],
        serde_json::json!({ "kind": "type", "value": "Int" })
    );
    assert_eq!(
        value["actual"][0],
        serde_json::json!({ "kind": "type", "value": "String" })
    );
}

#[test]
fn suggestions_carry_confidence_and_edits() {
    let (map, diag) = fixture();
    let value = json::diagnostic_to_json(&diag, &map);
    let suggestion = &value["suggestions"][0];
    assert_eq!(keys(suggestion), ["confidence", "edits", "message"]);
    assert_eq!(suggestion["confidence"], Value::from("probable"));
    let edit = &suggestion["edits"][0];
    assert_eq!(keys(edit), ["replacement", "span"]);
    assert_eq!(edit["replacement"], Value::from("String"));
    assert_eq!(edit["span"]["start"], Value::from(30));
}

#[test]
fn all_confidence_levels_serialize_to_documented_strings() {
    let expected = [
        (Confidence::MachineApplicable, "machine-applicable"),
        (Confidence::Probable, "probable"),
        (Confidence::Speculative, "speculative"),
    ];
    for (confidence, name) in expected {
        assert_eq!(confidence.name(), name);
    }
}

#[test]
fn unresolvable_spans_keep_offsets_and_null_derived_fields() {
    let map = SourceMap::new(); // knows no sources
    let diag = Diagnostic::error(
        DiagnosticCode::new(Namespace::Mir, 2),
        "internal location lost",
        span(SourceId::from_raw(99), 3, 7),
    );
    let value = json::diagnostic_to_json(&diag, &map);
    let span = &value["primary_span"];
    // Canonical byte offsets survive …
    assert_eq!(span["source"], Value::from(99));
    assert_eq!(span["start"], Value::from(3));
    assert_eq!(span["end"], Value::from(7));
    // … while every derived field is explicitly null, not invented.
    assert_eq!(span["file"], Value::Null);
    assert_eq!(span["start_line_col"], Value::Null);
    assert_eq!(span["end_line_col"], Value::Null);
    // Absent optional parts are null / empty, not missing keys.
    assert_eq!(value["primary_label"], Value::Null);
    assert_eq!(value["help"], Value::Null);
    assert_eq!(value["suggestions"].as_array().map(Vec::len), Some(0));
}

#[test]
fn diagnostic_codes_cover_all_reserved_namespaces() {
    let mut map = SourceMap::new();
    let file = map.intern_file("x.tuo");
    let src = map.add_source(file, "fn f() {}\n").expect("fixture fits");
    for (ns, letter) in Namespace::ALL.iter().zip("LPRTOMSC".chars()) {
        let diag = Diagnostic::error(
            DiagnosticCode::new(*ns, 1),
            "namespace probe",
            span(src, 0, 2),
        );
        let value = json::diagnostic_to_json(&diag, &map);
        assert_eq!(value["code"], Value::from(format!("{letter}0001")));
    }
}
