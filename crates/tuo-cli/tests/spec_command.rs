//! End-to-end tests for `tuo spec` and `tuo verify`, run against the real
//! binary. `tuo check` still parses/resolves/type-checks specs but never
//! executes them; these commands do.

use std::path::PathBuf;
use std::process::{Command, Output};

fn fixture(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/specs/fixtures")
        .join(relative)
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tuo"))
        .args(args)
        .output()
        .expect("the tuo binary runs")
}

fn path(name: &str) -> String {
    fixture(name).to_str().expect("utf-8 path").to_owned()
}

#[test]
fn spec_passes_for_a_program_whose_specs_hold() {
    let output = run(&["spec", &path("passing.tuo")]);
    assert!(output.status.success(), "all specs pass → success exit");
    let stderr = String::from_utf8(output.stderr).expect("utf-8");
    // Every spec is reported ok, and the summary counts them.
    assert!(stderr.contains("ok add"));
    assert!(stderr.contains("ok doubling works"));
    // Two spec blocks (the `add` spec holds two assertions but is one spec).
    assert!(stderr.contains("2 passed, 0 failed"));
}

#[test]
fn spec_fails_and_reports_expected_and_actual() {
    let output = run(&["spec", &path("failing.tuo")]);
    assert!(!output.status.success(), "a failing spec → failure exit");
    let stderr = String::from_utf8(output.stderr).expect("utf-8");
    assert!(stderr.contains("FAILED double"));
    assert!(stderr.contains("assert failed: double(10) == 999"));
    assert!(stderr.contains("expected: 999I64"));
    assert!(stderr.contains("actual:   20I64"));
}

#[test]
fn spec_target_runs_only_the_named_function_s_specs() {
    // `--target double` in a program with `add` and `double` specs runs only
    // `double`'s (which fails here).
    let output = run(&["spec", "--target", "double", &path("passing.tuo")]);
    // `passing.tuo` has no `double` spec, so nothing runs → success.
    assert!(output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("utf-8");
    assert!(stderr.contains("0 passed, 0 failed of 0 spec"));
}

#[test]
fn verify_runs_the_static_checks_and_the_specs() {
    let passing = run(&["verify", &path("passing.tuo")]);
    assert!(passing.status.success(), "clean program + passing specs");

    let failing = run(&["verify", &path("failing.tuo")]);
    assert!(!failing.status.success(), "a failing spec fails verify");
}

#[test]
fn check_does_not_execute_specs() {
    // `failing.tuo` type-checks fine; only its *assertion* is false. `check`
    // must succeed (it never runs specs), unlike `spec`/`verify`.
    let check = run(&["check", &path("failing.tuo")]);
    assert!(
        check.status.success(),
        "check parses/type-checks but does not execute specs"
    );
}

#[test]
fn spec_reports_measured_timing() {
    let output = run(&["spec", &path("passing.tuo")]);
    let stderr = String::from_utf8(output.stderr).expect("utf-8");
    // A duration unit appears in the summary — timing is instrumented.
    assert!(
        stderr.contains("µs") || stderr.contains("ms") || stderr.contains("s\n"),
        "the summary reports a measured duration"
    );
}
