//! End-to-end tests for the spec runner: source in, executed spec results
//! out. These compile small programs, lower their specs to verified MIR, and
//! run them through the reference interpreter — exercising the whole
//! `tuo spec` / `tuo verify` path except the CLI presentation layer.

use tuo_source::SourceMap;
use tuo_spec::report::Outcome;
use tuo_spec::{Limits, RunOutcome, Selection, SpecReport};

/// Compile and run every spec in `source`, asserting the program checked.
fn run(source: &str) -> SpecReport {
    run_with(source, &Selection::All, Limits::default())
}

/// Run with an explicit selection and limits.
fn run_with(source: &str, selection: &Selection, limits: Limits) -> SpecReport {
    let mut map = SourceMap::new();
    let file = map.intern_file("test.tuo");
    let id = map.add_source(file, source).expect("source interns");
    match tuo_spec::run(&map, &[id], selection, limits) {
        RunOutcome::Ran(report) => report,
        RunOutcome::NotChecked(problems) => {
            panic!("expected the program to check, but it did not: {problems:#?}");
        }
    }
}

/// The number of passing assertions across a run.
fn passing_assertions(report: &SpecReport) -> usize {
    report
        .runs
        .iter()
        .flat_map(|run| &run.assertions)
        .filter(|assertion| matches!(assertion.outcome, Outcome::Passed))
        .count()
}

#[test]
fn a_true_assertion_passes() {
    let report = run(concat!(
        "fn add(take a: Int, take b: Int) -> Int { a + b }\n",
        "spec add { then add(2, 3) == 5; }\n",
    ));
    assert!(report.passed(), "the spec should pass");
    assert_eq!(report.ran(), 1);
    assert_eq!(passing_assertions(&report), 1);
}

#[test]
fn a_false_equality_reports_actual_and_expected() {
    let report = run(concat!(
        "fn double(take x: Int) -> Int { x * 2 }\n",
        "spec double { assert double(10) == 999; }\n",
    ));
    assert!(!report.passed(), "the spec should fail");
    let assertion = &report.runs[0].assertions[0];
    match &assertion.outcome {
        Outcome::Failed(failure) => {
            assert_eq!(failure.actual.as_deref(), Some("20I64"));
            assert_eq!(failure.expected.as_deref(), Some("999I64"));
        }
        other => panic!("expected a failure, got {other:?}"),
    }
}

#[test]
fn a_non_equality_false_assertion_has_no_operands() {
    // `!(1 == 1)` is `false` but is not an `==` at the top level, so there is
    // no actual/expected pair to report — just a plain failure.
    let report = run("spec \"plain\" { assert !(1 == 1); }\n");
    assert!(!report.passed());
    match &report.runs[0].assertions[0].outcome {
        Outcome::Failed(failure) => {
            assert!(failure.actual.is_none());
            assert!(failure.expected.is_none());
        }
        other => panic!("expected a plain failure, got {other:?}"),
    }
}

#[test]
fn given_and_when_setup_is_visible_to_the_assertion() {
    let report = run(concat!(
        "fn add(take a: Int, take b: Int) -> Int { a + b }\n",
        "spec add {\n",
        "    given a: Int = 2, b: Int = 3;\n",
        "    when let sum = add(a, b);\n",
        "    then sum == 5;\n",
        "}\n",
    ));
    assert!(report.passed(), "setup bindings should reach the assertion");
    assert_eq!(passing_assertions(&report), 1);
}

#[test]
fn a_language_trap_is_reported_with_a_call_trace() {
    let report = run(concat!(
        "fn div(take a: Int, take b: Int) -> Int { a / b }\n",
        "spec div {\n",
        "    given x: Int = 10, y: Int = 0;\n",
        "    then div(x, y) == 0;\n",
        "}\n",
    ));
    assert!(!report.passed(), "division by zero should fail the spec");
    match &report.runs[0].assertions[0].outcome {
        Outcome::Errored(trap) => {
            assert_eq!(trap.label, "division_by_zero");
            assert!(
                !trap.trace.is_empty(),
                "a trap should carry a TDG call trace"
            );
        }
        other => panic!("expected a trap, got {other:?}"),
    }
}

