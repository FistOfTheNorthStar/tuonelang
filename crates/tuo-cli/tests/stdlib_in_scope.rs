//! ADR-0038 through the real binary: a one-file program calls the standard
//! library by path, with no copy of it, on every command.
//!
//! `std::collections` is the module that matters most here. It holds `pair`,
//! a generic struct helper the native backends do not lay out yet, so before
//! the native path compiled only what `main` reaches, *any* program that used
//! `std::collections` was refused by `tuo run` while `tuo spec` ran it fine.

use std::path::PathBuf;
use std::process::{Command, Output};

const PROGRAM: &str = "\
module app;

fn total(in xs: Array[Int]) -> Int {
    std::collections::sum(xs) + std::core::abs(0 - 1)
}

spec total {
    then total(std::collections::of3(1, 2, 3)) == 7;
}

fn main() -> Int {
    let _ = std::io::println(\"hi\");
    total(std::collections::of2(4, 5))
}
";

fn write_program(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tuo-stdlib-scope-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("app.tuo");
    std::fs::write(&path, PROGRAM).expect("write the program");
    path
}

fn tuo(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tuo"))
        .args(args)
        .output()
        .expect("the `tuo` binary runs")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn check_resolves_the_library_by_path() {
    let path = write_program("check");
    let out = tuo(&["check", path.to_str().unwrap()]);
    assert!(out.status.success(), "{}", text(&out.stderr));
}

#[test]
fn verify_runs_the_programs_spec_and_not_the_librarys() {
    let path = write_program("verify");
    let out = tuo(&["verify", path.to_str().unwrap()]);
    // The human report is diagnostics-shaped, so it is on stderr.
    let report = text(&out.stderr);
    assert!(out.status.success(), "{report}");
    assert!(
        report.contains("1 passed, 0 failed of 1 spec"),
        "only `total`'s spec runs: {report}"
    );
}

#[test]
fn verify_affected_by_selects_through_the_same_program() {
    // Affected-spec selection over a program in which one file calls the
    // library: an edit to `a.tuo` selects only `a`'s spec, and `b.tuo`'s
    // library call neither breaks the selection nor drags its spec in.
    let dir =
        std::env::temp_dir().join(format!("tuo-stdlib-scope-affected-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let a = dir.join("a.tuo");
    let b = dir.join("b.tuo");
    std::fs::write(
        &a,
        "module a;\n\npub fn one() -> Int {\n    1\n}\n\nspec one {\n    then one() == 1;\n}\n",
    )
    .expect("write a");
    std::fs::write(
        &b,
        "module b;\n\npub fn far(take x: Int) -> Int {\n    std::core::abs(x)\n}\n\nspec far {\n    then far(0 - 2) == 2;\n}\n",
    )
    .expect("write b");
    let (a, b) = (a.to_str().unwrap(), b.to_str().unwrap());
    let out = tuo(&["verify", "--affected-by", a, a, b]);
    let report = text(&out.stderr);
    assert!(out.status.success(), "{report}");
    assert!(
        report.contains("1 passed, 0 failed of 1 spec"),
        "only `one`'s spec: {report}"
    );
}

#[test]
fn run_compiles_only_what_main_reaches_on_both_backends() {
    let path = write_program("run");
    for flags in [&[][..], &["--release"][..]] {
        let mut args = vec!["run"];
        args.extend_from_slice(flags);
        args.push(path.to_str().unwrap());
        let out = tuo(&args);
        assert_eq!(
            text(&out.stdout),
            "hi\n",
            "{flags:?}: {}",
            text(&out.stderr)
        );
        // sum(4, 5) + abs(-1)
        assert_eq!(
            out.status.code(),
            Some(10),
            "{flags:?}: {}",
            text(&out.stderr)
        );
    }
}

#[test]
fn a_misspelled_module_is_still_an_error() {
    let dir = std::env::temp_dir().join(format!("tuo-stdlib-scope-typo-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("typo.tuo");
    std::fs::write(
        &path,
        "module app;\n\nfn main() -> Int {\n    std::colections::sum(std::array::empty())\n}\n",
    )
    .expect("write");
    let out = tuo(&["check", path.to_str().unwrap()]);
    assert!(!out.status.success());
    assert!(text(&out.stderr).contains("R0002"), "{}", text(&out.stderr));
}
