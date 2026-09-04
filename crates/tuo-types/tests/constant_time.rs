//! The ADR-0020 Stage C constant-time gate: `#[constant_time]` and the
//! diagnostics `T0017`–`T0021`.
//!
//! The gate is a *syntactic* discipline over a marked function's body. It
//! rejects anything whose control flow or running time can depend on the data:
//! branches, bounds-checked indexing, trapping arithmetic, and calls to
//! functions that carry no such guarantee themselves.
//!
//! Two properties matter more than the individual rules, and both are pinned
//! below. First, the marking must be **load-bearing** — an unmarked function is
//! not checked, and an unrecognized attribute is an error rather than a
//! decoration, so nobody can believe in a guarantee that was never verified.
//! Second, the rule is **sufficient rather than necessary**: it refuses some
//! code that is genuinely constant time, because the compiler cannot follow the
//! argument for why it is safe, and accepting on an unverifiable argument is
//! the exact failure this discipline exists to prevent.

use tuo_ast::Ast;
use tuo_resolve::Resolution;
use tuo_types::TypeckResult;

fn check(source: &str) -> (Resolution, TypeckResult) {
    let mut map = tuo_source::SourceMap::new();
    let file = map.intern_file("test.tuo");
    let id = map.add_source(file, source).expect("test source fits");
    let parse = tuo_parser::parse(map.source(id));
    assert_eq!(parse.diagnostics, vec![], "test programs must parse");
    let asts = [Ast::new(&parse.tree, source)];
    let resolution = tuo_resolve::resolve(&asts);
    assert_eq!(
        resolution.diagnostics(),
        &[],
        "test programs must resolve cleanly"
    );
    let types = tuo_types::check(&asts, &resolution);
    (resolution, types)
}

/// The diagnostic codes a program produces, rendered as `T0017`-style strings.
fn codes(source: &str) -> Vec<String> {
    let (_, types) = check(source);
    types
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.code.to_string())
        .collect()
}

/// The branchless subset is accepted: shifts, bitwise operators, `let`
/// bindings, and calls to other marked functions.
///
/// This is the shape `std::ct` is written in, so a regression here would break
/// the library the whole ADR was opened to make checkable.
#[test]
fn the_branchless_subset_is_accepted() {
    assert_eq!(
        codes(
            "#[constant_time]\n\
             fn mask(take bit: Int) -> Int { (bit << 63) >> 63 }\n\
             #[constant_time]\n\
             fn select(take c: Int, take a: Int, take b: Int) -> Int {\n\
                 let m = mask(c);\n\
                 (a & m) | (b & ~m)\n\
             }\n"
        ),
        Vec::<String>::new()
    );
}

/// Every data-dependent control-flow form is rejected (`T0017`).
#[test]
fn branches_are_rejected() {
    for body in [
        "if x == 1 { 1 } else { 0 }",
        "match x { 1 => 1, _ => 0 }",
        "{ var i = x; while i > 0 { i = i >> 1; } i }",
    ] {
        let source = format!("#[constant_time]\nfn f(take x: Int) -> Int {{ {body} }}\n");
        assert!(
            codes(&source).contains(&"T0017".to_string()),
            "expected T0017 for a body containing `{body}`, got {:?}",
            codes(&source)
        );
    }
}

/// Array indexing is rejected (`T0018`): its bounds check is a branch on the
/// index.
#[test]
fn indexing_is_rejected() {
    assert!(
        codes(
            "#[constant_time]\n\
             fn f(in xs: Array[Int], take i: Usize) -> Int { xs[i] }\n"
        )
        .contains(&"T0018".to_string())
    );
}

/// Trapping arithmetic is rejected (`T0019`): the overflow check is a branch
/// on the operands.
///
/// This is the rule that shaped `std::ct`. The conventional way to build an
/// all-ones mask is `0 - bit`, and negation traps on the most negative
/// integer, so the idiom every constant-time library in C reaches for is the
/// one tuonelang must refuse.
#[test]
fn trapping_arithmetic_is_rejected() {
    for body in ["a + b", "a - b", "a * b", "a / b", "a % b"] {
        let source =
            format!("#[constant_time]\nfn f(take a: Int, take b: Int) -> Int {{ {body} }}\n");
        assert!(
            codes(&source).contains(&"T0019".to_string()),
            "expected T0019 for `{body}`, got {:?}",
            codes(&source)
        );
    }
}

