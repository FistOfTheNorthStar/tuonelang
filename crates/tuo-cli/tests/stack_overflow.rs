//! The stack-overflow trap (`TrapCode::StackOverflow`), end to end.
//!
//! Before it, a native program that ran out of stack died with `SIGSEGV`
//! (exit 139) — deep recursion on the main thread, the out-of-line drop glue of
//! a long `Box` chain (ADR-0028), and anything deep on a `par_map` worker,
//! whose platform-default stack was a sixteenth of the main thread's. Each is
//! now the runtime's ordinary trap: the stable message on stderr and the
//! fixed abort status, on both backends. A worker also gets the main thread's
//! stack size, so recursion that returns on one returns on the other.

use std::fs;
use std::os::unix::process::ExitStatusExt as _;
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Output};

use tuo_runtime::{TRAP_EXIT_STATUS, TrapCode};

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("stack_overflow");
    fs::create_dir_all(&dir).expect("scratch dir is creatable");
    dir.join(name)
}

/// The main thread's stack limit every program here runs under, in KiB: the
/// common 8 MiB default, set explicitly because the limit is inherited from
/// whoever runs the test. Whether a 10^6-node drop overflows depends on it —
/// LLVM's x86-64 glue frame is 16 bytes, so the drop needs about 16 MB and
/// completes under a 16 MiB limit, which is how this suite once passed on a
/// laptop and failed on a CI runner.
const MAIN_STACK_KIB: u32 = 8192;

/// Build `source` with the default (Cranelift) and the `--release` (LLVM)
/// backend and run each binary with `args` under [`MAIN_STACK_KIB`],
/// returning both outputs.
fn build_and_run(name: &str, source: &str, args: &[&str]) -> Vec<(&'static str, Output)> {
    let path = scratch(&format!("{name}.tuo"));
    fs::write(&path, source).expect("scratch source is writable");
    let mut outputs = Vec::new();
    for (backend, release) in [("cranelift", false), ("llvm", true)] {
        let exe = scratch(&format!("{name}-{backend}"));
        let mut build = Command::new(env!("CARGO_BIN_EXE_tuo"));
        build.arg("build");
        if release {
            build.arg("--release");
        }
        let built = build
            .arg("-o")
            .arg(&exe)
            .arg(&path)
            .output()
            .expect("the tuo binary runs");
        assert!(
            built.status.success(),
            "{name} builds on {backend}: {built:?}"
        );
        let ran = Command::new("sh")
            .arg("-c")
            .arg(format!("ulimit -s {MAIN_STACK_KIB} && exec \"$0\" \"$@\""))
            .arg(&exe)
            .args(args)
            .output()
            .expect("the built program runs");
        outputs.push((backend, ran));
    }
    outputs
}

/// The shell-visible status: a process killed by a signal reports
/// `128 + signal`, which is how `abort()` reaches [`TRAP_EXIT_STATUS`].
fn shell_status(status: ExitStatus) -> Option<i32> {
    status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal))
}

fn assert_stack_overflow_trap(name: &str, outputs: &[(&str, Output)]) {
    for (backend, output) in outputs {
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            shell_status(output.status),
            Some(TRAP_EXIT_STATUS),
            "{name} on {backend} must abort with the trap status, not crash: {output:?}"
        );
        assert!(
            stderr.contains(TrapCode::StackOverflow.message()),
            "{name} on {backend} must report the stack-overflow trap: {stderr}"
        );
    }
}

const RECURSION: &str = r#"fn depth(take n: Int) -> Int {
    if n == 0 {
        return 0;
    }
    1 + depth(n - 1)
}

fn task(take n: Int) -> Int {
    depth(n) % 7
}

fn main() -> Int {
    if std::rt::arg_count() > 1 {
        var tasks = std::array::empty();
        std::array::push(tasks, DEPTH);
        std::array::push(tasks, DEPTH);
        let out = std::rt::par_map(task, tasks, 2);
        return std::array::get(out, 0) + std::array::get(out, 1);
    }
    depth(DEPTH) % 7
}
"#;

fn recursion(depth: i64) -> String {
    RECURSION.replace("DEPTH", &depth.to_string())
}

#[test]
fn unbounded_recursion_on_the_main_thread_traps() {
    let outputs = build_and_run("main-deep", &recursion(100_000_000), &[]);
    assert_stack_overflow_trap("main-thread recursion", &outputs);
}

#[test]
fn unbounded_recursion_on_a_par_map_worker_traps() {
    let outputs = build_and_run("worker-deep", &recursion(100_000_000), &["worker"]);
    assert_stack_overflow_trap("worker recursion", &outputs);
}

#[test]
fn a_worker_has_the_main_threads_stack() {
    // 40,000 frames overflowed the 512 KiB macOS thread default; with an
    // 8 MiB worker stack it returns: 40000 % 7 == 2, twice.
    for (backend, output) in build_and_run("worker-40k", &recursion(40_000), &["worker"]) {
        assert_eq!(
            output.status.code(),
            Some(4),
            "40,000-deep worker recursion returns on {backend}: {output:?}"
        );
    }
}

#[test]
fn dropping_a_very_long_box_chain_traps_rather_than_crashing() {
    // The out-of-line drop glue recurses once per node (ADR-0028 Stage B), so
    // a 10^6-node list's drop is deeper than the 8 MiB stack.
    let source = r#"struct Cell {
    value: Int,
    next: Option[Box[Cell]],
}

fn main() -> Int {
    var cell = Cell { value: 0, next: None };
    var i = 0;
    while i < 1000000 {
        cell = Cell { value: i, next: Some { value: std::boxed::new(cell) } };
        i = i + 1;
    }
    7
}
"#;
    let outputs = build_and_run("long-chain", source, &[]);
    assert_stack_overflow_trap("dropping a 10^6-node box chain", &outputs);
}
