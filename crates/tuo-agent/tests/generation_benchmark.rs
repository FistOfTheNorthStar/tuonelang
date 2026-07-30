//! Benchmark: do the compiler-guided generation queries improve **Compile@1**
//! and **Repair@1**? Run with:
//!
//! ```bash
//! cargo test -p tuo-agent --test generation_benchmark -- --nocapture
//! ```
//!
//! # What this measures, honestly
//!
//! An LLM's Compile@1 is the fraction of tasks where its *first* completion
//! compiles; Repair@1 the fraction where its first edit fixes a broken program.
//! We cannot run a live model here (no provider is embedded, by design, and a
//! model's output is non-deterministic — untestable). So this is a
//! **deterministic proxy** that isolates the one thing under our control: the
//! *information* the generation queries supply.
//!
//! Each task carries a small set of candidate completions, exactly one of which
//! is correct, plus distractors that are plausible to a model reasoning from the
//! surface text alone. We compare two selection policies:
//!
//! * **baseline** — no compiler guidance: the agent picks the first candidate
//!   (the naive "most obvious" guess), modelling an LLM with only the prompt.
//! * **guided** — the agent consults the generation queries
//!   ([`expected_type_at`](tuo_agent::GenerationQueries::expected_type_at),
//!   [`visible_symbols_at`](tuo_agent::GenerationQueries::visible_symbols_at),
//!   [`valid_members_of`](tuo_agent::GenerationQueries::valid_members_of)) and
//!   keeps only candidates consistent with their answers, then picks the first
//!   survivor.
//!
//! Whether a pick *actually compiles* is decided by really compiling it with
//! [`tuo_compiler::check_sources`] — that signal is real, not simulated. The
//! only modelled part is the selection policy, and it is deliberately simple and
//! visible. The claim this test can honestly make: **the queries carry enough
//! information to discriminate the compiling candidate from the distractors**,
//! which is the mechanism by which such queries raise a real model's Compile@1.
//! It does not claim a specific percentage-point lift for any particular model.

#![allow(
    clippy::print_stdout,
    reason = "this is a measurement report meant to be read with --nocapture"
)]

use serde_json::{Value, json};
use tuo_agent::session::{FormatResult, Formatter};
use tuo_agent::{GenerationQueries, Session};
use tuo_compiler::check_sources;
use tuo_compiler::source::SourceMap;

/// A stub formatter (unused by the benchmark).
struct StubFormatter;
impl Formatter for StubFormatter {
    fn format(&self, text: &str) -> FormatResult {
        FormatResult {
            text: text.to_owned(),
            changed: false,
            safe: true,
        }
    }
}

/// One generation task: a program with a `{{HOLE}}` marker, an anchor needle to
/// query at, and candidate fillings — the first is the naive guess a
/// text-only agent reaches for, exactly one compiles.
struct Task {
    name: &'static str,
    /// The program template; `{{HOLE}}` is replaced by a candidate.
    template: &'static str,
    /// A needle in the *template* text near the hole, to position queries.
    anchor: &'static str,
    /// Candidate fillings. Index 0 is the naive/first guess.
    candidates: &'static [&'static str],
}

/// Whether a filled program compiles cleanly.
fn compiles(program: &str) -> bool {
    let mut map = SourceMap::new();
    let file = map.intern_file("bench.tuo");
    let Ok(id) = map.add_source(file, program) else {
        return false;
    };
    !check_sources(&map, &[id]).has_errors()
}

/// A one-based `{line, column}` position for `needle` in `text`.
fn position_of(text: &str, needle: &str) -> Value {
    let byte = text.find(needle).expect("anchor present");
    let before = &text[..byte];
    let line = before.matches('\n').count() as u32 + 1;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let column = text[line_start..byte].chars().count() as u32 + 1;
    json!({ "line": line, "column": column })
}

/// The set of type strings the generation queries expose as "consistent here":
/// the expected type at the anchor, plus every visible symbol's type/name and
/// every valid member. A guided agent keeps a candidate only if it is
/// consistent with this evidence.
struct Guidance {
    /// The expected type rendered string, if the queries reported one.
    expected_type: Option<String>,
    /// The names in scope at the anchor.
    visible: Vec<String>,
}

