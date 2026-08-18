//! The dogfooding examples, kept honest by the real compiler.
//!
//! Prompt 39 built five real programs under top-level `examples/` (see
//! `DOGFOODING.md`). This suite drives the **real `tuo` binary** over every one
//! of them and asserts the exact contract each example's own doc comment and
//! specs promise, so the committed examples can never silently rot into
//! programs the compiler no longer accepts — the same discipline the shipped
//! corpus and the benchmark task-sets hold themselves to.
//!
//! For each example it asserts:
//!
//! * `tuo check` accepts it (front end: parse → resolve → type-check →
//!   ownership, specs included);
//! * `tuo test --manifest <pkg>` runs its specs green with **no** failures (the
//!   package is real, its manifest resolves, and every `spec` passes); and
//! * for the programs whose logic lives in the runnable core, `tuo run`
//!   compiles-links-runs them to the exact exit byte each example documents —
//!   proving "it runs natively" is a fact, not a claim. Since ADR-0006 landed
//!   the effect boundary, two examples also pin their **observable stdout**
//!   byte-for-byte: `cli-stats` prints its report through `std::io::println`
//!   (the ADR's "a CLI example gains a real println whose output is observed
//!   by a test" oracle) and `http-service` prints its response status line
//!   through its `std::rt::write` shell.
//!
//! `cli-stats` consumes the standard library as input: its `src/std_io.tuo` is
//! a verbatim copy of `tuo-stdlib`'s `std::io` module, and this suite pins the
//! copy byte-for-byte against the catalog so the two can never drift.
//!
//! The multi-package `workspace/app` binary is validated via
//! `tuo build --manifest … -o …` then executed, because `tuo run` is file-based
//! and has no package-aware form (DOGFOODING.md finding D-7).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The repository root, reached from this crate's manifest directory.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The absolute path to one example's directory under `examples/`.
fn example_dir(name: &str) -> PathBuf {
    repo_root().join("examples").join(name)
}

/// Run `tuo` with `args` and return the captured output.
fn tuo(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tuo"))
        .args(args)
        .output()
        .expect("the tuo binary runs")
}

