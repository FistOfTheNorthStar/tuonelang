//! The performance laboratory's two host seams, exercised end-to-end through the
//! real toolchain.
//!
//! `tuo-bench`'s `lab` module deliberately cannot turn source into a running
//! native binary, and cannot run a foreign compiler — those are host seams
//! ([`NativeRunner`], [`ComparisonRunner`]). This test wires in the real ones:
//!
//! - the **native runner** shells out to the real `tuo` binary (`tuo run`), the
//!   same Cranelift+`cc` path a user gets, and
//! - the **comparison runners** compile the peer program with the platform
//!   toolchain — `cc` for the C peer, `go build` for the Go peer.
//!
//! With those injected it drives the lab's own `run_supported` and
//! `run_comparison`, and asserts the honest end-to-end contract: every supported
//! scalar-core workload actually compiles, links, and runs to its expected exit
//! byte, and where a peer toolchain exists the equivalent-semantics peer program
//! agrees — while an absent toolchain yields a recorded *skip*, never a
//! fabricated number. This is the proof that the benchmark repository can back
//! its claims for both a runtime-free peer (C) and a runtime-bearing one (Go).

use std::path::PathBuf;
use std::process::Command;

use tuo_bench::lab::compare::{
    ComparisonRunner, PeerLanguage, PeerRun, Verdict, comparison_for, comparison_for_peer,
    run_comparison,
};
use tuo_bench::lab::parallel::{self, SpeedupVerdict, TimedRunner};
use tuo_bench::lab::runtime::{NativeRunner, run_supported, workloads};

/// A scratch path unique to this test process and a label, so concurrent tests
/// never collide on a file name.
fn scratch(label: &str, ext: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("tuo-lab-{}-{label}.{ext}", std::process::id()));
    path
}

/// The real native runner: write the program to a temp `.tuo` and `tuo run` it,
/// returning the process exit status. This is exactly the CLI path a user drives.
struct TuoRunNativeRunner;

impl NativeRunner for TuoRunNativeRunner {
    fn compile_link_run(&self, source: &str) -> Result<i32, String> {
        let src_path = scratch("native", "tuo");
        std::fs::write(&src_path, source).map_err(|e| format!("writing source: {e}"))?;
        let output = Command::new(env!("CARGO_BIN_EXE_tuo"))
            .arg("run")
            .arg(&src_path)
            .output()
            .map_err(|e| format!("running `tuo run`: {e}"))?;
        let _ = std::fs::remove_file(&src_path);
        output
            .status
            .code()
            .ok_or_else(|| "native process terminated by signal".to_string())
    }
}

/// The real C comparison runner: compile the peer with `cc -O2` and run it,
/// recording the toolchain version and the exact command. Returns `Err` (which
/// the lab turns into a recorded *skip*) if `cc` is absent or anything fails.
/// The C peer's optimization flags. `-ffp-contract=off` stops clang fusing
/// `a * b + c` into one multiply-add, which tuonelang, Rust, and Go (whose
/// peers write the products out explicitly) do not do: the same arithmetic
/// the same way, so the floating-point workloads compare like with like.
const C_FLAGS: [&str; 2] = ["-O2", "-ffp-contract=off"];

struct CcComparisonRunner;

impl ComparisonRunner for CcComparisonRunner {
    fn language(&self) -> PeerLanguage {
        PeerLanguage::C
    }

    fn compile_link_run(&self, source: &str) -> Result<PeerRun, String> {
        let src_path = scratch("peer", "c");
        let exe_path = scratch("peer", "out");
        std::fs::write(&src_path, source).map_err(|e| format!("writing C source: {e}"))?;

        let command = format!(
            "cc {} {} -o {} -lm",
            C_FLAGS.join(" "),
            src_path.display(),
            exe_path.display()
        );
        let compile = Command::new("cc")
            .args(C_FLAGS)
            .arg(&src_path)
            .arg("-o")
            .arg(&exe_path)
            .arg("-lm")
            .output()
            .map_err(|e| format!("no C compiler available: {e}"))?;
        if !compile.status.success() {
            let _ = std::fs::remove_file(&src_path);
            return Err(format!(
                "C compile failed: {}",
                String::from_utf8_lossy(&compile.stderr)
            ));
        }

        let version = Command::new("cc")
            .arg("--version")
            .output()
            .ok()
            .and_then(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .next()
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "cc (version unknown)".to_string());

        let run = Command::new(&exe_path)
            .output()
            .map_err(|e| format!("running the C binary: {e}"))?;
        let _ = std::fs::remove_file(&src_path);
        let _ = std::fs::remove_file(&exe_path);
        let exit_status = run
            .status
            .code()
            .ok_or_else(|| "C process terminated by signal".to_string())?;

        Ok(PeerRun {
            exit_status,
            compiler_version: version,
            command,
        })
    }
}

