//! The standard library, really compiled and really run.
//!
//! `tuo-stdlib` is a catalog of `.tuo` source; this suite is the proof that the
//! catalog is *true*. It loads every module through the real front end and
//! asserts:
//!
//!   * every module parses, resolves, type-checks, and ownership-checks with
//!     **zero** errors — the whole library, and each module on its own;
//!   * every `spec` in the library **executes** through the reference
//!     interpreter and **passes**, with **no** skipped specs (a skip would mean
//!     the library shipped a spec the executable subset cannot run — dishonest);
//!   * the catalog's machine-queryable surface is coherent (every module is
//!     reachable by path, the count is what the prompt asked for).
//!
//! Because these tests compile the exact source `tuo-stdlib` embeds, the
//! library cannot drift from its promises without turning this suite red.

use tuo_compiler::check_sources;
use tuo_compiler::source::{SourceId, SourceMap};
use tuo_spec::{Limits, RunOutcome, Selection};

/// Intern every catalog module into a fresh source map, returning the map and
/// the source ids in catalog order.
fn load_all() -> (SourceMap, Vec<SourceId>) {
    let mut map = SourceMap::new();
    let mut sources = Vec::new();
    for module in tuo_stdlib::MODULES {
        let file = map.intern_file(module.name);
        let id = map
            .add_source(file, module.source)
            .expect("a stdlib module is not too large");
        sources.push(id);
    }
    (map, sources)
}

/// Intern one module into a fresh source map.
fn load_one(module: tuo_stdlib::Module) -> (SourceMap, SourceId) {
    let mut map = SourceMap::new();
    let file = map.intern_file(module.name);
    let id = map
        .add_source(file, module.source)
        .expect("a stdlib module is not too large");
    (map, id)
}

#[test]
fn the_catalog_lists_the_eight_prompt_modules() {
    // The prompt names exactly these eight initial modules.
    let expected = [
        "std::core",
        "std::collections",
        "std::io",
        "std::fs",
        "std::time",
        "std::process",
        "std::sync",
        "std::test",
    ];
    let paths: Vec<&str> = tuo_stdlib::MODULES.iter().map(|m| m.path).collect();
    for path in expected {
        assert!(paths.contains(&path), "catalog is missing {path}");
        assert!(
            tuo_stdlib::module(path).is_some(),
            "{path} is not reachable by lookup"
        );
    }
    assert_eq!(
        tuo_stdlib::MODULES.len(),
        expected.len(),
        "the catalog holds exactly the eight initial modules"
    );
}

#[test]
fn every_module_checks_cleanly_on_its_own() {
    // Each module must stand alone: no module depends on another today, so each
    // type-checks in isolation with zero errors.
    for &module in tuo_stdlib::MODULES {
        let (map, id) = load_one(module);
        let result = check_sources(&map, &[id]);
        assert!(
            !result.has_errors(),
            "{} has front-end errors:\n{:#?}",
            module.path,
            result
                .diagnostics
                .iter()
                .filter(|d| d.severity == tuo_compiler::diagnostics::Severity::Error)
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn the_whole_library_checks_cleanly_together() {
    // Loaded as one program, the modules must not collide: no duplicate
    // top-level definition, no cross-module resolution error.
    let (map, sources) = load_all();
    let result = check_sources(&map, &sources);
    assert!(
        !result.has_errors(),
        "the standard library does not check as one program:\n{:#?}",
        result
            .diagnostics
            .iter()
            .filter(|d| d.severity == tuo_compiler::diagnostics::Severity::Error)
            .collect::<Vec<_>>()
    );
}

#[test]
fn every_spec_in_the_library_runs_and_passes() {
    // The executable promise: every spec the library ships runs through the
    // interpreter and passes — and nothing is skipped. A skip would mean a spec
    // that the v0 executable subset cannot run slipped in; the library must not
    // ship one (the contract-tier functions are deliberately unspecced).
    let (map, sources) = load_all();
    match tuo_spec::run(&map, &sources, &Selection::All, Limits::default()) {
        RunOutcome::Ran(report) => {
            assert!(
                report.skipped.is_empty(),
                "the standard library shipped a spec the executable subset skips: {:#?}",
                report.skipped
            );
            assert!(
                report.ran() > 0,
                "the standard library must ship executable specs"
            );
            assert!(
                report.passed(),
                "a standard-library spec failed ({} of {} specs):\n{:#?}",
                report.failures(),
                report.ran(),
                report
                    .runs
                    .iter()
                    .filter(|r| !r.passed())
                    .collect::<Vec<_>>()
            );
        }
        RunOutcome::NotChecked(diagnostics) => {
            panic!("the standard library did not check, so no spec ran:\n{diagnostics:#?}");
        }
    }
}

#[test]
fn each_module_runs_its_own_specs_green() {
    // A per-module view of the same guarantee, so a failure names the module.
    for &module in tuo_stdlib::MODULES {
        let (map, id) = load_one(module);
        match tuo_spec::run(&map, &[id], &Selection::All, Limits::default()) {
            RunOutcome::Ran(report) => {
                assert!(
                    report.skipped.is_empty(),
                    "{} skipped a spec: {:#?}",
                    module.path,
                    report.skipped
                );
                assert!(
                    report.passed(),
                    "{} has a failing spec:\n{:#?}",
                    module.path,
                    report
                        .runs
                        .iter()
                        .filter(|r| !r.passed())
                        .collect::<Vec<_>>()
                );
            }
            RunOutcome::NotChecked(diagnostics) => {
                panic!("{} did not check:\n{diagnostics:#?}", module.path);
            }
        }
    }
}
