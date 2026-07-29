//! The [`IncrementalSession::with_semantics`] accessor — the seam through which
//! interactive hosts (the LSP, the agent protocol) read the compiler's semantic
//! results with their spans, driven by the same incremental engine.

use tuo_compiler::IncrementalSession;
use tuo_diagnostics::Severity;
use tuo_resolve::SymbolKind;

const GOOD: &str = "\
fn add(take a: Int, take b: Int) -> Int {
    a + b
}

spec add {
    then add(2, 3) == 5;
}
";

#[test]
fn semantics_exposes_resolution_types_and_a_clean_diagnostic_set() {
    let mut session = IncrementalSession::new();
    session.set_file("lib.tuo", GOOD);

    session.with_semantics(|sema| {
        assert!(sema.accepted, "the program is accepted");
        assert!(
            sema.diagnostics.is_empty(),
            "a clean program has no diagnostics"
        );
        // Resolution is reachable and carries the function symbol.
        let add = sema
            .resolution
            .symbols()
            .find(|(_, symbol)| symbol.name == "add" && symbol.kind == SymbolKind::Function)
            .map(|(id, _)| id)
            .expect("add resolves");
        // Types are reachable and give add a function signature.
        assert!(
            sema.types.type_of(add).is_some(),
            "add has a checked signature type"
        );
        // The source map is the one the spans are anchored to.
        assert!(sema.map.file_id("lib.tuo").is_some());
    });
}

#[test]
fn semantics_retains_error_diagnostics_with_spans() {
    let mut session = IncrementalSession::new();
    session.set_file("bad.tuo", "fn main() -> Int {\n    missing()\n}\n");

    session.with_semantics(|sema| {
        assert!(!sema.accepted, "the program is rejected");
        let error = sema
            .diagnostics
            .iter()
            .find(|d| d.severity == Severity::Error)
            .expect("an error diagnostic is retained");
        // The diagnostic carries a real span into the program's source.
        assert!(
            sema.map.get_source(error.primary_span.source()).is_some(),
            "the diagnostic's span is anchored to the snapshot map"
        );
    });
}

#[test]
fn semantics_tracks_edits_across_revisions() {
    let mut session = IncrementalSession::new();
    session.set_file("m.tuo", "fn main() -> Int {\n    gone()\n}\n");
    let had_error = session.with_semantics(|sema| !sema.accepted);
    assert!(had_error, "the first version is rejected");

    session.set_file(
        "m.tuo",
        "fn helper() -> Int {\n    1\n}\n\nfn main() -> Int {\n    helper()\n}\n",
    );
    let clean = session.with_semantics(|sema| sema.accepted && sema.diagnostics.is_empty());
    assert!(clean, "the fixed version is accepted with no diagnostics");
}
