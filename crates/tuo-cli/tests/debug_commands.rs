//! End-to-end tests for the `tuo debug syntax` / `ast` / `mir` developer
//! tools, run against the real binary.

use std::path::PathBuf;
use std::process::{Command, Output};

fn fixture(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/parser/fixtures")
        .join(relative)
}

fn mir_fixture(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/mir/golden")
        .join(relative)
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tuo"))
        .args(args)
        .output()
        .expect("the tuo binary runs")
}

#[test]
fn debug_syntax_dumps_the_cst_with_tokens_and_comments() {
    let path = fixture("ok/showcase.tuo");
    let output = run(&["debug", "syntax", path.to_str().expect("utf-8 path")]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf-8 dump");
    // Structure, tokens with lexemes, and exactness markers of the CST.
    assert!(stdout.starts_with("SourceFile"));
    assert!(stdout.contains("FunctionItem"));
    assert!(stdout.contains("KwFn \"fn\""));
    assert!(stdout.contains("DocComment"));
    // A clean file produces no diagnostics.
    assert!(output.stderr.is_empty());
}

#[test]
fn debug_ast_dumps_typed_views() {
    let path = fixture("ok/showcase.tuo");
    let output = run(&["debug", "ast", path.to_str().expect("utf-8 path")]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf-8 dump");
    assert!(stdout.starts_with("SourceFile"));
    for expected in ["FnDecl", "StructDecl", "EnumDecl", "SpecDecl", "ImplDecl"] {
        assert!(stdout.contains(expected), "missing {expected} in AST dump");
    }
    assert!(output.stderr.is_empty());
}

#[test]
fn malformed_files_still_dump_and_report_diagnostics_on_stderr() {
    let path = fixture("err/broken_items.tuo");
    let output = run(&["debug", "ast", path.to_str().expect("utf-8 path")]);
    // The dump succeeds (the parser is total); diagnostics go to stderr.
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf-8 dump");
    assert!(stdout.contains("Error «"), "recovered material is shown");
    assert!(stdout.contains("FnDecl"), "intact items are shown");
    let stderr = String::from_utf8(output.stderr).expect("utf-8 diagnostics");
    assert!(
        stderr.contains("P0002"),
        "recovery diagnostics are reported"
    );
}

#[test]
fn debug_mir_lowers_an_accepted_program() {
    let path = mir_fixture("control_flow.tuo");
    let output = run(&["debug", "mir", path.to_str().expect("utf-8 path")]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf-8 dump");
    // Functions, basic blocks, and terminators of the lowered MIR.
    assert!(stdout.contains("fn max("), "functions are shown");
    assert!(stdout.contains("bb0:"), "basic blocks are labeled");
    assert!(stdout.contains("branch "), "terminators are shown");
    assert!(output.stderr.is_empty());
}

#[test]
fn debug_mir_can_restrict_to_one_function() {
    let path = mir_fixture("control_flow.tuo");
    let output = run(&["debug", "mir", path.to_str().expect("utf-8 path"), "max"]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf-8 dump");
    assert!(stdout.contains("fn max("));
    assert!(
        !stdout.contains("fn classify("),
        "other functions are filtered out"
    );
}

#[test]
fn debug_mir_refuses_a_program_with_front_end_errors() {
    let path = fixture("err/broken_items.tuo");
    let output = run(&["debug", "mir", path.to_str().expect("utf-8 path")]);
    // MIR is only defined for accepted programs.
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("utf-8 diagnostics");
    assert!(stderr.contains("cannot lower MIR"));
}

#[test]
fn an_unreadable_file_is_a_failure_exit() {
    let output = run(&["debug", "syntax", "no/such/file.tuo"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("utf-8 error");
    assert!(stderr.contains("cannot read"));
    assert!(output.stdout.is_empty());
}

#[test]
fn debug_requires_a_subcommand() {
    let output = run(&["debug"]);
    assert!(
        !output.status.success(),
        "bare `tuo debug` is a usage error"
    );
}
