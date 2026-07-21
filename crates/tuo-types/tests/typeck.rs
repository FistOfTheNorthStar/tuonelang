//! Integration tests for the type checker: real parses, real resolution,
//! then assertions over types, diagnostics, and their structured payloads.

use tuo_ast::Ast;
use tuo_resolve::{Resolution, SymbolId, SymbolKind};
use tuo_source::SourceMap;
use tuo_syntax::SyntaxTree;
use tuo_types::TypeckResult;

struct Program {
    files: Vec<(SyntaxTree, String)>,
}

impl Program {
    fn parse(sources: &[&str]) -> Self {
        let mut map = SourceMap::new();
        let files = sources
            .iter()
            .enumerate()
            .map(|(index, text)| {
                let file = map.intern_file(&format!("file{index}.tuo"));
                let id = map.add_source(file, *text).expect("test source fits");
                let result = tuo_parser::parse(map.source(id));
                (result.tree, (*text).to_owned())
            })
            .collect();
        Self { files }
    }

    fn asts(&self) -> Vec<Ast<'_>> {
        self.files
            .iter()
            .map(|(tree, text)| Ast::new(tree, text))
            .collect()
    }
}

/// Parse, resolve, and check one source file; the resolution must be clean
/// (the test targets *type* errors, not name errors).
fn check_one(source: &str) -> (Resolution, TypeckResult) {
    let program = Program::parse(&[source]);
    let asts = program.asts();
    let resolution = tuo_resolve::resolve(&asts);
    assert_eq!(
        resolution.diagnostics(),
        &[],
        "test source must resolve cleanly"
    );
    let result = tuo_types::check(&asts, &resolution);
    (resolution, result)
}

fn codes(result: &TypeckResult) -> Vec<String> {
    result
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.code.to_string())
        .collect()
}

fn assert_clean(result: &TypeckResult) {
    assert_eq!(result.diagnostics(), &[], "expected no type diagnostics");
}

fn find(resolution: &Resolution, name: &str, kind: SymbolKind) -> SymbolId {
    resolution
        .symbols()
        .find(|(_, symbol)| symbol.name == name && symbol.kind == kind)
        .map(|(id, _)| id)
        .unwrap_or_else(|| panic!("no {kind:?} symbol named `{name}`"))
}

fn rendered_type(resolution: &Resolution, result: &TypeckResult, name: &str) -> String {
    let symbol = resolution
        .symbols()
        .find(|(_, symbol)| {
            symbol.name == name
                && matches!(
                    symbol.kind,
                    SymbolKind::Local | SymbolKind::Param | SymbolKind::Const
                )
        })
        .map(|(id, _)| id)
        .unwrap_or_else(|| panic!("no binding named `{name}`"));
    result
        .type_of(symbol)
        .unwrap_or_else(|| panic!("no type recorded for `{name}`"))
        .render(resolution)
}

// ----------------------------------------------------------------------
// Positive: signatures, calls, returns
// ----------------------------------------------------------------------

#[test]
fn explicit_signatures_check_calls_and_returns() {
    let (resolution, result) = check_one(
        "fn add(in a: Int, in b: Int) -> Int { a + b }\n\
         fn double(in n: Int) -> Int { add(n, n) }\n\
         fn main() -> Int { double(21) }\n",
    );
    assert_clean(&result);
    let add = find(&resolution, "add", SymbolKind::Function);
    assert_eq!(
        result.type_of(add).expect("fn type").render(&resolution),
        "fn(I64, I64) -> I64"
    );
}

#[test]
fn omitted_return_type_means_unit() {
    let (_, result) = check_one("fn noop() {}\nfn call_it() { noop() }\n");
    assert_clean(&result);
}

#[test]
fn early_return_checks_against_the_declared_type() {
    let (_, result) = check_one(
        "fn pick(in flag: Bool) -> Int {\n\
             if flag { return 1; }\n\
             0\n\
         }\n",
    );
    assert_clean(&result);
}

// ----------------------------------------------------------------------
// Positive: local inference and literal defaulting
// ----------------------------------------------------------------------

