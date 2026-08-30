//! Benchmark: does the context-injectable brief (ADR-0018) actually help a
//! model write tuonelang that compiles? Run with:
//!
//! ```bash
//! cargo test -p tuo-cli --test cheatsheet_benchmark -- --nocapture
//! ```
//!
//! # What this measures, honestly
//!
//! The brief exists on a claim — that priming a model with it raises the rate
//! at which its first attempt compiles. This repository does not accept a
//! generation-quality claim on plausibility (`tuo-agent`'s
//! `generation_benchmark.rs` and `stdlib_hallucination.rs` both measure theirs),
//! so the brief is measured the same way.
//!
//! No live model runs here: no provider is embedded, by design, and a model's
//! output is non-deterministic and so untestable. This is a **deterministic
//! proxy** that isolates the one thing under our control — the *information the
//! brief carries*.
//!
//! Each task is a real programming problem with several candidate spellings.
//! Index 0 is the **cross-language prior**: what a model reasoning from
//! Rust/Python/Go familiarity writes first. Exactly one candidate compiles. Two
//! policies are compared:
//!
//! * **unprimed** — no tuonelang knowledge: take the prior, modelling a model
//!   that has never seen the language.
//! * **primed** — consult the brief and keep only candidates consistent with
//!   what it states, then take the first survivor.
//!
//! The primed policy reads the **real generated brief** (the same text `tuo
//! cheatsheet` emits), not a hand-written summary of it — so a brief that
//! stopped carrying a fact would stop scoring for it.
//!
//! Whether a pick *actually compiles* is decided by really compiling it through
//! [`tuo_compiler::check_sources`]; that signal is never simulated. The only
//! modelled part is the selection policy, which is deliberately simple and
//! visible. The honest claim: **the brief carries enough information to reject
//! the cross-language guess and keep the tuonelang one.** It does not claim a
//! specific percentage-point improvement for any particular model.

#![allow(
    clippy::print_stdout,
    reason = "this is a measurement report meant to be read with --nocapture"
)]

use std::process::Command;

use tuo_compiler::check_sources;
use tuo_compiler::source::{SourceId, SourceMap};

/// One task: a program to write, with candidate spellings.
struct Task {
    /// A label for the report.
    name: &'static str,
    /// What a model must get right, and what the brief says about it.
    ///
    /// `evidence` is a fragment that must appear in the brief for the primed
    /// policy to have learned this — if the brief stops saying it, the policy
    /// loses its grounds and the task scores as unprimed.
    evidence: &'static str,
    /// A program template with a `{{CODE}}` marker.
    template: &'static str,
    /// Candidates. Index 0 is the cross-language prior; exactly one compiles.
    candidates: &'static [&'static str],
    /// A substring that a *wrong* candidate contains and the right one does
    /// not — the discriminator the brief teaches.
    rejects: &'static [&'static str],
}

/// The corpus: problems whose obvious cross-language spelling does not compile.
const TASKS: &[Task] = &[
    Task {
        name: "mutable binding (`var`, not `let mut`)",
        evidence: "var total = 0;",
        template: "\
module caller;
fn use_it() -> Int {
    {{CODE}}
}
",
        candidates: &[
            "let mut total = 0; total = total + 1; total",
            "var total = 0; total = total + 1; total",
        ],
        rejects: &["let mut"],
    },
    Task {
        name: "reassignment (no compound assignment)",
        evidence: "total = total + 1;",
        template: "\
module caller;
fn use_it() -> Int {
    var total = 0;
    {{CODE}}
    total
}
",
        candidates: &["total += 1;", "total = total + 1;"],
        rejects: &["+="],
    },
    Task {
        name: "generic arguments (square brackets)",
        evidence: "Option[Int]",
        template: "\
module caller;
fn use_it({{CODE}}) -> Bool {
    true
}
",
        candidates: &["take o: Option<Int>", "take o: Option[Int]"],
        rejects: &["Option<", "Result<"],
    },
    Task {
        name: "Option payload (named field, not positional)",
        evidence: "Some { value: x }",
        template: "\
module caller;
fn use_it(take x: Int) -> Option[Int] {
    {{CODE}}
}
",
        candidates: &["Some(x)", "Some { value: x }"],
        rejects: &["Some("],
    },
    Task {
        name: "parameter mode is mandatory",
        evidence: "Every parameter needs BOTH a mode and a type.",
        template: "\
module caller;
{{CODE}}
",
        candidates: &[
            "fn use_it(x: Int) -> Int { x }",
            "fn use_it(take x: Int) -> Int { x }",
        ],
        // A parameter written as `name: Type` with no leading mode keyword.
        rejects: &["(x: Int)"],
    },
    Task {
        name: "imports use `import`, not `use`",
        evidence: "`import`, NEVER `use`",
        template: "\
module caller;
{{CODE}}
fn use_it() -> Int { 0 }
",
        candidates: &["use util::helpers;", "import util::helpers;"],
        rejects: &["use "],
    },
    Task {
        name: "comparisons do not chain",
        evidence: "a < b < c",
        template: "\
module caller;
fn use_it(take a: Int, take b: Int, take c: Int) -> Bool {
    {{CODE}}
}
",
        candidates: &["a < b < c", "a < b && b < c"],
        rejects: &["< b <"],
    },
    Task {
        name: "match on a named payload, not a positional one",
        evidence: "Some { value } => value,",
        template: "\
module caller;
fn use_it(in o: Option[Int]) -> Int {
    match o {
        {{CODE}}
        None => 0,
    }
}
",
        candidates: &["Some(value) => value,", "Some { value } => value,"],
        rejects: &["Some("],
    },
];

