//! The runtime-benchmark suite: how a *compiled* tuonelang program performs —
//! and, just as importantly, an honest account of what the v0 language cannot
//! yet run at all.
//!
//! The prompt lists eight base runtime workloads: startup, integer computation,
//! allocation, collections, string processing, function calls, recursion, and
//! networking; ADR-0008 Tier 1 adds a ninth, **indirect-calls**, the
//! function-value sibling of function-calls. tuonelang v0 compiles and runs the
//! **scalar, control-flow core** — `Int` arithmetic, comparison, `if`/`else`,
//! function calls, and recursion — plus, since ADR-0004 Stage 2, the
//! **fixed-capacity array** `[T; N]` (inline, stack-allocated), which made the
//! collections workload measurable; since ADR-0006, the **borrowed `Str`**
//! (literals, equality, `std::str::{len, byte_at, slice}`), which makes the
//! string-processing workload measurable; since ADR-0009, the **allocator
//! core** — owned `String` and growable `Array[Int]` allocating and freeing real
//! heap memory natively — which makes the allocation workload measurable; and,
//! since ADR-0008 Tier 1, **first-class (non-capturing) function values** — a
//! bare `fn` name as a `Copy` code pointer, called indirectly with the identical
//! direct-call ABI — which makes the indirect-calls workload measurable. It
//! still has **no socket effect** (ADR-0006 landed descriptor I/O and process
//! exit only), so exactly one workload (networking) has no program to measure.
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
/// measure. When a feature lands (a heap, a socket effect), the corresponding
/// entry moves from [`Support::Unsupported`] to [`Support::Supported`] with a
/// program, and the lab measures it — no other change required. The
/// collections entry made exactly that move when ADR-0004 Stage 2 landed the
/// fixed-capacity array, the string-processing entry when ADR-0006 landed the
/// borrowed `Str` core, and the allocation entry when ADR-0009 landed the
/// allocator core (owned `String` + growable `Array[Int]`); the indirect-calls
/// entry was *added* supported when ADR-0008 Tier 1 landed first-class function
/// values.
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
            "indirect-calls",
            "a hot loop calling through a first-class function value (ADR-0008 \
             Tier 1) — the indirect-call sibling of function-calls, measured \
             against a C peer calling through a function pointer",
            include_str!("../../../../benchmarks/runtime/programs/tuo/indirect-calls.tuo"),
            // 2_000_000 indirect calls, each adding 1: acc = 2_000_000; observable
            // exit byte = 2_000_000 & 0xff = 128.
            128,
        ),
        RuntimeWorkload::supported(
            "recursion",
            "deep-ish recursion (naive Fibonacci, fib(20))",
            include_str!("../../../../benchmarks/runtime/programs/tuo/recursion.tuo"),
            // fib(20) = 6765; observable exit byte = 6765 & 0xff = 109.
            109,
        ),
        RuntimeWorkload::supported(
            "collections",
            "bulk construction, indexed lookups, and scans over the builtin \
             fixed-size array `[Int; N]` (ADR-0004 Stage 2), the v0 collection",
            include_str!("../../../../benchmarks/runtime/programs/tuo/collections.tuo"),
            // 200 rounds × (scan 31 + probes 10) = 8200; exit byte 8200 & 0xff = 8.
            8,
        ),
        RuntimeWorkload::supported(
            "string-processing",
            "byte-level tokenizing, scanning, and slice comparison over the borrowed \
             `Str` (ADR-0006): space/slash/digit counts and method/version slice checks \
             over a fixed request-log line",
            include_str!("../../../../benchmarks/runtime/programs/tuo/string-processing.tuo"),
            // 200 rounds × (4 spaces + 3 slashes + 11 digits + GET + HTTP/1.1) = 4000;
            // observable exit byte = 4000 & 0xff = 160.
            160,
        ),
        RuntimeWorkload::supported(
            "allocation",
            "heap allocation and deallocation throughput over the ADR-0009 allocator core: \
             a growable `Array[Int]` and an owned `String`, each built by repeated \
             push/append (doubling growth) and freed at scope end, over many rounds",
            include_str!("../../../../benchmarks/runtime/programs/tuo/allocation.tuo"),
            // 2000 rounds of round(16); each round's contribution (reassigned, not
            // accumulated) is array_sum(16) = 120 plus string_len(16) = 16, so main
            // returns 136; observable exit byte = 136.
            136,
        ),
        RuntimeWorkload::supported(
            "map-lookup",
            "keyed insert/lookup/churn throughput over the ADR-0011 hash map \
             `Map[Int, Int]`: 1000 inserts (doubling table growth), 1000 lookups \
             summed modulo 1000, and 500 removals per round, over many rounds",
            include_str!("../../../../benchmarks/runtime/programs/tuo/map-lookup.tuo"),
            // 50 rounds of round(1000); each round's contribution (reassigned, not
            // accumulated) is the lookup sum 3*999*1000/2 % 1000 = 500 plus the 500
            // surviving entries, so main returns 1000; exit byte 1000 & 0xff = 232.
            232,
        ),
        RuntimeWorkload::supported(
            "file-io",
            "effect-crossing throughput over the ADR-0013 OS boundary: per round, \
             open/write/close a 240-byte scratch file, reopen and read it back \
             byte-at-a-time through the read_byte seam, and remove it — the \
             deferred ADR-0006 effect-crossing benchmark, measured against a C \
             peer making the identical open/write/read/close/unlink calls",
            include_str!("../../../../benchmarks/runtime/programs/tuo/file-io.tuo"),
            // 200 rounds of round(15); each round's count (reassigned, not
            // accumulated) is 15 chunks × 16 bytes = 240; exit byte 240.
            240,
        ),
        // --- Unsupported: no program exists, and none is faked. ---
        RuntimeWorkload::unsupported(
            "networking",
            "a basic socket round-trip",
            "the effect seam covers descriptors, process exit, and — since \
             ADR-0013 — the clock, argv, and file open/close/remove; but no \
             socket-open effect primitive exists (no \
             socket/bind/listen/accept/connect), so no program can create a \
             connection (ADR-0006, amendment 2). Awaits a successor effect ADR.",
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
            "indirect-calls",
            "recursion",
            "map-lookup",
            "file-io",
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
    fn exactly_the_runnable_core_is_supported() {
        let supported: Vec<String> = workloads()
            .into_iter()
            .filter(|w| w.is_supported())
            .map(|w| w.label)
            .collect();
        // Precisely the four scalar-core workloads plus the fixed-array
        // collections workload (ADR-0004 Stage 2), the borrowed-`Str`
        // string-processing workload (ADR-0006), the allocator-core allocation
        // workload (ADR-0009), the function-value indirect-calls workload
        // (ADR-0008 Tier 1), the hash-map map-lookup workload (ADR-0011), and
        // the OS-boundary file-io workload (ADR-0013), and no more.
        assert_eq!(
            supported,
            vec![
                "startup".to_string(),
                "integer-computation".to_string(),
                "function-calls".to_string(),
                "indirect-calls".to_string(),
                "recursion".to_string(),
                "collections".to_string(),
                "string-processing".to_string(),
                "allocation".to_string(),
                "map-lookup".to_string(),
                "file-io".to_string(),
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
        assert_eq!(results.len(), 10, "only the ten supported workloads run");
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
