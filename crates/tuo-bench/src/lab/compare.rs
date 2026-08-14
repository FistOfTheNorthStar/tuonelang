//! Cross-language comparison — but only against a language with **equivalent
//! semantics**, and only when a real number can back the claim.
//!
//! The prompt is precise: *compare against relevant established languages only
//! with equivalent semantics*, and *never publish unsupported "blazing fast"
//! claims*. Two rules follow, and this module enforces both structurally.
//!
//! 1. **Equivalent semantics only.** A comparison workload pairs a tuonelang
//!    scalar-core program with a program in the other language that computes the
//!    **same result the same way** — the same integer arithmetic, the same
//!    recursion, the same byte scans. For the v0 core the natural peer is **C**:
//!    a language that, like tuonelang, compiles ahead-of-time to native code,
//!    traps on nothing here, and has a matching `int`/byte model for these
//!    programs — including the allocation workload, whose peer uses
//!    `malloc`/`realloc`/`free` with the same doubling growth. A workload whose
//!    tuonelang side is
//!    [`Unsupported`](super::runtime::Support::Unsupported) (networking) has
//!    **no comparison** — you cannot compare a feature that does not exist.
//!
//! 2. **No claim without both numbers.** A [`Comparison`] can only reach a
//!    [`Verdict::Measured`] when *both* sides actually compiled and ran under the
//!    recorded toolchains. If the comparison compiler is absent, the result is
//!    [`Verdict::Skipped`] with the reason — never a fabricated or one-sided
//!    figure, and never a superlative.
//!
//! Running the other language's compiler is a host seam ([`ComparisonRunner`]):
//! this crate does not shell out to `cc` itself (it names no toolchain and stays
//! offline-testable), so the host injects the ability, records the version and
//! the exact command, and this module decides — from whether a number came
//! back — what may honestly be said.

use serde::{Deserialize, Serialize};

use crate::lab::runtime::{RuntimeWorkload, Support};

/// A peer language a workload may be compared against. Only languages with
/// semantics equivalent to the v0 scalar core belong here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerLanguage {
    /// C, compiled ahead-of-time to native code. The apt peer for the scalar,
    /// control-flow core: matching integer arithmetic and recursion, native
    /// codegen, no runtime between the program and the CPU.
    C,
}

impl PeerLanguage {
    /// A stable label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::C => "c",
        }
    }
}

/// A workload paired with its equivalent-semantics peer program.
///
/// Only supported (scalar-core) tuonelang workloads produce a comparison — the
/// pairing function refuses to invent a peer for a feature tuonelang cannot run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComparisonWorkload {
    /// The workload label, shared with the tuonelang runtime workload.
    pub label: String,
    /// The peer language.
    pub peer: PeerLanguage,
    /// The peer-language source, computing the same result the same way.
    pub peer_source: String,
    /// The exact integer the peer program returns (must equal the tuonelang
    /// workload's expected exit for the comparison to be semantically equal).
    pub expected_exit: i32,
}

/// Build the equivalent-semantics comparison for one runtime workload, or `None`
/// when the workload is unsupported in tuonelang (nothing to compare against).
#[must_use]
pub fn comparison_for(workload: &RuntimeWorkload) -> Option<ComparisonWorkload> {
    // Only a supported workload can be compared; an unsupported one has no
    // tuonelang program, so any peer program would be comparing against nothing.
    let expected_exit = match &workload.support {
        Support::Supported { expected_exit, .. } => *expected_exit,
        Support::Unsupported { .. } => return None,
    };
    let peer_source = c_equivalent(&workload.label)?;
    Some(ComparisonWorkload {
        label: workload.label.clone(),
        peer: PeerLanguage::C,
        peer_source: peer_source.to_string(),
        expected_exit,
    })
}

/// The C program equivalent to a supported scalar-core workload, computing the
/// same value the same way (same arithmetic, same recursion). Returns `None` for
/// a label with no defined equivalent, so no comparison is fabricated.
fn c_equivalent(label: &str) -> Option<&'static str> {
    // Each peer program is the committed file under
    // `benchmarks/runtime/programs/c/`, embedded via `include_str!` so the
    // recorded C source lives on disk beside its tuonelang counterpart and the
    // two cannot drift out of sync.
    Some(match label {
        "startup" => include_str!("../../../../benchmarks/runtime/programs/c/startup.c"),
        "integer-computation" => {
            include_str!("../../../../benchmarks/runtime/programs/c/integer-computation.c")
        }
        "function-calls" => {
            include_str!("../../../../benchmarks/runtime/programs/c/function-calls.c")
        }
        "recursion" => include_str!("../../../../benchmarks/runtime/programs/c/recursion.c"),
        "collections" => include_str!("../../../../benchmarks/runtime/programs/c/collections.c"),
        "string-processing" => {
            include_str!("../../../../benchmarks/runtime/programs/c/string-processing.c")
        }
        "allocation" => include_str!("../../../../benchmarks/runtime/programs/c/allocation.c"),
        _ => return None,
    })
}

/// The outcome of trying to compile-and-run a peer program.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum Verdict {
    /// The peer program compiled and ran; here is its exit status and the
    /// toolchain that produced it.
    Measured {
        /// The exit status the peer program produced.
        exit_status: i32,
        /// The compiler version string that produced it (recorded verbatim).
        compiler_version: String,
        /// The exact command used to compile it.
        command: String,
    },
    /// The comparison could not be run (toolchain absent, compile failed, …).
    /// No number is claimed; the reason is recorded.
    Skipped {
        /// Why no comparison figure is reported.
        reason: String,
    },
}

