//! End-to-end tests for ADR-0004 Stage 2 fixed-capacity arrays through the
//! real binary: `check` accepts real array programs, their specs run green
//! on the reference interpreter, an out-of-bounds index traps
//! deterministically, `fmt` is idempotent on the new syntax, the lowered MIR
//! never contains a `Len` of a fixed array (the length is a constant), array
//! programs compile and **run natively on both backends** (Cranelift debug
//! and LLVM `--release`) to the interpreter's exit byte — including the
//! deterministic out-of-bounds trap — borrow-mode parameters now run
//! natively too, and everything still outside the native subset (heap-backed
//! types such as `Str`) keeps refusing loudly, pointing back to the
//! interpreter.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("arrays_stage2");
    fs::create_dir_all(&dir).expect("scratch dir is creatable");
    dir.join(name)
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tuo"))
        .args(args)
        .output()
        .expect("the tuo binary runs")
}

fn write(name: &str, text: &str) -> String {
    let path = scratch(name);
    fs::write(&path, text).expect("scratch file is writable");
    path.to_str().expect("utf-8 path").to_owned()
}

/// A real array program: both literal forms, the type form, indexing,
/// iteration, nesting, and a generic struct with a fixed-array field (the
/// substitution-walker pin), each pinned by a spec.
const PROGRAM: &str = r#"struct Buffer[T] {
    xs: [T; 2],
}

fn sum_list() -> Int {
    let xs: [Int; 4] = [1, 2, 3, 4];
    xs[0] + xs[1] + xs[2] + xs[3]
}

fn repeat_fill() -> Int {
    let sevens = [7; 5];
    var total = 0;
    for x in sevens {
        total = total + x;
    }
    total
}

fn nested_pick() -> Bool {
    let grid: [[Bool; 2]; 3] = [[true, false]; 3];
    grid[2][0]
}

fn generic_buffer() -> Int {
    let buffer = Buffer { xs: [40, 2] };
    buffer.xs[0] + buffer.xs[1]
}

fn literal_iter() -> Int {
    var total = 0;
    for x in [1, 2, 3] {
        total = total + x;
    }
    total
}

spec sum_list {
    then sum_list() == 10;
}

spec repeat_fill {
    then repeat_fill() == 35;
}

spec nested_pick {
    then nested_pick() == true;
}

spec generic_buffer {
    then generic_buffer() == 42;
}

spec literal_iter {
    then literal_iter() == 6;
}
"#;

#[test]
fn check_accepts_and_specs_run_green_on_the_interpreter() {
    let path = write("program.tuo", PROGRAM);
    let check = run(&["check", &path]);
    assert!(check.status.success(), "check must accept: {check:?}");
    let spec = run(&["spec", &path]);
    assert!(spec.status.success(), "specs must pass: {spec:?}");
    let stderr = String::from_utf8(spec.stderr).expect("utf-8");
    assert!(
        stderr.contains("5 passed, 0 failed of 5 specs"),
        "all five specs run: {stderr}"
    );
}

#[test]
fn an_out_of_bounds_index_traps_deterministically() {
    let path = write(
        "oob.tuo",
        "fn pick(take i: Usize) -> Int {\n    let xs = [1, 2];\n    xs[i]\n}\n\nspec pick {\n    then pick(5) == 0;\n}\n",
    );
    let spec = run(&["spec", &path]);
    assert!(!spec.status.success(), "the oob spec must fail");
    let stderr = String::from_utf8(spec.stderr).expect("utf-8");
    assert!(
        stderr.contains("index_out_of_bounds"),
        "the trap is named: {stderr}"
    );
}

#[test]
fn fmt_is_idempotent_on_array_syntax() {
    let path = write("fmt.tuo", PROGRAM);
    let first = run(&["fmt", &path]);
    assert!(first.status.success(), "fmt succeeds: {first:?}");
    let check = run(&["fmt", "--check", &path]);
    assert!(
        check.status.success(),
        "formatted output is canonical (idempotent): {check:?}"
    );
}

#[test]
fn lowered_mir_has_constant_lengths_and_no_len_of_a_fixed_array() {
    let path = write("mir.tuo", PROGRAM);
    for target in ["sum_list", "repeat_fill", "literal_iter"] {
        let dump = run(&["debug", "mir", &path, target]);
        assert!(dump.status.success(), "mir dump for {target}: {dump:?}");
        let text = String::from_utf8(dump.stdout).expect("utf-8");
        assert!(
            !text.contains("len("),
            "{target}: a fixed array's length must lower as a constant, never `Len`:\n{text}"
        );
    }
}

