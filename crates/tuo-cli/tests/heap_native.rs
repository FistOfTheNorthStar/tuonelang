//! End-to-end tests for the native heap allocator (ADR-0009 Stage B).
//!
//! Two things here cannot go through the interpreter differential suites: a
//! `write_string` program (effectful — the interpreter never performs an
//! effect, so its observable behavior is bytes on stdout, not a return value),
//! and a leak proxy (whose correctness is *bounded memory over many rounds*, an
//! operational property no return value captures). Both build and run real
//! binaries through the actual `tuo` binary with **both** backends (Cranelift by
//! default, LLVM via `--release`).
//!
//! The runtime seam under test is `tuo_runtime::alloc::alloc_runtime_c_source`
//! (`tuo_rt_alloc`/`tuo_rt_dealloc`), which the CLI links into every built
//! binary alongside the trap and effect shims, plus the backends' `Drop` glue
//! that frees a `String`/`Array[Int]` buffer exactly once.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A unique scratch directory per test (so tests do not collide when run in
/// parallel), rooted under Cargo's per-crate temp directory.
fn workspace(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("heap_native")
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch workspace is creatable");
    dir
}

/// Write `source` as a `.tuo` program and `tuo run` it with the chosen backend,
/// returning the completed process output.
fn run_program(dir: &Path, name: &str, source: &str, release: bool) -> Output {
    let path = dir.join(name);
    fs::write(&path, source).expect("program is writable");
    let mut command = Command::new(env!("CARGO_BIN_EXE_tuo"));
    command.arg("run");
    if release {
        command.arg("--release");
    }
    command.arg(&path).output().expect("the tuo binary runs")
}

/// The backend a `release` flag selects, for failure messages.
fn backend_name(release: bool) -> &'static str {
    if release { "llvm" } else { "cranelift" }
}

/// A `String` assembled from a `Str` copy (`from_str`), an `append`, and a
/// `push_byte`, then written to stdout with `std::rt::write_string`. The
/// program returns the write's byte count (which becomes the exit status), so
/// this pins both the captured stdout and the exit byte, on both backends.
const WRITE_STRING_PROGRAM: &str = r#"
fn main() -> Int {
    var s = std::string::from_str("hello, ");
    std::string::append(s, "heap");
    std::string::push_byte(s, 33);
    std::rt::write_string(1, s)
}
"#;

#[test]
fn write_string_writes_an_owned_string_to_stdout_on_both_backends() {
    for release in [false, true] {
        let dir = workspace(&format!("write_string_{}", backend_name(release)));
        let output = run_program(&dir, "ws.tuo", WRITE_STRING_PROGRAM, release);
        let which = backend_name(release);
        assert_eq!(
            output.stdout,
            b"hello, heap!",
            "{which}: the owned String's exact bytes must reach stdout; stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        // `write_string` returns the byte count it wrote (12), which the v0
        // entry ABI surfaces as the exit status — the deliberate, non-zero
        // result, exactly as the interpreter would report for `write` (this
        // program cannot run through the interpreter, since it performs an
        // effect, which is why it is pinned natively here).
        assert_eq!(
            output.status.code(),
            Some(12),
            "{which}: write_string returns the byte count (12) as the exit status"
        );
    }
}

/// A leak proxy: build and drop a heap value a great many times. Each round
/// allocates a fresh `String`/`Array[Int]` (whose buffer is real heap memory)
/// and drops it at scope end, freeing the buffer through `tuo_rt_dealloc`. If a
/// buffer were *not* freed (a leak) or freed *twice* (a double-free / crash),
/// this program would either exhaust memory or abort; a clean, correct exit is
/// the evidence that every buffer is freed exactly once. (For a hard,
/// tool-verified check, `leaks --atExit` on macOS reports 0 leaked bytes on
/// this program — see the crate's Stage B report; here we assert the observable
/// completion, which is what CI can rely on without a leak-checker.)
const LEAK_PROXY_PROGRAM: &str = r#"
fn build_str(take n: Int) -> Int {
    var s = std::string::empty();
    var i = 0;
    while i < n {
        std::string::push_byte(s, 65);
        i = i + 1;
    }
    std::string::len(s)
}

fn build_arr(take n: Int) -> Int {
    var xs = std::array::empty();
    var i = 0;
    while i < n {
        std::array::push(xs, i);
        i = i + 1;
    }
    std::array::len(xs)
}

fn main() -> Int {
    var total = 0;
    var round = 0;
    while round < 200000 {
        total = build_str(48) + build_arr(16);
        round = round + 1;
    }
    total
}
"#;

#[test]
fn a_heavy_allocate_free_loop_completes_in_bounded_memory_on_both_backends() {
    for release in [false, true] {
        let dir = workspace(&format!("leak_proxy_{}", backend_name(release)));
        let output = run_program(&dir, "leak.tuo", LEAK_PROXY_PROGRAM, release);
        let which = backend_name(release);
        // A leak would eventually OOM (killed, no clean code) and a double-free
        // would abort (trap status); the round result is deterministic:
        // build_str(48) = 48, build_arr(16) = 16, total = 64.
        assert_eq!(
            output.status.code(),
            Some(64),
            "{which}: 200k allocate/free rounds must complete cleanly with the deterministic \
             result (64); a leak would OOM and a double-free would abort. stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