#[test]
fn literals_default_to_i64_and_f64() {
    let (resolution, result) = check_one(
        "fn f() {\n\
             let n = 1;\n\
             let x = 1.5;\n\
             let s = \"text\";\n\
             let c = 'a';\n\
             let b = true;\n\
             let u = ();\n\
         }\n",
    );
    assert_clean(&result);
    assert_eq!(rendered_type(&resolution, &result, "n"), "I64");
    assert_eq!(rendered_type(&resolution, &result, "x"), "F64");
    assert_eq!(rendered_type(&resolution, &result, "s"), "Str");
    assert_eq!(rendered_type(&resolution, &result, "c"), "Char");
    assert_eq!(rendered_type(&resolution, &result, "b"), "Bool");
    assert_eq!(rendered_type(&resolution, &result, "u"), "()");
}

#[test]
fn annotations_steer_literal_widths() {
    let (resolution, result) = check_one(
        "fn f() {\n\
             let small: U8 = 7;\n\
             let wide: F32 = 1.5;\n\
             let sum = small + 1;\n\
         }\n",
    );
    assert_clean(&result);
    assert_eq!(rendered_type(&resolution, &result, "small"), "U8");
    assert_eq!(rendered_type(&resolution, &result, "wide"), "F32");
    assert_eq!(rendered_type(&resolution, &result, "sum"), "U8");
}

#[test]
fn inference_flows_through_generic_calls() {
    let (resolution, result) = check_one(
        "fn identity[T](in value: T) -> T { value }\n\
         fn f() {\n\
             let n: Int = identity(41);\n\
             let s = identity(\"hello\");\n\
             let pinned = identity::[Bool](true);\n\
         }\n",
    );
    assert_clean(&result);
    assert_eq!(rendered_type(&resolution, &result, "s"), "Str");
    assert_eq!(rendered_type(&resolution, &result, "pinned"), "Bool");
}

// ----------------------------------------------------------------------
// Positive: structs, enums, Option/Result
// ----------------------------------------------------------------------

#[test]
fn struct_literals_field_access_and_generics_check() {
    let (resolution, result) = check_one(
        "struct Point { x: F64, y: F64 }\n\
         struct Pair[A, B] { first: A, second: B }\n\
         fn f() -> F64 {\n\
             let p = Point { x: 1.0, y: 2.0 };\n\
             let q = Pair { first: 1, second: true };\n\
             let shorthand_y = 3.0;\n\
             let r = Point { x: p.x, y: shorthand_y };\n\
             p.x + r.y\n\
         }\n",
    );
    assert_clean(&result);
    assert_eq!(rendered_type(&resolution, &result, "q"), "Pair[I64, Bool]");
}

#[test]
fn option_and_result_are_first_class() {
    let (resolution, result) = check_one(
        "fn find(in n: Int) -> Option[Int] {\n\
             if n > 0 { Some { value: n } } else { None }\n\
         }\n\
         fn parse(in ok: Bool) -> Result[Int, Str] {\n\
             if ok { Ok { value: 1 } } else { Err { error: \"bad\" } }\n\
         }\n\
         fn chain(in n: Int) -> Option[Int] {\n\
             let found = find(n)?;\n\
             Some { value: found + 1 }\n\
         }\n\
         fn chain_result(in ok: Bool) -> Result[Int, Str] {\n\
             let v = parse(ok)?;\n\
             Ok { value: v }\n\
         }\n\
         fn f() {\n\
             let missing: Option[Int] = None;\n\
         }\n",
    );
    assert_clean(&result);
    assert_eq!(
        rendered_type(&resolution, &result, "missing"),
        "Option[I64]"
    );
}

#[test]
fn matches_on_enums_options_and_bools_check_exhaustively() {
    let (_, result) = check_one(
        "enum Shape { Circle { radius: F64 }, Rect { w: F64, h: F64 }, Dot }\n\
         fn area(in s: Shape) -> F64 {\n\
             match s {\n\
                 Shape::Circle { radius } => radius * radius * 3.0,\n\
                 Shape::Rect { w, h } => w * h,\n\
                 Shape::Dot => 0.0,\n\
             }\n\
         }\n\
         fn unwrap_or_zero(in v: Option[Int]) -> Int {\n\
             match v {\n\
                 Some { value } => value,\n\
                 None => 0,\n\
             }\n\
         }\n\
         fn flip(in b: Bool) -> Bool {\n\
             match b {\n\
                 true => false,\n\
                 false => true,\n\
             }\n\
         }\n\
         fn catch_all(in n: Int) -> Int {\n\
             match n {\n\
                 0 => 1,\n\
                 _ => n,\n\
             }\n\
         }\n",
    );
    assert_clean(&result);
}