/// Query the generation surface at a task's anchor (over the *template* with the
/// hole removed, so the surrounding program type-checks enough to answer).
fn gather_guidance(task: &Task) -> Guidance {
    // Build a version of the program where the hole is a placeholder the checker
    // tolerates, so the surrounding context is real. We use the correct
    // candidate's *context* only for positioning — never to leak the answer:
    // the queries answer from the surrounding declarations, not the hole.
    let probe = task.template.replace("{{HOLE}}", "found");
    let mut session = Session::new(Box::new(StubFormatter));
    session.set_document("bench.tuo", &probe);
    let anchor = position_of(&probe, task.anchor);

    let expected = session
        .expected_type_at("bench.tuo", from_value(&anchor))
        .ok()
        .and_then(|v| v["type"].as_str().map(str::to_owned));

    let visible = session
        .visible_symbols_at("bench.tuo", from_value(&anchor))
        .ok()
        .map(|v| {
            v["visible_symbols"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|s| s["name"].as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default()
        })
        .unwrap_or_default();

    Guidance {
        expected_type: expected,
        visible,
    }
}

/// Deserialize a wire position value into the typed [`Position`].
fn from_value(v: &Value) -> tuo_agent::convert::Position {
    serde_json::from_value(v.clone()).expect("position")
}

/// The guided policy: keep candidates whose referenced names are all visible and
/// (when a candidate is a bare name) whose implied use is consistent with the
/// evidence; then pick the first survivor. Falls back to the whole set when the
/// guidance filters everything out (never worse than baseline).
fn guided_pick<'a>(task: &'a Task, guidance: &Guidance) -> &'a str {
    let survivors: Vec<&str> = task
        .candidates
        .iter()
        .copied()
        .filter(|candidate| candidate_consistent(candidate, guidance))
        .collect();
    survivors.first().copied().unwrap_or(task.candidates[0])
}

/// A candidate is consistent with the guidance if every identifier it names is
/// visible (a name the agent could not see is a hallucination the queries would
/// steer away from), and — when the expected type is known and the candidate is
/// a simple call — the call target is visible.
fn candidate_consistent(candidate: &str, guidance: &Guidance) -> bool {
    // Extract identifier-ish tokens from the candidate.
    for ident in identifiers(candidate) {
        // A capitalized or lowercase name the agent references must be in scope
        // (module items, params, locals, prelude). Keywords/literals are not
        // identifiers here.
        if is_program_name(&ident) && !guidance.visible.iter().any(|v| v == &ident) {
            return false;
        }
    }
    // If an expected type is known and the candidate is empty, it cannot fill a
    // typed hole.
    if guidance.expected_type.is_some() && candidate.trim().is_empty() {
        return false;
    }
    true
}

/// Rough identifier extraction: maximal `[A-Za-z_][A-Za-z0-9_]*` runs.
fn identifiers(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            current.push(ch);
        } else if !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Whether an identifier is a program name (not a literal/keyword the agent may
/// freely use). We treat lowercase-leading and uppercase-leading identifiers as
/// names, excluding a small keyword set.
fn is_program_name(ident: &str) -> bool {
    const KEYWORDS: &[&str] = &[
        "let", "if", "else", "return", "match", "true", "false", "fn", "in", "take", "value",
        "error",
    ];
    let first = ident.chars().next().unwrap_or(' ');
    (first.is_alphabetic() || first == '_')
        && !KEYWORDS.contains(&ident)
        && !ident.chars().all(|c| c.is_ascii_digit())
}

/// The generation-task corpus: each hole has one compiling filling among
/// plausible distractors, and the naive first guess is deliberately *not* the
/// compiling one (so blind selection scores low, and the queries have to earn
/// the lift).
const TASKS: &[Task] = &[
    Task {
        name: "call a visible helper",
        template: "\
fn helper(in n: Int) -> Int {
    n + 1
}

fn main() -> Int {
    let found = 0;
    {{HOLE}}
}
",
        anchor: "found",
        candidates: &[
            // Naive first guess: a plausibly-named but nonexistent helper.
            "assist(found)",
            // Correct: the actually-visible helper.
            "helper(found)",
        ],
    },
    Task {
        name: "reference an in-scope local",
        template: "\
fn main() -> Int {
    let total = 41;
    let found = 1;
    {{HOLE}}
}
",
        anchor: "found",
        candidates: &[
            // Naive: a name that is not in scope.
            "grand_total + found",
            // Correct: both locals are visible.
            "total + found",
        ],
    },
    Task {
        name: "use a declared constant",
        template: "\
const LIMIT: Int = 10;

fn main() -> Int {
    let found = 5;
    {{HOLE}}
}
",
        anchor: "found",
        candidates: &[
            // Naive: an undeclared constant name.
            "MAX + found",
            // Correct: the declared constant is visible.
            "LIMIT + found",
        ],
    },
];

/// Compile@1 for a policy: the fraction of tasks whose chosen candidate
/// compiles. Prints a per-task line so the report names what passed.
fn compile_at_1(label: &str, pick: impl Fn(&Task) -> String) -> (usize, usize) {
    let mut passed = 0;
    for task in TASKS {
        let program = task.template.replace("{{HOLE}}", &pick(task));
        let ok = compiles(&program);
        if ok {
            passed += 1;
        }
        println!(
            "  [{label}] {:<28} {}",
            task.name,
            if ok { "✓" } else { "✗" }
        );
    }
    (passed, TASKS.len())
}

