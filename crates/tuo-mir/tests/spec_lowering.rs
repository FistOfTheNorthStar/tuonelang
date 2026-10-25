//! Tests for `lower_specs`: every spec block lowers to synthetic MIR
//! functions that **verify**, and the returned [`SpecMir`] records the
//! assertion shape the runner needs (a condition function per `then`/`assert`,
//! plus operand functions for `==` comparisons).

use tuo_ast::Ast;
use tuo_mir::SpecProgram;
use tuo_source::SourceMap;

/// Run the front end and lower specs; panics on any front-end diagnostic.
fn lower_specs(text: &str) -> SpecProgram {
    let mut map = SourceMap::new();
    let file = map.intern_file("spec.tuo");
    let id = map.add_source(file, text).expect("fixture fits");
    let parse = tuo_parser::parse(map.source(id));
    assert_eq!(parse.diagnostics, vec![], "parse errors");
    let asts = [Ast::new(&parse.tree, text)];
    let resolution = tuo_resolve::resolve(&asts);
    assert_eq!(resolution.diagnostics(), &[], "resolution errors");
    let types = tuo_types::check(&asts, &resolution);
    assert_eq!(types.diagnostics(), &[], "type errors");
    let ownership = tuo_ownership::check(&asts, &resolution, &types);
    assert_eq!(ownership.diagnostics(), &[], "ownership errors");
    let hir = tuo_hir::lower(&asts, &resolution);
    let specs = tuo_mir::lower_specs(&hir, &resolution, &types);
    // The whole lowered program — dependency functions plus synthetic
    // assertion functions — must verify: the interpreter and every backend
    // reject unverified MIR.
    let problems = tuo_mir::verify(&specs.program, &types);
    assert!(
        problems.is_empty(),
        "lowered spec MIR failed verification: {}",
        problems
            .iter()
            .map(|d| format!("{}: {}", d.code, d.message))
            .collect::<Vec<_>>()
            .join("; ")
    );
    specs
}

#[test]
fn a_comparison_assertion_lowers_condition_and_both_operands() {
    let specs = lower_specs(concat!(
        "fn add(take a: Int, take b: Int) -> Int { a + b }\n",
        "spec add { then add(1, 2) == 3; }\n",
    ));
    assert_eq!(specs.specs.len(), 1);
    assert!(specs.skipped.is_empty());
    let spec = &specs.specs[0];
    assert_eq!(spec.assertions.len(), 1);
    let assertion = &spec.assertions[0];
    // Every synthetic function named by the assertion exists in the program.
    let names: Vec<&str> = specs
        .program
        .functions
        .iter()
        .map(|function| function.name.as_str())
        .collect();
    assert!(names.contains(&assertion.condition.as_str()));
    let comparison = assertion
        .comparison
        .as_ref()
        .expect("`==` assertion exposes its operands");
    assert!(names.contains(&comparison.actual.as_str()));
    assert!(names.contains(&comparison.expected.as_str()));
}

#[test]
fn a_non_comparison_assertion_lowers_only_a_condition() {
    let specs = lower_specs("spec \"plain\" { assert 1 < 2; }\n");
    let assertion = &specs.specs[0].assertions[0];
    assert!(
        assertion.comparison.is_none(),
        "a non-`==` assertion has no operand functions"
    );
}

#[test]
fn setup_clauses_lower_without_becoming_assertions() {
    let specs = lower_specs(concat!(
        "fn add(take a: Int, take b: Int) -> Int { a + b }\n",
        "spec add {\n",
        "    given a: Int = 2, b: Int = 3;\n",
        "    when let sum = add(a, b);\n",
        "    then sum == 5;\n",
        "}\n",
    ));
    // Exactly one assertion (the `then`); `given`/`when` are setup, not
    // assertions.
    assert_eq!(specs.specs[0].assertions.len(), 1);
}

#[test]
fn several_specs_on_one_function_lower_as_distinct_specs() {
    let specs = lower_specs(concat!(
        "fn id(take x: Int) -> Int { x }\n",
        "spec id { then id(1) == 1; }\n",
        "spec id { then id(2) == 2; }\n",
    ));
    assert_eq!(specs.specs.len(), 2, "two specs, not merged");
    assert_ne!(
        specs.specs[0].symbol, specs.specs[1].symbol,
        "each spec has its own identity"
    );
}