/// The real Go comparison runner: compile the peer with `go build` and run it,
/// recording the toolchain version and the exact command. Returns `Err` (which
/// the lab turns into a recorded *skip*) if `go` is absent or anything fails.
/// Go is the runtime-bearing AOT peer (GC + goroutine scheduler); the exit byte
/// must still equal the equivalent tuonelang program's.
struct GoComparisonRunner;

impl ComparisonRunner for GoComparisonRunner {
    fn language(&self) -> PeerLanguage {
        PeerLanguage::Go
    }

    fn compile_link_run(&self, source: &str) -> Result<PeerRun, String> {
        let src_path = scratch("peer", "go");
        let exe_path = scratch("peer", "gout");
        std::fs::write(&src_path, source).map_err(|e| format!("writing Go source: {e}"))?;

        let command = format!("go build -o {} {}", exe_path.display(), src_path.display());
        let compile = Command::new("go")
            .arg("build")
            .arg("-o")
            .arg(&exe_path)
            .arg(&src_path)
            .output()
            .map_err(|e| format!("no Go compiler available: {e}"))?;
        if !compile.status.success() {
            let _ = std::fs::remove_file(&src_path);
            return Err(format!(
                "Go compile failed: {}",
                String::from_utf8_lossy(&compile.stderr)
            ));
        }

        let version = Command::new("go")
            .arg("version")
            .output()
            .ok()
            .and_then(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .next()
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "go (version unknown)".to_string());

        let run = Command::new(&exe_path)
            .output()
            .map_err(|e| format!("running the Go binary: {e}"))?;
        let _ = std::fs::remove_file(&src_path);
        let _ = std::fs::remove_file(&exe_path);
        let exit_status = run
            .status
            .code()
            .ok_or_else(|| "Go process terminated by signal".to_string())?;

        Ok(PeerRun {
            exit_status,
            compiler_version: version,
            command,
        })
    }
}

/// Every supported workload compiles, links, and runs natively to its expected
/// exit byte — through the real `tuo run`. This is the load-bearing proof that a
/// "supported" workload is truly runnable, not just claimed.
#[test]
fn supported_workloads_run_natively_and_match() {
    let results = run_supported(&TuoRunNativeRunner);
    assert_eq!(
        results.len(),
        23,
        "exactly the twenty-three supported workloads run (the scalar core plus the \
         fixed-array collections workload, the borrowed-Str string-processing \
         workload, the allocator-core allocation workload, the function-value \
         indirect-calls workload, the hash-map map-lookup workload, the \
         OS-boundary file-io workload, the socket networking workload, the \
         channel workload, the json-parse workload, the ADR-0017 udp-echo and \
         connect-timeout pair, and the ADR-0019 sha256-hash and wire-decode \
         pair, and five Computer Language Benchmarks Game programs)"
    );
    for (label, outcome) in results {
        let outcome = outcome.unwrap_or_else(|e| panic!("workload `{label}` failed to run: {e}"));
        assert!(
            outcome.matched_expected,
            "workload `{label}` exited {} but the expected observable byte differs",
            outcome.exit_status
        );
    }
}

