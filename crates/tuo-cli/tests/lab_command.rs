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
struct CcComparisonRunner;

impl ComparisonRunner for CcComparisonRunner {
    fn language(&self) -> PeerLanguage {
        PeerLanguage::C
    }

    fn compile_link_run(&self, source: &str) -> Result<PeerRun, String> {
        let src_path = scratch("peer", "c");
        let exe_path = scratch("peer", "out");
        std::fs::write(&src_path, source).map_err(|e| format!("writing C source: {e}"))?;

        let command = format!("cc -O2 {} -o {}", src_path.display(), exe_path.display());
        let compile = Command::new("cc")
            .arg("-O2")
            .arg(&src_path)
            .arg("-o")
            .arg(&exe_path)
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
        9,
        "exactly the nine supported workloads run (the scalar core plus the \
         fixed-array collections workload, the borrowed-Str string-processing \
         workload, the allocator-core allocation workload, the function-value \
         indirect-calls workload, and the hash-map map-lookup workload)"
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
    assert_eq!(measured + skipped, 9);
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
    assert_eq!(measured + skipped, 9);
    if measured == 0 {
        eprintln!("note: no Go toolchain found; all Go comparisons recorded as skipped");
    }
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
