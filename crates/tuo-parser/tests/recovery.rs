//! Error-recovery tests for invalid files, verifying the four properties the
//! recovery design promises: the diagnostic **code**, its **location**, the
//! **continuation** of parsing past the error, and the **number of
//! subsequent valid constructs retained** in the tree.

use tuo_parser::{ParseResult, parse};
use tuo_source::SourceMap;
use tuo_syntax::SyntaxKind;

fn parse_str(text: &str) -> ParseResult {
    let mut map = SourceMap::new();
    let file = map.intern_file("test.tuo");
    let source = map.add_source(file, text).expect("test source fits");
    parse(map.source(source))
}

fn count(result: &ParseResult, kind: SyntaxKind) -> usize {
    result.tree.root.descendants_of_kind(kind).len()
}

#[test]
fn a_broken_item_reports_p0002_at_its_location_and_later_items_survive() {
    let text = "fn first() -> Int {\n    1\n}\n\nfn broken( {\n\nstruct Point { x: F64 }\n\nfn last() -> Int {\n    2\n}\n";
    let result = parse_str(text);

    // Code and location: one P0002 pointing at the broken item's first token.
    assert_eq!(result.diagnostics.len(), 1);
    let diagnostic = &result.diagnostics[0];
    assert_eq!(diagnostic.code.to_string(), "P0002");
    let broken_at = text.find("fn broken").expect("marker exists");
    assert_eq!(
        diagnostic.primary_span.range().start().as_usize(),
        broken_at
    );

    // Continuation: parsing resumed and retained BOTH subsequent valid items
    // plus the one before the error.
    assert_eq!(count(&result, SyntaxKind::FunctionItem), 2);
    assert_eq!(count(&result, SyntaxKind::StructItem), 1);
    assert_eq!(count(&result, SyntaxKind::Error), 1);
}

#[test]
fn multiple_broken_constructs_produce_multiple_diagnostics() {
    let text = "fn a() -> Int { 1 }\n\nfn bad1( {\n\nfn b() -> Int { 2 }\n\nstruct bad2 {{\n\nfn c() -> Int { 3 }\n";
    let result = parse_str(text);
    let codes: Vec<String> = result
        .diagnostics
        .iter()
        .map(|d| d.code.to_string())
        .collect();
    assert_eq!(codes, ["P0002", "P0002"]);
    // All three valid functions retained around two error islands.
    assert_eq!(count(&result, SyntaxKind::FunctionItem), 3);
    assert_eq!(count(&result, SyntaxKind::Error), 2);
}

#[test]
fn statement_recovery_resynchronizes_at_the_semicolon() {
    let text = "fn f() -> Int {\n    let ok = 1;\n    let = broken;\n    var kept = 2;\n    ok + kept\n}\n";
    let result = parse_str(text);

    assert_eq!(result.diagnostics.len(), 1);
    let diagnostic = &result.diagnostics[0];
    assert_eq!(diagnostic.code.to_string(), "P0002");
    // Location: the first skipped token of the malformed statement.
    let broken_at = text.find("let = broken").expect("marker exists");
    assert_eq!(
        diagnostic.primary_span.range().start().as_usize(),
        broken_at
    );

    // Continuation inside the same block: the statements before and after
    // the error and the tail expression all survive.
    assert_eq!(count(&result, SyntaxKind::LetStatement), 1);
    assert_eq!(count(&result, SyntaxKind::VarStatement), 1);
    assert_eq!(count(&result, SyntaxKind::Error), 1);
    let binaries = count(&result, SyntaxKind::BinaryExpr);
    assert_eq!(binaries, 1, "tail expression `ok + kept` retained");
}

#[test]
fn recovery_stops_before_the_closing_brace() {
    // The garbage has no `;`, so recovery must stop at `}` without eating
    // it — the item still closes and the next item is untouched.
    let text = "fn f() {\n    ) ) )\n}\n\nfn g() -> Int { 4 }\n";
    let result = parse_str(text);
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.diagnostics[0].code.to_string(), "P0002");
    assert_eq!(count(&result, SyntaxKind::FunctionItem), 2);
}

#[test]
fn malformed_structure_is_retained_for_tooling_not_discarded() {
    let text = "fn f() {\n    let x = ;\n}\n";
    let result = parse_str(text);
    let errors = result.tree.root.descendants_of_kind(SyntaxKind::Error);
    assert_eq!(errors.len(), 1);
    // The Error node holds the skipped tokens (`let x = ;`), so an IDE can
    // still see what the user was typing.
    let span = errors[0]
        .span(&result.tree.lex.tokens)
        .expect("error node is non-empty");
    let snippet = &text[span.range().start().as_usize()..span.range().end().as_usize()];
    assert_eq!(snippet, "let x = ;");
    // And the file as a whole is still byte-reconstructable.
    assert_eq!(result.tree.reconstruct(text), text);
}

#[test]
fn lexical_errors_flow_through_and_the_parser_still_recovers() {
    let text = "fn f() -> Int {\n    let x = 0b2;\n    7\n}\n";
    let result = parse_str(text);
    let all: Vec<String> = result
        .all_diagnostics()
        .iter()
        .map(|d| d.code.to_string())
        .collect();
    // One parser recovery (pointing at the statement start) and one lexical
    // error (pointing at the bad literal inside it), in source order.
    assert_eq!(all, ["P0002", "L0008"]);
    assert_eq!(count(&result, SyntaxKind::FunctionItem), 1);
}

#[test]
fn an_unclosed_item_at_eof_is_one_error_island() {
    let text = "fn ok() -> Int { 1 }\n\nfn open() {\n    let x = 2;\n";
    let result = parse_str(text);
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.diagnostics[0].code.to_string(), "P0002");
    assert_eq!(count(&result, SyntaxKind::FunctionItem), 1);
    assert_eq!(count(&result, SyntaxKind::Error), 1);
}

#[test]
fn nesting_beyond_the_limit_is_p0003_not_a_crash() {
    let text = format!(
        "fn f() {{ let x = {}1{}; }}\n",
        "(".repeat(500),
        ")".repeat(500)
    );
    let result = parse_str(&text);
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.diagnostics[0].code.to_string(), "P0003");
    // Losslessness holds even on the guarded path.
    result.tree.check_coverage().expect("coverage");
    assert_eq!(result.tree.reconstruct(&text), text);
}

#[test]
fn diagnostics_arrive_in_source_order() {
    let text = "fn a() { ! ; }\nfn b() { ) ; }\nfn c() { ] ; }\n";
    let result = parse_str(text);
    assert_eq!(result.diagnostics.len(), 3);
    let starts: Vec<usize> = result
        .diagnostics
        .iter()
        .map(|d| d.primary_span.range().start().as_usize())
        .collect();
    let mut sorted = starts.clone();
    sorted.sort_unstable();
    assert_eq!(starts, sorted);
    assert_eq!(count(&result, SyntaxKind::FunctionItem), 3);
}