/// A native array program exercising every Stage-2 backend obligation:
/// construction (both literal forms), indexing (constant and loop-counter),
/// a for-fold, nesting, whole-array copy, and array parameters/returns
/// through the by-pointer/sret call ABI. `main` returns 60:
/// `sum(make(2)) = 12`, `xs[0] = 1`, fold over `[1,2,3,4] = 10`,
/// `grid[2][1] + grid[0][0] = 3`, `repeat fold = 28`, `copied[3] = 6`.
const NATIVE_PROGRAM: &str = "fn make(take seed: Int) -> [Int; 3] {\n    [seed, seed * 2, seed * 3]\n}\n\nfn sum(take xs: [Int; 3]) -> Int {\n    var t = 0;\n    for x in xs {\n        t = t + x;\n    }\n    t\n}\n\nfn main() -> Int {\n    let xs = [1, 2, 3, 4];\n    var total = sum(make(2)) + xs[0];\n    for x in xs {\n        total = total + x;\n    }\n    let grid: [[Int; 2]; 3] = [[1, 2]; 3];\n    total = total + grid[2][1] + grid[0][0];\n    let sevens = [7; 4];\n    for s in sevens {\n        total = total + s;\n    }\n    let copied = [0, 2, 4, 6];\n    let ys = copied;\n    total + ys[3]\n}\n";

/// The exit byte the reference interpreter assigns to [`NATIVE_PROGRAM`],
/// pinned by its arithmetic above and proven live by running `main` through
/// `spec` semantics in `check_accepts_and_specs_run_green_on_the_interpreter`
/// (the interpreter is the reference; the native runs below must match it).
const NATIVE_EXIT: i32 = 60;

#[test]
fn array_programs_run_natively_on_both_backends() {
    let path = write("native.tuo", NATIVE_PROGRAM);

    // The reference first: the interpreter must agree with the pinned exit.
    let spec_program =
        format!("{NATIVE_PROGRAM}\nspec main {{\n    then main() == {NATIVE_EXIT};\n}}\n");
    let spec_path = write("native_spec.tuo", &spec_program);
    let spec = run(&["spec", &spec_path]);
    assert!(
        spec.status.success(),
        "the interpreter must compute the pinned exit: {spec:?}"
    );

    // Cranelift (debug) and LLVM (--release), each to the interpreter's byte.
    for args in [
        &["run", path.as_str()][..],
        &["run", "--release", &path][..],
    ] {
        let output = run(args);
        assert_eq!(
            output.status.code(),
            Some(NATIVE_EXIT),
            "`tuo {}` must exit with the interpreter's byte: {output:?}",
            args.join(" ")
        );
    }
}

#[test]
fn an_out_of_bounds_index_traps_deterministically_on_both_backends() {
    let path = write(
        "native_oob.tuo",
        "fn pick(take i: Usize) -> Int {\n    let xs = [1, 2];\n    xs[i]\n}\n\nfn main() -> Int {\n    pick(5)\n}\n",
    );
    for args in [
        &["run", path.as_str()][..],
        &["run", "--release", &path][..],
    ] {
        let output = run(args);
        assert!(
            !output.status.success(),
            "`tuo {}` must trap, not return: {output:?}",
            args.join(" ")
        );
        let stderr = String::from_utf8(output.stderr).expect("utf-8");
        assert!(
            stderr.contains("index out of bounds"),
            "`tuo {}` names the trap: {stderr}",
            args.join(" ")
        );
    }
}

#[test]
fn borrow_mode_parameters_now_run_natively() {
    // Borrow-mode parameters joined the native subset: the program checks
    // clean and both backends compile and run it to the interpreter's value
    // (the callee reads the caller's place through a pointer). The
    // loud-refusal contract for what remains outside the subset is pinned
    // below on a heap-backed type.
    let path = write(
        "borrow.tuo",
        "fn peek(in x: Int) -> Int {\n    x\n}\n\nfn main() -> Int {\n    let v = 7;\n    peek(v)\n}\n",
    );
    let check = run(&["check", &path]);
    assert!(check.status.success(), "the front end accepts: {check:?}");
    for args in [
        &["run", path.as_str()][..],
        &["run", "--release", &path][..],
    ] {
        let output = run(args);
        assert_eq!(
            output.status.code(),
            Some(7),
            "`tuo {}` runs the borrow-mode program to the interpreter's value: {output:?}",
            args.join(" ")
        );
    }
}

#[test]
fn native_backends_still_refuse_the_unsupported_loudly() {
    // The heap *wrappers* (`Box`/`Shared`/`Weak`) stay outside the native
    // subset (`Str` is lowered since ADR-0006 Stage B, and the owned `String`
    // and growable `Array[Int]` since ADR-0009 Stage B): the program checks
    // clean, and both backends refuse it loudly at classification time —
    // naming the concrete type — instead of mis-compiling it.
    let path = write(
        "heap.tuo",
        "fn keep(take b: Box[Int]) -> Int {\n    1\n}\n\nfn main() -> Int {\n    7\n}\n",
    );
    let check = run(&["check", &path]);
    assert!(check.status.success(), "the front end accepts: {check:?}");
    for args in [
        &["build", path.as_str()][..],
        &["build", "--release", &path][..],
    ] {
        let build = run(args);
        assert!(
            !build.status.success(),
            "`tuo {}` must refuse a heap-wrapper type: {build:?}",
            args.join(" ")
        );
        let stderr = String::from_utf8(build.stderr).expect("utf-8");
        assert!(
            stderr.contains("`Box[T]` heap wrapper") && stderr.contains("does not lower yet"),
            "the refusal names the unsupported type: {stderr}"
        );
        assert!(
            stderr.contains("remains the reference"),
            "the refusal points back to the interpreter: {stderr}"
        );
    }
}