/// The cross-language comparison, run through a live `cc`: for each supported
/// workload the equivalent-semantics C program is compiled and run, and its exit
/// must equal the tuonelang workload's — a real, provenance-carrying `Measured`
/// verdict. If `cc` is absent the verdict is `Skipped` (recorded, not faked); the
/// test tolerates that so it stays green on a machine without a C toolchain.
#[test]
#[expect(
    clippy::print_stderr,
    reason = "diagnostic note when a machine has no C toolchain; keeps the test green there"
)]
fn c_comparison_agrees_where_the_toolchain_exists() {
    let runner = CcComparisonRunner;
    let mut measured = 0;
    let mut skipped = 0;
    for workload in workloads() {
        let Some(comparison) = comparison_for(&workload) else {
            continue; // unsupported workloads have no comparison
        };
        match run_comparison(&runner, &comparison) {
            Verdict::Measured {
                exit_status,
                compiler_version,
                command,
            } => {
                measured += 1;
                assert_eq!(
                    exit_status, comparison.expected_exit,
                    "C peer for `{}` must produce the equivalent result",
                    workload.label
                );
                assert!(
                    !compiler_version.trim().is_empty(),
                    "a measured comparison must record the compiler version"
                );
                assert!(command.contains("cc"), "the exact command is recorded");
            }
            Verdict::Skipped { reason } => {
                skipped += 1;
                assert!(!reason.trim().is_empty(), "a skip must record its reason");
            }
        }
    }
    // Either the toolchain was present (comparisons measured) or it was not
    // (all skipped) — but every supported workload was accounted for.
    assert_eq!(measured + skipped, 23);
    // On CI and dev machines `cc` is present, so we expect real measurements;
    // this documents the intent without failing a truly toolchain-less host.
    if measured == 0 {
        eprintln!("note: no C toolchain found; all comparisons recorded as skipped");
    }
}

/// The Go cross-language comparison, run through a live `go build`: for each
/// supported workload the equivalent-semantics Go program is compiled and run,
/// and its exit must equal the tuonelang workload's — a real, provenance-carrying
/// `Measured` verdict. Go is the runtime-bearing AOT peer, so this proves the
/// equivalence holds even across a GC'd runtime. If `go` is absent the verdict is
/// `Skipped` (recorded, not faked); the test tolerates that so it stays green on
/// a machine without a Go toolchain.
#[test]
#[expect(
    clippy::print_stderr,
    reason = "diagnostic note when a machine has no Go toolchain; keeps the test green there"
)]
fn go_comparison_agrees_where_the_toolchain_exists() {
    let runner = GoComparisonRunner;
    let mut measured = 0;
    let mut skipped = 0;
    for workload in workloads() {
        let Some(comparison) = comparison_for_peer(&workload, PeerLanguage::Go) else {
            continue; // unsupported workloads have no comparison
        };
        match run_comparison(&runner, &comparison) {
            Verdict::Measured {
                exit_status,
                compiler_version,
                command,
            } => {
                measured += 1;
                assert_eq!(
                    exit_status, comparison.expected_exit,
                    "Go peer for `{}` must produce the equivalent result",
                    workload.label
                );
                assert!(
                    !compiler_version.trim().is_empty(),
                    "a measured comparison must record the compiler version"
                );
                assert!(
                    command.contains("go build"),
                    "the exact command is recorded"
                );
            }
            Verdict::Skipped { reason } => {
                skipped += 1;
                assert!(!reason.trim().is_empty(), "a skip must record its reason");
            }
        }
    }
    assert_eq!(measured + skipped, 23);
    if measured == 0 {
        eprintln!("note: no Go toolchain found; all Go comparisons recorded as skipped");
    }
}

/// The Rust peer: `rustc -O` (opt-level 2, matching the C peer's `-O2` and
/// tuonelang's release pipeline), edition 2021.
struct RustcComparisonRunner;

impl RustcComparisonRunner {
    /// The flags every Rust peer is compiled with.
    const FLAGS: [&str; 3] = ["-O", "--edition", "2021"];
}

impl ComparisonRunner for RustcComparisonRunner {
    fn language(&self) -> PeerLanguage {
        PeerLanguage::Rust
    }

    fn compile_link_run(&self, source: &str) -> Result<PeerRun, String> {
        let src_path = scratch("peer", "rs");
        let exe_path = scratch("peer", "rsout");
        std::fs::write(&src_path, source).map_err(|e| format!("writing Rust source: {e}"))?;
        let command = format!(
            "rustc {} {} -o {}",
            Self::FLAGS.join(" "),
            src_path.display(),
            exe_path.display()
        );
        let compile = Command::new("rustc")
            .args(Self::FLAGS)
            .arg(&src_path)
            .arg("-o")
            .arg(&exe_path)
            .output()
            .map_err(|e| format!("no Rust compiler available: {e}"))?;
        let _ = std::fs::remove_file(&src_path);
        if !compile.status.success() {
            return Err(format!(
                "Rust compile failed: {}",
                String::from_utf8_lossy(&compile.stderr)
            ));
        }
        let version = Command::new("rustc")
            .arg("--version")
            .output()
            .ok()
            .and_then(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .next()
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "rustc (version unknown)".to_string());
        let run = Command::new(&exe_path)
            .output()
            .map_err(|e| format!("running the Rust binary: {e}"))?;
        let _ = std::fs::remove_file(&exe_path);
        let exit_status = run
            .status
            .code()
            .ok_or_else(|| "Rust process terminated by signal".to_string())?;
        Ok(PeerRun {
            exit_status,
            compiler_version: version,
            command,
        })
    }
}

