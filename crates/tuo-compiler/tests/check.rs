//! Integration tests for [`tuo_compiler::check_sources`] — the parse →
//! resolve → type-check pipeline behind `tuo check` — with a focus on
//! first-class specs (ADR-0002): specs are checked in every compilation,
//! but never executed here.

use tuo_compiler::source::SourceMap;
use tuo_compiler::{CheckResult, check_sources, resolve::SymbolKind};

fn check(sources: &[&str]) -> CheckResult {
    let mut map = SourceMap::new();
    let ids: Vec<_> = sources
        .iter()
        .enumerate()
        .map(|(index, text)| {
            let file = map.intern_file(&format!("file{index}.tuo"));
            map.add_source(file, *text).expect("test source fits")
        })
        .collect();
    check_sources(&map, &ids)
}

fn codes(result: &CheckResult) -> Vec<String> {
    result
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.to_string())
        .collect()
}

#[test]
fn a_clean_program_with_specs_checks_without_diagnostics() {
    let result = check(&["fn double(in n: Int) -> Int { n * 2 }\n\
         spec double {\n\
             given n: Int = 21;\n\
             when let result = double(n);\n\
             then result == 42;\n\
             assert double(0) == 0;\n\
         }\n"]);
    assert_eq!(result.diagnostics, &[], "expected a clean check");
    assert!(!result.has_errors());

    // The spec semantics are queryable straight off the check result.
    let (double, _) = result
        .resolution
        .symbols()
        .find(|(_, symbol)| symbol.name == "double" && symbol.kind == SymbolKind::Function)
        .expect("`double` resolves");
    let specs = result.resolution.specs_for(double);
    assert_eq!(specs.len(), 1);
    assert_eq!(result.resolution.target_of(specs[0]), Some(double));
    assert_eq!(result.resolution.dependencies_of(specs[0]), &[double]);
}

#[test]
fn spec_type_errors_fail_the_check() {
    let result = check(&["fn double(in n: Int) -> Int { n * 2 }\n\
         spec double { assert double(1) + 1; }\n"]);
    assert_eq!(
        codes(&result),
        ["T0001"],
        "a non-Bool assertion is rejected"
    );
    assert!(result.has_errors());
}

#[test]
fn spec_attachment_errors_fail_the_check() {
    let result = check(&["spec vanished { assert 1 == 1; }\n"]);
    assert_eq!(
        codes(&result),
        ["R0002"],
        "an unresolved target is rejected"
    );
    assert!(result.has_errors());
}

#[test]
fn cross_file_programs_check_as_one_snapshot() {
    let result = check(&[
        "module m;\npub fn double(in n: Int) -> Int { n * 2 }\n",
        "module m;\nspec double { assert double(2) == 4; }\n",
    ]);
    assert_eq!(result.diagnostics, &[], "expected a clean check");
}
