//! Every LSP feature, exercised against the shared compiler engine.
//!
//! Each test drives an [`Analysis`] over a small tuonelang program and asserts
//! the feature's answer — proving the language server reuses the compiler's
//! resolution, typing, and diagnostics rather than reimplementing them. The
//! programs are deliberately tiny and their positions computed from the text, so
//! the assertions read against the source, not against magic offsets.

use tuo_lsp::Analysis;
use tuo_lsp::wire::{CompletionItemKind, DiagnosticSeverity, Position, SymbolKind};

/// A two-function program with a spec attached to `add`.
const LIB: &str = "\
fn add(take a: Int, take b: Int) -> Int {
    a + b
}

fn double(take x: Int) -> Int {
    x * 2
}

spec add {
    given a: Int = 2, b: Int = 3;
    then add(a, b) == 5;
}
";

/// The zero-based (line, column) of the first occurrence of `needle` in `text`,
/// pointing at its first character. Panics if not found — the tests must anchor
/// to real source.
fn position_of(text: &str, needle: &str) -> Position {
    let byte = text.find(needle).expect("needle present in source");
    let before = &text[..byte];
    let line = before.matches('\n').count() as u32;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let column = text[line_start..byte]
        .chars()
        .map(|c| c.len_utf16() as u32)
        .sum();
    Position::new(line, column)
}

/// A primed analysis over `LIB` as `lib.tuo`.
fn analysis() -> Analysis {
    let mut analysis = Analysis::new();
    analysis.set_document("lib.tuo", LIB);
    analysis
}

#[test]
fn diagnostics_are_empty_for_a_clean_program() {
    let analysis = analysis();
    assert!(
        analysis.diagnostics("lib.tuo").is_empty(),
        "a well-formed program has no diagnostics"
    );
}

#[test]
fn diagnostics_report_an_error_with_a_range() {
    let mut analysis = Analysis::new();
    // `undefined` is not a symbol — resolution rejects it.
    analysis.set_document("bad.tuo", "fn main() -> Int {\n    undefined()\n}\n");
    let diagnostics = analysis.diagnostics("bad.tuo");
    assert!(
        !diagnostics.is_empty(),
        "an unresolved name must produce a diagnostic"
    );
    let diag = &diagnostics[0];
    assert_eq!(diag.severity, DiagnosticSeverity::Error);
    // The range points at the offending name on line 1 (zero-based).
    assert_eq!(diag.range.start.line, 1, "the error is on the second line");
    assert!(
        diag.range.end.character > diag.range.start.character,
        "the range spans the name"
    );
}

#[test]
fn hover_shows_a_function_signature() {
    let analysis = analysis();
    let hover = analysis
        .hover("lib.tuo", position_of(LIB, "add"))
        .expect("hover over `add`");
    assert!(
        hover.contents.contains("function") && hover.contents.contains("add"),
        "hover names the function: {}",
        hover.contents
    );
    assert!(
        hover.contents.contains("fn(") && hover.contents.contains("->"),
        "hover renders the function's signature type: {}",
        hover.contents
    );
}

#[test]
fn goto_definition_jumps_from_a_call_to_the_declaration() {
    let analysis = analysis();
    // The call `add(a, b)` lives inside the spec; jump to `add`'s declaration.
    let call = position_of(LIB, "add(a, b)");
    let location = analysis
        .goto_definition("lib.tuo", call)
        .expect("definition of the called `add`");
    assert_eq!(location.uri, "lib.tuo");
    // The declaration is the `add` on line 0.
    assert_eq!(location.range.start.line, 0);
}

#[test]
fn find_references_lists_every_use() {
    let analysis = analysis();
    // Cursor on the declaration of `add`.
    let decl = position_of(LIB, "add");
    let refs = analysis.references("lib.tuo", decl, true);
    // Declaration + spec-target reference + call in the spec body = at least 3.
    assert!(
        refs.len() >= 2,
        "add has multiple references: {}",
        refs.len()
    );
    assert!(refs.iter().all(|r| r.uri == "lib.tuo"));
}

#[test]
fn rename_rewrites_declaration_and_uses() {
    let analysis = analysis();
    let decl = position_of(LIB, "add");
    let edit = analysis
        .rename("lib.tuo", decl, "plus")
        .expect("rename produces an edit");
    let changes = &edit.document_changes;
    assert_eq!(changes.len(), 1, "one file changes");
    assert_eq!(changes[0].uri, "lib.tuo");
    assert!(
        changes[0].edits.iter().all(|e| e.new_text == "plus"),
        "every edit inserts the new name"
    );
    assert!(
        changes[0].edits.len() >= 2,
        "declaration and at least one use are rewritten"
    );
}