/// Assert a command exited zero, printing both streams on failure.
fn expect_ok(output: &Output, what: &str) {
    assert!(
        output.status.success(),
        "{what} failed (status {:?})\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

/// `tuo check <srcs…>` must accept the program (all files form one program).
fn assert_checks(srcs: &[&Path]) {
    let mut args = vec!["check".to_string()];
    args.extend(srcs.iter().map(|s| s.display().to_string()));
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = tuo(&arg_refs);
    expect_ok(&out, &format!("tuo check {args:?}"));
}

/// `tuo test --manifest <pkg>` must run every spec green. We assert on success
/// status *and* on the absence of any failure marker in the human report, so a
/// silently-skipped or failing spec cannot pass this gate.
fn assert_specs_green(pkg: &Path) {
    let out = tuo(&["test", "--manifest", &pkg.display().to_string()]);
    expect_ok(&out, &format!("tuo test --manifest {}", pkg.display()));
    let report = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    // A green run reports `N passed, 0 failed` and names no FAILED spec. Both
    // are asserted so a silently-skipped or failing spec cannot slip through.
    assert!(
        !report.contains("FAILED"),
        "a spec was reported FAILED for {}:\n{report}",
        pkg.display()
    );
    assert!(
        report.contains("0 failed"),
        "expected `0 failed` in the spec report for {}:\n{report}",
        pkg.display()
    );
}

/// `tuo run <srcs…>` must compile, link, run, and exit with `expected` (the
/// documented observable exit byte). Returns the output so callers can also
/// assert the program's observable stdout.
fn assert_runs_to(srcs: &[&Path], expected: i32) -> Output {
    let mut args = vec!["run".to_string()];
    args.extend(srcs.iter().map(|s| s.display().to_string()));
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = tuo(&arg_refs);
    let code = out.status.code();
    assert_eq!(
        code,
        Some(expected),
        "tuo run {args:?} exited {code:?}, expected {expected}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stderr),
    );
    out
}

/// cli-stats: a runnable command-line statistics tool. Since ADR-0006 it
/// prints its report through `std::io::println` (consuming the stdlib module
/// as input); stdout is the exact four-line report and exit = 18.
#[test]
fn cli_stats_checks_specs_runs_and_prints_its_report() {
    let dir = example_dir("cli-stats");
    let main = dir.join("src/main.tuo");
    let std_io = dir.join("src/std_io.tuo");
    assert_checks(&[&main, &std_io]);
    assert_specs_green(&dir);
    let out = assert_runs_to(&[&main, &std_io], 18);
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "count 7\nmean 12\nsd 6\nreport 18\n",
        "cli-stats must print its documented report, byte for byte"
    );
}

/// cli-stats consumes the standard library as input: its vendored
/// `src/std_io.tuo` must equal the `tuo-stdlib` catalog's `std::io` module
/// byte-for-byte, so the example can never drift from the shipped library.
#[test]
fn cli_stats_vendored_std_io_matches_the_catalog() {
    let vendored = example_dir("cli-stats").join("src/std_io.tuo");
    let on_disk = std::fs::read_to_string(&vendored)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", vendored.display()));
    assert_eq!(
        on_disk,
        tuo_stdlib::IO.source,
        "examples/cli-stats/src/std_io.tuo drifted from tuo-stdlib's std/io.tuo; \
         re-copy the catalog module"
    );
}

/// data-pipeline: a runnable record-processing pipeline. total_for(2) = 400,
/// exit = 400 & 0xff = 144.
#[test]
fn data_pipeline_checks_specs_and_runs() {
    let dir = example_dir("data-pipeline");
    let main = dir.join("src/main.tuo");
    assert_checks(&[&main]);
    assert_specs_green(&dir);
    assert_runs_to(&[&main], 144);
}

/// http-service: the pure parsing/routing core is spec-checked, and since
/// ADR-0006 the demo `main` runs natively — it parses "GET /health HTTP/1.1",
/// prints the response status line through its `std::rt::write` shell, and
/// exits with the routed status code (200).
#[test]
fn http_service_parses_routes_prints_and_runs() {
    let dir = example_dir("http-service");
    let main = dir.join("src/main.tuo");
    assert_checks(&[&main]);
    assert_specs_green(&dir);
    let out = assert_runs_to(&[&main], 200);
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "HTTP/1.1 200 OK\n",
        "http-service must print its documented response line, byte for byte"
    );
}

/// concurrent-worker: the scheduling model runs (execution is contract-tier).
/// The makespan is 15, which `main` returns.
#[test]
fn concurrent_worker_scheduling_core_checks_specs_and_runs() {
    let dir = example_dir("concurrent-worker");
    let main = dir.join("src/main.tuo");
    assert_checks(&[&main]);
    assert_specs_green(&dir);
    assert_runs_to(&[&main], 15);
}

/// The medium multi-package workspace: `app → geometry → numeric`. Every
/// package's specs are green, and the whole-graph binary builds and executes to
/// the documented exit 26. Built (not `run`) because `tuo run` is file-based.
#[test]
fn workspace_graph_checks_specs_and_builds_to_a_runnable_binary() {
    let numeric = example_dir("workspace/numeric");
    let geometry = example_dir("workspace/geometry");
    let app = example_dir("workspace/app");

    // Each package checks and its specs pass (app's run covers the graph).
    for pkg in [&numeric, &geometry, &app] {
        let out = tuo(&["check", "--manifest", &pkg.display().to_string()]);
        expect_ok(&out, &format!("tuo check --manifest {}", pkg.display()));
    }
    assert_specs_green(&app);

    // Build the whole-graph binary and execute it: it must exit 26.
    let out_bin = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("dogfood_examples")
        .join("workspace-app");
    if let Some(parent) = out_bin.parent() {
        std::fs::create_dir_all(parent).expect("scratch dir is creatable");
    }
    let build = tuo(&[
        "build",
        "--manifest",
        &app.display().to_string(),
        "-o",
        &out_bin.display().to_string(),
    ]);
    expect_ok(&build, "tuo build --manifest workspace/app");

    let run = Command::new(&out_bin)
        .output()
        .expect("the built workspace binary runs");
    assert_eq!(
        run.status.code(),
        Some(26),
        "the workspace binary exited {:?}, expected 26",
        run.status.code(),
    );
}
