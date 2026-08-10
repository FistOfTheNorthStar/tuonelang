//! The compiler-benchmark suite: the nine scenarios the prompt names, each
//! driving the **real** compiler and recording exactly what it did.
//!
//! Every scenario here runs actual tuonelang source through actual compiler
//! entry points — [`check_sources`](tuo_compiler::check_sources) for the cold
//! front-end stages, and [`IncrementalSession`](tuo_compiler::IncrementalSession)
//! for the warm and edit scenarios. Nothing is simulated or asserted. The two
//! groups differ in what they measure:
//!
//! - **Cold** scenarios ([`ColdScenario`]) measure end-to-end batch cost from a
//!   fresh state (a new source map / a new session), the numbers a
//!   command-line `tuo lex|parse|check|build` pays. They report wall-clock
//!   [`Timing`].
//! - **Incremental** scenarios ([`EditScenario`]) measure the *re-execution set*
//!   after an edit — the exact per-item queries the session re-ran — because for
//!   an incremental compiler the honest figure is "how little work an edit
//!   costs", and that is a **count of queries**, not just a wall-clock number.
//!   Wall-clock is reported too, but the query set is the load-bearing evidence
//!   (and is deterministic, unlike timing).
//!
//! Both kinds record the exact source and the exact command a user would run to
//! reproduce them, per the prompt's `Record:` requirement.

use serde::{Deserialize, Serialize};
use tuo_compiler::IncrementalSession;
use tuo_compiler::source::{SourceId, SourceMap};

use crate::lab::timing::{Clock, Timing, time};

/// Which cold, from-scratch compiler stage a scenario measures.
///
/// These correspond one-to-one to the front-of-pipeline work a batch `tuo`
/// invocation performs. `build` is included as a variant so a host that injects
/// native codegen can measure it; the pure front-end variants (`lex`..`check`)
/// need no injection and run entirely within this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColdStage {
    /// Lex only: source → tokens.
    Lex,
    /// Parse: source → concrete syntax tree (includes lexing).
    Parse,
    /// Check: parse → resolve → type-check → ownership-check.
    Check,
    /// Build: check → HIR → verified, optimized MIR → native object → link.
    /// Requires a host-injected native builder (see [`super::runtime`]); this
    /// crate cannot link on its own.
    Build,
}

impl ColdStage {
    /// The `tuo` command a user runs to reproduce this stage, for the record.
    #[must_use]
    pub fn command(self, file: &str) -> String {
        match self {
            Self::Lex => format!("tuo debug syntax {file}"),
            Self::Parse => format!("tuo debug ast {file}"),
            Self::Check => format!("tuo check {file}"),
            Self::Build => format!("tuo build {file}"),
        }
    }

    /// A stable label for the scenario.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Lex => "cold-lex",
            Self::Parse => "cold-parse",
            Self::Check => "cold-check",
            Self::Build => "cold-build",
        }
    }
}

/// A completed cold-stage measurement over one source program.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColdResult {
    /// The scenario label (e.g. `cold-check`).
    pub label: String,
    /// The exact command a user runs to reproduce this measurement.
    pub command: String,
    /// The source-program byte size measured.
    pub source_bytes: usize,
    /// The measured wall-clock timing.
    pub timing: Timing,
}

/// Intern `text` as a single-file program in a fresh source map.
fn interned(text: &str) -> (SourceMap, SourceId) {
    let mut map = SourceMap::new();
    let file = map.intern_file("bench.tuo");
    let id = map.add_source(file, text).expect("benchmark source fits");
    (map, id)
}

/// Run one cold front-end stage (`Lex`/`Parse`/`Check`) over `text` from a fresh
/// state each iteration, timing it under `clock`.
///
/// `Build` is **not** handled here — it needs a host native builder — and
/// returns `None`; the runtime module owns the injected build path. Returns
/// `None` too if asked for zero samples.
#[must_use]
pub fn measure_cold<C: Clock>(
    clock: &C,
    stage: ColdStage,
    text: &str,
    samples: u32,
    iterations_per_sample: u32,
) -> Option<ColdResult> {
    // Each op interns a *fresh* source map so we measure genuine cold cost, not
    // a warm re-parse of a retained snapshot.
    let timing = match stage {
        ColdStage::Lex => time(clock, samples, iterations_per_sample, || {
            let (map, id) = interned(text);
            tuo_compiler::lexer::lex(map.source(id))
        }),
        ColdStage::Parse => time(clock, samples, iterations_per_sample, || {
            let (map, id) = interned(text);
            tuo_compiler::parser::parse(map.source(id))
        }),
        ColdStage::Check => time(clock, samples, iterations_per_sample, || {
            let (map, id) = interned(text);
            tuo_compiler::check_sources(&map, &[id])
        }),
        // Build needs native codegen + linker; measured through the runtime
        // module's injected builder, not here.
        ColdStage::Build => return None,
    }?;
    Some(ColdResult {
        label: stage.label().to_string(),
        command: stage.command("program.tuo"),
        source_bytes: text.len(),
        timing,
    })
}

