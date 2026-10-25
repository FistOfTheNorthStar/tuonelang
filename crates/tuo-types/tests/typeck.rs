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
    // A function's value type spells its parameter modes (ADR-0008 Tier 1):
    // modes are part of the function type and its exact equality.
    assert_eq!(
        result.type_of(add).expect("fn type").render(&resolution),
        "fn(in I64, in I64) -> I64"
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
    // `self.count` types against the target (no `T0001`); the only reports are
    // ADR-0032 Stage A's two `T0026` warnings — the declarations lower to nothing.
    assert_eq!(codes(&result), vec!["T0026".to_owned(), "T0026".to_owned()]);
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

/// ADR-0019. `&`/`|`/`^` are integers-only and same-type; the result is the
/// operand type, so a bitwise expression can be returned as `Int`.
#[test]
fn bitwise_operators_type_check_on_integers() {
    let (_, result) = check_one(
        "fn f(take a: Int, take b: Int) -> Int {\n\
             (a & b) | (a ^ b)\n\
         }\n",
    );
    assert_eq!(codes(&result), Vec::<String>::new());
}

/// A shift's amount is deliberately NOT unified with the shifted value's
/// type (ADR-0019) — it is a bit count, not a value of that type — while the
/// result still takes the *left* operand's type.
#[test]
fn a_shift_takes_the_left_operand_type_and_an_independent_amount() {
    let (_, result) = check_one(
        "fn f(take a: Int, take n: Int) -> Int {\n\
             (a << n) >> 2\n\
         }\n",
    );
    assert_eq!(codes(&result), Vec::<String>::new());
}

/// Bitwise operators are refused on floats: a bit pattern is not defined on
/// an IEEE-754 value in v0, so this must be a type error rather than a
/// silently reinterpreted operation.
#[test]
fn bitwise_operators_are_refused_on_floats() {
    let (_, result) = check_one(
        "fn f(take a: Float, take b: Float) -> Float {\n\
             a & b\n\
         }\n",
    );
    assert_eq!(codes(&result), ["T0006"]);
}

/// `~` complements an integer and `!` negates a `Bool`; neither accepts the
/// other's operand, so the two stay distinct operators (ADR-0019).
#[test]
fn bitwise_not_and_logical_not_have_distinct_operand_types() {
    let (_, result) = check_one(
        "fn f(take a: Int) -> Int {\n\
             ~a\n\
         }\n",
    );
    assert_eq!(codes(&result), Vec::<String>::new());

    let (_, bad) = check_one(
        "fn f(take a: Bool) -> Bool {\n\
             ~a\n\
         }\n",
    );
    assert_eq!(codes(&bad), ["T0006"]);
}

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
fn a_struct_pattern_binding_every_field_is_exhaustive() {
    let (_, result) = check_one(
        "struct Point { x: Int, y: Int }\n\
         fn f(in p: Point) -> Int {\n\
             match p {\n\
                 Point { x, y } => x + y,\n\
             }\n\
         }\n",
    );
    assert_clean(&result);
}

#[test]
fn a_constructor_counts_as_covered_only_when_its_fields_are() {
    // `Some { value: 0 }` leaves every other `Some`: the shallow check this
    // replaced accepted the match, and the run trapped as unreachable.
    let (_, partial) = check_one(
        "fn f(take o: Option[Int]) -> Int {\n\
             match o {\n\
                 Some { value: 0 } => 1,\n\
                 None => 2,\n\
             }\n\
         }\n",
    );
    assert_eq!(codes(&partial), ["T0007"]);
    let diagnostic = &partial.diagnostics()[0];
    assert!(
        diagnostic.message.contains("missing `Some`"),
        "{}",
        diagnostic.message
    );
    assert!(
        diagnostic
            .notes
            .iter()
            .any(|note| note.contains("only for some")),
        "the note says `Some` is partly matched: {:?}",
        diagnostic.notes
    );
    // Nested patterns that together cover every value are exhaustive.
    let (_, nested) = check_one(
        "enum Shape { Circle { r: Int }, Rect { w: Int, h: Int } }\n\
         fn f(take o: Option[Bool], take r: Result[Shape, Int]) -> Int {\n\
             let a = match o {\n\
                 Some { value: true } => 1,\n\
                 Some { value: false } => 2,\n\
                 None => 3,\n\
             };\n\
             let b = match r {\n\
                 Ok { value: Shape::Circle { r } } => r,\n\
                 Ok { value: Shape::Rect { w, .. } } => w,\n\
                 Err { error: 0 } => 0,\n\
                 Err { error } => error,\n\
             };\n\
             a + b\n\
         }\n",
    );
    assert_clean(&nested);
    let (_, inner) = check_one(
        "fn f(take o: Option[Bool]) -> Int {\n\
             match o {\n\
                 Some { value: true } => 1,\n\
                 None => 3,\n\
             }\n\
         }\n",
    );
    assert_eq!(codes(&inner), ["T0007"]);
}

#[test]
fn a_bare_variant_pattern_is_a_constructor_not_a_catch_all() {
    // `None` alone used to count as a binding that matched everything.
    let (_, lone) = check_one(
        "fn f(take o: Option[Int]) -> Int {\n\
             match o {\n\
                 None => 2,\n\
             }\n\
         }\n",
    );
    assert_eq!(codes(&lone), ["T0007"]);
    // It must be a variant of the scrutinee's type...
    let (_, wrong_type) = check_one(
        "fn f(take n: Int) -> Int {\n\
             match n {\n\
                 None => 2,\n\
                 _ => 3,\n\
             }\n\
         }\n",
    );
    assert_eq!(codes(&wrong_type), ["T0001"]);
    // ...and one with no fields to leave unmatched.
    let (_, payload) = check_one(
        "fn f(take o: Option[Int]) -> Int {\n\
             match o {\n\
                 Some => 2,\n\
                 None => 3,\n\
             }\n\
         }\n",
    );
    assert_eq!(codes(&payload), ["T0005"]);
}

#[test]
fn a_let_pattern_must_match_every_value() {
    let (_, refutable) = check_one(
        "struct P { x: Int, y: Int }\n\
         fn f(take o: Option[Int], take p: P) -> Int {\n\
             let Some { value } = o;\n\
             let P { x: 0, y } = p;\n\
             value + y\n\
         }\n",
    );
    assert_eq!(codes(&refutable), ["T0007", "T0007"]);
    let (_, irrefutable) = check_one(
        "struct P { x: Int, y: Int }\n\
         fn f(take p: P) -> Int {\n\
             let P { x, y } = p;\n\
             x + y\n\
         }\n",
    );
    assert_clean(&irrefutable);
}

#[test]
fn a_const_initializer_must_be_a_constant_expression() {
    let (_, folded) = check_one(
        "const BASE: Int = 10;\n\
         const LIMIT: Int = BASE * BASE + (3 as Int) - -1;\n\
         const OK: Bool = LIMIT > 100 || BASE == 0;\n",
    );
    assert_clean(&folded);
    let (_, call) = check_one(
        "fn base() -> Int { 7 }\n\
         const X: Int = base() * 2;\n",
    );
    assert_eq!(codes(&call), ["T0028"]);
    let (_, local) = check_one(
        "fn f() -> Int {\n\
             let x = 2;\n\
             const Y: Int = x + 1;\n\
             Y\n\
         }\n",
    );
    assert_eq!(codes(&local), ["T0028"]);
    let (_, aggregate) = check_one(
        "struct P { x: Int }\n\
         const ORIGIN: P = P { x: 0 };\n",
    );
    assert_eq!(codes(&aggregate), ["T0028"]);
}

#[test]
fn a_const_defined_in_terms_of_itself_is_reported() {
    let (_, result) = check_one(
        "const A: Int = B + 1;\n\
         const B: Int = A + 1;\n\
         const C: Int = C;\n",
    );
    assert_eq!(codes(&result), ["T0028", "T0028", "T0028"]);
    assert!(
        result.diagnostics()[0]
            .message
            .contains("`const A` is defined in terms of itself")
    );
}

#[test]
fn a_generic_body_may_store_its_type_parameter_in_an_array() {
    let (_, result) = check_one(
        "fn reversed[T](in xs: Array[T]) -> Array[T] {\n\
             var out = std::array::empty();\n\
             var i = std::array::len(xs);\n\
             while i > 0 {\n\
                 i = i - 1;\n\
                 std::array::push(out, std::array::get(xs, i));\n\
             }\n\
             out\n\
         }\n\
         fn g() -> Int {\n\
             std::array::len(reversed(std::array::filled(2, 1.5)))\n\
         }\n",
    );
    assert_clean(&result);
}

#[test]
fn a_generic_array_element_is_checked_at_each_instantiation() {
    // `wrap` stores its `T` in an `Array`, and `outer` passes its `U` there
    // inside a struct, so both reach `Array[Array[I64]]` from a concrete
    // call. Accepting `T` in the body without this check let a generic
    // build an element type the v0 set refuses.
    let (_, result) = check_one(
        "struct Pair[T] { first: T, second: T }\n\
         fn wrap[T](take x: T) -> Array[T] {\n\
             var xs = std::array::empty();\n\
             std::array::push(xs, x);\n\
             xs\n\
         }\n\
         fn outer[U](take x: U, take y: U) -> Array[Pair[U]] {\n\
             wrap(Pair { first: x, second: y })\n\
         }\n\
         fn ok() -> Int {\n\
             std::array::len(outer(3, 4)) + std::array::len(wrap(\"s\"))\n\
         }\n\
         fn bad() -> Int {\n\
             std::array::len(outer(wrap(1), wrap(2)))\n\
         }\n\
         fn bad2() -> Int {\n\
             std::array::len(wrap(wrap(1)))\n\
         }\n",
    );
    assert_eq!(codes(&result), ["T0001", "T0001"]);
    for diagnostic in result.diagnostics() {
        assert!(
            diagnostic
                .message
                .contains("`Array[Array[I64]]` is not supported"),
            "{}",
            diagnostic.message
        );
    }
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

// ----------------------------------------------------------------------
// First-class function values (ADR-0008 Tier 1)
// ----------------------------------------------------------------------

#[test]
fn a_bare_fn_name_binds_to_a_function_value_with_modes() {
    let (resolution, result) = check_one(
        "fn add(take a: Int, in b: Int) -> Int { a + b }\n\
         fn main() -> Int { var f = add; f(1, 2) }\n",
    );
    assert_clean(&result);
    // The local `f` has the function's signature-as-function-type, modes
    // included — exactly the declaration's modes.
    assert_eq!(
        rendered_type(&resolution, &result, "f"),
        "fn(take I64, in I64) -> I64"
    );
}

#[test]
fn function_type_equality_includes_modes() {
    // `add` is `fn(take Int, take Int) -> Int`; a parameter typed
    // `fn(in Int, in Int) -> Int` is a *different* type, so passing `add` is a
    // T0001 mismatch — proving modes are part of exact type equality.
    let (_, result) = check_one(
        "fn add(take a: Int, take b: Int) -> Int { a + b }\n\
         fn takes(take f: fn(in Int, in Int) -> Int) -> Int { f(1, 2) }\n\
         fn main() -> Int { takes(add) }\n",
    );
    assert_eq!(codes(&result), vec!["T0001"]);
}

#[test]
fn an_indirect_call_type_checks_against_the_function_type() {
    let (_, result) = check_one(
        "fn add(take a: Int, take b: Int) -> Int { a + b }\n\
         fn apply(take f: fn(take Int, take Int) -> Int, take a: Int, take b: Int) -> Int {\n\
             f(a, b)\n\
         }\n\
         fn main() -> Int { apply(add, 2, 3) }\n",
    );
    assert_clean(&result);
}

#[test]
fn an_indirect_call_with_a_wrong_argument_type_is_t0001() {
    let (_, result) = check_one(
        "fn apply(take f: fn(take Int) -> Int, take a: Int) -> Int { f(a) }\n\
         fn main() -> Int {\n\
             var g: fn(take Int) -> Int = apply_helper;\n\
             g(true)\n\
         }\n\
         fn apply_helper(take n: Int) -> Int { n }\n",
    );
    assert_eq!(codes(&result), vec!["T0001"]);
}

#[test]
fn a_builtin_used_as_a_value_is_t0015() {
    let (_, result) = check_one("fn use_it() -> Int { var f = std::str::len; 0 }\n");
    assert_eq!(codes(&result), vec!["T0015"]);
}

#[test]
fn a_generic_function_used_as_a_value_is_t0015() {
    let (_, result) = check_one(
        "fn id[T](take x: T) -> T { x }\n\
         fn use_it() -> Int { var f = id; 0 }\n",
    );
    assert_eq!(codes(&result), vec!["T0015"]);
}

#[test]
fn calling_a_non_function_value_is_t0003() {
    let (_, result) = check_one("fn main() -> Int { var n = 5; n(1) }\n");
    assert_eq!(codes(&result), vec!["T0003"]);
}

// ---------------------------------------------------------------------------
// ADR-0023 Stage A — the map operation surface is enforced *after* inference.
//
// `reject_unsupported_map_pair` used to run only at the call site and return
// early when either half of the pair was still an inference variable. For a map
// built with `std::map::empty()` — whose `K`/`V` are solved from *later* uses —
// that meant the check never ran at all, and an unsupported pair reached
// codegen: `tuo check` accepted the program and the backend then failed with a
// raw verifier error, violating the invariant that the compiler refuses what it
// cannot compile rather than mis-compiling it.
//
// Every case below is written in the INFERRED spelling, because that is the one
// that regressed. The annotated spelling was always caught, so a test using it
// would have passed throughout the defect's life and proved nothing.
// ---------------------------------------------------------------------------

/// Build a map through `empty()`, so `K`/`V` are only determined by the
/// `insert` that follows — the shape the eager check could not see.
fn inferred_map_program(insert_args: &str) -> String {
    format!(
        "module m;\n\
         fn f() -> Int {{\n\
         \x20   var m = std::map::empty();\n\
         \x20   std::map::insert(m, {insert_args});\n\
         \x20   std::map::len(m)\n\
         }}\n"
    )
}

#[test]
fn inferred_map_with_copy_scalar_values_is_accepted() {
    // ADR-0023 Stage B widened the value set to the `Copy` scalars. These were
    // refused before Stage B (and, before Stage A, wrongly *accepted* here and
    // then rejected by the backend); now they are genuinely supported on all
    // three engines, which the codegen fixtures pin at runtime.
    assert_clean(&check_one(&inferred_map_program("1, true")).1);
    assert_clean(&check_one(&inferred_map_program("1, 2.5")).1);
    assert_clean(&check_one(&inferred_map_program("\"k\", true")).1);
    assert_clean(&check_one(&inferred_map_program("\"k\", 2.5")).1);
}

#[test]
fn inferred_map_with_owned_string_value_is_refused() {
    let (_, result) = check_one(&inferred_map_program("1, std::string::from_str(\"s\")"));
    assert!(
        codes(&result).iter().any(|code| code == "T0001"),
        "`Map[Int, String]` built through `empty()` must be refused; got {:?}",
        codes(&result)
    );
}

#[test]
fn inferred_map_error_names_the_solved_pair() {
    // The diagnostic must name the pair inference actually produced, so the
    // message is actionable rather than mentioning an unsolved variable.
    // `String` is used because it is still outside the value set after
    // Stage B2 (the borrowed `Str` is now in it).
    let (_, result) = check_one(&inferred_map_program("1, std::string::from_str(\"s\")"));
    let message = result
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.message.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        message.contains("String"),
        "the diagnostic should name the solved value type; got: {message}"
    );
}

#[test]
fn inferred_supported_map_pairs_still_check() {
    // The fix must not over-refuse: both v0 pairs are built the same way.
    assert_clean(&check_one(&inferred_map_program("1, 2")).1);
    assert_clean(&check_one(&inferred_map_program("\"k\", 2")).1);
}

#[test]
fn an_unconstrained_empty_map_is_an_annotation_error_not_a_pair_error() {
    // A map whose type nothing determines is `T0011` ("type annotation
    // needed"), reported by the unsolved-variable path. The deferred re-check
    // must stay silent here rather than inventing a second complaint about a
    // pair the program never chose.
    let (_, result) =
        check_one("module m;\nfn f() -> Int {\n    let m = std::map::empty();\n    0\n}\n");
    let codes = codes(&result);
    assert!(
        codes.iter().any(|code| code == "T0011"),
        "expected T0011 for an unconstrained `empty()`; got {codes:?}"
    );
    assert!(
        !codes.iter().any(|code| code == "T0001"),
        "an unconstrained map must not also report a pair error; got {codes:?}"
    );
}

#[test]
fn a_map_pair_solved_by_a_later_call_is_still_refused() {
    // The value type is determined by `insert` *after* `get` has been checked.
    // Both sites defer, and both must be re-checked once inference has run.
    // The value is an owned `String`, still outside the Stage B2 value set.
    let source = "module m;\n\
                  fn f() -> Int {\n\
                  \x20   var m = std::map::empty();\n\
                  \x20   let seen = std::map::get(m, 1);\n\
                  \x20   std::map::insert(m, 1, std::string::from_str(\"s\"));\n\
                  \x20   std::map::len(m)\n\
                  }\n";
    let (_, result) = check_one(source);
    assert!(
        codes(&result).iter().any(|code| code == "T0001"),
        "a pair solved after the call site must still be refused; got {:?}",
        codes(&result)
    );
}

#[test]
fn map_values_that_own_heap_stay_refused() {
    // ADR-0023 Stage B2 widened the value set to include the *borrowed* `Str`,
    // which needs no deep copy and no drop glue. A value that genuinely
    // **owns** heap still needs the deep-copy-on-read and recursive-drop path
    // (the second half of Stage B2) and must not slip in behind `Str`:
    // `tuo_rt_map_drop` does not yet run per-value glue, so admitting one
    // would leak an allocation per entry.
    let value = "std::string::from_str(\"s\")";
    let result = check_one(&inferred_map_program(&format!("1, {value}"))).1;
    assert!(
        codes(&result).iter().any(|code| code == "T0001"),
        "a heap-owning map value ({value}) must stay refused; got {:?}",
        codes(&result)
    );
}

#[test]
fn inferred_map_with_str_values_is_accepted() {
    // ADR-0023 Stage B2: the borrowed `Str` joins the value set. It is a
    // fixed-size two-word view that borrows rather than owns, so the table
    // copies it bytewise with no deep copy and no drop glue — the reason it
    // lands before the owned `String`. Written in the *inferred* spelling,
    // which is the one the Stage A defect made unsound.
    for value in ["1, \"s\"", "\"k\", \"v\""] {
        assert_clean(&check_one(&inferred_map_program(value)).1);
    }
}

#[test]
fn nested_maps_stay_refused_as_map_values() {
    // A `Map` value is deliberately out of scope (ADR-0023): the recursion
    // boundary and the deep-copy cost of a nested container are their own
    // decision. `is_supported_array_element` — which the value predicate
    // mirrors — excludes `Ty::Map` only via its catch-all arm, so pin it
    // explicitly rather than leaving it to an implicit default.
    let source = "module m;\n\
                  fn f() -> Int {\n\
                  \x20   var inner = std::map::empty();\n\
                  \x20   std::map::insert(inner, 1, 2);\n\
                  \x20   var outer = std::map::empty();\n\
                  \x20   std::map::insert(outer, 1, inner);\n\
                  \x20   std::map::len(outer)\n\
                  }\n";
    let (_, result) = check_one(source);
    assert!(
        codes(&result).iter().any(|code| code == "T0001"),
        "a `Map`-valued map must stay refused; got {:?}",
        codes(&result)
    );
}

#[test]
fn map_keys_are_not_widened_by_the_value_widening() {
    // Keys are gated by the trait system's `Hash`/`Eq` (ADR-0011), a different
    // decision from the value set. Widening values must not widen keys.
    for key in ["true", "2.5"] {
        let result = check_one(&inferred_map_program(&format!("{key}, 1"))).1;
        assert!(
            codes(&result).iter().any(|code| code == "T0001"),
            "a `{key}` key must stay refused after the value widening; got {:?}",
            codes(&result)
        );
    }
}
