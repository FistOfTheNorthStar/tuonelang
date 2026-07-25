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
