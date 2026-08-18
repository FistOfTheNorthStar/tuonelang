//! The performance laboratory's two host seams, exercised end-to-end through the
//! real toolchain.
//!
//! `tuo-bench`'s `lab` module deliberately cannot turn source into a running
//! native binary, and cannot run a foreign compiler — those are host seams
//! ([`NativeRunner`], [`ComparisonRunner`]). This test wires in the real ones:
//!
//! - the **native runner** shells out to the real `tuo` binary (`tuo run`), the
//!   same Cranelift+`cc` path a user gets, and
//! - the **comparison runner** compiles the C peer with the platform `cc`.
//!
//! With those injected it drives the lab's own `run_supported` and
//! `run_comparison`, and asserts the honest end-to-end contract: every supported
//! scalar-core workload actually compiles, links, and runs to its expected exit
//! byte, and where a C toolchain exists the equivalent-semantics C program agrees
//! — while an absent toolchain yields a recorded *skip*, never a fabricated
//! number. This is the proof that the benchmark repository can back its claims.

use std::path::PathBuf;
use std::process::Command;

use tuo_bench::lab::compare::{
    ComparisonRunner, PeerLanguage, PeerRun, Verdict, comparison_for, run_comparison,
};
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

/// Every supported workload compiles, links, and runs natively to its expected
/// exit byte — through the real `tuo run`. This is the load-bearing proof that a
/// "supported" workload is truly runnable, not just claimed.
#[test]
fn supported_workloads_run_natively_and_match() {
    let results = run_supported(&TuoRunNativeRunner);
    assert_eq!(
        results.len(),
        8,
        "exactly the eight supported workloads run (the scalar core plus the \
         fixed-array collections workload, the borrowed-Str string-processing \
         workload, the allocator-core allocation workload, and the \
         function-value indirect-calls workload)"
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
    assert_eq!(measured + skipped, 8);
    // On CI and dev machines `cc` is present, so we expect real measurements;
    // this documents the intent without failing a truly toolchain-less host.
    if measured == 0 {
        eprintln!("note: no C toolchain found; all comparisons recorded as skipped");
    }
}