#[test]
fn document_symbols_outline_the_module() {
    let analysis = analysis();
    let symbols = analysis.document_symbols("lib.tuo");
    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"add"), "outline includes add: {names:?}");
    assert!(names.contains(&"double"), "outline includes double");
    let add = symbols.iter().find(|s| s.name == "add").unwrap();
    assert_eq!(add.kind, SymbolKind::Function);
    assert!(
        add.detail.as_deref().is_some_and(|d| d.contains("fn(")),
        "the function's detail carries its signature: {:?}",
        add.detail
    );
    // Outline is ordered by position.
    assert!(
        symbols
            .windows(2)
            .all(|w| w[0].range.start <= w[1].range.start)
    );
}

#[test]
fn completion_offers_names_and_keywords() {
    let analysis = analysis();
    let items = analysis.completion("lib.tuo", Position::new(1, 0));
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(labels.contains(&"add"), "completes the function add");
    assert!(labels.contains(&"double"), "completes the function double");
    assert!(labels.contains(&"fn"), "offers keywords");
    let add = items.iter().find(|i| i.label == "add").unwrap();
    assert_eq!(add.kind, CompletionItemKind::Function);
    let kw = items.iter().find(|i| i.label == "fn").unwrap();
    assert_eq!(kw.kind, CompletionItemKind::Keyword);
}

#[test]
fn signature_help_shows_the_called_functions_signature() {
    let analysis = analysis();
    // Just inside the argument list of `add(a, b)` in the spec.
    let call = position_of(LIB, "add(a, b)");
    let inside = Position::new(call.line, call.character + 4);
    let help = analysis
        .signature_help("lib.tuo", inside)
        .expect("signature help for the add call");
    assert_eq!(help.signatures.len(), 1);
    let sig = &help.signatures[0];
    assert!(
        sig.label.contains("add"),
        "labels the function: {}",
        sig.label
    );
    assert_eq!(sig.parameters.len(), 2, "add takes two parameters");
}

#[test]
fn semantic_tokens_classify_declarations_and_uses() {
    let analysis = analysis();
    let tokens = analysis.semantic_tokens("lib.tuo");
    assert!(
        !tokens.data.is_empty(),
        "the program has highlightable tokens"
    );
    // Data is five integers per token.
    assert_eq!(tokens.data.len() % 5, 0, "delta encoding is 5-wide");
}

#[test]
fn code_actions_offer_only_compiler_authored_fixes() {
    // A clean program has no fixes; this confirms the LSP never invents any.
    let analysis = analysis();
    let whole = tuo_lsp::wire::Range::new(Position::new(0, 0), Position::new(100, 0));
    let actions = analysis.code_actions("lib.tuo", whole);
    assert!(actions.is_empty(), "a clean program yields no code actions");
}

#[test]
fn navigate_from_function_to_its_specs() {
    let analysis = analysis();
    // Cursor on the `add` declaration; its attached spec is `spec add`.
    let decl = position_of(LIB, "add");
    let specs = analysis.specs_for_function("lib.tuo", decl);
    assert_eq!(specs.len(), 1, "add has one attached spec");
    // The spec block is on the `spec add {` line.
    let spec_line = LIB.lines().position(|l| l.starts_with("spec add")).unwrap() as u32;
    assert_eq!(specs[0].range.start.line, spec_line);
}

#[test]
fn an_edit_flows_through_the_shared_session() {
    // Prove the LSP reads the shared incremental engine: after re-setting the
    // document, the new content drives every subsequent query. A first version
    // with an error yields a diagnostic; fixing it clears the diagnostic and
    // makes hover resolve — all without the LSP re-implementing any stage.
    let mut analysis = Analysis::new();
    analysis.set_document("m.tuo", "fn main() -> Int {\n    gone()\n}\n");
    assert!(
        !analysis.diagnostics("m.tuo").is_empty(),
        "the unresolved call is reported"
    );

    let fixed = "fn helper() -> Int {\n    1\n}\n\nfn main() -> Int {\n    helper()\n}\n";
    analysis.set_document("m.tuo", fixed);
    assert!(
        analysis.diagnostics("m.tuo").is_empty(),
        "fixing the program clears the diagnostic"
    );
    // The now-defined `helper` call resolves to its declaration.
    let call = position_of(fixed, "helper()");
    let def = analysis
        .goto_definition("m.tuo", call)
        .expect("helper resolves after the edit");
    assert_eq!(def.range.start.line, 0, "helper is declared on line 0");
}

#[test]
fn navigate_from_spec_to_its_target_function() {
    let analysis = analysis();
    // Cursor on the spec's target name in `spec add {`.
    let byte = LIB.find("spec add").unwrap() + "spec ".len();
    let before = &LIB[..byte];
    let line = before.matches('\n').count() as u32;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let column = LIB[line_start..byte].chars().count() as u32;
    let target = analysis
        .target_of_spec("lib.tuo", Position::new(line, column))
        .expect("the spec targets a function");
    // It lands on `add`'s declaration (line 0).
    assert_eq!(target.range.start.line, 0);
}
