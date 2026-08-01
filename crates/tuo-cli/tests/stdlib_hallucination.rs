//! Benchmark: does the standard library's machine-queryable symbol information
//! prevent LLM **API hallucination**? Run with:
//!
//! ```bash
//! cargo test -p tuo-cli --test stdlib_hallucination -- --nocapture
//! ```
//!
//! # What this measures, honestly
//!
//! When a language model calls into a standard library it has not memorised, it
//! *hallucinates*: it invents a plausible function name (`maximum` for `max`),
//! guesses a wrong arity (`unwrap_or` with one argument), or reaches for a name
//! that lives in a different module. Each such guess fails to compile.
//!
//! We cannot run a live model here — no provider is embedded, by design, and a
//! model's output is non-deterministic and so untestable. This is therefore a
//! **deterministic proxy** that isolates the one thing under our control: the
//! *symbol information* the standard library exposes to a machine.
//!
//! Each task is a real call into a stdlib module with several candidate
//! spellings. Index 0 is the **hallucination** — the plausible-but-wrong guess a
//! model reasoning from surface familiarity (other languages' stdlibs) would
//! reach for first. Exactly one candidate is the real API. We compare two
//! policies:
//!
//! * **baseline** — no library knowledge: pick the first (hallucinated) guess,
//!   modelling a model with only its priors.
//! * **grounded** — consult the library's machine-queryable symbols (the exact
//!   set of public function names each module defines, read straight out of the
//!   compiler's [`Resolution`](tuo_compiler::resolve::Resolution)) and keep only
//!   candidates that call a function the module actually exports, then pick the
//!   first survivor.
//!
//! Whether a pick *actually compiles* is decided by really compiling it with
//! [`tuo_compiler::check_sources`] against the real module source — that signal
//! is not simulated. The only modelled part is the selection policy, and it is
//! deliberately simple and visible. The honest claim: **the library's symbol
//! surface carries enough information to reject hallucinated calls and keep the
//! real one**, which is the mechanism by which exposing it lowers a real model's
//! hallucination rate. It does not claim a specific percentage-point reduction
//! for any particular model.

#![allow(
    clippy::print_stdout,
    reason = "this is a measurement report meant to be read with --nocapture"
)]

use std::collections::HashSet;

use tuo_compiler::check_sources;
use tuo_compiler::resolve::SymbolKind;
use tuo_compiler::source::{SourceId, SourceMap};

/// One API-use task against a single stdlib module.
struct Task {
    /// A label for the report.
    name: &'static str,
    /// The stdlib module this task calls into.
    module: tuo_stdlib::Module,
    /// A caller-program template with a `{{CALL}}` marker; each candidate is
    /// substituted in and the whole thing compiled against `module`.
    template: &'static str,
    /// Candidate call expressions. Index 0 is the hallucinated guess; exactly
    /// one candidate compiles.
    candidates: &'static [&'static str],
}

/// The corpus: real calls whose obvious guess is a hallucination.
const TASKS: &[Task] = &[
    Task {
        name: "core::max (not `maximum`)",
        module: tuo_stdlib::CORE,
        template: "\
module caller;
import std::core;
fn use_it(take a: Int, take b: Int) -> Int {
    {{CALL}}
}
",
        // `maximum` is the plausible other-language name; `max` is real.
        candidates: &["std::core::maximum(a, b)", "std::core::max(a, b)"],
    },
    Task {
        name: "core::unwrap_or (not `unwrap`)",
        module: tuo_stdlib::CORE,
        template: "\
module caller;
import std::core;
fn use_it(in o: Option[Int]) -> Int {
    {{CALL}}
}
",
        // `unwrap` (Rust-ism) does not exist here; the library's one obvious
        // spelling is `unwrap_or` with an explicit default.
        candidates: &["std::core::unwrap(o)", "std::core::unwrap_or(o, 0)"],
    },
    Task {
        name: "collections::range_sum (not `sum_range`)",
        module: tuo_stdlib::COLLECTIONS,
        template: "\
module caller;
import std::collections;
fn use_it() -> Int {
    {{CALL}}
}
",
        candidates: &[
            "std::collections::sum_range(1, 5)",
            "std::collections::range_sum(1, 5)",
        ],
    },
    Task {
        name: "time::from_millis (not `millis`)",
        module: tuo_stdlib::TIME,
        template: "\
module caller;
import std::time;
fn use_it() -> Int {
    std::time::as_nanos({{CALL}})
}
",
        candidates: &["std::time::millis(2)", "std::time::from_millis(2)"],
    },
    Task {
        name: "process::is_success (not `succeeded`)",
        module: tuo_stdlib::PROCESS,
        // The status is built through the module's own constructor, so the
        // caller needs no type import — only the call under test varies.
        template: "\
module caller;
import std::process;
fn use_it() -> Bool {
    {{CALL}}
}
",
        candidates: &[
            "std::process::succeeded(std::process::success())",
            "std::process::is_success(std::process::success())",
        ],
    },
    Task {
        name: "fs::is_absolute (not `is_abs`)",
        module: tuo_stdlib::FS,
        template: "\
module caller;
import std::fs;
fn use_it(in path: Str) -> Bool {
    {{CALL}}
}
",
        candidates: &["std::fs::is_abs(path)", "std::fs::is_absolute(path)"],
    },
];

