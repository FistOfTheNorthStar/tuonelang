//! The parallel-speedup benchmark category (ADR-0007).
//!
//! Concurrency is the one capability whose entire *point* is performance, so
//! ADR-0007 may not reach "accepted" without a benchmark category of its own:
//! **parallel speedup of a CPU-bound workload versus the serial baseline**,
//! with an equivalent-semantics C peer using the same thread count. This
//! module owns that category.
//!
//! One [`ParallelWorkload`] carries four committed programs (embedded via
//! `include_str!`, so the files on disk are the recorded sources): a
//! tuonelang serial baseline, its `std::rt::par_map` parallel sibling, and
//! the C serial/parallel (pthreads) peers — all computing the identical
//! reduction to the identical documented exit byte, differing only in thread
//! count. A [`SpeedupMeasurement`] is [`SpeedupVerdict::Measured`] **only
//! when both sides really compiled, ran, and produced the expected exit**;
//! otherwise it is `Skipped` with the reason — never a one-sided or
//! fabricated figure. The measured values are the raw wall-clock
//! nanoseconds; the speedup ratio is *derived at render time* from what was
//! measured, and the report carries no promise — the honesty rules the rest
//! of the lab already enforces (core count comes from the environment
//! capture, `Environment::logical_cpus`).
//!
//! Like every other lab measurement, the crate cannot build or time a binary
//! itself: the host injects a [`TimedRunner`] (the CLI wires in its real
//! build-then-time machinery, mirroring `runtime::NativeRunner`).

use serde::{Deserialize, Serialize};

/// The parallel-speedup workload catalog. One entry today: the CPU-bound
/// `parallel-reduction`, four chunks of range-bounded integer mixing.
#[must_use]
pub fn workloads() -> Vec<ParallelWorkload> {
    vec![ParallelWorkload {
        label: "parallel-reduction".to_string(),
        description: "a CPU-bound integer reduction (4 × 60000 seeds of bounded mixing), \
                      computed serially and via std::rt::par_map on 4 OS threads, with C \
                      serial/pthreads peers running the identical chunks"
            .to_string(),
        workers: 4,
        expected_exit: 64,
        serial_source: include_str!(
            "../../../../benchmarks/runtime/programs/parallel/parallel-reduction-serial.tuo"
        )
        .to_string(),
        parallel_source: include_str!(
            "../../../../benchmarks/runtime/programs/parallel/parallel-reduction.tuo"
        )
        .to_string(),
        c_serial_source: include_str!(
            "../../../../benchmarks/runtime/programs/parallel/parallel-reduction-serial.c"
        )
        .to_string(),
        c_parallel_source: include_str!(
            "../../../../benchmarks/runtime/programs/parallel/parallel-reduction.c"
        )
        .to_string(),
    }]
}

/// One parallel-speedup workload: the four committed programs and the
/// observable exit they must all agree on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParallelWorkload {
    /// A stable label (`parallel-reduction`).
    pub label: String,
    /// A one-line description of the reduction.
    pub description: String,
    /// The thread count the parallel programs use (and the C peer must match).
    pub workers: u32,
    /// The exit byte every one of the four programs must produce.
    pub expected_exit: i32,
    /// The tuonelang serial baseline.
    pub serial_source: String,
    /// The tuonelang `std::rt::par_map` parallel program.
    pub parallel_source: String,
    /// The C serial peer.
    pub c_serial_source: String,
    /// The C pthreads peer (same thread count).
    pub c_parallel_source: String,
}

/// A host seam that builds one program and times **its execution** (not its
/// compilation): the returned nanoseconds cover the produced binary's run
/// only. Implemented by the CLI (build via `tuo build` / `cc`, then time the
/// executable); this crate names no backend and no toolchain.
pub trait TimedRunner {
    /// Build the tuonelang `source` and run it, returning the exit status and
    /// the run's wall-clock nanoseconds.
    ///
    /// # Errors
    ///
    /// The reason the program could not be built or run.
    fn run_tuonelang(&self, source: &str) -> Result<(i32, u128), String>;

    /// Build the C `source` (linking pthreads) and run it, returning the exit
    /// status and the run's wall-clock nanoseconds. `Err` when no C toolchain
    /// is available — the measurement is then skipped, never faked.
    ///
    /// # Errors
    ///
    /// The reason the program could not be built or run.
    fn run_c(&self, source: &str) -> Result<(i32, u128), String>;
}

/// The verdict for one language's serial/parallel pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum SpeedupVerdict {
    /// Both the serial and parallel programs compiled, ran, and produced the
    /// expected exit; the raw wall-clock figures are recorded. The speedup
    /// ratio is derived at render time, never stored.
    Measured {
        /// The serial program's run, in nanoseconds.
        serial_nanos: u128,
        /// The parallel program's run, in nanoseconds.
        parallel_nanos: u128,
    },
    /// The pair could not be measured (missing toolchain, wrong exit, build
    /// failure); carries the exact reason and no number.
    Skipped {
        /// Why no figure is reported.
        reason: String,
    },
}

/// One workload's speedup measurement: the tuonelang pair and the C pair,
/// each measured or skipped independently and honestly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeedupMeasurement {
    /// The workload's label.
    pub label: String,
    /// The thread count the parallel side used.
    pub workers: u32,
    /// The exit byte all four programs must produce.
    pub expected_exit: i32,
    /// The tuonelang serial/parallel verdict.
    pub tuonelang: SpeedupVerdict,
    /// The C serial/parallel verdict.
    pub c: SpeedupVerdict,
}

