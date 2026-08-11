//! End-to-end tests for the native effect boundary (ADR-0006 Stage B).
//!
//! Effectful programs cannot go through the interpreter suites — the reference
//! interpreter never performs an effect, and the spec-purity gate (`R0007`)
//! only shields *specs*, so `tuo run` of an effectful `main` must simply work.
//! These tests therefore build and run real binaries through the actual `tuo`
//! binary with **both** backends (Cranelift by default, LLVM via `--release`)
//! and assert the observable process behavior: bytes written to the right
//! stream, the write's return value surfacing as the exit status, `exit`
//! terminating mid-`main` without running the rest, and `read_byte` echoing a
//! piped stdin byte (and reporting `-1`, exit byte 255, at EOF).
//!
//! The runtime seam under test is `tuo_runtime::effect::effect_runtime_c_source`
//! (`tuo_rt_write`/`tuo_rt_read_byte`/`tuo_rt_exit`), which the CLI links into
//! every built binary alongside the trap shim.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// A unique scratch directory per test (so tests do not collide when run in
/// parallel), rooted under Cargo's per-crate temp directory.
fn workspace(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("effects_native")
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch workspace is creatable");
    dir
}

/// Write `source` as a `.tuo` program and `tuo run` it with the chosen
/// backend, returning the completed process output.
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

/// Like [`run_program`], but with `stdin_bytes` piped to the program's stdin.
fn run_program_with_stdin(
    dir: &Path,
    name: &str,
    source: &str,
    release: bool,
    stdin_bytes: &[u8],
) -> Output {
    let path = dir.join(name);
    fs::write(&path, source).expect("program is writable");
    let mut command = Command::new(env!("CARGO_BIN_EXE_tuo"));
    command.arg("run");
    if release {
        command.arg("--release");
    }
    let mut child = command
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the tuo binary runs");
    child
        .stdin
        .as_mut()
        .expect("stdin is piped")
        .write_all(stdin_bytes)
        .expect("stdin bytes are writable");
    child.wait_with_output().expect("the program completes")
}

/// The backend a `release` flag selects, for failure messages.
fn backend_name(release: bool) -> &'static str {
    if release { "llvm" } else { "cranelift" }
}

/// `std::rt::write` to fd 1: the bytes land on stdout exactly, and the
/// write's return value (the byte count, 13) is `main`'s result and therefore
/// the process exit status — on both backends.
#[test]
fn write_to_stdout_returns_the_byte_count_and_lands_on_stdout() {
    let dir = workspace("write_stdout");
    let source = "fn main() -> Int {\n    std::rt::write(1, \"hello, world\\n\")\n}\n";
    for release in [false, true] {
        let output = run_program(&dir, "hello.tuo", source, release);
        let which = backend_name(release);
        assert_eq!(
            output.status.code(),
            Some(13),
            "{which}: main returns the write's return value (13 bytes written); stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "hello, world\n",
            "{which}: the written bytes must land on stdout, exactly"
        );
    }
}

/// `std::rt::exit(7)` mid-`main` terminates with status 7 without executing
/// the rest of the function: the later write never happens and the would-be
/// return value 99 is never observed — on both backends.
#[test]
fn exit_mid_main_terminates_without_running_the_rest() {
    let dir = workspace("exit_mid");
    let source = "fn main() -> Int {\n    std::rt::write(1, \"before\");\n    \
                  std::rt::exit(7);\n    std::rt::write(1, \"after\");\n    99\n}\n";
    for release in [false, true] {
        let output = run_program(&dir, "exit_mid.tuo", source, release);
        let which = backend_name(release);
        assert_eq!(
            output.status.code(),
            Some(7),
            "{which}: exit(7) is the process status, not the 99 main would return; stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "before",
            "{which}: output before the exit is flushed; nothing after it runs"
        );
    }
}

/// `std::rt::write` to fd 2 lands on stderr, not stdout — on both backends.
/// (stderr also carries toolchain noise such as linker warnings, so the
/// assertion is containment there and exact emptiness on stdout.)
#[test]
fn write_to_fd_2_lands_on_stderr_not_stdout() {
    let dir = workspace("write_stderr");
    let source = "fn main() -> Int {\n    std::rt::write(2, \"to-stderr-marker\")\n}\n";
    for release in [false, true] {
        let output = run_program(&dir, "err.tuo", source, release);
        let which = backend_name(release);
        assert_eq!(
            output.status.code(),
            Some(16),
            "{which}: 16 bytes written to fd 2"
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("to-stderr-marker"),
            "{which}: the bytes must land on stderr; got:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "",
            "{which}: nothing may land on stdout"
        );
    }
}

/// `std::rt::read_byte(0)` reads the piped stdin byte and `main` returns it:
/// the exit status is the byte's value — on both backends. At end of input it
/// returns `-1`, whose exit byte is 255.
#[test]
fn read_byte_echoes_piped_stdin_and_reports_eof() {
    let dir = workspace("read_byte");
    let source = "fn main() -> Int {\n    std::rt::read_byte(0)\n}\n";
    for release in [false, true] {
        let which = backend_name(release);
        let output = run_program_with_stdin(&dir, "echo.tuo", source, release, b"A");
        assert_eq!(
            output.status.code(),
            Some(65),
            "{which}: the piped byte 'A' (65) echoes as the exit status; stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let eof = run_program_with_stdin(&dir, "echo.tuo", source, release, b"");
        assert_eq!(
            eof.status.code(),
            Some(255),
            "{which}: EOF is -1, whose exit byte is 255"
        );
    }
}
