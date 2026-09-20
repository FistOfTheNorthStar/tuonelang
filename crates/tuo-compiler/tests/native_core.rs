//! The runnable-core advisory (`T0022`): `tuo check` accepts a larger
//! language than the native backends lower, and this warning makes that gap
//! *local and visible* instead of a spanless whole-program failure at build
//! time.
//!
//! The load-bearing properties pinned here:
//!
//! 1. A heap-wrapper **value** in storage position warns, at the span of the
//!    written type.
//! 2. A heap wrapper in a **field or variant payload declaration** does
//!    **not** warn — such a declaration lowers fine, and `T0016` actively
//!    tells the user to reach for one to break a recursive type. Warning
//!    there would contradict the compiler's own advice.
//! 3. The advisory is a **warning**, never an error: these programs are
//!    legal tuonelang that the reference interpreter executes, so
//!    `has_errors` must stay false and the accepted language must not
//!    shrink.

use tuo_compiler::source::SourceMap;
use tuo_compiler::{CheckResult, check_sources};

fn check(source: &str) -> CheckResult {
    let mut map = SourceMap::new();
    let file = map.intern_file("advisory.tuo");
    let id = map.add_source(file, source).expect("test source fits");
    check_sources(&map, &[id])
}

/// The `T0022` diagnostics of a program, as `(start, end, message)`.
fn advisories(result: &CheckResult) -> Vec<(usize, usize, String)> {
    coded(result, "T0022")
}

/// The `T0023` (generic-declaration) diagnostics, same shape.
fn generic_advisories(result: &CheckResult) -> Vec<(usize, usize, String)> {
    coded(result, "T0023")
}

/// The diagnostics carrying `want`, as `(start, end, message)`.
fn coded(result: &CheckResult, want: &str) -> Vec<(usize, usize, String)> {
    result
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code.to_string() == want)
        .map(|diagnostic| {
            let span = diagnostic.primary_span;
            (
                span.range().start().as_usize(),
                span.range().end().as_usize(),
                diagnostic.message.clone(),
            )
        })
        .collect()
}

/// Every `T0022` is a warning and nothing else in the program is an error.
fn is_accepted_with_warnings(result: &CheckResult, expected: usize) {
    accepted_with(result, "T0022", expected);
}

/// Every `T0023` is a warning and nothing else in the program is an error.
fn is_accepted_with_generic_warnings(result: &CheckResult, expected: usize) {
    accepted_with(result, "T0023", expected);
}

/// The shared contract both advisories hold: `want` appears exactly
/// `expected` times, always as a warning, and the program is still accepted.
fn accepted_with(result: &CheckResult, want: &str, expected: usize) {
    assert!(
        !result.has_errors(),
        "the advisory must never reject a program; diagnostics: {:#?}",
        result
            .diagnostics
            .iter()
            .map(|d| (d.code.to_string(), d.message.clone()))
            .collect::<Vec<_>>()
    );
    let warnings = result
        .diagnostics
        .iter()
        .filter(|d| d.code.to_string() == want)
        .count();
    assert_eq!(
        warnings, expected,
        "unexpected number of `{want}` advisories"
    );
    for diagnostic in result.diagnostics.iter() {
        if diagnostic.code.to_string() == want {
            assert_eq!(
                diagnostic.severity,
                tuo_diagnostics::Severity::Warning,
                "`{want}` must be a warning, never an error"
            );
        }
    }
}

#[test]
fn a_wrapper_parameter_warns_at_the_type_span() {
    let source = "fn keep(take b: Box[Int]) -> Int {\n    1\n}\n";
    let result = check(source);
    is_accepted_with_warnings(&result, 1);

    let found = advisories(&result);
    let (start, end, message) = &found[0];
    assert_eq!(
        &source[*start..*end],
        "Box[Int]",
        "the advisory must point at the written type, not the function"
    );
    assert!(
        message.contains("parameter"),
        "the message must name the storage position, got: {message}"
    );
}

#[test]
fn a_wrapper_return_type_warns() {
    let result = check("fn f() -> Weak[Int] {\n    f()\n}\n");
    is_accepted_with_warnings(&result, 1);
    assert!(advisories(&result)[0].2.contains("return type"));
}

#[test]
fn a_nested_wrapper_is_found_inside_a_generic_argument() {
    // `Array[Box[Int]]` is refused by the backends at the same
    // classification step as a bare `Box[Int]`, so the walk must descend
    // into type arguments rather than matching only the head.
    let result = check("fn f(take xs: Array[Box[Int]]) -> Int {\n    0\n}\n");
    is_accepted_with_warnings(&result, 1);
}

/// The consistency property with `T0016`: breaking a recursive type with a
/// wrapper is the compiler's own recommendation, so the declaration that
/// does it must not be warned about.
#[test]
fn wrapper_declarations_do_not_warn() {
    let result = check(
        "struct Node {\n\
         \x20   value: Int,\n\
         \x20   parent: Weak[Node],\n\
         }\n\
         \n\
         struct Tree {\n\
         \x20   root: Shared[Node],\n\
         }\n\
         \n\
         enum List {\n\
         \x20   Nil,\n\
         \x20   Cons { head: Int, tail: Box[List] },\n\
         }\n\
         \n\
         fn main() -> Int {\n\
         \x20   0\n\
         }\n",
    );
    is_accepted_with_warnings(&result, 0);
}