/// Compile `module` + a caller that makes `call`, returning whether it checks
/// cleanly. This is the real, unsimulated compile signal.
fn call_compiles(module: tuo_stdlib::Module, template: &str, call: &str) -> bool {
    let caller_src = template.replace("{{CALL}}", call);
    let mut map = SourceMap::new();

    let lib_file = map.intern_file(module.name);
    let lib_id: SourceId = map
        .add_source(lib_file, module.source)
        .expect("stdlib source fits");

    let caller_file = map.intern_file("caller.tuo");
    let caller_id: SourceId = map
        .add_source(caller_file, caller_src)
        .expect("caller source fits");

    !check_sources(&map, &[lib_id, caller_id]).has_errors()
}

/// The machine-queryable symbol surface a grounded agent reads: the set of
/// public function names `module` actually defines, straight from the compiler's
/// resolution. This is exactly the information the agent protocol's `symbols`
/// query projects.
fn exported_functions(module: tuo_stdlib::Module) -> HashSet<String> {
    let mut map = SourceMap::new();
    let file = map.intern_file(module.name);
    let id = map.add_source(file, module.source).expect("fits");
    let checked = check_sources(&map, &[id]);
    checked
        .resolution
        .symbols()
        .filter(|(_, sym)| sym.kind == SymbolKind::Function && sym.is_pub)
        .map(|(_, sym)| sym.name.clone())
        .collect()
}

/// The function name a candidate call targets: the identifier right before `(`,
/// after the last `::`. A grounded agent checks this against the exported set.
fn called_function(call: &str) -> &str {
    let before_paren = call.split('(').next().unwrap_or(call);
    before_paren.rsplit("::").next().unwrap_or(before_paren)
}

/// Baseline: the first candidate — the hallucinated guess.
fn baseline_pick(task: &Task) -> &'static str {
    task.candidates[0]
}

/// Grounded: keep only candidates whose called function is one the module
/// actually exports, then take the first survivor. Falls back to the first
/// candidate if the symbols reject everything (never happens here — one is real).
fn grounded_pick(task: &Task, exported: &HashSet<String>) -> &'static str {
    task.candidates
        .iter()
        .copied()
        .find(|call| exported.contains(called_function(call)))
        .unwrap_or(task.candidates[0])
}

#[test]
fn grounding_in_stdlib_symbols_beats_hallucination() {
    let mut baseline_ok = 0usize;
    let mut grounded_ok = 0usize;

    println!("\n stdlib API hallucination benchmark (Compile@1 proxy)");
    println!(" {:<48} {:>10} {:>10}", "task", "baseline", "grounded");
    println!(" {}", "-".repeat(70));

    for task in TASKS {
        // Sanity: exactly one candidate compiles (the corpus is well-formed).
        let compiling: Vec<&str> = task
            .candidates
            .iter()
            .copied()
            .filter(|c| call_compiles(task.module, task.template, c))
            .collect();
        assert_eq!(
            compiling.len(),
            1,
            "task `{}` must have exactly one compiling candidate, got {:?}",
            task.name,
            compiling
        );

        let exported = exported_functions(task.module);

        let baseline = baseline_pick(task);
        let grounded = grounded_pick(task, &exported);

        let b_ok = call_compiles(task.module, task.template, baseline);
        let g_ok = call_compiles(task.module, task.template, grounded);
        if b_ok {
            baseline_ok += 1;
        }
        if g_ok {
            grounded_ok += 1;
        }

        println!(
            " {:<48} {:>10} {:>10}",
            task.name,
            if b_ok { "compiles" } else { "HALLUC." },
            if g_ok { "compiles" } else { "HALLUC." },
        );
    }

    let total = TASKS.len();
    println!(" {}", "-".repeat(70));
    println!(
        " {:<48} {:>9}% {:>9}%",
        "Compile@1",
        baseline_ok * 100 / total,
        grounded_ok * 100 / total,
    );
    println!();

    // The hallucinated first guess never compiles: grounding is a strict win.
    assert_eq!(
        baseline_ok, 0,
        "the corpus models hallucination: the naive guess should never compile"
    );
    assert_eq!(
        grounded_ok, total,
        "grounding in the library's real symbols should recover every real API"
    );
    assert!(
        grounded_ok > baseline_ok,
        "exposing the library's symbols strictly reduces hallucination on this corpus"
    );
}
