//! Where a return-type mismatch is reported (DOGFOODING finding D-5a).
//!
//! A function whose body evaluates to the wrong type used to be reported at
//! the body's opening brace. The brace sits directly after the return
//! annotation, so the caret read as blaming the signature for what the body
//! got wrong — a correct diagnosis at a misleading location. The mismatch now
//! points at the expression the body evaluates to.
//!
//! The tests assert on the **text under the span** rather than on offsets, so
//! they state the contract a reader cares about: which source the caret
//! covers.

use tuo_ast::Ast;
use tuo_source::SourceMap;

/// Type-check `source` and return the source text covered by the primary span
/// of each `T0001` diagnostic, in report order.
fn mismatch_texts(source: &str) -> Vec<String> {
    let mut map = SourceMap::new();
    let file = map.intern_file("return_span.tuo");
    let id = map.add_source(file, source).expect("test source fits");
    let parse = tuo_parser::parse(map.source(id));
    let asts = [Ast::new(&parse.tree, source)];
    let resolution = tuo_resolve::resolve(&asts);
    let types = tuo_types::check(&asts, &resolution);
    types
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.code.to_string() == "T0001")
        .map(|diagnostic| {
            map.source(id)
                .slice(diagnostic.primary_span.range())
                .expect("a diagnostic span lies inside its source")
                .to_owned()
        })
        .collect()
}

#[test]
fn a_wrong_tail_expression_is_reported_at_the_expression() {
    let texts = mismatch_texts("fn f(take n: Int) -> Int {\n    n < 10\n}\n");
    assert_eq!(texts, vec!["n < 10".to_owned()]);
}

#[test]
fn statements_before_the_tail_do_not_move_the_report() {
    let texts = mismatch_texts(
        "fn f(take n: Int) -> Int {\n    let m = n + 1;\n    let k = m + 1;\n    k == 3\n}\n",
    );
    assert_eq!(texts, vec!["k == 3".to_owned()]);
}

#[test]
fn a_block_form_tail_is_reported_as_a_whole() {
    // An `if` standing last is the body's value. Its branches agree with each
    // other, so the narrowest honest location is the whole expression.
    let texts =
        mismatch_texts("fn f(take n: Int) -> Int {\n    if n > 0 { true } else { false }\n}\n");
    assert_eq!(texts.len(), 1, "exactly one mismatch: {texts:?}");
    assert!(texts[0].starts_with("if n > 0"), "got: {:?}", texts[0]);
    assert!(texts[0].ends_with('}'), "got: {:?}", texts[0]);
}

#[test]
fn a_body_with_no_value_is_still_reported_at_the_body() {
    // The last statement ends in `;`, so the body evaluates to `()` and there
    // is no expression to blame. Pointing at the `let` would be wrong: it is
    // well typed. The body itself is the only honest location.
    let texts = mismatch_texts("fn f(take n: Int) -> Int {\n    let m = n;\n}\n");
    assert_eq!(texts.len(), 1, "exactly one mismatch: {texts:?}");
    assert!(texts[0].starts_with('{'), "got: {:?}", texts[0]);
}

#[test]
fn a_well_typed_body_reports_nothing() {
    assert!(mismatch_texts("fn f(take n: Int) -> Int {\n    n + 1\n}\n").is_empty());
}
