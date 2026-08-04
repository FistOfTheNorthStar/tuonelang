//! The runtime-benchmark suite: how a *compiled* tuonelang program performs —
//! and, just as importantly, an honest account of what the v0 language cannot
//! yet run at all.
//!
//! The prompt lists eight runtime workloads: startup, integer computation,
//! allocation, collections, string processing, function calls, recursion, and
//! networking. tuonelang v0 compiles and runs only the **scalar, control-flow
//! core** — `Int` arithmetic, comparison, `if`/`else`, function calls, and
//! recursion (see the codegen and stdlib conventions). It has **no heap, no
//! collections, no string values, and no effect boundary** yet. Four of the
//! eight workloads therefore have no program to measure.
//!
//! The prompt's final rule governs this directly: *never publish unsupported
//! claims; make the repository capable of proving them.* So every workload is a
//! [`RuntimeWorkload`] with an explicit [`Support`]:
//!
//! - a **supported** workload carries a real scalar-core program and is measured
//!   by a host-injected [`NativeRunner`] (this crate cannot link or run a binary
//!   on its own — the CLI wires in the `cc`-backed builder, mirroring how the
//!   corpus injects its `NativeExecutor`);
//! - an **unsupported** workload carries the *exact reason* the v0 core cannot
//!   express it and **emits no number**. It is a standing, machine-readable
//!   record of a capability the lab will measure the moment the feature lands —
//!   not a fabricated figure and not silence.

use serde::{Deserialize, Serialize};

/// Whether a runtime workload can be measured on the current language, and if
/// not, precisely why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "support", rename_all = "snake_case")]
pub enum Support {
    /// The workload is expressible in the v0 scalar core; a real program exists
    /// to run.
    Supported {
        /// The tuonelang source that exercises the workload.
        source: String,
        /// The **observable process exit status** the program produces — i.e.
        /// the integer `main` returns truncated to a byte (`& 0xff`), exactly as
        /// the v0 entry ABI and a C `main` return both surface it. Using the
        /// observable byte (not the mathematical result) is what lets a native
        /// tuonelang run and an equivalent C run be compared like-for-like.
        expected_exit: i32,
    },
    /// The workload cannot be expressed in the v0 core. Carries the exact reason;
    /// no measurement is (or may be) reported.
    Unsupported {
        /// Why the v0 language cannot run this workload yet.
        reason: String,
    },
}

/// One runtime workload the lab tracks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeWorkload {
    /// A stable label (e.g. `integer-computation`).
    pub label: String,
    /// A one-line description of what the workload exercises.
    pub description: String,
    /// Whether it is measurable on v0, with the source or the reason.
    pub support: Support,
}

impl RuntimeWorkload {
    /// A supported scalar-core workload.
    fn supported(label: &str, description: &str, source: &str, expected_exit: i32) -> Self {
        Self {
            label: label.to_string(),
            description: description.to_string(),
            support: Support::Supported {
                source: source.to_string(),
                expected_exit,
            },
        }
    }

    /// A workload the v0 core cannot express, with the exact reason.
    fn unsupported(label: &str, description: &str, reason: &str) -> Self {
        Self {
            label: label.to_string(),
            description: description.to_string(),
            support: Support::Unsupported {
                reason: reason.to_string(),
            },
        }
    }

    /// Is this workload measurable on the current language?
    #[must_use]
    pub fn is_supported(&self) -> bool {
        matches!(self.support, Support::Supported { .. })
    }
}

/// The complete, honest v0 runtime-workload catalog — every workload the prompt
/// names, each tagged supported (with a real program) or unsupported (with the
/// exact reason it cannot run yet).
///
/// This is the single source of truth for what the runtime lab can and cannot
/// measure. When a feature lands (a heap, collections, strings, an effect
/// boundary), the corresponding entry moves from [`Support::Unsupported`] to
/// [`Support::Supported`] with a program, and the lab measures it — no other
/// change required.
#[must_use]
pub fn workloads() -> Vec<RuntimeWorkload> {
    vec![
        // --- Supported: the scalar, control-flow core. ---
        //
        // Each supported workload's source is the committed file under
        // `benchmarks/runtime/programs/tuo/`, embedded via `include_str!` — so
        // the file on disk *is* the recorded source (the prompt's "record the
        // source code"), and there is one measurement of record with no drift.
        RuntimeWorkload::supported(
            "startup",
            "process startup and exit of a trivial program",
            include_str!("../../../../benchmarks/runtime/programs/tuo/startup.tuo"),
            0,
        ),
        RuntimeWorkload::supported(
            "integer-computation",
            "a tight integer arithmetic reduction (sum 1..=1000 by recursion)",
            include_str!("../../../../benchmarks/runtime/programs/tuo/integer-computation.tuo"),
            // sum(1..=1000) = 500500; observable exit byte = 500500 & 0xff = 20.
            20,
        ),
        RuntimeWorkload::supported(
            "function-calls",
            "many non-recursive function calls",
            include_str!("../../../../benchmarks/runtime/programs/tuo/function-calls.tuo"),
            30,
        ),
        RuntimeWorkload::supported(
            "recursion",
            "deep-ish recursion (naive Fibonacci, fib(20))",
            include_str!("../../../../benchmarks/runtime/programs/tuo/recursion.tuo"),
            // fib(20) = 6765; observable exit byte = 6765 & 0xff = 109.
            109,
        ),
        // --- Unsupported: no program exists, and none is faked. ---
        RuntimeWorkload::unsupported(
            "allocation",
            "heap allocation and deallocation throughput",
            "v0 has no heap-allocating types (Box/Shared/String/Array) lowered to native \
             code: the runtime allocator seam exists in the ABI spec but no allocating \
             construct is compiled yet. No program can exercise allocation.",
        ),
        RuntimeWorkload::unsupported(
            "collections",
            "insert/lookup over a standard collection",
            "v0 lowers no collection type. std::collections is a documented catalog of \
             free-function contracts, not executable collection code. No program can \
             exercise collections.",
        ),
        RuntimeWorkload::unsupported(
            "string-processing",
            "building and scanning string data",
            "v0 has no runtime String value: string literals exist in the grammar but \
             no String operations are lowered to native code. No program can exercise \
             string processing.",
        ),
        RuntimeWorkload::unsupported(
            "networking",
            "a basic socket round-trip",
            "v0 has no effect boundary (no FFI/syscalls); std::io/std::process are \
             contract-tier only. No program can perform I/O, so networking cannot be \
             measured.",
        ),
    ]
}