#[test]
fn arrays_ranges_and_loops_check() {
    let (resolution, result) = check_one(
        "fn head(in items: Array[Int]) -> Int {\n\
             items[0]\n\
         }\n\
         fn sum_to(in limit: Int) -> Int {\n\
             var total = 0;\n\
             for n in 0 .. limit {\n\
                 total = total + n;\n\
             }\n\
             while total > 100 {\n\
                 total = total - 1;\n\
             }\n\
             total\n\
         }\n\
         fn spin() -> Int {\n\
             var n = 0;\n\
             loop {\n\
                 n = n + 1;\n\
                 if n > 3 { break n; }\n\
             }\n\
         }\n",
    );
    assert_clean(&result);
    assert_eq!(rendered_type(&resolution, &result, "total"), "I64");
}

#[test]
fn impl_bodies_type_self_as_the_target() {
    let (_, result) = check_one(
        "struct Counter { count: Int }\n\
         interface Reset {\n\
             fn reset(in self) -> Int;\n\
         }\n\
         impl Reset for Counter {\n\
             fn reset(in self) -> Int {\n\
                 self.count\n\
             }\n\
         }\n",
    );
    assert_clean(&result);
}

#[test]
fn specs_require_boolean_assertions() {
    let (_, result) = check_one(
        "fn double(in n: Int) -> Int { n * 2 }\n\
         spec double {\n\
             given n: Int = 21;\n\
             when let result = double(n);\n\
             then result == 42;\n\
             assert double(0) == 0;\n\
         }\n",
    );
    assert_clean(&result);
}

#[test]
fn non_boolean_assert_and_then_expectations_are_rejected() {
    let (_, result) = check_one(
        "fn double(in n: Int) -> Int { n * 2 }\n\
         spec double {\n\
             then double(3);\n\
             assert double(2) + 1;\n\
         }\n",
    );
    // One mismatch per non-`Bool` expectation.
    assert_eq!(codes(&result), ["T0001", "T0001"]);
}

#[test]
fn given_bindings_check_their_declared_type() {
    let (_, result) = check_one(
        "fn double(in n: Int) -> Int { n * 2 }\n\
         spec double {\n\
             given n: Int = true;\n\
             assert double(n) == 0;\n\
         }\n",
    );
    assert_eq!(codes(&result), ["T0001"]);
}

#[test]
fn spec_bodies_check_calls_like_function_bodies() {
    let (_, result) = check_one(
        "fn add(in a: Int, in b: Int) -> Int { a + b }\n\
         spec add {\n\
             when let sum = add(1, true);\n\
             assert sum == 2;\n\
         }\n",
    );
    assert_eq!(codes(&result), ["T0001"]);
}

#[test]
fn every_spec_attached_to_a_function_is_checked() {
    // Multiple specs on one target (ADR-0002): each is checked
    // independently, so a broken second spec still reports.
    let (_, result) = check_one(
        "fn double(in n: Int) -> Int { n * 2 }\n\
         spec double { assert double(1) == 2; }\n\
         spec double { assert double(1); }\n",
    );
    assert_eq!(codes(&result), ["T0001"]);
}

#[test]
fn numeric_casts_are_explicit_and_allowed() {
    let (resolution, result) = check_one(
        "fn f(in n: Int) -> F64 {\n\
             let narrowed = n as I32;\n\
             let index = n as Usize;\n\
             n as F64\n\
         }\n",
    );
    assert_clean(&result);
    assert_eq!(rendered_type(&resolution, &result, "narrowed"), "I32");
    assert_eq!(rendered_type(&resolution, &result, "index"), "Usize");
}

// ----------------------------------------------------------------------
// Negative: calls, returns, operators
// ----------------------------------------------------------------------

