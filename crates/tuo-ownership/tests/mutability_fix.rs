//! `O0004` carries the keyword edit that makes the place mutable, so the
//! LSP's quick fix and the agent's `apply_safe_fix` can apply it.
//!
//! A `let` becomes `var` and a `Copy` `in` parameter becomes `take`, both
//! machine-applicable. A non-`Copy` `in` parameter becomes `mut` only as a
//! probable fix, since its callers would then have to pass a mutable place
//! and would see the write. A `Copy` `in` parameter of a function that is
//! also named as a value becomes `take` only probably too, since the mode is
//! part of the `fn(…)` type its use site expects. A pattern binding has no
//! keyword, so it keeps the help text and gets no edit.
//!
//! The load-bearing test is the last one: applying every machine-applicable
//! fix to the `O0004` fixture leaves a program that passes the whole front
//! end with no `O0004` left except those that had no such fix. A fix that
//! did not compile, or that broke a caller, would fail it.

use std::fs;
use std::path::PathBuf;

use tuo_ast::Ast;
use tuo_diagnostics::{Confidence, Diagnostic, Edit, Suggestion};
use tuo_source::SourceMap;

/// Every front-end diagnostic for `text`, then the ownership ones.
fn diagnose(text: &str) -> (Vec<Diagnostic>, Vec<Diagnostic>) {
    let mut map = SourceMap::new();
    let file = map.intern_file("fix.tuo");
    let id = map.add_source(file, text).expect("source fits");
    let parse = tuo_parser::parse(map.source(id));
    let asts = [Ast::new(&parse.tree, text)];
    let resolution = tuo_resolve::resolve(&asts);
    let types = tuo_types::check(&asts, &resolution);
    let mut front: Vec<Diagnostic> = parse.diagnostics.clone();
    front.extend(resolution.diagnostics().iter().cloned());
    front.extend(types.diagnostics().iter().cloned());
    let ownership = tuo_ownership::check(&asts, &resolution, &types)
        .diagnostics()
        .to_vec();
    (front, ownership)
}

fn o0004(text: &str) -> Vec<Diagnostic> {
    let (front, ownership) = diagnose(text);
    assert_eq!(front, vec![], "the sample must pass the front end");
    ownership
        .into_iter()
        .filter(|diagnostic| diagnostic.code.to_string() == "O0004")
        .collect()
}

/// The one `O0004` in `text`, and its one suggestion.
fn only_fix(text: &str) -> (Diagnostic, Option<Suggestion>) {
    let diagnostics = o0004(text);
    assert_eq!(diagnostics.len(), 1, "expected one O0004: {diagnostics:#?}");
    let diagnostic = diagnostics.into_iter().next().expect("one");
    assert!(diagnostic.suggestions.len() <= 1, "at most one fix");
    let suggestion = diagnostic.suggestions.first().cloned();
    (diagnostic, suggestion)
}

/// The text `edit` replaces.
fn replaced<'a>(text: &'a str, edit: &Edit) -> &'a str {
    let range = edit.span.range();
    &text[range.start().as_usize()..range.end().as_usize()]
}

fn assert_keyword_fix(text: &str, from: &str, to: &str, confidence: Confidence) {
    let (diagnostic, suggestion) = only_fix(text);
    let suggestion = suggestion.unwrap_or_else(|| panic!("no fix on {diagnostic:#?}"));
    assert_eq!(suggestion.confidence, confidence, "{suggestion:#?}");
    let [edit] = suggestion.edits.as_slice() else {
        panic!("one edit expected: {suggestion:#?}");
    };
    assert_eq!(replaced(text, edit), from);
    assert_eq!(edit.replacement, to);
}

#[test]
fn an_assigned_let_becomes_var() {
    let text = "fn f(take n: Int) -> Int {\n    let x = n;\n    x = n + 1;\n    x\n}\n";
    assert_keyword_fix(text, "let", "var", Confidence::MachineApplicable);
}

#[test]
fn a_reinitialized_deferred_let_becomes_var() {
    let text = "fn f() -> Int {\n    let x: Int;\n    x = 1;\n    x = 2;\n    x\n}\n";
    assert_keyword_fix(text, "let", "var", Confidence::MachineApplicable);
}

#[test]
fn a_let_passed_as_mut_becomes_var() {
    let text = "fn bump(mut n: Int) {\n    n = n + 1;\n}\n\n\
                fn f(take n: Int) -> Int {\n    let x = n;\n    bump(x);\n    x\n}\n";
    assert_keyword_fix(text, "let", "var", Confidence::MachineApplicable);
}

#[test]
fn a_let_written_through_an_index_becomes_var() {
    let text = "fn f() -> Int {\n    let xs = [1, 2, 3];\n    xs[0] = 5;\n    xs[0]\n}\n";
    assert_keyword_fix(text, "let", "var", Confidence::MachineApplicable);
}

#[test]
fn an_assigned_copy_in_parameter_becomes_take() {
    let text = "fn f(in n: Int) -> Int {\n    n = 5;\n    n\n}\n";
    assert_keyword_fix(text, "in", "take", Confidence::MachineApplicable);
}

#[test]
fn a_non_copy_in_parameter_becomes_mut_only_probably() {
    let text = "struct Named {\n    name: String,\n    count: Int,\n}\n\n\
                fn f(in p: Named) {\n    p.count = 5;\n}\n";
    assert_keyword_fix(text, "in", "mut", Confidence::Probable);
}

