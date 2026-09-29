//! End-to-end tests for the package commands, driven through the real `tuo`
//! binary on temporary package workspaces.
//!
//! These prove the whole package lifecycle works as one system: `tuo new`
//! scaffolds a package that immediately `check`s and `test`s green; `tuo add`
//! wires up a path dependency and rewrites the lockfile; a build resolves the
//! transitive graph, loads every module, and runs every spec; the lockfile
//! pins dependency checksums so tampered dependency bytes are refused; and
//! `tuo package symbols` reports a package's real exported symbols in the
//! machine protocol — the "query installed package symbols without guessing"
//! surface.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A unique scratch directory per test (so tests do not collide when run in
/// parallel), rooted under Cargo's per-crate temp directory.
fn workspace(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("package_command")
        .join(name);
    // Start each run from a clean slate.
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch workspace is creatable");
    dir
}

/// Run `tuo` with `args`, in working directory `cwd`.
fn run_in(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tuo"))
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("the tuo binary runs")
}

/// Assert a command succeeded, printing its streams on failure.
fn expect_success(output: &Output, what: &str) {
    assert!(
        output.status.success(),
        "{what} failed (status {:?})\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn new_scaffolds_a_package_that_checks_and_tests_green() {
    let ws = workspace("new_scaffold");
    expect_success(&run_in(&ws, &["new", "app"]), "tuo new");

    let app = ws.join("app");
    assert!(app.join("tdg.toml").is_file(), "manifest was written");
    assert!(
        app.join("src/main.tuo").is_file(),
        "main module was written"
    );

    // The freshly scaffolded package checks with no errors …
    expect_success(&run_in(&app, &["check"]), "tuo check (package mode)");
    // … and its starter spec runs green.
    let test = run_in(&app, &["test"]);
    expect_success(&test, "tuo test");

    // The lockfile is written on the first compile command and pins the root.
    let lock = fs::read_to_string(app.join("tdg.lock")).expect("lockfile exists");
    assert!(
        lock.contains("name = \"app\""),
        "lockfile names the package"
    );
    assert!(lock.contains("source = \"root\""), "root package is marked");
    assert!(
        lock.contains("version = 1"),
        "lockfile format version present"
    );
}

#[test]
fn new_refuses_to_clobber_an_existing_package() {
    let ws = workspace("new_clobber");
    expect_success(&run_in(&ws, &["new", "app"]), "first new");
    let second = run_in(&ws, &["new", "app"]);
    assert!(
        !second.status.success(),
        "a second `new` over an existing manifest must fail"
    );
}

#[test]
fn add_wires_a_path_dependency_and_a_build_resolves_the_graph() {
    let ws = workspace("add_dependency");
    expect_success(&run_in(&ws, &["new", "app"]), "new app");
    expect_success(&run_in(&ws, &["new", "util"]), "new util");
    let app = ws.join("app");
    let util = ws.join("util");

    // Give `util` a real public function and a spec.
    fs::write(
        util.join("src/lib.tuo"),
        "module util;\n\n\
         /// Double an integer.\n\
         pub fn double(take x: Int) -> Int {\n    x + x\n}\n\n\
         spec double {\n    then double(21) == 42;\n}\n",
    )
    .expect("write util lib");

    // Wire the dependency and rewrite app to use it.
    expect_success(
        &run_in(&app, &["add", "util", "--path", "../util"]),
        "tuo add",
    );
    let manifest = fs::read_to_string(app.join("tdg.toml")).expect("manifest");
    assert!(
        manifest.contains("util = { path = \"../util\" }"),
        "the dependency was recorded: {manifest}"
    );
    // `add` re-resolves, so the lockfile now lists both packages.
    let lock = fs::read_to_string(app.join("tdg.lock")).expect("lockfile");
    assert!(
        lock.contains("name = \"util\""),
        "lock lists the dependency"
    );
    assert!(
        lock.contains("dependencies = [\"util\"]"),
        "app depends on util in the lock: {lock}"
    );

    fs::write(
        app.join("src/main.tuo"),
        "module app;\n\n\
         import util::double;\n\n\
         fn main() -> Int {\n    double(0)\n}\n\n\
         spec main {\n    then main() == 0;\n}\n",
    )
    .expect("write app main");

    // The whole graph checks and every spec across it runs green.
    expect_success(&run_in(&app, &["check"]), "tuo check with a dependency");
    let test = run_in(&app, &["test"]);
    expect_success(&test, "tuo test across the graph");
    let out = String::from_utf8_lossy(&test.stderr);
    assert!(
        out.contains("double") && out.contains("main"),
        "specs from both packages ran: {out}"
    );
}

#[test]
fn a_tampered_dependency_is_refused_by_the_lockfile_checksum() {
    let ws = workspace("checksum_drift");
    expect_success(&run_in(&ws, &["new", "app"]), "new app");
    expect_success(&run_in(&ws, &["new", "util"]), "new util");
    let app = ws.join("app");
    let util = ws.join("util");
    expect_success(
        &run_in(&app, &["add", "util", "--path", "../util"]),
        "tuo add",
    );
    // A first successful check writes/refreshes the lock with util's checksum.
    expect_success(&run_in(&app, &["check"]), "initial check");

    // Now tamper with the *dependency's* bytes after it was locked.
    let mut lib = fs::read_to_string(util.join("src/main.tuo")).expect("util main");
    lib.push_str("\n// drift\n");
    fs::write(util.join("src/main.tuo"), lib).expect("tamper util");

    // The build must refuse: the dependency no longer matches its locked hash.
    let checked = run_in(&app, &["check"]);
    assert!(
        !checked.status.success(),
        "a drifted dependency must be refused"
    );
    let err = String::from_utf8_lossy(&checked.stderr);
    assert!(
        err.contains("checksum mismatch") && err.contains("util"),
        "the refusal names the checksum drift: {err}"
    );
}

#[test]
fn editing_the_root_package_never_trips_the_checksum_guard() {
    let ws = workspace("root_edit");
    expect_success(&run_in(&ws, &["new", "app"]), "new app");
    let app = ws.join("app");
    expect_success(&run_in(&app, &["check"]), "first check writes the lock");

    // Editing the root package (the thing under development) must not be
    // treated as drift, even though its checksum changes.
    fs::write(
        app.join("src/main.tuo"),
        "module app;\n\nfn main() -> Int {\n    41 + 1\n}\n\nspec main {\n    then main() == 42;\n}\n",
    )
    .expect("edit root");
    expect_success(
        &run_in(&app, &["check"]),
        "checking after a root edit still succeeds",
    );
}

#[test]
fn remove_drops_a_dependency() {
    let ws = workspace("remove_dependency");
    expect_success(&run_in(&ws, &["new", "app"]), "new app");
    expect_success(&run_in(&ws, &["new", "util"]), "new util");
    let app = ws.join("app");
    expect_success(
        &run_in(&app, &["add", "util", "--path", "../util"]),
        "tuo add",
    );
    expect_success(&run_in(&app, &["remove", "util"]), "tuo remove");
    let manifest = fs::read_to_string(app.join("tdg.toml")).expect("manifest");
    assert!(
        !manifest.contains("util = {"),
        "the dependency was removed: {manifest}"
    );
    // Removing an absent dependency is an error, not a silent success.
    let again = run_in(&app, &["remove", "util"]);
    assert!(!again.status.success(), "removing an absent dep fails");
}

#[test]
fn package_symbols_reports_real_exports_in_the_machine_protocol() {
    let ws = workspace("symbols_query");
    expect_success(&run_in(&ws, &["new", "util"]), "new util");
    let util = ws.join("util");
    fs::write(
        util.join("src/lib.tuo"),
        "module util;\n\n\
         /// Triple an integer.\n\
         pub fn triple(take x: Int) -> Int {\n    x + x + x\n}\n\n\
         /// A private helper, not exported.\n\
         fn helper(take x: Int) -> Int {\n    x\n}\n",
    )
    .expect("write util lib");

    let output = run_in(&util, &["--message-format=json", "package", "symbols"]);
    expect_success(&output, "package symbols");
    let stdout = String::from_utf8_lossy(&output.stdout);

    // The public function is reported; the private helper is not.
    assert!(
        stdout.contains("\"triple\""),
        "the exported function is listed: {stdout}"
    );
    assert!(
        !stdout.contains("\"helper\""),
        "a private function is not exported: {stdout}"
    );
    assert!(
        stdout.contains("\"kind\":\"function\""),
        "the symbol kind is reported: {stdout}"
    );
    assert!(
        stdout.contains("\"package\":\"util\""),
        "the query names the package: {stdout}"
    );

    // `symbols` is a machine query: it refuses a human format rather than
    // emitting an unstable human dump.
    let human = run_in(&util, &["package", "symbols"]);
    assert!(
        !human.status.success(),
        "package symbols refuses a human format"
    );
}

#[test]
fn resolution_is_deterministic() {
    // The same workspace resolves to a byte-identical lockfile every time.
    let ws = workspace("determinism");
    expect_success(&run_in(&ws, &["new", "app"]), "new app");
    expect_success(&run_in(&ws, &["new", "util"]), "new util");
    let app = ws.join("app");
    expect_success(
        &run_in(&app, &["add", "util", "--path", "../util"]),
        "tuo add",
    );
    let first = fs::read_to_string(app.join("tdg.lock")).expect("first lock");
    expect_success(&run_in(&app, &["check"]), "re-resolve");
    let second = fs::read_to_string(app.join("tdg.lock")).expect("second lock");
    assert_eq!(first, second, "the lockfile is deterministic");
}

/// Build a two-package graph whose `main` returns a value that can only come
/// from the dependency: `app` calls `util::double(21)`. Returns `(app, util)`.
fn app_depending_on_util(ws: &Path) -> (PathBuf, PathBuf) {
    expect_success(&run_in(ws, &["new", "app"]), "new app");
    expect_success(&run_in(ws, &["new", "util"]), "new util");
    let app = ws.join("app");
    let util = ws.join("util");
    fs::write(
        util.join("src/lib.tuo"),
        "module util;\n\n\
         /// Double an integer.\n\
         pub fn double(take x: Int) -> Int {\n    x + x\n}\n\n\
         spec double {\n    then double(21) == 42;\n}\n",
    )
    .expect("write util lib");
    expect_success(
        &run_in(&app, &["add", "util", "--path", "../util"]),
        "tuo add",
    );
    fs::write(
        app.join("src/main.tuo"),
        "module app;\n\n\
         import util::double;\n\n\
         fn main() -> Int {\n    double(21)\n}\n\n\
         spec main {\n    then main() == 42;\n}\n",
    )
    .expect("write app main");
    (app, util)
}

#[test]
fn run_with_no_files_runs_the_package_across_its_graph() {
    let ws = workspace("package_run");
    let (app, _util) = app_depending_on_util(&ws);

    // From inside the package: the exit status is `main`'s own value, which
    // exists only if the dependency's code was resolved, compiled, and linked.
    let inside = run_in(&app, &["run"]);
    assert_eq!(
        inside.status.code(),
        Some(42),
        "stderr:\n{}",
        String::from_utf8_lossy(&inside.stderr)
    );

    // From elsewhere, naming the package directory.
    let outside = run_in(&ws, &["run", "--manifest", "app"]);
    assert_eq!(
        outside.status.code(),
        Some(42),
        "stderr:\n{}",
        String::from_utf8_lossy(&outside.stderr)
    );

    // A run leaves no executable behind in the package, unlike a build.
    assert!(
        !app.join("app").exists(),
        "`run` wrote a binary into the package"
    );
}

#[test]
fn run_refuses_a_tampered_dependency_before_executing_anything() {
    let ws = workspace("package_run_drift");
    let (app, util) = app_depending_on_util(&ws);
    assert_eq!(run_in(&app, &["run"]).status.code(), Some(42));

    // Change the dependency's bytes behind the lockfile's back. Were the
    // program to run, it would now exit 63 — so any status other than the
    // refusal proves drifted code executed.
    fs::write(
        util.join("src/lib.tuo"),
        "module util;\n\npub fn double(take x: Int) -> Int {\n    x + x + x\n}\n",
    )
    .expect("tamper with util");
    let refused = run_in(&app, &["run"]);
    assert_ne!(refused.status.code(), Some(63), "drifted code was executed");
    assert!(!refused.status.success());
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        stderr.contains("checksum"),
        "the refusal names the cause: {stderr}"
    );
}

#[test]
fn the_written_lockfile_is_portable() {
    let ws = workspace("portable_lock");
    let (app, _util) = app_depending_on_util(&ws);
    let lock = fs::read_to_string(app.join("tdg.lock")).expect("lockfile");
    assert!(
        lock.contains("source = \"path+../util\""),
        "the dependency is recorded relative to the root package: {lock}"
    );
    let here = ws.canonicalize().expect("workspace exists");
    assert!(
        !lock.contains(&here.display().to_string()),
        "the lockfile embeds this machine's path: {lock}"
    );
}