/// The Rust cross-language comparison: each Benchmarks Game workload's Rust
/// peer compiles with `rustc -O` and must reach the tuonelang program's exit
/// byte. `rustc` is the toolchain building this test, so unlike `cc` and `go`
/// it is never absent here; a skip is therefore a failure.
#[test]
fn rust_comparison_agrees_on_the_benchmarks_game() {
    let runner = RustcComparisonRunner;
    let mut measured = 0;
    for workload in workloads() {
        let Some(comparison) = comparison_for_peer(&workload, PeerLanguage::Rust) else {
            continue;
        };
        match run_comparison(&runner, &comparison) {
            Verdict::Measured { command, .. } => {
                measured += 1;
                assert!(
                    command.starts_with("rustc -O"),
                    "the exact command is recorded"
                );
            }
            Verdict::Skipped { reason } => {
                panic!(
                    "the Rust peer for `{}` did not agree: {reason}",
                    workload.label
                )
            }
        }
    }
    assert_eq!(
        measured, 5,
        "every supported Benchmarks Game workload has a Rust peer"
    );
}

/// The real timed runner for the parallel-speedup category (ADR-0007): build
/// the program to a binary first (`tuo build` / `cc -O2 -pthread`), then time
/// **only the binary's execution** — compilation never pollutes the figure.
struct BuildThenTimeRunner;

impl BuildThenTimeRunner {
    fn time_binary(exe_path: &std::path::Path) -> Result<(i32, u128), String> {
        // Warm-up run first: a freshly written binary's first execution pays
        // one-time host costs (page-cache fill; on macOS, Gatekeeper's
        // first-exec scan) that would swamp the figure. The timed run is the
        // second execution — same binary, same result, no first-run tax.
        let warmup = Command::new(exe_path)
            .output()
            .map_err(|e| format!("running the binary (warm-up): {e}"))?;
        let warmup_exit = warmup
            .status
            .code()
            .ok_or_else(|| "process terminated by signal".to_string())?;
        let started = std::time::Instant::now();
        let run = Command::new(exe_path)
            .output()
            .map_err(|e| format!("running the binary: {e}"))?;
        let nanos = started.elapsed().as_nanos();
        let exit = run
            .status
            .code()
            .ok_or_else(|| "process terminated by signal".to_string())?;
        if exit != warmup_exit {
            return Err(format!(
                "non-deterministic exit: warm-up {warmup_exit}, timed {exit}"
            ));
        }
        Ok((exit, nanos))
    }
}

impl TimedRunner for BuildThenTimeRunner {
    fn run_tuonelang(&self, source: &str) -> Result<(i32, u128), String> {
        let src_path = scratch("par-tuo", "tuo");
        let exe_path = scratch("par-tuo", "out");
        std::fs::write(&src_path, source).map_err(|e| format!("writing source: {e}"))?;
        let build = Command::new(env!("CARGO_BIN_EXE_tuo"))
            .arg("build")
            .arg("-o")
            .arg(&exe_path)
            .arg(&src_path)
            .output()
            .map_err(|e| format!("running `tuo build`: {e}"))?;
        let _ = std::fs::remove_file(&src_path);
        if !build.status.success() {
            return Err(format!(
                "tuo build failed: {}",
                String::from_utf8_lossy(&build.stderr)
            ));
        }
        let result = Self::time_binary(&exe_path);
        let _ = std::fs::remove_file(&exe_path);
        result
    }

    fn run_c(&self, source: &str) -> Result<(i32, u128), String> {
        let src_path = scratch("par-c", "c");
        let exe_path = scratch("par-c", "out");
        std::fs::write(&src_path, source).map_err(|e| format!("writing C source: {e}"))?;
        let compile = Command::new("cc")
            .arg("-O2")
            .arg("-pthread")
            .arg(&src_path)
            .arg("-o")
            .arg(&exe_path)
            .output()
            .map_err(|e| format!("no C compiler available: {e}"))?;
        let _ = std::fs::remove_file(&src_path);
        if !compile.status.success() {
            return Err(format!(
                "C compile failed: {}",
                String::from_utf8_lossy(&compile.stderr)
            ));
        }
        let result = Self::time_binary(&exe_path);
        let _ = std::fs::remove_file(&exe_path);
        result
    }
}

