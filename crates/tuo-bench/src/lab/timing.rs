//! Wall-clock timing: how a benchmark takes samples and reduces them to a
//! reported figure.
//!
//! The prompt's rule — *never publish unsupported claims* — starts with honest
//! numbers. This module measures, it does not promise: a [`Timing`] reports the
//! **minimum** and **median** of the samples it actually took, plus the raw
//! sample count and per-iteration batching, and nothing about "typical" or
//! "guaranteed" latency. The minimum is the least-noisy estimate of the work's
//! own cost (the machine can only add overhead to a sample, never remove it);
//! the median resists outliers. Both are reported so a reader draws their own
//! conclusion.
//!
//! The clock is a seam ([`Clock`]) so the reduction logic is testable with a
//! deterministic fake and so no wall-clock reading ever leaks into a committed
//! result file's *shape*. Real runs use [`SystemClock`].

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// A monotonic clock. Injected so timing reduction is deterministic in tests.
pub trait Clock {
    /// Nanoseconds elapsed since some fixed, monotonic origin.
    ///
    /// Only *differences* are meaningful; the origin is unspecified. Must be
    /// monotonic non-decreasing within one process.
    fn now_nanos(&self) -> u128;
}

/// The real monotonic clock, backed by [`std::time::Instant`].
#[derive(Debug, Clone)]
pub struct SystemClock {
    origin: Instant,
}

impl SystemClock {
    /// A clock whose origin is the moment of construction.
    #[must_use]
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now_nanos(&self) -> u128 {
        self.origin.elapsed().as_nanos()
    }
}

/// The reduced result of timing one benchmark: order statistics over the raw
/// per-iteration durations, in nanoseconds.
///
/// A `Timing` is a *measurement*, never a guarantee. It records what was
/// observed (`samples` runs, each of `iterations_per_sample` inner repetitions)
/// and the reduction (min/median). Consumers must not read latency promises into
/// it; the human renderer states this explicitly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timing {
    /// How many independent samples were taken.
    pub samples: u32,
    /// How many times the benchmarked operation ran inside each sample (batching
    /// amortizes clock resolution for very fast operations). The per-operation
    /// figures below already divide by this.
    pub iterations_per_sample: u32,
    /// The fastest observed per-operation time, in nanoseconds. The least-noisy
    /// estimate of the operation's own cost.
    pub min_nanos: u64,
    /// The median observed per-operation time, in nanoseconds. Resists outliers.
    pub median_nanos: u64,
}

impl Timing {
    /// Reduce a set of raw per-sample durations (each already the total for
    /// `iterations_per_sample` operations) into order statistics.
    ///
    /// Returns `None` if `samples` is empty (there is nothing honest to report).
    #[must_use]
    pub fn from_samples(raw: &[Duration], iterations_per_sample: u32) -> Option<Self> {
        if raw.is_empty() || iterations_per_sample == 0 {
            return None;
        }
        let iters = u128::from(iterations_per_sample);
        // Per-operation nanoseconds for each sample.
        let mut per_op: Vec<u64> = raw
            .iter()
            .map(|d| (d.as_nanos() / iters).min(u128::from(u64::MAX)) as u64)
            .collect();
        per_op.sort_unstable();
        let min = per_op[0];
        let median = per_op[per_op.len() / 2];
        Some(Self {
            samples: raw.len() as u32,
            iterations_per_sample,
            min_nanos: min,
            median_nanos: median,
        })
    }

    /// The minimum per-operation time as a fractional microsecond, for display.
    #[must_use]
    pub fn min_micros(&self) -> f64 {
        self.min_nanos as f64 / 1000.0
    }

    /// The median per-operation time as a fractional microsecond, for display.
    #[must_use]
    pub fn median_micros(&self) -> f64 {
        self.median_nanos as f64 / 1000.0
    }
}

/// Time `op` under `clock`: run it `iterations_per_sample` times per sample,
/// take `samples` samples, and reduce to a [`Timing`].
///
/// `op` is run for its side effect of doing work; its return value is dropped
/// (callers should return a value derived from the work so the optimizer cannot
/// elide it — see [`std::hint::black_box`]). Returns `None` only if asked for
/// zero samples or zero iterations.
#[must_use]
pub fn time<C: Clock, T>(
    clock: &C,
    samples: u32,
    iterations_per_sample: u32,
    mut op: impl FnMut() -> T,
) -> Option<Timing> {
    if samples == 0 || iterations_per_sample == 0 {
        return None;
    }
    let mut raw = Vec::with_capacity(samples as usize);
    for _ in 0..samples {
        let start = clock.now_nanos();
        for _ in 0..iterations_per_sample {
            std::hint::black_box(op());
        }
        let end = clock.now_nanos();
        raw.push(Duration::from_nanos(
            (end - start).min(u128::from(u64::MAX)) as u64,
        ));
    }
    Timing::from_samples(&raw, iterations_per_sample)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// A deterministic clock that advances by a fixed step each reading.
    struct FakeClock {
        step: u128,
        now: Cell<u128>,
    }

    impl FakeClock {
        fn new(step: u128) -> Self {
            Self {
                step,
                now: Cell::new(0),
            }
        }
    }

    impl Clock for FakeClock {
        fn now_nanos(&self) -> u128 {
            let value = self.now.get();
            self.now.set(value + self.step);
            value
        }
    }

    #[test]
    fn from_samples_reports_min_and_median() {
        let raw = [
            Duration::from_nanos(300),
            Duration::from_nanos(100),
            Duration::from_nanos(200),
        ];
        let timing = Timing::from_samples(&raw, 1).expect("non-empty");
        assert_eq!(timing.samples, 3);
        assert_eq!(timing.min_nanos, 100);
        assert_eq!(timing.median_nanos, 200);
    }

    #[test]
    fn from_samples_divides_by_iterations() {
        // One sample of 1000ns covering 10 operations => 100ns/op.
        let raw = [Duration::from_nanos(1000)];
        let timing = Timing::from_samples(&raw, 10).expect("non-empty");
        assert_eq!(timing.min_nanos, 100);
        assert_eq!(timing.iterations_per_sample, 10);
    }

    #[test]
    fn empty_or_zero_is_none_not_a_fabricated_zero() {
        assert!(Timing::from_samples(&[], 1).is_none());
        assert!(Timing::from_samples(&[Duration::from_nanos(1)], 0).is_none());
    }

    #[test]
    fn time_reduces_under_a_deterministic_clock() {
        // Each now_nanos() advances by 50ns; a sample brackets two readings, so
        // each sample measures exactly 50ns of "work".
        let clock = FakeClock::new(50);
        let timing = time(&clock, 4, 1, || 1 + 1).expect("non-empty");
        assert_eq!(timing.samples, 4);
        assert_eq!(timing.min_nanos, 50);
        assert_eq!(timing.median_nanos, 50);
    }

    #[test]
    fn micros_conversion() {
        let timing = Timing::from_samples(&[Duration::from_nanos(1500)], 1).expect("non-empty");
        assert!((timing.min_micros() - 1.5).abs() < f64::EPSILON);
    }
}