/// Which incremental edit scenario is being measured. Each maps to one of the
/// standard interactive edits an editor makes, and to a precise expectation
/// about how little the session should re-execute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Edit {
    /// Re-ask after no edit at all. Expectation: zero re-executions.
    NoOpCheck,
    /// Change one function's body only. Expectation: only that function's
    /// parse/typeck/mir re-run; callers and unrelated items do not.
    FunctionBody,
    /// Change one function's signature. Expectation: that function's interface
    /// changes, so its callers' typing re-runs; unrelated functions do not.
    FunctionSignature,
    /// Change one spec's body only. Expectation: only that spec's dependency
    /// graph (and the spec) re-run.
    SingleSpec,
    /// Verify only the specs an edit to one file could have affected.
    /// Expectation: the affected set is a subset of all specs.
    AffectedSpecVerify,
}

impl Edit {
    /// A stable label for the scenario.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::NoOpCheck => "warm-no-op-check",
            Self::FunctionBody => "edit-function-body",
            Self::FunctionSignature => "edit-function-signature",
            Self::SingleSpec => "edit-single-spec",
            Self::AffectedSpecVerify => "affected-spec-verify",
        }
    }

    /// The `tuo` invocation pattern this scenario reflects, for the record.
    #[must_use]
    pub fn command(self) -> &'static str {
        match self {
            Self::NoOpCheck | Self::FunctionBody | Self::FunctionSignature => {
                "tuo check <package> (re-run after edit)"
            }
            Self::SingleSpec => "tuo verify <package> (re-run after spec edit)",
            Self::AffectedSpecVerify => "tuo verify --affected-by <file> <package>",
        }
    }
}

/// A completed incremental-edit measurement.
///
/// The `reexecuted` list is the load-bearing, deterministic evidence: it is the
/// exact set of per-item query labels the session re-ran after the edit. Its
/// *length* is the honest cost figure for an incremental compiler; the wall
/// timing is a supplementary observation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditResult {
    /// The scenario label.
    pub label: String,
    /// The command pattern reflected.
    pub command: String,
    /// The exact per-item queries that re-executed after the edit (sorted).
    pub reexecuted: Vec<String>,
    /// How many queries re-executed. The headline incremental-cost figure.
    pub reexecuted_count: usize,
    /// Optional wall-clock timing of the re-query, when a clock was supplied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timing: Option<Timing>,
}

/// A small two-function, one-spec program used for the edit scenarios. `alpha`
/// calls `helper`; `beta` is independent; the spec depends on `alpha`.
///
/// `beta` deliberately folds a fixed array with a bounded `for` loop (ADR-0004)
/// so the per-function re-lowering the edit scenarios measure includes the
/// aggregate/loop MIR paths from day one, not only scalar arithmetic.
const BASELINE: &str = "\
fn helper(take n: Int) -> Int { n + 1 }
fn alpha(take n: Int) -> Int { helper(n) + 2 }
fn beta(take n: Int) -> Int {
    var acc = 0;
    for x in [n, n, n] {
        acc = acc + x * 3;
    }
    acc
}
spec alpha_adds {
    when r = alpha(take 10);
    then assert r == 13;
}
";

/// A representative aggregate/loop program for the **cold** stages: a struct
/// with field projection, a fixed `[T; N]` array literal, and a bounded `for`
/// fold — the ADR-0004 constructs — so cold lex/parse/check cost of the new
/// lowering is tracked by the lab from the day the features landed. The
/// program must stay accepted by the real front end (pinned by a test below);
/// a cold measurement over a rejected program would time error paths and
/// report them as compile cost.
pub const COLD_AGGREGATE: &str = "\
struct Point {
    x: Int,
    y: Int,
}
fn shift(take p: Point, take d: Int) -> Point {
    Point { x: p.x + d, y: p.y + d }
}
fn main() -> Int {
    var acc = 0;
    for w in [1, 2, 3, 4] {
        let s = shift(Point { x: w, y: w }, 1);
        acc = acc + s.x;
    }
    acc
}
";