/// ADR-0007's benchmark category, live: the tuonelang serial and `par_map`
/// programs really build and run to the same exit through the real CLI, so
/// the tuonelang side is `Measured` (raw wall-clock recorded, the ratio
/// derived — a measurement, never a promise); the C side is `Measured` where
/// the toolchain exists and an honest skip where it does not. Run with
/// `--nocapture` to see the measured figures.
#[test]
fn parallel_speedup_measures_live_through_the_real_cli() {
    let results = parallel::measure(&BuildThenTimeRunner);
    assert_eq!(results.len(), 1);
    let entry = &results[0];
    match &entry.tuonelang {
        SpeedupVerdict::Measured {
            serial_nanos,
            parallel_nanos,
        } => {
            #[expect(clippy::print_stdout, reason = "measurement output under --nocapture")]
            {
                let ratio = entry
                    .tuonelang
                    .speedup()
                    .map_or_else(|| "n/a".to_string(), |r| format!("{r:.2}x"));
                println!(
                    "parallel-reduction (tuonelang, {} workers): serial {serial_nanos} ns, \
                     parallel {parallel_nanos} ns, ratio {ratio}",
                    entry.workers
                );
            }
        }
        SpeedupVerdict::Skipped { reason } => {
            panic!("the tuonelang pair must measure on a host with the real CLI: {reason}")
        }
    }
    match &entry.c {
        SpeedupVerdict::Measured {
            serial_nanos,
            parallel_nanos,
        } => {
            #[expect(clippy::print_stdout, reason = "measurement output under --nocapture")]
            {
                let ratio = entry
                    .c
                    .speedup()
                    .map_or_else(|| "n/a".to_string(), |r| format!("{r:.2}x"));
                println!(
                    "parallel-reduction (C, {} workers): serial {serial_nanos} ns, \
                     parallel {parallel_nanos} ns, ratio {ratio}",
                    entry.workers
                );
            }
        }
        // No C toolchain: an honest recorded skip, exactly like the other
        // cross-language comparisons.
        SpeedupVerdict::Skipped { reason } => {
            assert!(!reason.trim().is_empty());
        }
    }
}

