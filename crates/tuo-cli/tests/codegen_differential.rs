//! End-to-end differential tests for the native backend.
//!
//! The reference semantics of a tuonelang program is what the MIR interpreter
//! computes for its MIR; a native backend must agree with the interpreter, and
//! where they diverge the backend is wrong. These tests pin exactly that: for
//! each fixture they
//!
//! 1. run the front end and lower the program to verified MIR,
//! 2. execute `main` on the reference [`Interpreter`](tuo_mir_interp::Interpreter),
//! 3. build and run the same program natively with the real `tuo` binary
//!    (`tuo run`), and
//! 4. assert the native process's exit status equals the interpreter's result.
//!
//! By the v0 entry ABI the process exit status is the integer `main` returns,
//! truncated to a byte (exactly as a C `main` return becomes the exit code), so
//! the comparison is `native_exit == interpreter_value & 0xff`. A program that
//! traps aborts the interpreter and, natively, terminates with the runtime's
//! fixed trap status — the two must agree on *that* too.

use std::path::PathBuf;
use std::process::Command;

use tuo_compiler::source::SourceMap;
use tuo_mir_interp::{Interpreter, Value};

/// The runtime's fixed trap exit status (see `tuo_runtime::TRAP_EXIT_STATUS`).
const TRAP_EXIT_STATUS: i32 = 134;

/// The path to a fixture in `tests/codegen/fixtures/`.
fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/codegen/fixtures")
        .join(name)
}

/// The interpreter's result for `main` of the program in `path`: `Ok(value)`
/// for a normal return, `Err(())` if execution trapped.
fn interpret_main(path: &PathBuf) -> Result<i128, ()> {
    let text = std::fs::read_to_string(path).expect("fixture is readable");
    let mut map = SourceMap::new();
    let file = map.intern_file(&path.display().to_string());
    let id = map
        .add_source(file, text.as_str())
        .expect("fixture is valid source");

    // Front end → verified MIR (the same lowering the `build`/`run` path uses).
    let check = tuo_compiler::check_sources(&map, &[id]);
    assert!(
        !check.has_errors(),
        "fixture {} should type-check cleanly",
        path.display()
    );
    let parse = tuo_compiler::parser::parse(map.source(id));
    let asts = [tuo_compiler::ast::Ast::new(
        &parse.tree,
        map.source(id).text(),
    )];
    let lowered_hir = tuo_compiler::hir::lower(&asts, &check.resolution);
    let program = tuo_compiler::mir::lower(&lowered_hir, &check.resolution, &check.types);

    let interpreter =
        Interpreter::new(&program, &check.types).expect("lowered MIR verifies for the interpreter");
    match interpreter.run("main", Vec::new()) {
        Ok(Value::Int(value, _)) => Ok(value),
        Ok(other) => panic!("main returned a non-integer value: {}", other.render()),
        Err(_) => Err(()),
    }
}

/// Build and run the fixture natively with `tuo run`; return its exit status.
fn run_native(path: &PathBuf) -> i32 {
    let output = Command::new(env!("CARGO_BIN_EXE_tuo"))
        .arg("run")
        .arg(path)
        .output()
        .expect("the tuo binary runs");
    output
        .status
        .code()
        .expect("the native process exits with a code, not a signal")
}

/// A fixture whose `main` returns normally: the native exit status must equal
/// the interpreter's returned value, truncated to a byte.
fn assert_agrees(name: &str) {
    let path = fixture(name);
    let expected = interpret_main(&path).expect("this fixture returns normally");
    let native = run_native(&path);
    assert_eq!(
        native,
        (expected & 0xff) as i32,
        "native `tuo run` of {name} exited {native}, but the reference interpreter \
         computed {expected} (exit byte {}); the backend must match the interpreter",
        expected & 0xff
    );
}

#[test]
fn arithmetic_matches_the_interpreter() {
    assert_agrees("arithmetic.tuo");
}

#[test]
fn comparison_and_branching_match_the_interpreter() {
    assert_agrees("comparison.tuo");
}

#[test]
fn recursion_matches_the_interpreter() {
    assert_agrees("recursion.tuo");
}

#[test]
fn direct_calls_match_the_interpreter() {
    assert_agrees("calls.tuo");
}

#[test]
fn integer_division_matches_the_interpreter() {
    assert_agrees("division.tuo");
}

#[test]
fn stage1_aggregates_match_the_interpreter() {
    // ADR-0004 Stage 1 aggregate lowering (product types, scalar fields): the
    // default backend must agree with the reference interpreter on structs,
    // tuples, enums, `Option`, and `Result` crossing call boundaries.
    for name in [
        "unsupported_struct.tuo",
        "agg_struct_param.tuo",
        "agg_struct_return.tuo",
        "agg_struct_both.tuo",
        "agg_enum_match.tuo",
        "agg_enum_payload.tuo",
        "agg_option.tuo",
        "agg_result.tuo",
        "agg_nested.tuo",
        "agg_move.tuo",
    ] {
        assert_agrees(name);
    }
}

#[test]
fn a_trap_aborts_both_the_interpreter_and_the_native_binary() {
    // The reference interpreter traps on this program, and the native binary
    // must likewise abort — with the runtime's fixed trap status, distinct from
    // any value a program could deliberately return.
    let path = fixture("trap_div_zero.tuo");
    assert!(
        interpret_main(&path).is_err(),
        "the reference interpreter should trap on division by zero"
    );
    let native = run_native(&path);
    assert_eq!(
        native, TRAP_EXIT_STATUS,
        "a native trap must terminate with the runtime's fixed trap status"
    );
}

#[test]
fn a_program_outside_the_backend_subset_is_refused_not_miscompiled() {
    // A program the native backend does not lower yet (an aggregate carrying a
    // `String`/`Str`, which ADR-0004 Stage 1 does not cover) must be *refused*
    // with a failure exit, never silently mis-compiled. The interpreter remains
    // the reference and can still run it (checked elsewhere); here we assert
    // `tuo build` declines cleanly.
    let path = fixture("unsupported_string.tuo");
    let output = Command::new(env!("CARGO_BIN_EXE_tuo"))
        .arg("build")
        .arg("-o")
        .arg(std::env::temp_dir().join("tuo-unsupported-should-not-exist"))
        .arg(&path)
        .output()
        .expect("the tuo binary runs");
    assert!(
        !output.status.success(),
        "building an unsupported program must fail rather than emit a wrong binary"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("subset") || stderr.contains("does not lower"),
        "the refusal should explain the program is outside the backend subset; got: {stderr}"
    );
}