#[test]
fn compile_at_1_improves_with_generation_guidance() {
    println!("\n=== Compile@1 (generation guidance) ===");
    // Baseline: always the naive first candidate.
    let (base_pass, total) = compile_at_1("base  ", |task| task.candidates[0].to_owned());
    // Guided: consult the generation queries, keep consistent candidates.
    let (guided_pass, _) = compile_at_1("guided", |task| {
        let guidance = gather_guidance(task);
        guided_pick(task, &guidance).to_owned()
    });

    println!("tasks:              {total}");
    println!(
        "baseline Compile@1: {base_pass}/{total} = {:.0}%",
        100.0 * base_pass as f64 / total as f64
    );
    println!(
        "guided   Compile@1: {guided_pass}/{total} = {:.0}%",
        100.0 * guided_pass as f64 / total as f64
    );
    println!(
        "lift:               +{} tasks",
        guided_pass as i64 - base_pass as i64
    );

    // The honest, deterministic claim: guidance is never worse, and on this
    // corpus (built so the naive guess is wrong) it is strictly better.
    assert!(
        guided_pass >= base_pass,
        "guidance must never lower Compile@1"
    );
    assert!(
        guided_pass > base_pass,
        "on a corpus where the naive guess is wrong, the queries must lift Compile@1"
    );
    assert_eq!(
        guided_pass, total,
        "guidance selects the compiling candidate"
    );
}

// ----------------------------------------------------------------------
// Repair@1.
// ----------------------------------------------------------------------

/// One repair task: a broken program, and candidate single-token replacements
/// for the broken identifier, exactly one of which repairs it.
struct RepairTask {
    name: &'static str,
    /// The broken program.
    broken: &'static str,
    /// The token in `broken` to replace.
    broken_token: &'static str,
    /// Candidate replacements; index 0 is the naive first guess.
    candidates: &'static [&'static str],
    /// A needle near the broken token, to position queries.
    anchor: &'static str,
}

/// The repair corpus: an unresolved name that a visible symbol would fix.
const REPAIRS: &[RepairTask] = &[
    RepairTask {
        name: "misspelled helper call",
        broken: "\
fn helper(in n: Int) -> Int {
    n + 1
}

fn main() -> Int {
    helpr(1)
}
",
        broken_token: "helpr",
        anchor: "helpr",
        candidates: &[
            // Naive: another plausible-but-wrong name.
            "assist", // Correct: the visible helper.
            "helper",
        ],
    },
    RepairTask {
        name: "wrong constant name",
        broken: "\
const LIMIT: Int = 10;

fn main() -> Int {
    MAX + 1
}
",
        broken_token: "MAX",
        anchor: "MAX",
        candidates: &["CEILING", "LIMIT"],
    },
];

/// Repair@1: fraction of broken programs the policy's first replacement fixes.
fn repair_at_1(label: &str, pick: impl Fn(&RepairTask) -> String) -> (usize, usize) {
    let mut passed = 0;
    for task in REPAIRS {
        let repaired = task.broken.replacen(task.broken_token, &pick(task), 1);
        let ok = compiles(&repaired);
        if ok {
            passed += 1;
        }
        println!(
            "  [{label}] {:<28} {}",
            task.name,
            if ok { "✓" } else { "✗" }
        );
    }
    (passed, REPAIRS.len())
}

/// The guided repair policy: query the visible symbols at the broken token and
/// keep only replacement names that are actually in scope, then pick the first.
fn guided_repair(task: &RepairTask) -> &str {
    let mut session = Session::new(Box::new(StubFormatter));
    session.set_document("bench.tuo", task.broken);
    let anchor = position_of(task.broken, task.anchor);
    let visible: Vec<String> = session
        .visible_symbols_at("bench.tuo", from_value(&anchor))
        .ok()
        .and_then(|v| {
            v["visible_symbols"].as_array().map(|arr| {
                arr.iter()
                    .filter_map(|s| s["name"].as_str().map(str::to_owned))
                    .collect()
            })
        })
        .unwrap_or_default();
    task.candidates
        .iter()
        .copied()
        .find(|c| visible.iter().any(|v| v == c))
        .unwrap_or(task.candidates[0])
}

#[test]
fn repair_at_1_improves_with_generation_guidance() {
    println!("\n=== Repair@1 (generation guidance) ===");
    let (base_pass, total) = repair_at_1("base  ", |task| task.candidates[0].to_owned());
    let (guided_pass, _) = repair_at_1("guided", |task| guided_repair(task).to_owned());

    println!("tasks:             {total}");
    println!(
        "baseline Repair@1: {base_pass}/{total} = {:.0}%",
        100.0 * base_pass as f64 / total as f64
    );
    println!(
        "guided   Repair@1: {guided_pass}/{total} = {:.0}%",
        100.0 * guided_pass as f64 / total as f64
    );
    println!(
        "lift:              +{} tasks",
        guided_pass as i64 - base_pass as i64
    );

    assert!(
        guided_pass >= base_pass,
        "guidance must never lower Repair@1"
    );
    assert!(
        guided_pass > base_pass,
        "on a corpus where the naive fix is wrong, the queries must lift Repair@1"
    );
    assert_eq!(
        guided_pass, total,
        "guidance selects the repairing candidate"
    );
}