/// A host seam that compiles and runs a peer-language program.
///
/// The host (the CLI, in a test) supplies a real toolchain and records its
/// version and command. This crate stays free of any C-toolchain dependency and
/// remains testable offline with a fake runner.
pub trait ComparisonRunner {
    /// The peer language this runner handles.
    fn language(&self) -> PeerLanguage;

    /// Compile and run `source`, returning the exit status plus the toolchain
    /// version and the exact command used.
    ///
    /// # Errors
    ///
    /// Returns a human-readable reason on any failure (toolchain absent, compile
    /// error, run error) — which the caller turns into a [`Verdict::Skipped`].
    fn compile_link_run(&self, source: &str) -> Result<PeerRun, String>;
}

/// A successful peer-language compile-and-run, with full provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerRun {
    /// The exit status the peer program produced.
    pub exit_status: i32,
    /// The peer compiler's version string.
    pub compiler_version: String,
    /// The exact command used to compile.
    pub command: String,
}

/// Attempt one comparison through `runner`, producing a [`Verdict`].
///
/// A [`Verdict::Measured`] is only returned when the peer program ran **and**
/// produced the semantically-required exit status — otherwise the semantics were
/// not actually equivalent and the comparison is [`Verdict::Skipped`] with the
/// discrepancy, never a misleading number.
#[must_use]
pub fn run_comparison<R: ComparisonRunner>(runner: &R, workload: &ComparisonWorkload) -> Verdict {
    if runner.language() != workload.peer {
        return Verdict::Skipped {
            reason: format!(
                "runner is for {:?}, workload peer is {:?}",
                runner.language(),
                workload.peer
            ),
        };
    }
    match runner.compile_link_run(&workload.peer_source) {
        Ok(run) if run.exit_status == workload.expected_exit => Verdict::Measured {
            exit_status: run.exit_status,
            compiler_version: run.compiler_version,
            command: run.command,
        },
        Ok(run) => Verdict::Skipped {
            reason: format!(
                "peer program exited {} but the equivalent tuonelang program returns {}; \
                 not semantically equal, so no comparison is reported",
                run.exit_status, workload.expected_exit
            ),
        },
        Err(reason) => Verdict::Skipped { reason },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lab::runtime::workloads;

    #[test]
    fn only_supported_workloads_get_a_comparison() {
        for workload in workloads() {
            let comparison = comparison_for(&workload);
            assert_eq!(
                comparison.is_some(),
                workload.is_supported(),
                "workload `{}`: comparison presence must track tuonelang support",
                workload.label
            );
        }
    }

    #[test]
    fn peer_expected_exit_matches_the_tuonelang_workload() {
        for workload in workloads() {
            if let (Some(cmp), Support::Supported { expected_exit, .. }) =
                (comparison_for(&workload), &workload.support)
            {
                assert_eq!(
                    cmp.expected_exit, *expected_exit,
                    "the C peer for `{}` must target the same result",
                    workload.label
                );
            }
        }
    }

    struct FakeC {
        status: i32,
    }

    impl ComparisonRunner for FakeC {
        fn language(&self) -> PeerLanguage {
            PeerLanguage::C
        }
        fn compile_link_run(&self, _source: &str) -> Result<PeerRun, String> {
            Ok(PeerRun {
                exit_status: self.status,
                compiler_version: "fake-cc 1.0".to_string(),
                command: "fake-cc -O2 peer.c -o peer".to_string(),
            })
        }
    }

    struct AbsentC;

    impl ComparisonRunner for AbsentC {
        fn language(&self) -> PeerLanguage {
            PeerLanguage::C
        }
        fn compile_link_run(&self, _source: &str) -> Result<PeerRun, String> {
            Err("no C compiler found on PATH".to_string())
        }
    }

    #[test]
    fn a_matching_peer_run_is_measured_with_provenance() {
        let workload = &workloads()[0]; // startup, expected 0
        let cmp = comparison_for(workload).expect("startup is supported");
        let verdict = run_comparison(&FakeC { status: 0 }, &cmp);
        match verdict {
            Verdict::Measured {
                compiler_version,
                command,
                ..
            } => {
                assert!(compiler_version.contains("fake-cc"));
                assert!(command.contains("peer"));
            }
            Verdict::Skipped { reason } => panic!("expected a measured verdict, got {reason}"),
        }
    }

    #[test]
    fn an_absent_toolchain_skips_rather_than_fabricates() {
        let cmp = comparison_for(&workloads()[0]).expect("supported");
        let verdict = run_comparison(&AbsentC, &cmp);
        assert!(matches!(verdict, Verdict::Skipped { .. }));
    }

    #[test]
    fn a_semantically_unequal_peer_is_not_reported_as_a_number() {
        // startup expects 0; a peer that exits 5 is not equivalent -> skipped.
        let cmp = comparison_for(&workloads()[0]).expect("supported");
        let verdict = run_comparison(&FakeC { status: 5 }, &cmp);
        match verdict {
            Verdict::Skipped { reason } => assert!(reason.contains("not semantically equal")),
            Verdict::Measured { .. } => panic!("an unequal peer must not be reported as measured"),
        }
    }
}