#[test]
fn a_pattern_binding_has_no_keyword_to_change() {
    let text = "fn f(take o: Option[Int]) -> Int {\n    match o {\n        \
                Some { value } => {\n            value = 1;\n            value\n        },\n        \
                None => 0,\n    }\n}\n";
    let (diagnostic, suggestion) = only_fix(text);
    assert_eq!(suggestion, None, "{diagnostic:#?}");
    assert!(
        diagnostic
            .help
            .as_deref()
            .is_some_and(|help| help.contains("var value")),
        "the help text stays: {diagnostic:#?}"
    );
}

/// A function named as a value has its modes in a `fn(…)` type some use
/// site expects: `fn(in Int) -> Int` no longer accepts it once `in` becomes
/// `take`, so the edit is not caller-neutral and is only probable. The value
/// use comes after the function, so the checker must not depend on order.
#[test]
fn a_copy_in_parameter_of_a_function_value_becomes_take_only_probably() {
    let text = "fn bump(in n: Int) -> Int {\n    n = n + 1;\n    n\n}\n\n\
                fn apply(in f: fn(in Int) -> Int, in x: Int) -> Int {\n    f(x)\n}\n\n\
                fn caller() -> Int {\n    bump(1) + apply(bump, 41)\n}\n";
    assert_keyword_fix(text, "in", "take", Confidence::Probable);
}

#[test]
fn a_function_value_bound_to_a_let_counts_as_a_value_use() {
    let text = "fn bump(in n: Int) -> Int {\n    n = n + 1;\n    n\n}\n\n\
                fn caller() -> Int {\n    let g = bump;\n    g(1)\n}\n";
    assert_keyword_fix(text, "in", "take", Confidence::Probable);
}

/// Apply `edits` to `text`; overlapping edits (the same keyword fixed for
/// two diagnostics) are applied once.
fn apply(text: &str, mut edits: Vec<Edit>) -> String {
    edits.sort_by_key(|edit| edit.span.range().start().as_usize());
    edits.dedup_by(|a, b| a.span == b.span && a.replacement == b.replacement);
    let mut out = text.to_owned();
    for edit in edits.iter().rev() {
        let range = edit.span.range();
        out.replace_range(
            range.start().as_usize()..range.end().as_usize(),
            &edit.replacement,
        );
    }
    out
}

/// Apply every machine-applicable `O0004` fix in `text`.
fn apply_machine_fixes(text: &str) -> String {
    let edits: Vec<Edit> = o0004(text)
        .iter()
        .flat_map(|diagnostic| &diagnostic.suggestions)
        .filter(|suggestion| suggestion.confidence == Confidence::MachineApplicable)
        .flat_map(|suggestion| suggestion.edits.iter().cloned())
        .collect();
    assert!(!edits.is_empty(), "the sample must carry a fix");
    apply(text, edits)
}

/// The fixture's functions are never called, so it cannot catch a fix that
/// breaks a caller. Here the fixed functions are called with a `let`
/// binding, a literal, and a field of an `in` parameter, none of which is a
/// mutable place: `take` accepts all three, `mut` would accept none.
#[test]
fn a_take_fix_leaves_every_caller_compiling() {
    let text = "struct Pair {\n    a: Int,\n    b: Int,\n}\n\n\
                fn clamp(in n: Int) -> Int {\n    if n > 9 {\n        n = 9;\n    }\n    n\n}\n\n\
                fn swap(in p: Pair) -> Pair {\n    p.a = p.b;\n    p\n}\n\n\
                fn caller(in q: Pair) -> Int {\n    let x = 12;\n    let p = Pair { a: 1, b: 2 };\n    \
                clamp(x) + clamp(3) + clamp(q.a) + swap(p).a + swap(q).a + x\n}\n";
    let fixed = apply_machine_fixes(text);
    assert_eq!(fixed.matches("take").count(), 2, "{fixed}");
    let (front, ownership) = diagnose(&fixed);
    assert_eq!(front, vec![], "{fixed}");
    assert_eq!(ownership, vec![], "{fixed}");
}

#[test]
fn every_machine_applicable_fix_in_the_fixture_compiles_and_clears_its_error() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/ownership/fixtures/err/immutable_places.tuo");
    let text = fs::read_to_string(path).expect("fixture is readable");
    let before = o0004(&text);
    let (fixable, unfixable): (Vec<&Diagnostic>, Vec<&Diagnostic>) =
        before.iter().partition(|diagnostic| {
            diagnostic
                .suggestions
                .iter()
                .any(|suggestion| suggestion.confidence == Confidence::MachineApplicable)
        });
    // Most of the fixture is `let` bindings and `Copy` `in` parameters.
    assert!(
        fixable.len() >= 10,
        "only {} of {} O0004s carry a machine-applicable fix",
        fixable.len(),
        before.len()
    );
    let edits: Vec<Edit> = fixable
        .iter()
        .flat_map(|diagnostic| &diagnostic.suggestions)
        .filter(|suggestion| suggestion.confidence == Confidence::MachineApplicable)
        .flat_map(|suggestion| suggestion.edits.iter().cloned())
        .collect();
    let fixed = apply(&text, edits);
    let (front, ownership) = diagnose(&fixed);
    assert_eq!(front, vec![], "the fixed program must pass the front end");
    let after: Vec<&str> = ownership
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect();
    let expected: Vec<&str> = unfixable
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect();
    assert_eq!(
        after, expected,
        "only the O0004s without a machine-applicable fix may remain, and nothing new"
    );
}
