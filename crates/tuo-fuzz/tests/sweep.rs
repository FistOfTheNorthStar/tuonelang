//! The stable, deterministic robustness sweep — the CI half of the whole-
//! compiler fuzzing harness.
//!
//! This is the stable-toolchain companion to the coverage-guided `cargo fuzz`
//! targets under each stage's `fuzz/` directory. It drives the *same* per-stage
//! invariant checkers ([`tuo_fuzz::stages`]) over the fixed-seed corpus
//! ([`tuo_fuzz::corpus`]), so every invariant the fuzzers enforce is also
//! enforced on every `cargo test` — reproducibly, without a nightly toolchain.
//!
//! A failure here reproduces exactly from its printed `(flavor, seed)`: the
//! generator is a pure function of the seed. The final test replays every
//! committed regression fixture, so a previously discovered-and-fixed crash can
//! never silently return.

use tuo_fuzz::corpus::{Flavor, input};
use tuo_fuzz::regression;
use tuo_fuzz::stages::all_checkers;

/// How many seeds each (flavor, stage) pair sweeps. Chosen so the whole matrix
/// runs in well under a second in debug while covering thousands of inputs.
const SEEDS_PER_STAGE: u64 = 400;

/// Every stage checker upholds its invariants over every corpus flavor. This is
/// the core gate: `arbitrary source input must not crash the compiler`, plus
/// each stage's structural contract, over the full generated population.
#[test]
fn every_stage_survives_the_corpus() {
    for (stage, check) in all_checkers() {
        for flavor in Flavor::all() {
            for seed in 0..SEEDS_PER_STAGE {
                // Each checker is total: it either upholds its invariants or
                // panics (a finding). The panic message already carries the
                // offending input; we add the coordinates for reproduction.
                let text = input(flavor, seed);
                let hook_stage = stage;
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(&text)))
                    .unwrap_or_else(|_| {
                        panic!(
                            "stage `{hook_stage}` crashed on flavor {flavor:?} seed {seed}: {text:?}"
                        )
                    });
            }
        }
    }
}

/// A handful of pathological inputs — deeply nested, very wide, and adversarial
/// literals — must terminate and uphold every invariant. These complement the
/// random corpus with the shapes a PRNG rarely produces but a real bug hides in.
#[test]
fn pathological_inputs_terminate_and_uphold_invariants() {
    let pathological = [
        String::new(),
        "(".repeat(10_000),
        "{".repeat(10_000),
        "fn ".repeat(5_000),
        format!("fn f() {{ {} 0 }}", "x + ".repeat(5_000)),
        "\"".repeat(2_000),
        "🦀".repeat(2_000),
        "\0".repeat(1_000),
        "spec s { then ".repeat(1_000),
        format!("fn main() -> Int {{ {} }}", "1 + ".repeat(3_000) + "1"),
    ];
    for (stage, check) in all_checkers() {
        for text in &pathological {
            let hook_stage = stage;
            let snippet: String = text.chars().take(40).collect();
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(text))).unwrap_or_else(
                |_| panic!("stage `{hook_stage}` crashed on pathological input {snippet:?}…"),
            );
        }
    }
}

/// Every committed regression fixture replays clean: no fixed bug has come back.
///
/// This is the read side of the automatic-fixture mechanism. When a sweep or a
/// `cargo fuzz` run discovers a new crash, its input is recorded under
/// `regressions/<stage>/` (see [`regression::record`]); committing that file
/// makes this test replay it forever. A newly recorded but not-yet-fixed
/// fixture will fail here by design.
#[test]
#[expect(
    clippy::print_stdout,
    reason = "a measurement test: report the replayed-fixture count under --nocapture"
)]
fn committed_regressions_stay_fixed() {
    let replayed = regression::replay_all();
    // The corpus may legitimately be empty on a clean toolchain (no bug ever
    // found), so we only assert the replay itself did not panic. The count is
    // reported for visibility.
    println!("replayed {replayed} committed regression fixture(s)");
}