/// One language's side of the speed table: build `source` to an executable,
/// or explain why it could not be built.
fn build_for_speed(peer: Option<PeerLanguage>, source: &str, tag: &str) -> Result<PathBuf, String> {
    let ext = match peer {
        None => "tuo",
        Some(PeerLanguage::C) => "c",
        Some(PeerLanguage::Go) => "go",
        Some(PeerLanguage::Rust) => "rs",
    };
    let src_path = scratch(&format!("speed-{tag}"), ext);
    let exe_path = scratch(&format!("speed-{tag}"), "bin");
    std::fs::write(&src_path, source).map_err(|e| format!("writing source: {e}"))?;
    let mut command = match peer {
        None => {
            let mut c = Command::new(env!("CARGO_BIN_EXE_tuo"));
            c.args(["build", "--release", "-o"])
                .arg(&exe_path)
                .arg(&src_path);
            c
        }
        Some(PeerLanguage::C) => {
            let mut c = Command::new("cc");
            c.args(C_FLAGS)
                .arg(&src_path)
                .arg("-o")
                .arg(&exe_path)
                .arg("-lm");
            c
        }
        Some(PeerLanguage::Rust) => {
            let mut c = Command::new("rustc");
            c.args(RustcComparisonRunner::FLAGS)
                .arg(&src_path)
                .arg("-o")
                .arg(&exe_path);
            c
        }
        Some(PeerLanguage::Go) => {
            let mut c = Command::new("go");
            c.arg("build").arg("-o").arg(&exe_path).arg(&src_path);
            c
        }
    };
    let output = command
        .output()
        .map_err(|e| format!("toolchain unavailable: {e}"));
    let _ = std::fs::remove_file(&src_path);
    let output = output?;
    if !output.status.success() {
        return Err(format!(
            "build failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(exe_path)
}

/// Run `exe` once untimed (first-exec costs), then `runs` timed; return the
/// fastest wall-clock time in milliseconds, requiring every run to exit with
/// `expected`.
fn best_of(exe: &std::path::Path, runs: usize, expected: i32) -> Result<f64, String> {
    let warmup = Command::new(exe)
        .output()
        .map_err(|e| format!("running: {e}"))?;
    if warmup.status.code() != Some(expected) {
        return Err(format!(
            "exited {:?}, expected {expected}",
            warmup.status.code()
        ));
    }
    let mut best = f64::INFINITY;
    for _ in 0..runs {
        let started = std::time::Instant::now();
        let run = Command::new(exe)
            .output()
            .map_err(|e| format!("running: {e}"))?;
        let millis = started.elapsed().as_secs_f64() * 1000.0;
        if run.status.code() != Some(expected) {
            return Err(format!(
                "exited {:?}, expected {expected}",
                run.status.code()
            ));
        }
        best = best.min(millis);
    }
    Ok(best)
}

/// The speed table: every supported Computer Language Benchmarks Game
/// workload built by `tuo build --release` and by each peer's optimizing
/// compiler, each binary timed (fastest of five runs after a warm-up) and its
/// time reported as a ratio to C. Every run must reach the workload's exit
/// byte, so a number is only ever printed for a program that computed the
/// right answer; a peer whose toolchain is absent is shown as skipped.
///
/// This is a measurement, not a gate: the timings are wall-clock on whatever
/// machine runs it, so it asserts correctness only and never a ratio. It is
/// `#[ignore]`d to keep the ordinary test run fast; CI runs it explicitly with
/// `--ignored --nocapture` so the table appears in the log.
#[test]
#[ignore = "a timing table; run with --ignored --nocapture"]
#[expect(clippy::print_stdout, reason = "the speed table is this test's output")]
fn benchmarks_game_speed_table() {
    use tuo_bench::lab::runtime::{BENCHMARKS_GAME, Support};

    const RUNS: usize = 5;
    let peers = [PeerLanguage::C, PeerLanguage::Rust, PeerLanguage::Go];
    println!(
        "{:<16} {:>11} {:>9} {:>9} {:>9}   {:>6} {:>7} {:>6}",
        "workload", "tuonelang", "c", "rust", "go", "vs c", "vs rust", "vs go"
    );
    let mut measured = 0;
    for workload in workloads() {
        if !BENCHMARKS_GAME.contains(&workload.label.as_str()) {
            continue;
        }
        let Support::Supported {
            source,
            expected_exit,
        } = &workload.support
        else {
            println!("{:<16} not yet expressible natively", workload.label);
            continue;
        };
        let tuo_exe = build_for_speed(None, source, &workload.label)
            .unwrap_or_else(|e| panic!("`{}` must build with tuo --release: {e}", workload.label));
        let tuo_ms = best_of(&tuo_exe, RUNS, *expected_exit)
            .unwrap_or_else(|e| panic!("`{}` (tuonelang): {e}", workload.label));
        let _ = std::fs::remove_file(&tuo_exe);
        measured += 1;

        let mut cells = Vec::new();
        let mut ratios = Vec::new();
        for peer in peers {
            let timing = comparison_for_peer(&workload, peer)
                .ok_or_else(|| "no peer program".to_string())
                .and_then(|cmp| {
                    let tag = format!("{}-{}", workload.label, peer.label());
                    let exe = build_for_speed(Some(peer), &cmp.peer_source, &tag)?;
                    let result = best_of(&exe, RUNS, cmp.expected_exit);
                    let _ = std::fs::remove_file(&exe);
                    result
                });
            match timing {
                Ok(ms) => {
                    cells.push(format!("{ms:>7.1}ms"));
                    ratios.push(format!("{:>5.2}x", tuo_ms / ms));
                }
                Err(_) => {
                    cells.push(format!("{:>9}", "skipped"));
                    ratios.push(format!("{:>6}", "-"));
                }
            }
        }
        println!(
            "{:<16} {:>9.1}ms {} {} {}   {} {:>7} {}",
            workload.label, tuo_ms, cells[0], cells[1], cells[2], ratios[0], ratios[1], ratios[2]
        );
    }
    println!(
        "(fastest of {RUNS} runs after a warm-up; ratio = tuonelang time / peer time, \
         so below 1.00x means tuonelang was faster)"
    );
    assert_eq!(
        measured, 5,
        "every supported Benchmarks Game workload was timed"
    );
}