/// The outcome of running one supported workload natively.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeOutcome {
    /// The process exit status the built binary produced.
    pub exit_status: i32,
    /// Whether the exit status matched the workload's expected value. A run that
    /// disagrees is a correctness failure, not a performance result.
    pub matched_expected: bool,
}

/// A host-injected native compile-link-run seam.
///
/// `tuo-bench` names no concrete backend and cannot invoke `cc`, so the ability
/// to turn a source program into a running binary is supplied by the host (the
/// CLI wires in its Cranelift-backed `native_run`). This mirrors the corpus
/// pipeline's `NativeExecutor` and the codegen benchmark's `ModelAdapter`: the
/// crate owns the *what*, the host owns the *how*.
pub trait NativeRunner {
    /// Compile, link, and run `source` (a whole program whose entry is `main`),
    /// returning its process exit status, or an error string on any failure.
    ///
    /// # Errors
    ///
    /// Returns a human-readable reason if compilation, linking, or execution
    /// failed. A returned `Ok(status)` means the program actually ran.
    fn compile_link_run(&self, source: &str) -> Result<i32, String>;
}

/// Run every **supported** workload through `runner` and report each outcome.
///
/// Unsupported workloads are skipped here by construction — there is no source to
/// run — and remain visible only in the [`workloads`] catalog with their reason.
/// A workload whose native exit disagrees with its expected value is reported
/// with `matched_expected == false`, surfacing a correctness problem rather than
/// hiding it behind a timing number.
#[must_use]
pub fn run_supported<R: NativeRunner>(runner: &R) -> Vec<(String, Result<NativeOutcome, String>)> {
    workloads()
        .into_iter()
        .filter_map(|workload| match workload.support {
            Support::Supported {
                source,
                expected_exit,
            } => {
                let outcome = runner
                    .compile_link_run(&source)
                    .map(|exit_status| NativeOutcome {
                        exit_status,
                        matched_expected: exit_status == expected_exit,
                    });
                Some((workload.label, outcome))
            }
            Support::Unsupported { .. } => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_prompt_workload_is_present() {
        let all = workloads();
        let labels: Vec<&str> = all.iter().map(|w| w.label.as_str()).collect();
        for required in [
            "startup",
            "integer-computation",
            "allocation",
            "collections",
            "string-processing",
            "function-calls",
            "recursion",
            "networking",
        ] {
            assert!(labels.contains(&required), "missing workload `{required}`");
        }
    }

    #[test]
    fn unsupported_workloads_carry_a_reason_and_no_source() {
        for workload in workloads() {
            if let Support::Unsupported { reason } = &workload.support {
                assert!(
                    !reason.trim().is_empty(),
                    "unsupported workload `{}` must explain itself",
                    workload.label
                );
            }
        }
    }

    #[test]
    fn exactly_the_scalar_core_is_supported() {
        let supported: Vec<String> = workloads()
            .into_iter()
            .filter(|w| w.is_supported())
            .map(|w| w.label)
            .collect();
        // Precisely the four scalar-core workloads, and no more.
        assert_eq!(
            supported,
            vec![
                "startup".to_string(),
                "integer-computation".to_string(),
                "function-calls".to_string(),
                "recursion".to_string(),
            ]
        );
    }

    /// A fake runner that "runs" a program by returning a fixed status, letting
    /// us exercise `run_supported` offline without a real backend.
    struct FakeRunner {
        status: i32,
    }

    impl NativeRunner for FakeRunner {
        fn compile_link_run(&self, _source: &str) -> Result<i32, String> {
            Ok(self.status)
        }
    }

    #[test]
    fn run_supported_only_runs_supported_workloads() {
        // Return the startup workload's expected value; only startup will match.
        let results = run_supported(&FakeRunner { status: 0 });
        assert_eq!(results.len(), 4, "only the four supported workloads run");
        let startup = results
            .iter()
            .find(|(label, _)| label == "startup")
            .expect("startup ran");
        let outcome = startup.1.as_ref().expect("ran");
        assert!(outcome.matched_expected);
    }

    #[test]
    fn a_wrong_exit_is_reported_as_a_mismatch_not_a_number() {
        // A runner returning the wrong status flags the correctness failure.
        let results = run_supported(&FakeRunner { status: 42 });
        let recursion = results
            .iter()
            .find(|(label, _)| label == "recursion")
            .expect("recursion ran");
        assert!(!recursion.1.as_ref().expect("ran").matched_expected);
    }
}