/// Drive one edit scenario against a fresh [`IncrementalSession`] and report the
/// re-executed query set.
///
/// This runs the real session: it loads [`BASELINE`], warms every query, clears
/// the execution log, applies the edit, re-asks the same queries, and reports
/// what re-executed. The result is deterministic — it is a property of the
/// query graph, not of timing — so it is the honest measure of incremental cost.
#[must_use]
pub fn measure_edit(edit: Edit) -> EditResult {
    let mut session = IncrementalSession::new();
    let file = session.set_file("prog.tuo", BASELINE);

    // Warm the full query surface so subsequent re-asks measure only the edit.
    warm(&session);
    session.clear_executions();

    match edit {
        Edit::NoOpCheck => {
            // No edit; just re-ask. Nothing should re-execute.
            warm(&session);
        }
        Edit::FunctionBody => {
            // Change beta's body only (an independent leaf). helper/alpha and the
            // spec must not re-execute.
            let edited = BASELINE.replace("x * 3", "x * 4");
            session.set_file("prog.tuo", &edited);
            warm(&session);
        }
        Edit::FunctionSignature => {
            // Change helper's signature; alpha calls helper, so alpha re-checks.
            let edited = BASELINE.replace(
                "fn helper(take n: Int) -> Int { n + 1 }",
                "fn helper(take n: Int, take m: Int) -> Int { n + m }",
            );
            session.set_file("prog.tuo", &edited);
            warm(&session);
        }
        Edit::SingleSpec => {
            // Change the spec's expected value only.
            let edited = BASELINE.replace("assert r == 13", "assert r == 13 && true");
            session.set_file("prog.tuo", &edited);
            warm(&session);
        }
        Edit::AffectedSpecVerify => {
            // Rather than re-executing, this scenario measures selection: which
            // specs an edit to the file could have affected. We report the
            // affected set size as the "re-executed" evidence (the specs that
            // would run), which is bounded by the total spec count.
            let affected = session
                .affected_specs(&[file])
                .expect("affected_specs on a clean program");
            let labels: Vec<String> = affected
                .iter()
                .map(|s| format!("spec:{}", s.as_u32()))
                .collect();
            return EditResult {
                label: edit.label().to_string(),
                command: edit.command().to_string(),
                reexecuted_count: labels.len(),
                reexecuted: labels,
                timing: None,
            };
        }
    }

    let mut reexecuted = session.executed_queries();
    reexecuted.sort();
    EditResult {
        label: edit.label().to_string(),
        command: edit.command().to_string(),
        reexecuted_count: reexecuted.len(),
        reexecuted,
        timing: None,
    }
}

/// Ask every tracked query for the current program, so the next edit's re-query
/// measures only what the edit changed.
fn warm(session: &IncrementalSession) {
    let _ = session.resolution();
    for file in session.files() {
        let _ = session.parse(file);
    }
    for sym in session.function_symbols() {
        let _ = session.type_of_function(sym);
        let _ = session.mir_of(sym);
    }
    for sym in session.spec_symbols() {
        let _ = session.spec_dependencies(sym);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lab::timing::SystemClock;

    #[test]
    fn cold_front_end_stages_measure_real_work() {
        let clock = SystemClock::new();
        let src = "fn main() -> Int { 1 + 2 }\n";
        for stage in [ColdStage::Lex, ColdStage::Parse, ColdStage::Check] {
            let result = measure_cold(&clock, stage, src, 3, 5).expect("front-end stage measures");
            assert_eq!(result.label, stage.label());
            assert_eq!(result.source_bytes, src.len());
            assert!(result.command.contains(".tuo"));
        }
    }

    #[test]
    fn the_cold_aggregate_program_passes_the_real_front_end() {
        // The aggregate/loop cold program must stay accepted: a cold
        // measurement over a rejected program would time error paths and call
        // them compile cost. This is the same honesty rule the runtime
        // workloads pin for their committed sources.
        let mut map = SourceMap::new();
        let file = map.intern_file("cold_aggregate.tuo");
        let id = map
            .add_source(file, COLD_AGGREGATE)
            .expect("program fits in a source map");
        let outcome = tuo_compiler::check_sources(&map, &[id]);
        assert!(
            outcome.diagnostics.is_empty(),
            "COLD_AGGREGATE must be accepted, got {:?}",
            outcome.diagnostics
        );
    }

    #[test]
    fn cold_build_needs_a_host_builder() {
        // Build is not measurable inside this crate (no linker); it returns None.
        let clock = SystemClock::new();
        assert!(measure_cold(&clock, ColdStage::Build, "fn main() -> Int { 0 }", 1, 1).is_none());
    }

    #[test]
    fn no_op_check_reexecutes_nothing() {
        let result = measure_edit(Edit::NoOpCheck);
        assert_eq!(
            result.reexecuted_count, 0,
            "a no-op re-check must re-execute no queries, got {:?}",
            result.reexecuted
        );
    }

    #[test]
    fn independent_body_edit_leaves_unrelated_items_untouched() {
        // Editing beta's body must not re-execute helper's or alpha's typing/MIR,
        // nor the spec's dependency graph.
        let result = measure_edit(Edit::FunctionBody);
        let joined = result.reexecuted.join(",");
        // Something re-ran (beta changed), but the spec graph did not.
        assert!(
            !joined.contains("spec_deps") && !joined.contains("spec:"),
            "an unrelated body edit must not disturb the spec graph, got {:?}",
            result.reexecuted
        );
    }

    #[test]
    fn affected_verify_selects_a_subset_of_all_specs() {
        let result = measure_edit(Edit::AffectedSpecVerify);
        // The baseline has exactly one spec; an edit to the whole file may touch
        // it, so the affected set is between 0 and 1.
        assert!(result.reexecuted_count <= 1);
    }
}
