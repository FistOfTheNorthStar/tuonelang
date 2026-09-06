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
    result
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code.to_string() == "T0022")
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
        .filter(|d| d.code.to_string() == "T0022")
        .count();
    assert_eq!(
        warnings, expected,
        "unexpected number of `T0022` advisories"
    );
    for diagnostic in result.diagnostics.iter() {
        if diagnostic.code.to_string() == "T0022" {
            assert_eq!(
                diagnostic.severity,
                tuo_diagnostics::Severity::Warning,
                "`T0022` must be a warning, never an error"
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
