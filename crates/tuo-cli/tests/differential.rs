//! Cross-engine semantic differential suite — a **mandatory CI gate**.
//!
//! The reference meaning of a tuonelang program is what the MIR interpreter
//! computes; the native Cranelift backend must agree with it on every observable
//! behavior. This suite is the machine that enforces that agreement across a
//! whole population of programs rather than a handful of hand-written fixtures:
//!
//! 1. it **generates** small, well-typed programs inside the backend's scalar
//!    subset ([`generator`]) from a fixed, deterministic range of seeds;
//! 2. it runs each through **both engines** and compares the full observable
//!    outcome — return value and deterministic traps ([`harness`]); and
//! 3. on any disagreement it **minimizes** the program to a small reproduction
//!    and saves it to disk ([`shrink`]), then fails the test with the reduced
//!    program inline.
//!
//! **Any disagreement is a compiler-correctness failure**, full stop — the
//! backend is wrong wherever it parts from the interpreter. Because these are
//! ordinary `#[test]`s, `cargo test --workspace` runs them, and CI runs
//! `cargo test --workspace` with `-D warnings`; so this suite is a gate that
//! blocks a merge the moment the backend and interpreter diverge. See
//! `tests/differential/README.md` for the charter and the repro workflow.
//!
//! The two behaviors the differential charter names but the language cannot yet
//! express — **stdout** and **observable destruction** — are handled honestly:
//! [`harness::Outcome`] reserves a slot for stdout so the comparison extends the
//! day an I/O facility exists, and no drop-observable program is generated
//! because the backend lowers no type with a destructor. The suite never claims
//! coverage the compiler cannot back.

#[path = "differential/harness.rs"]
mod harness;

#[path = "differential/generator.rs"]
mod generator;

#[path = "differential/shrink.rs"]
mod shrink;

use std::path::PathBuf;

use harness::diff_program;

/// A per-test scratch directory under the target dir (never `/tmp` hard-coded),
/// isolated by name so parallel tests don't collide.
fn scratch_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("differential-{name}"));
    std::fs::create_dir_all(&dir).expect("scratch dir is creatable");
    dir
}

/// The fixed seed range the native sweep covers. Deterministic and
/// reproducible: the same seeds run on every host and every CI job, so a failure
/// always reproduces from its seed. Each seed is a native compile+link+run, so
/// the range is sized to give broad coverage while keeping the gate's wall-clock
/// reasonable; the interpreter-only corpus test below sweeps far wider cheaply.
const SEED_RANGE: std::ops::Range<u64> = 0..150;

/// The (much wider) seed range the interpreter-only corpus checks sweep — no
/// native build per program, so it is cheap to cover a large population.
const CORPUS_RANGE: std::ops::Range<u64> = 0..1000;

/// The core gate: generate a population of well-typed programs and require the
/// native backend to agree with the reference interpreter on every one. On the
/// first divergence, minimize it, save the repro, and fail with the reduced
/// program inline.
#[test]
fn generated_programs_agree_across_engines() {
    let dir = scratch_dir("sweep");
    let scratch = dir.join("candidate.tuo");

    for seed in SEED_RANGE {
        let source = generator::program(seed);

        // The generator's contract: every program it emits is accepted and inside
        // the backend subset. Assert it here so a generator regression surfaces
        // as a clear generator bug, not a confusing "native did not run".
        assert!(
            harness::accepts(&source),
            "generator emitted a program the front end rejects (seed {seed}):\n{source}"
        );

        if let Some(divergence) = diff_program(&source, &scratch) {
            let minimized = shrink::minimize(&divergence, &scratch);
            let repro =
                shrink::save_repro(&minimized, &scratch_dir("repro"), &format!("seed-{seed}"));
            panic!(
                "differential failure on seed {seed}. Minimized reproduction saved to {}.\n\n{}",
                repro.display(),
                minimized.report(),
            );
        }
    }
}

/// The generator is a pure function of its seed — same seed, same program — so a
/// reported failure reproduces exactly. This pins that determinism directly.
#[test]
fn the_generator_is_deterministic() {
    for seed in [0, 1, 7, 42, 399] {
        assert_eq!(
            generator::program(seed),
            generator::program(seed),
            "seed {seed} must generate an identical program every time"
        );
    }
}

/// Every program in the corpus must be accepted by the front end, and the corpus
/// must actually exercise *both* observable outcomes — normal returns and
/// deterministic traps — or the trap half of the differential comparison would
/// never be tested by the random path. This runs the interpreter only (no native
/// build), so it sweeps a wide seed range cheaply. The native sweep above then
/// confirms the backend agrees on this same population.
#[test]
fn the_corpus_is_accepted_and_exercises_both_returns_and_traps() {
    let mut returns = 0u32;
    let mut traps = 0u32;
    for seed in CORPUS_RANGE {
        let source = generator::program(seed);
        assert!(
            harness::accepts(&source),
            "seed {seed} produced a non-accepted program:\n{source}"
        );
        match harness::interpret(&source) {
            harness::Outcome::Returned { .. } => returns += 1,
            harness::Outcome::Trapped { .. } => traps += 1,
        }
    }
    assert!(
        returns > 0,
        "the corpus should contain programs that return"
    );
    assert!(
        traps > 0,
        "the corpus should contain programs that trap (arithmetic overflow / \
         division by zero); otherwise the trap comparison is never exercised"
    );
}

/// The minimizer, given a synthetic divergence, must reduce it and produce a
/// saved, still-valid repro. We can't manufacture a real backend bug (there is
/// none), so this drives the shrinker over an *injected* divergence to prove the
/// reduction workflow and the saved-repro artifact both work end to end.
#[test]
fn the_minimizer_reduces_and_saves_a_repro() {
    let dir = scratch_dir("minimizer");
    let scratch = dir.join("candidate.tuo");

    // A deliberately padded program whose meaning is a plain `7`, wrapped so the
    // shrinker has real structure to strip (helpers, nesting, dead arithmetic).
    let bloated = "\
fn f0(take p0: Int) -> Int { (p0 + (100 - 100)) }
fn main() -> Int {
    let v0: Int = f0((3 + 4));
    if (v0 * 1) >= 0 { (v0 + (0 * 999)) } else { (0 - 1) }
}
";
    // Treat the interpreter's own result as both sides of a synthetic
    // "divergence" so the oracle (`still_diverges`) is satisfied by any program
    // that keeps the same observable value. This exercises the reduction machine
    // without a real backend disagreement.
    let observed = harness::interpret(bloated);
    let synthetic = harness::Divergence {
        source: bloated.to_owned(),
        interpreter: observed.clone(),
        // A distinct native outcome so `agrees_with` is false and the oracle
        // keeps reducing as long as the interpreter still yields `observed`.
        native: harness::Outcome::Trapped { label: None },
    };

    let minimized = shrink::minimize_with_oracle(&synthetic, &scratch, &observed);
    assert!(
        minimized.source.len() < bloated.len(),
        "the minimizer should shrink the program; got:\n{}",
        minimized.source
    );
    assert!(
        harness::accepts(&minimized.source),
        "the minimized program must still type-check:\n{}",
        minimized.source
    );

    let repro = shrink::save_repro(&minimized, &scratch_dir("minimizer-repro"), "synthetic");
    assert!(
        repro.exists(),
        "the saved repro program should exist on disk"
    );
    let report = repro.with_extension("report");
    assert!(
        report.exists(),
        "the saved repro report should exist on disk"
    );
}