/// A program entirely inside the runnable core stays completely silent —
/// the advisory must not become background noise on ordinary code.
#[test]
fn a_runnable_core_program_produces_no_advisory() {
    let result = check(
        "fn add(take a: Int, take b: Int) -> Int {\n\
         \x20   a + b\n\
         }\n\
         \n\
         fn main() -> Int {\n\
         \x20   add(1, 2)\n\
         }\n",
    );
    is_accepted_with_warnings(&result, 0);
    assert!(
        result.diagnostics.is_empty(),
        "a runnable-core program must check completely clean"
    );
}

// --- The generic advisory (`T0023`, ADR-0027 Stage A) --------------------
//
// The same gap as `T0022`, reached by a different construct. A generic `fn`
// checks, ownership-checks, and *executes on the reference interpreter*,
// while both backends refuse it because `Ty::Param` has no layout until
// monomorphized. Before this advisory that refusal surfaced only at build
// time, spanless.

/// The load-bearing case: a generic declaration warns at the span of its
/// parameter list, and the program is still accepted.
#[test]
fn a_generic_function_warns_at_the_parameter_list_span() {
    let source = "fn ident[T](take x: T) -> T {\n    x\n}\n";
    let result = check(source);
    is_accepted_with_generic_warnings(&result, 1);

    let found = generic_advisories(&result);
    let (start, end, message) = &found[0];
    assert_eq!(
        &source[*start..*end],
        "[T]",
        "the advisory must point at the generic parameter list, which is what \
         makes the body unlowerable and what a reader would delete to fix it"
    );
    assert!(
        message.contains("ident"),
        "the message must name the function, got: {message}"
    );
}

/// Several parameters are all named, so the reader sees which types are
/// unlowerable rather than just that some are.
#[test]
fn every_generic_parameter_is_named_in_one_advisory() {
    let result = check("fn pick[A, B](take a: A, take b: B) -> A {\n    let _ = b;\n    a\n}\n");
    is_accepted_with_generic_warnings(&result, 1);

    let found = generic_advisories(&result);
    let labels = found[0].2.clone();
    assert!(labels.contains("pick"), "got: {labels}");
}

/// One warning per *declaration*, never per call site: the instantiation is
/// not what the backend refuses, the body is.
#[test]
fn a_generic_function_warns_once_however_often_it_is_called() {
    let result = check(
        "fn ident[T](take x: T) -> T {\n\
         \x20   x\n\
         }\n\
         \n\
         fn main() -> Int {\n\
         \x20   ident(1) + ident(2) + ident(3)\n\
         }\n",
    );
    is_accepted_with_generic_warnings(&result, 1);
}

/// A non-generic program stays silent — the advisory must not become
/// background noise on ordinary code.
#[test]
fn a_concrete_function_produces_no_generic_advisory() {
    let result = check("fn twice(take x: Int) -> Int {\n    x + x\n}\n");
    is_accepted_with_generic_warnings(&result, 0);
    assert!(
        result.diagnostics.is_empty(),
        "a concrete program must check completely clean"
    );
}

/// The advisory is a warning, so the accepted language is unchanged: a
/// generic program still checks clean enough to reach `tuo spec`, which is
/// the whole reason it is not an error.
#[test]
fn a_generic_program_is_still_accepted() {
    let result = check("fn ident[T](take x: T) -> T {\n    x\n}\n");
    assert!(
        !result.has_errors(),
        "a generic function is legal tuonelang the interpreter executes; \
         warning about it must not shrink the accepted language"
    );
}

/// A *bounded* generic still warns. The bound itself is a separate error —
/// v0 declares no interface types, so `T: Ord` fails resolution with
/// `R0002` — but the advisory is about the parameter's missing layout, which
/// is true regardless of whether the bound resolves. The two diagnostics are
/// independent and both belong.
#[test]
fn a_bounded_generic_still_warns_about_its_parameter() {
    let source = "fn f[T: Ord](take x: T) -> T {\n    x\n}\n";
    let result = check(source);

    let found = generic_advisories(&result);
    assert_eq!(found.len(), 1, "a bounded generic is still a generic");
    let (start, end, _) = &found[0];
    assert_eq!(
        &source[*start..*end],
        "[T: Ord]",
        "the span covers the whole parameter list, bound included"
    );
}

/// The grammar allows a trailing comma in the parameter list, so the
/// advisory must handle it: the span covers the written list verbatim and
/// the comma is not mistaken for a second, unnamed parameter.
#[test]
fn a_trailing_comma_in_the_parameter_list_is_handled() {
    let source = "fn g[T,](take x: T) -> T {\n    x\n}\n";
    let result = check(source);
    is_accepted_with_generic_warnings(&result, 1);

    let found = generic_advisories(&result);
    let (start, end, _) = &found[0];
    assert_eq!(&source[*start..*end], "[T,]");
}

/// `fn f[]` is now its own `P0001` parse error (rather than two whole-item
/// recovery skips) and still produces a `GenericParams` node. The advisory
/// must stay silent on it: the function declares no type parameters, so
/// there is nothing to monomorphize, and a layout warning stacked on a
/// parse error would be noise rather than help.
#[test]
fn an_empty_generic_parameter_list_produces_no_advisory() {
    let result = check("fn f[](take x: Int) -> Int {\n    1\n}\n");
    assert!(
        generic_advisories(&result).is_empty(),
        "an empty list declares no type parameters; the parse error is the \
         only diagnostic that belongs here"
    );
}