#[test]
fn multiple_specs_on_one_function_are_distinct() {
    // Two specs of the same name and target are two specs (ADR-0002).
    let report = run(concat!(
        "fn id(take x: Int) -> Int { x }\n",
        "spec id { then id(1) == 1; }\n",
        "spec id { then id(2) == 2; }\n",
    ));
    assert_eq!(report.ran(), 2, "both specs run as distinct specs");
    assert!(report.passed());
}

#[test]
fn target_selection_runs_only_the_named_function_s_specs() {
    let source = concat!(
        "fn a(take x: Int) -> Int { x }\n",
        "fn b(take x: Int) -> Int { x }\n",
        "spec a { then a(1) == 1; }\n",
        "spec b { then b(1) == 1; }\n",
    );
    let report = run_with(
        source,
        &Selection::Target("a".to_owned()),
        Limits::default(),
    );
    assert_eq!(report.ran(), 1, "only `a`'s spec should run");
    assert_eq!(report.runs[0].name, "a");
}

#[test]
fn a_string_named_spec_is_selectable_by_its_written_name() {
    let source = "spec \"arithmetic holds\" { then 1 + 1 == 2; }\n";
    let report = run_with(
        source,
        &Selection::Target("arithmetic holds".to_owned()),
        Limits::default(),
    );
    assert_eq!(report.ran(), 1);
    assert!(report.passed());
}

#[test]
fn resource_limits_bound_a_runaway_recursion() {
    // An unbounded recursion trapped by a resource ceiling — the sandbox
    // never hangs. A low recursion depth trips first (and keeps the test off
    // the host stack); fuel is also capped as a backstop.
    let report = run_with(
        concat!(
            "fn spin(take n: Int) -> Int { spin(n) }\n",
            "spec spin { then spin(1) == 0; }\n",
        ),
        &Selection::All,
        Limits::with_fuel(100_000).max_depth(64),
    );
    match &report.runs[0].assertions[0].outcome {
        Outcome::Errored(trap) => assert!(
            trap.label == "recursion_limit" || trap.label == "out_of_fuel",
            "a runaway spec must be bounded, got {}",
            trap.label
        ),
        other => panic!("expected a resource abort, got {other:?}"),
    }
}

#[test]
fn a_program_with_errors_does_not_run_specs() {
    let mut map = SourceMap::new();
    let file = map.intern_file("bad.tuo");
    // `nope` is undefined — a resolution error.
    let id = map
        .add_source(file, "spec \"x\" { then nope() == 1; }\n")
        .expect("source interns");
    match tuo_spec::run(&map, &[id], &Selection::All, Limits::default()) {
        RunOutcome::NotChecked(problems) => {
            assert!(!problems.is_empty(), "front-end errors are reported");
        }
        RunOutcome::Ran(_) => panic!("a broken program must not run its specs"),
    }
}

#[test]
fn every_run_records_a_measured_duration() {
    let report = run(concat!(
        "fn add(take a: Int, take b: Int) -> Int { a + b }\n",
        "spec add { then add(1, 1) == 2; }\n",
    ));
    // Timing is instrumented; we assert it is present and additive, not that
    // it hits any particular latency (no promise is made).
    assert_eq!(report.total_duration(), report.runs[0].duration);
}

#[test]
fn a_spec_with_no_assertions_still_runs_its_setup() {
    // A spec whose body is only setup lowers and runs (proving the setup
    // executes); it has no assertions, so it trivially passes.
    let report = run(concat!(
        "fn touch(take x: Int) -> Int { x }\n",
        "spec touch { when let y = touch(3); }\n",
    ));
    assert_eq!(report.ran(), 1);
    assert!(report.passed());
    assert!(report.runs[0].assertions.is_empty());
}

#[test]
fn results_are_deterministic() {
    let source = concat!(
        "fn add(take a: Int, take b: Int) -> Int { a + b }\n",
        "spec add { then add(2, 2) == 4; assert add(0, 1) == 9; }\n",
    );
    let first = run(source);
    let second = run(source);
    assert_eq!(first.passed(), second.passed());
    assert_eq!(first.runs.len(), second.runs.len());
    // Same pass/fail shape across runs.
    for (a, b) in first.runs[0]
        .assertions
        .iter()
        .zip(&second.runs[0].assertions)
    {
        assert_eq!(
            matches!(a.outcome, Outcome::Passed),
            matches!(b.outcome, Outcome::Passed),
        );
    }
}