#[test]
fn wrong_arity_and_wrong_argument_types_are_reported() {
    let (_, result) = check_one(
        "fn add(in a: Int, in b: Int) -> Int { a + b }\n\
         fn f() -> Int {\n\
             add(1);\n\
             add(1, true)\n\
         }\n",
    );
    assert_eq!(codes(&result), ["T0002", "T0001"]);
}

#[test]
fn mismatch_diagnostics_carry_structured_expected_and_actual_types() {
    let (_, result) = check_one("fn f() -> Int { true }\n");
    assert_eq!(codes(&result), ["T0001"]);
    let diagnostic = &result.diagnostics()[0];
    let expected: Vec<String> = diagnostic
        .expected
        .iter()
        .map(ToString::to_string)
        .collect();
    let actual: Vec<String> = diagnostic.actual.iter().map(ToString::to_string).collect();
    assert_eq!(expected, ["`I64`"]);
    assert_eq!(actual, ["`Bool`"]);
    assert_eq!(diagnostic.expected[0].kind(), "type");
}

#[test]
fn calling_a_non_function_is_rejected() {
    let (_, result) = check_one(
        "fn f() -> Int {\n\
             let n = 1;\n\
             n(2)\n\
         }\n",
    );
    assert_eq!(codes(&result), ["T0003"]);
}

#[test]
fn there_are_no_implicit_numeric_conversions() {
    let (_, result) = check_one(
        "fn f(in a: I32, in b: I64) -> I64 {\n\
             let widened: I64 = a;\n\
             a + b;\n\
             b\n\
         }\n",
    );
    assert_eq!(codes(&result), ["T0001", "T0001"]);
}

#[test]
fn operator_misuse_is_reported() {
    let (_, result) = check_one(
        "struct P { x: Int }\n\
         fn f(in p: P, in u: U32) {\n\
             1 + true;\n\
             p < p;\n\
             !1;\n\
             -u;\n\
         }\n",
    );
    assert_eq!(codes(&result), ["T0001", "T0006", "T0001", "T0006"]);
}

#[test]
fn conditions_and_logic_require_bool() {
    let (_, result) = check_one(
        "fn f(in n: Int) {\n\
             if n { }\n\
             while 1 { }\n\
             let b = n && true;\n\
         }\n",
    );
    assert_eq!(codes(&result), ["T0001", "T0001", "T0001"]);
}

#[test]
fn if_branches_must_agree() {
    let (_, result) = check_one(
        "fn f(in flag: Bool) -> Int {\n\
             if flag { 1 } else { \"one\" }\n\
         }\n",
    );
    assert_eq!(codes(&result), ["T0001"]);
}

#[test]
fn a_valueless_if_requires_a_unit_then_block() {
    let (_, result) = check_one("fn f(in flag: Bool) { if flag { 1 } }\n");
    assert_eq!(codes(&result), ["T0001"]);
}

// ----------------------------------------------------------------------
// Negative: fields, literals, constructors
// ----------------------------------------------------------------------

#[test]
fn unknown_and_missing_fields_are_reported() {
    let (_, result) = check_one(
        "struct Point { x: F64, y: F64 }\n\
         fn f(in p: Point) -> F64 {\n\
             let partial = Point { x: 1.0 };\n\
             let wrong = Point { x: 1.0, y: 2.0, z: 3.0 };\n\
             let twice = Point { x: 1.0, x: 2.0, y: 3.0 };\n\
             p.z\n\
         }\n",
    );
    assert_eq!(codes(&result), ["T0005", "T0004", "T0005", "T0004"]);
}

#[test]
fn field_access_on_a_non_struct_is_reported() {
    let (_, result) = check_one("fn f(in n: Int) -> Int { n.x }\n");
    assert_eq!(codes(&result), ["T0004"]);
}

#[test]
fn types_are_not_values_and_payload_variants_need_braces() {
    let (_, result) = check_one(
        "enum Shape { Circle { radius: F64 }, Dot }\n\
         fn f() {\n\
             let a = Shape;\n\
             let b = Shape::Circle;\n\
         }\n",
    );
    assert_eq!(codes(&result), ["T0012", "T0012"]);
}

// ----------------------------------------------------------------------
// Negative: matches
// ----------------------------------------------------------------------