#[test]
fn synthetic_functions_do_not_collide_with_real_symbols() {
    let specs = lower_specs(concat!(
        "fn f(take x: Int) -> Int { x }\n",
        "spec f { then f(1) == 1; }\n",
    ));
    // Real function `f` and the synthetic assertion functions all have
    // distinct symbols.
    let mut symbols: Vec<_> = specs
        .program
        .functions
        .iter()
        .map(|function| function.symbol)
        .collect();
    let count = symbols.len();
    symbols.sort();
    symbols.dedup();
    assert_eq!(symbols.len(), count, "function symbols are unique");
}

#[test]
fn a_spec_naming_a_const_lowers_instead_of_skipping() {
    // The spec context once started with an empty const table, so any
    // spec naming a const was skipped — even `const K: Int = 3`.
    let specs = lower_specs(concat!(
        "const K: Int = 3;\n",
        "fn triple(take n: Int) -> Int { n * K }\n",
        "spec triple { then triple(1) == K; }\n",
    ));
    assert!(specs.skipped.is_empty(), "skipped: {:?}", specs.skipped);
    assert_eq!(specs.specs.len(), 1);
}

#[test]
fn a_computed_const_initializer_is_evaluated() {
    // `LIMIT` used to stay out of the const table (literals only), which
    // dropped `cap` from the program and left the spec calling nothing.
    let specs = lower_specs(concat!(
        "const LIMIT: Int = BASE * BASE;\n",
        "const BASE: Int = 10;\n",
        "fn cap(take n: Int) -> Int {\n",
        "    const LOCAL: Int = LIMIT - 7;\n",
        "    if n < LOCAL { n } else { LOCAL }\n",
        "}\n",
        "spec cap { then cap(500) == 93; }\n",
    ));
    assert!(
        specs.program.skipped.is_empty(),
        "{:?}",
        specs.program.skipped
    );
    assert!(specs.skipped.is_empty());
}

#[test]
fn a_const_that_traps_names_the_trap_where_it_is_used() {
    let specs = lower_specs(concat!(
        "const BIG: I8 = 100 * 2;\n",
        "fn f() -> Int { BIG as Int }\n",
        "spec f { then f() == 0; }\n",
    ));
    let skipped = &specs.program.skipped;
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].name, "f");
    assert!(
        skipped[0]
            .reason
            .contains("`const BIG` cannot be evaluated: it overflows `I8`"),
        "{}",
        skipped[0].reason
    );
    assert!(
        skipped[0].symbol.is_some(),
        "a skipped body records its symbol"
    );
}

#[test]
fn a_generic_struct_field_read_in_a_spec_verifies() {
    // The verifier typed `_0.0` of a `Pair[I64]` as the declared `T`, so
    // comparing it with an `I64` failed verification (`M0006`).
    let specs = lower_specs(concat!(
        "struct Pair[T] { first: T, second: T }\n",
        "fn make[T](take a: T, take b: T) -> Pair[T] { Pair { first: a, second: b } }\n",
        "spec make { then make(1, 2).first == 1; }\n",
    ));
    assert_eq!(specs.specs.len(), 1);
}

#[test]
fn a_destructuring_let_lowers() {
    let specs = lower_specs(concat!(
        "struct Pair[T] { first: T, second: T }\n",
        "struct Named { name: String, age: Int }\n",
        "fn swap[T](take p: Pair[T]) -> Pair[T] {\n",
        "    let Pair { first, second } = p;\n",
        "    Pair { first: second, second: first }\n",
        "}\n",
        "fn name_len(take n: Named) -> Int {\n",
        "    let Named { name, .. } = n;\n",
        "    std::string::len(name)\n",
        "}\n",
        "spec swap { then swap(Pair { first: 1, second: 2 }).first == 2; }\n",
    ));
    assert!(
        specs.program.skipped.is_empty(),
        "{:?}",
        specs.program.skipped
    );
}