/// A companion module so an `import` in a task resolves.
const COMPANION: &str = "module util::helpers;\n\npub fn helper() -> Int { 0 }\n";

/// Really compile `program`, returning whether the front end accepts it.
fn compiles(program: &str) -> bool {
    let mut map = SourceMap::new();
    let file = map.intern_file("caller.tuo");
    let Ok(id) = map.add_source(file, program) else {
        return false;
    };
    let companion_file = map.intern_file("helpers.tuo");
    let Ok(companion) = map.add_source(companion_file, COMPANION) else {
        return false;
    };
    let sources: Vec<SourceId> = vec![id, companion];
    !check_sources(&map, &sources).has_errors()
}

/// The brief as `tuo cheatsheet` emits it.
fn brief() -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_tuo"))
        .arg("cheatsheet")
        .output()
        .expect("the `tuo` binary runs");
    assert!(output.status.success(), "`tuo cheatsheet` failed");
    String::from_utf8(output.stdout).expect("the brief is UTF-8")
}

/// Build the program for `candidate`.
fn program(task: &Task, candidate: &str) -> String {
    task.template.replace("{{CODE}}", candidate)
}

/// The **unprimed** policy: no tuonelang knowledge, so take the
/// cross-language prior.
fn unprimed(task: &Task) -> &'static str {
    task.candidates[0]
}

/// The **primed** policy: keep only candidates consistent with what the brief
/// states, then take the first survivor.
///
/// The policy consults the real brief for its evidence. If the brief no longer
/// carries the fact, the policy has no grounds to discriminate and falls back
/// to the prior — so this measures the brief's *content*, not this test's.
fn primed(task: &Task, brief: &str) -> &'static str {
    if !brief.contains(task.evidence) {
        return unprimed(task);
    }
    task.candidates
        .iter()
        .find(|candidate| !task.rejects.iter().any(|bad| candidate.contains(bad)))
        .copied()
        .unwrap_or_else(|| unprimed(task))
}

#[test]
fn every_task_has_exactly_one_compiling_candidate() {
    // The corpus is only meaningful if each task really is discriminating: the
    // prior must really fail and exactly one candidate must really compile.
    for task in TASKS {
        let compiling: Vec<&str> = task
            .candidates
            .iter()
            .filter(|candidate| compiles(&program(task, candidate)))
            .copied()
            .collect();
        assert_eq!(
            compiling.len(),
            1,
            "task `{}` must have exactly one compiling candidate, got {:?}",
            task.name,
            compiling
        );
        assert!(
            !compiles(&program(task, unprimed(task))),
            "task `{}`: the cross-language prior `{}` compiles, so the task \
             does not discriminate and proves nothing",
            task.name,
            unprimed(task)
        );
    }
}

#[test]
fn the_brief_carries_the_evidence_each_task_relies_on() {
    let brief = brief();
    for task in TASKS {
        assert!(
            brief.contains(task.evidence),
            "task `{}` relies on the brief stating `{}`, which it does not. \
             Either the brief lost the fact or the task's evidence is stale.",
            task.name,
            task.evidence
        );
    }
}

#[test]
fn priming_with_the_brief_raises_compile_at_1() {
    let brief = brief();
    let mut unprimed_ok = 0usize;
    let mut primed_ok = 0usize;
    let mut rows: Vec<(&str, bool, bool)> = Vec::new();

    for task in TASKS {
        let u = compiles(&program(task, unprimed(task)));
        let p = compiles(&program(task, primed(task, &brief)));
        unprimed_ok += usize::from(u);
        primed_ok += usize::from(p);
        rows.push((task.name, u, p));
    }

    let total = TASKS.len();
    println!();
    println!("Compile@1 — priming with the generated language brief (ADR-0018)");
    println!("A deterministic proxy; every pick is really compiled. No model runs.");
    println!();
    println!("  {:<48}  {:>8}  {:>6}", "task", "unprimed", "primed");
    println!("  {:-<48}  {:->8}  {:->6}", "", "", "");
    for (name, u, p) in &rows {
        println!(
            "  {:<48}  {:>8}  {:>6}",
            name,
            if *u { "pass" } else { "FAIL" },
            if *p { "pass" } else { "FAIL" }
        );
    }
    println!("  {:-<48}  {:->8}  {:->6}", "", "", "");
    println!(
        "  {:<48}  {:>7}%  {:>5}%",
        "Compile@1",
        unprimed_ok * 100 / total,
        primed_ok * 100 / total
    );
    println!();
    println!("Read: the brief carries enough information to reject the cross-language");
    println!("guess and keep the tuonelang one. It is not a claim about any specific");
    println!("model's improvement.");
    println!();

    assert!(
        primed_ok >= unprimed_ok,
        "priming must never make things worse: unprimed {unprimed_ok}/{total}, \
         primed {primed_ok}/{total}"
    );
    assert!(
        primed_ok > unprimed_ok,
        "on this corpus the brief must be strictly better than the priors it \
         is meant to correct: unprimed {unprimed_ok}/{total}, primed \
         {primed_ok}/{total}"
    );
    assert_eq!(
        primed_ok, total,
        "the brief should carry enough to get every task right; it got \
         {primed_ok}/{total}"
    );
}