#[test]
fn non_exhaustive_matches_name_the_missing_variants() {
    let (_, result) = check_one(
        "enum Shape { Circle { radius: F64 }, Rect { w: F64, h: F64 }, Dot }\n\
         fn f(in s: Shape) -> Int {\n\
             match s {\n\
                 Shape::Dot => 0,\n\
             }\n\
         }\n",
    );
    assert_eq!(codes(&result), ["T0007"]);
    let names: Vec<String> = result.diagnostics()[0]
        .expected
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(names, ["`Circle`", "`Rect`"]);
}

#[test]
fn option_bool_and_open_types_have_exhaustiveness_rules() {
    let (_, result) = check_one(
        "fn f(in v: Option[Int], in b: Bool, in n: Int) -> Int {\n\
             match v { Some { value } => value, };\n\
             match b { true => 1, };\n\
             match n { 0 => 1, }\n\
         }\n",
    );
    assert_eq!(codes(&result), ["T0007", "T0007", "T0007"]);
}

#[test]
fn guarded_arms_do_not_count_toward_exhaustiveness() {
    let (_, result) = check_one(
        "fn f(in v: Option[Int]) -> Int {\n\
             match v {\n\
                 Some { value } if value > 0 => value,\n\
                 Some { value } => value,\n\
                 None => 0,\n\
             }\n\
         }\n\
         fn g(in b: Bool) -> Int {\n\
             match b {\n\
                 true => 1,\n\
                 false if true => 2,\n\
             }\n\
         }\n",
    );
    assert_eq!(codes(&result), ["T0007"]);
}

#[test]
fn match_arms_must_yield_one_type() {
    let (_, result) = check_one(
        "fn f(in b: Bool) -> Int {\n\
             match b {\n\
                 true => 1,\n\
                 false => \"no\",\n\
             }\n\
         }\n",
    );
    assert_eq!(codes(&result), ["T0001"]);
}

#[test]
fn variant_patterns_check_their_fields() {
    let (_, result) = check_one(
        "enum Shape { Circle { radius: F64 }, Dot }\n\
         fn f(in s: Shape) -> F64 {\n\
             match s {\n\
                 Shape::Circle { diameter } => 0.0,\n\
                 Shape::Circle { } => 1.0,\n\
                 Shape::Dot => 2.0,\n\
             }\n\
         }\n",
    );
    // The first arm has an unknown field *and* misses `radius`; the second
    // misses `radius`.
    assert_eq!(codes(&result), ["T0004", "T0005", "T0005"]);
}

// ----------------------------------------------------------------------
// Negative: casts, `?`, type arguments, annotations, loops
// ----------------------------------------------------------------------

#[test]
fn non_numeric_casts_are_invalid() {
    let (_, result) = check_one(
        "fn f(in n: Int, in b: Bool) {\n\
             b as Int;\n\
             n as Bool;\n\
         }\n",
    );
    assert_eq!(codes(&result), ["T0008", "T0008"]);
}

#[test]
fn try_requires_matching_fallibility() {
    let (_, result) = check_one(
        "fn find() -> Option[Int] { None }\n\
         fn plain(in n: Int) -> Int {\n\
             let v = find()?;\n\
             n?\n\
         }\n",
    );
    assert_eq!(codes(&result), ["T0009", "T0009"]);
}

#[test]
fn type_argument_arity_is_checked() {
    let (_, result) = check_one(
        "struct Pair[A, B] { first: A, second: B }\n\
         fn f(in a: Option[Int, Int], in b: Pair[Int], in c: Array) {}\n",
    );
    assert_eq!(codes(&result), ["T0010", "T0010", "T0010"]);
}

#[test]
fn unresolvable_inference_demands_an_annotation() {
    let (_, result) = check_one("fn f() { let pending = None; }\n");
    assert_eq!(codes(&result), ["T0011"]);
}

#[test]
fn break_and_continue_need_a_loop() {
    let (_, result) = check_one("fn f() { break; continue; }\n");
    assert_eq!(codes(&result), ["T0013", "T0013"]);
}

#[test]
fn loop_breaks_must_agree_on_the_loop_value() {
    let (_, result) = check_one(
        "fn f() -> Int {\n\
             loop {\n\
                 break 1;\n\
                 break \"two\";\n\
             }\n\
         }\n",
    );
    assert_eq!(codes(&result), ["T0001"]);
}
