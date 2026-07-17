//! Golden tests for the human terminal renderer.
//!
//! Each test renders a diagnostic and compares it byte-for-byte against a
//! checked-in golden file under `tests/goldens/`. To regenerate the goldens
//! after an intentional layout change, run:
//!
//! ```sh
//! TUO_BLESS=1 cargo test -p tuo-diagnostics --test human_golden
//! ```
//!
//! and review the diff like any other code change.

use std::path::PathBuf;

use tuo_diagnostics::render::render;
use tuo_diagnostics::{Confidence, Diagnostic, DiagnosticCode, Edit, Namespace, StructuredValue};
use tuo_source::{SourceId, SourceMap, Span, TextRange};

fn golden_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/goldens")
        .join(name)
}

/// Compare `actual` against the golden file, or rewrite it under `TUO_BLESS=1`.
fn assert_golden(name: &str, actual: &str) {
    let path = golden_path(name);
    if std::env::var_os("TUO_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().expect("goldens dir has a parent"))
            .expect("create goldens dir");
        std::fs::write(&path, actual).expect("write blessed golden");
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("missing golden {}: {e} (bless with TUO_BLESS=1)", name));
    assert_eq!(
        actual, expected,
        "rendered output diverged from golden `{name}` (bless with TUO_BLESS=1 if intentional)",
    );
}

fn span(source: SourceId, start: u32, end: u32) -> Span {
    Span::new(
        source,
        TextRange::new(start, end).expect("test range must be forward"),
    )
}

/// A small program with a type error: `"hi"` assigned to an `Int`.
fn type_error_fixture() -> (SourceMap, SourceId) {
    let mut map = SourceMap::new();
    let file = map.intern_file("main.tuo");
    let source = map
        .add_source(
            file,
            "fn main() -> Int {\n    let x: Int = \"hi\";\n    x\n}\n",
        )
        .expect("fixture fits");
    (map, source)
}

#[test]
fn full_featured_type_error() {
    let (map, src) = type_error_fixture();
    // Byte offsets in line 2 (starts at 19): `x` = 27, `Int` = 30..33,
    // `"hi"` = 36..40.
    let diag = Diagnostic::error(
        DiagnosticCode::new(Namespace::Type, 301),
        "mismatched types",
        span(src, 36, 40),
    )
    .with_primary_label("expected `Int`, found `String`")
    .with_secondary_label(span(src, 30, 33), "the binding is declared `Int` here")
    .with_expected(StructuredValue::Type("Int".into()))
    .with_actual(StructuredValue::Type("String".into()))
    .with_note("`Int` and `String` never coerce into each other")
    .with_help("convert the string explicitly, or change the binding's type")
    .with_suggestion(
        "change the binding's type to `String`",
        vec![Edit {
            span: span(src, 30, 33),
            replacement: "String".into(),
        }],
        Confidence::Probable,
    );
    assert_golden("full_featured_type_error.txt", &render(&diag, &map));
}

#[test]
fn warning_with_insertion_suggestion() {
    let mut map = SourceMap::new();
    let file = map.intern_file("lib.tuo");
    let src = map
        .add_source(file, "fn id[T](in value: T) -> T {\n    value\n}\n")
        .expect("fixture fits");
    // `value` parameter name at bytes 12..17.
    let diag = Diagnostic::warning(
        DiagnosticCode::new(Namespace::Resolution, 45),
        "unused parameter `value`",
        span(src, 12, 17),
    )
    .with_primary_label("never read")
    .with_suggestion(
        "prefix the name with `_` to mark it deliberately unused",
        vec![Edit {
            span: span(src, 12, 12),
            replacement: "_".into(),
        }],
        Confidence::MachineApplicable,
    );
    assert_golden("warning_with_insertion.txt", &render(&diag, &map));
}

#[test]
fn multiline_span_and_multibyte_columns() {
    let mut map = SourceMap::new();
    let file = map.intern_file("näkymä.tuo");
    // Line 1 contains multibyte scalars before the span; the primary span
    // crosses from line 2 into line 3.
    let src = map
        .add_source(
            file,
            "let tervehdys = \"päivää\";\nspec greeting {\n    assert greets();\n}\n",
        )
        .expect("fixture fits");
    let text = map.source(src).text();
    let spec_start = text.find("spec").expect("fixture contains `spec`");
    let block_end = text.rfind('}').expect("fixture contains `}`") + 1;
    let diag = Diagnostic::error(
        DiagnosticCode::new(Namespace::Specification, 7),
        "spec references an undeclared function",
        span(
            src,
            u32::try_from(spec_start).expect("offset fits"),
            u32::try_from(block_end).expect("offset fits"),
        ),
    )
    .with_primary_label("this spec block")
    .with_secondary_label(
        span(src, 4, 13), // `tervehdys`
        "only this binding is declared in the module",
    )
    .with_note("a spec may refer to a function declared later in the same module");
    assert_golden("multiline_and_multibyte.txt", &render(&diag, &map));
}

#[test]
fn unresolved_span_is_reported_not_dropped() {
    let map = SourceMap::new(); // knows no sources at all
    let diag = Diagnostic::error(
        DiagnosticCode::new(Namespace::Mir, 2),
        "internal location lost",
        span(SourceId::from_raw(99), 0, 4),
    );
    assert_golden("unresolved_span.txt", &render(&diag, &map));
}
