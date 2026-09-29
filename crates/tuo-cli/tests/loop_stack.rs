//! Hot loops must run in constant stack on both backends.
//!
//! The LLVM backend once emitted the stack slots for call temporaries — an
//! aggregate argument's by-pointer copy, a `Str` literal's fat pointer, a map
//! shim's out-buffer — wherever the builder happened to be. Inside a loop body
//! that is a *dynamic* `alloca`, released only when the function returns, so
//! every iteration grew the stack and a long enough loop died with SIGSEGV
//! under `--release` alone (the Cranelift backend's stack slots are static).
//! The differential suites never saw it: their programs loop far too few times
//! to exhaust a stack, and the interpreter has no native stack to exhaust.
//!
//! Each program here drives one of those temporaries through a million
//! iterations — at the smallest leak (a 24-byte map buffer) that is 24 MB of
//! stack, well past the usual 8 MB default — and must return its exact
//! result on both backends.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Enough iterations that any per-iteration stack growth overflows a default
/// stack, while a correct build finishes in milliseconds.
const ITERATIONS: u64 = 1_000_000;

/// Build and run `source` with the chosen backend; return its exit status.
fn run(name: &str, source: &str, release: bool) -> i32 {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("loop_stack")
        .join(format!(
            "{name}_{}",
            if release { "llvm" } else { "cranelift" }
        ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch workspace is creatable");
    let path = dir.join(format!("{name}.tuo"));
    fs::write(&path, source).expect("program is writable");
    let mut command = Command::new(env!("CARGO_BIN_EXE_tuo"));
    command.arg("run");
    if release {
        command.arg("--release");
    }
    let output = command.arg(&path).output().expect("the tuo binary runs");
    output
        .status
        .code()
        .expect("`tuo run` reports a program killed by a signal as 128 + signo")
}

/// Assert `source` exits with `expected` under both backends.
fn assert_runs_on_both_backends(name: &str, source: &str, expected: i32) {
    for release in [false, true] {
        assert_eq!(
            run(name, source, release),
            expected,
            "{name} (release: {release}) must return its exact result \
             (139 = 128 + SIGSEGV: the loop overflowed the stack)"
        );
    }
}

#[test]
fn an_aggregate_argument_in_a_hot_loop_runs_in_constant_stack() {
    // `probe` takes the array by value and hands it on by value, so each
    // iteration materializes 64-byte by-pointer copies. (Passing a constant
    // array straight to a leaf lets LLVM fold the copy away, which hides the
    // bug; forwarding it does not.) 3 + 1 + 6 = 10 per call.
    let source = format!(
        "fn lookup(take table: [Int; 8], take i: Usize) -> Int {{
             table[i]
         }}
         fn probe(take table: [Int; 8]) -> Int {{
             lookup(table, 0) + lookup(table, 3) + lookup(table, 7)
         }}
         fn main() -> Int {{
             let table = [3, 1, 4, 1, 5, 9, 2, 6];
             var total = 0;
             var i = 0;
             while i < {ITERATIONS} {{
                 total = (total + probe(table)) % 251;
                 i = i + 1;
             }}
             total
         }}"
    );
    let expected = (10 * ITERATIONS) % 251;
    assert_runs_on_both_backends("aggregate_arg", &source, i32::try_from(expected).unwrap());
}

#[test]
fn a_str_literal_in_a_hot_loop_runs_in_constant_stack() {
    // Each call materializes the literal's two-word view into a temporary
    // slot, which `wrap` then forwards by value. 5 + 5 = 10 per call.
    let source = format!(
        "fn width(take s: Str) -> Int {{
             std::str::len(s)
         }}
         fn wrap(take s: Str) -> Int {{
             width(s) + width(s)
         }}
         fn main() -> Int {{
             var total = 0;
             var i = 0;
             while i < {ITERATIONS} {{
                 total = (total + wrap(\"hello\")) % 251;
                 i = i + 1;
             }}
             total
         }}"
    );
    let expected = (10 * ITERATIONS) % 251;
    assert_runs_on_both_backends("str_literal", &source, i32::try_from(expected).unwrap());
}

#[test]
fn map_operations_in_a_hot_loop_run_in_constant_stack() {
    // Every `insert` stages its value in a slot and every `get`/`insert`
    // receives its result through an out-buffer.
    let source = format!(
        "fn main() -> Int {{
             var m = std::map::empty();
             let _ = std::map::insert(m, 1, 7);
             var total = 0;
             var i = 0;
             while i < {ITERATIONS} {{
                 let _ = std::map::insert(m, 2, i);
                 let seven = match std::map::get(m, 1) {{
                     Some {{ value }} => value,
                     None => 0,
                 }};
                 total = (total + seven) % 251;
                 i = i + 1;
             }}
             total
         }}"
    );
    let expected = (7 * ITERATIONS) % 251;
    assert_runs_on_both_backends("map_ops", &source, i32::try_from(expected).unwrap());
}