/// Shifts and bitwise operators are *not* rejected, which is what makes the
/// subset usable at all.
///
/// Without this the arithmetic rule would be indiscriminate and `std::ct`
/// could not be written; it is asserted separately so a broadened `T0019`
/// cannot pass by rejecting everything.
#[test]
fn shifts_and_bitwise_operators_are_allowed() {
    assert_eq!(
        codes(
            "#[constant_time]\n\
             fn f(take a: Int, take b: Int) -> Int {\n\
                 ((a & b) | (a ^ b) | ~a) ^ ((a << 1) >> 1)\n\
             }\n"
        ),
        Vec::<String>::new()
    );
}

/// A marked function may not call an unmarked one (`T0020`).
#[test]
fn calling_an_unmarked_function_is_rejected() {
    assert!(
        codes(
            "fn helper(take x: Int) -> Int { x }\n\
             #[constant_time]\n\
             fn f(take x: Int) -> Int { helper(x) }\n"
        )
        .contains(&"T0020".to_string())
    );
}

/// Marking the callee too resolves it — the guarantee composes along the call
/// graph rather than stopping at one function.
#[test]
fn a_marked_callee_is_accepted() {
    assert_eq!(
        codes(
            "#[constant_time]\n\
             fn helper(take x: Int) -> Int { x >> 1 }\n\
             #[constant_time]\n\
             fn f(take x: Int) -> Int { helper(x) }\n"
        ),
        Vec::<String>::new()
    );
}

/// An unmarked function is not checked at all.
///
/// The attribute is what carries the guarantee, so its absence must mean
/// "unchecked" rather than "checked and fine". If ordinary code were silently
/// held to this rule, the whole language would stop compiling — and if marked
/// code were silently exempt, the attribute would mean nothing.
#[test]
fn an_unmarked_function_is_not_checked() {
    assert_eq!(
        codes(
            "fn ordinary(take a: Int, in xs: Array[Int]) -> Int {\n\
                 if a > 0 { a + 1 } else { 0 }\n\
             }\n"
        ),
        Vec::<String>::new()
    );
}

/// An unknown attribute is an error (`T0021`), never silently ignored.
///
/// This is the load-bearing honesty rule of the whole feature. A misspelled
/// `#[constant_tim]` that compiled clean would leave the author believing the
/// compiler had verified something it never looked at — which is strictly
/// worse than having no attribute at all, because it manufactures false
/// confidence.
#[test]
fn an_unknown_attribute_is_an_error() {
    let found = codes("#[constant_tim]\nfn f(take x: Int) -> Int { x }\n");
    assert!(
        found.contains(&"T0021".to_string()),
        "a misspelled attribute must be reported, got {found:?}"
    );
}

/// The gate is *sufficient, not necessary*: it rejects code that is in fact
/// constant time, when it cannot verify why.
///
/// `(a >> 1) - (b >> 1)` provably cannot overflow — halving bounds both
/// operands to `[MIN/2, MAX/2]`, so their difference is at most `MAX` in
/// magnitude and the trap is unreachable. The checker refuses it anyway,
/// because it cannot follow that argument, and accepting on an argument it
/// cannot check is the failure mode ADR-0020 exists to prevent.
///
/// This asymmetry is a design decision rather than an incompleteness to fix
/// later, so it is pinned: were the checker taught to accept this, the change
/// should be deliberate and should break this test.
#[test]
fn a_provably_safe_subtraction_is_still_rejected() {
    assert!(
        codes(
            "#[constant_time]\n\
             fn lt(take a: Int, take b: Int) -> Int { ((a >> 1) - (b >> 1)) >> 63 }\n"
        )
        .contains(&"T0019".to_string()),
        "the gate is deliberately conservative; it must refuse what it cannot verify"
    );
}

/// Every rejection is reported, not just the first, so one compile shows the
/// author all the work.
#[test]
fn all_violations_in_one_function_are_reported() {
    let found = codes(
        "fn helper(take x: Int) -> Int { x }\n\
         #[constant_time]\n\
         fn f(take a: Int, in xs: Array[Int], take i: Usize) -> Int {\n\
             let branch = if a > 0 { 1 } else { 0 };\n\
             let sum = a + 1;\n\
             let element = xs[i];\n\
             let called = helper(a);\n\
             branch ^ sum ^ element ^ called\n\
         }\n",
    );
    for code in ["T0017", "T0018", "T0019", "T0020"] {
        assert!(
            found.contains(&code.to_string()),
            "expected {code} among {found:?}"
        );
    }
}
