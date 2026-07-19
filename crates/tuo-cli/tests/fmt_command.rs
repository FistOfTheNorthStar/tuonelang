//! End-to-end tests for `tuo fmt` and `tuo fmt --check`, run against the
//! real binary on copies of the fixture corpus.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

fn fixture(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fmt")
        .join(relative)
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("fmt_command");
    fs::create_dir_all(&dir).expect("scratch dir is creatable");
    dir.join(name)
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tuo"))
        .args(args)
        .output()
        .expect("the tuo binary runs")
}

#[test]
fn fmt_rewrites_a_messy_file_to_its_golden_form() {
    let path = scratch("messy.tuo");
    fs::copy(fixture("fixtures/hello_messy.tuo"), &path).expect("copy fixture");
    let output = run(&["fmt", path.to_str().expect("utf-8 path")]);
    assert!(output.status.success(), "fmt failed: {output:?}");
    let formatted = fs::read_to_string(&path).expect("file readable");
    let golden = fs::read_to_string(fixture("golden/hello_messy.tuo")).expect("golden exists");
    assert_eq!(formatted, golden);
}

#[test]
fn fmt_is_a_no_op_on_canonical_files() {
    let path = scratch("canonical.tuo");
    fs::copy(fixture("golden/showcase.tuo"), &path).expect("copy golden");
    let before = fs::read_to_string(&path).expect("file readable");
    let output = run(&["fmt", path.to_str().expect("utf-8 path")]);
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(fs::read_to_string(&path).expect("file readable"), before);
}

#[test]
fn check_reports_non_canonical_files_without_touching_them() {
    let path = scratch("check_dirty.tuo");
    fs::copy(fixture("fixtures/spacing_mess.tuo"), &path).expect("copy fixture");
    let before = fs::read_to_string(&path).expect("file readable");
    let output = run(&["fmt", "--check", path.to_str().expect("utf-8 path")]);
    assert!(!output.status.success(), "--check must fail on dirty files");
    let stdout = String::from_utf8(output.stdout).expect("utf-8 output");
    assert!(stdout.contains("would reformat"));
    assert_eq!(
        fs::read_to_string(&path).expect("file readable"),
        before,
        "--check must not modify files"
    );
}

#[test]
fn check_passes_on_canonical_files() {
    let path = scratch("check_clean.tuo");
    fs::copy(fixture("golden/declarations.tuo"), &path).expect("copy golden");
    let output = run(&["fmt", "--check", path.to_str().expect("utf-8 path")]);
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
}

#[test]
fn malformed_files_format_conservatively() {
    let path = scratch("malformed.tuo");
    fs::copy(fixture("fixtures/malformed_items.tuo"), &path).expect("copy fixture");
    let output = run(&["fmt", path.to_str().expect("utf-8 path")]);
    assert!(output.status.success());
    let formatted = fs::read_to_string(&path).expect("file readable");
    // The broken island survives byte-for-byte; the valid items around it
    // are canonical.
    assert!(formatted.contains("fn broken( {"));
    assert!(formatted.contains("struct Point {\n    x: F64,\n}"));
}

#[test]
fn unreadable_files_are_a_failure() {
    let output = run(&["fmt", "no/such/file.tuo"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("utf-8 error");
    assert!(stderr.contains("cannot read"));
}

#[test]
fn fmt_requires_at_least_one_file() {
    let output = run(&["fmt"]);
    assert!(!output.status.success(), "bare `tuo fmt` is a usage error");
}