impl SpeedupVerdict {
    /// The measured speedup ratio (serial / parallel), derived from the raw
    /// figures; `None` when skipped or when the parallel run measured zero
    /// nanoseconds (an impossible clock reading is not a ratio).
    #[must_use]
    pub fn speedup(&self) -> Option<f64> {
        match self {
            #[expect(
                clippy::cast_precision_loss,
                reason = "a display ratio, not arithmetic"
            )]
            Self::Measured {
                serial_nanos,
                parallel_nanos,
            } if *parallel_nanos > 0 => Some(*serial_nanos as f64 / *parallel_nanos as f64),
            _ => None,
        }
    }
}

/// Measure one side (a serial/parallel source pair) through `run`, demanding
/// the expected exit from both runs before any figure is recorded.
fn measure_pair(
    run: impl Fn(&str) -> Result<(i32, u128), String>,
    serial: &str,
    parallel: &str,
    expected_exit: i32,
) -> SpeedupVerdict {
    let serial_run = match run(serial) {
        Ok(outcome) => outcome,
        Err(reason) => {
            return SpeedupVerdict::Skipped {
                reason: format!("serial program: {reason}"),
            };
        }
    };
    let parallel_run = match run(parallel) {
        Ok(outcome) => outcome,
        Err(reason) => {
            return SpeedupVerdict::Skipped {
                reason: format!("parallel program: {reason}"),
            };
        }
    };
    if serial_run.0 != expected_exit || parallel_run.0 != expected_exit {
        return SpeedupVerdict::Skipped {
            reason: format!(
                "exit mismatch: serial {}, parallel {}, expected {} — no figure is \
                 reported for runs that do not agree",
                serial_run.0, parallel_run.0, expected_exit
            ),
        };
    }
    SpeedupVerdict::Measured {
        serial_nanos: serial_run.1,
        parallel_nanos: parallel_run.1,
    }
}

/// Measure every parallel workload through the host's `runner`. Each side
/// reaches [`SpeedupVerdict::Measured`] only when its serial **and** parallel
/// programs ran to the expected exit.
pub fn measure<R: TimedRunner>(runner: &R) -> Vec<SpeedupMeasurement> {
    workloads()
        .into_iter()
        .map(|workload| SpeedupMeasurement {
            tuonelang: measure_pair(
                |source| runner.run_tuonelang(source),
                &workload.serial_source,
                &workload.parallel_source,
                workload.expected_exit,
            ),
            c: measure_pair(
                |source| runner.run_c(source),
                &workload.c_serial_source,
                &workload.c_parallel_source,
                workload.expected_exit,
            ),
            label: workload.label,
            workers: workload.workers,
            expected_exit: workload.expected_exit,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{SpeedupVerdict, TimedRunner, measure, workloads};

    #[test]
    fn the_catalog_carries_one_reduction_with_all_four_programs() {
        let all = workloads();
        assert_eq!(all.len(), 1);
        let workload = &all[0];
        assert_eq!(workload.label, "parallel-reduction");
        assert_eq!(workload.workers, 4);
        assert_eq!(workload.expected_exit, 64);
        // The parallel tuonelang program really goes through the primitive,
        // and the C peer really uses the same thread count via pthreads.
        assert!(workload.parallel_source.contains("std::rt::par_map("));
        // The serial baseline never *calls* the primitive (its header prose
        // may name it).
        assert!(!workload.serial_source.contains("par_map("));
        assert!(workload.c_parallel_source.contains("pthread_create"));
        assert!(!workload.c_serial_source.contains("pthread"));
    }

    struct FakeRunner {
        tuonelang_exit: i32,
        c_fails: bool,
    }

    impl TimedRunner for FakeRunner {
        fn run_tuonelang(&self, _source: &str) -> Result<(i32, u128), String> {
            Ok((self.tuonelang_exit, 10))
        }

        fn run_c(&self, _source: &str) -> Result<(i32, u128), String> {
            if self.c_fails {
                Err("no C toolchain".to_string())
            } else {
                Ok((64, 5))
            }
        }
    }

    #[test]
    fn a_wrong_exit_or_missing_toolchain_is_skipped_never_measured() {
        // Wrong tuonelang exit: skipped with the mismatch named; C measured.
        let results = measure(&FakeRunner {
            tuonelang_exit: 3,
            c_fails: false,
        });
        assert_eq!(results.len(), 1);
        match &results[0].tuonelang {
            SpeedupVerdict::Skipped { reason } => {
                assert!(reason.contains("exit mismatch"), "got: {reason}");
            }
            SpeedupVerdict::Measured { .. } => panic!("a wrong exit must not measure"),
        }
        assert!(matches!(results[0].c, SpeedupVerdict::Measured { .. }));
        // Missing C toolchain: the C side is skipped with the reason.
        let results = measure(&FakeRunner {
            tuonelang_exit: 64,
            c_fails: true,
        });
        assert!(matches!(
            results[0].tuonelang,
            SpeedupVerdict::Measured { .. }
        ));
        assert!(matches!(results[0].c, SpeedupVerdict::Skipped { .. }));
    }

    #[test]
    fn the_speedup_ratio_is_derived_from_the_raw_figures() {
        let measured = SpeedupVerdict::Measured {
            serial_nanos: 100,
            parallel_nanos: 25,
        };
        let ratio = measured.speedup().expect("measured pairs have a ratio");
        assert!((ratio - 4.0).abs() < f64::EPSILON);
        let skipped = SpeedupVerdict::Skipped {
            reason: "example".to_string(),
        };
        assert_eq!(skipped.speedup(), None);
    }
}
