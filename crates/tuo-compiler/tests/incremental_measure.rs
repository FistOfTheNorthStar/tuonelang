//! Measurement: for each edit scenario, report **exactly which per-item queries
//! re-execute** and the wall-clock cost of the incremental re-check versus a
//! cold whole-program compile. Run with:
//!
//! ```bash
//! cargo test -p tuo-compiler --test incremental_measure -- --nocapture
//! ```
//!
//! This is the "benchmarks showing exactly which queries re-execute" deliverable
//! of the incremental-compilation work. It is deterministic in *which* queries
//! run (asserted); the printed timings are measured, not promised (mirroring the
//! spec runner's no-latency-promise stance), so the test never fails on timing.

#![allow(
    clippy::print_stdout,
    reason = "this is a measurement report meant to be read with --nocapture"
)]

use std::time::Instant;

use tuo_compiler::IncrementalSession;

const LIB: &str = "\
fn add(take a: Int, take b: Int) -> Int {
    a + b
}

fn double(take x: Int) -> Int {
    x * 2
}
";

const MAIN: &str = "\
fn main() -> Int {
    add(2, 3)
}

spec add {
    then add(2, 3) == 5;
}
";

/// Ask every per-item query once (populates or revalidates every memo).
fn reask(session: &IncrementalSession) {
    session.resolution().unwrap();
    for file in session.files() {
        session.parse(file).unwrap();
    }
    for f in session.function_symbols() {
        session.type_of_function(f).unwrap();
        session.mir_of(f).unwrap();
    }
    for s in session.spec_symbols() {
        session.spec_dependencies(s).unwrap();
    }
}

/// A primed session over the two-file program.
fn primed() -> IncrementalSession {
    let mut session = IncrementalSession::new();
    session.set_file("lib.tuo", LIB);
    session.set_file("main.tuo", MAIN);
    reask(&session);
    session
}

/// Apply `edit`, re-ask everything, and return (executed query labels, elapsed).
fn measure(edit: impl FnOnce(&mut IncrementalSession)) -> (Vec<String>, std::time::Duration) {
    let mut session = primed();
    session.clear_executions();
    edit(&mut session);
    let started = Instant::now();
    reask(&session);
    let elapsed = started.elapsed();
    (session.executed_queries(), elapsed)
}

/// The cost of a cold whole-program compile (build a fresh session and ask
/// everything once) — the baseline the incremental re-check improves on.
fn cold_cost() -> std::time::Duration {
    let mut session = IncrementalSession::new();
    session.set_file("lib.tuo", LIB);
    session.set_file("main.tuo", MAIN);
    let started = Instant::now();
    reask(&session);
    started.elapsed()
}

fn report(name: &str, log: &[String], elapsed: std::time::Duration) {
    // Count only the fine-grained per-item queries (the whole-program digest
    // queries are early-cutoff shields, reported separately for context).
    let per_item = log
        .iter()
        .filter(|l| {
            l.starts_with("type_of(") || l.starts_with("mir_of(") || l.starts_with("spec_deps(")
        })
        .count();
    println!("── {name} ──");
    println!("  re-executed queries ({}): {log:?}", log.len());
    println!("  of which per-item stage queries: {per_item}");
    println!("  incremental re-check: {}µs", elapsed.as_micros());
}

#[test]
fn measure_the_five_scenarios() {
    let cold = cold_cost();
    println!("\n=== Incremental re-check: which queries re-execute per scenario ===");
    println!(
        "cold whole-program compile baseline: {}µs\n",
        cold.as_micros()
    );

    // 1. No-change.
    let (log, elapsed) = measure(|_| {});
    report("no-change check", &log, elapsed);
    assert!(log.is_empty(), "no-change must recompute nothing: {log:?}");

    // 2. Function-body-only edit (add: a + b -> b + a).
    let (log, elapsed) = measure(|s| {
        s.set_file("lib.tuo", &LIB.replace("    a + b\n", "    b + a\n"));
    });
    report("function-body-only edit", &log, elapsed);
    assert!(
        log.iter().any(|l| l.starts_with("mir_of(")),
        "a body edit re-lowers the edited function's MIR"
    );

    // 3. Function-signature edit (add gains a parameter).
    let (log, elapsed) = measure(|s| {
        s.set_file(
            "lib.tuo",
            &LIB.replace(
                "fn add(take a: Int, take b: Int) -> Int {\n    a + b\n}",
                "fn add(take a: Int, take b: Int, take c: Int) -> Int {\n    a + b + c\n}",
            ),
        );
    });
    report("function-signature edit", &log, elapsed);
    assert!(
        log.iter().any(|l| l.starts_with("type_of(")),
        "a signature edit re-checks a function's type"
    );

    // 4. Unrelated-file edit (main body only).
    let (log, elapsed) = measure(|s| {
        s.set_file(
            "main.tuo",
            &MAIN.replace("    add(2, 3)\n", "    add(3, 4)\n"),
        );
    });
    report("unrelated-file edit", &log, elapsed);
    assert!(
        !log.iter().any(|l| l == "type_of(sym8)"),
        "an edit to main.tuo does not re-check lib.tuo's functions: {log:?}"
    );

    // 5. Spec-only edit (same spec, its body gains a dependency on `double`).
    //    The symbol structure is unchanged, so function typing is untouched;
    //    only the spec's own dependency graph re-runs.
    let (log, elapsed) = measure(|s| {
        s.set_file(
            "main.tuo",
            "fn main() -> Int {\n    add(2, 3)\n}\n\nspec add {\n    then add(2, 3) == 5;\n    then double(21) == 42;\n}\n",
        );
    });
    report("spec-only edit", &log, elapsed);
    assert!(
        log.iter().any(|l| l.starts_with("spec_deps(")),
        "a spec dependency edit re-runs the spec's dependency graph"
    );
    assert!(
        !log.iter().any(|l| l.starts_with("mir_of(")),
        "a spec-only edit re-lowers no function MIR: {log:?}"
    );

    println!("\n=== end report ===\n");
}
