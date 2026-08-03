//! Aggregate metrics and the two report renderings.
//!
//! A [`BenchmarkSummary`] rolls the per-turn records of a [`BenchmarkRun`] up
//! into the metrics Prompt 36 requires — every one of them computed from what the
//! *compiler* actually reported, never asserted:
//!
//! - **Parse@1 / Check@1 / SpecPass@1 / TestPass@1** — fraction of task runs
//!   whose first generation parsed / checked / passed its specs / passed its
//!   held-out tests;
//! - **Repair@1** — fraction that fully passed within one repair turn;
//! - **average repair cycles** — mean repair turns across runs;
//! - **generated tokens** — total the models reported;
//! - **average feedback latency** — mean per-turn compiler-feedback latency;
//! - **invented symbols** — total undefined-name references across all turns;
//! - **unrelated edit rate** — fraction of *repair* turns that edited code the
//!   compiler had not flagged.
//!
//! Two renderings share this summary: [`BenchmarkSummary::to_json`] (the
//! machine-readable contract, versioned by [`crate::SCHEMA_VERSION`]) and
//! [`render_human`] (a reviewer-facing table). The two never disagree — the human
//! report is a projection of the same numbers.

use serde::{Deserialize, Serialize};

use crate::harness::{BenchmarkRun, TaskRun, TurnRecord};

/// The aggregate metrics for a benchmark run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchmarkSummary {
    /// The harness schema version ([`crate::SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// Number of (task, variant) runs summarized.
    pub run_count: usize,
    /// Fraction whose first generation parsed.
    pub parse_at_1: f64,
    /// Fraction whose first generation passed the whole front end.
    pub check_at_1: f64,
    /// Fraction whose first generation passed its specs.
    pub spec_pass_at_1: f64,
    /// Fraction whose held-out tests passed, over runs that *have* held-out
    /// tests and a checked final program. `0.0` when no run is testable.
    pub test_pass_at_1: f64,
    /// Number of runs that contributed to `test_pass_at_1` (had scorable tests).
    pub testable_runs: usize,
    /// Fraction that fully passed within one repair turn.
    pub repair_at_1: f64,
    /// Mean number of repair turns across runs.
    pub average_repair_cycles: f64,
    /// Total tokens the models reported generating across every turn.
    pub generated_tokens: u64,
    /// Mean per-turn compiler-feedback latency, in milliseconds.
    pub average_feedback_latency_ms: f64,
    /// Total invented-symbol (undefined-name) references across every turn.
    pub invented_symbols: u64,
    /// Fraction of *repair* turns that edited code the compiler had not flagged.
    /// `0.0` when there were no repair turns.
    pub unrelated_edit_rate: f64,
}

impl BenchmarkSummary {
    /// Compute the summary over a benchmark run's task runs.
    #[must_use]
    pub fn from_run(run: &BenchmarkRun) -> Self {
        Self::from_runs(&run.runs)
    }

    /// Compute the summary over a slice of task runs.
    #[must_use]
    pub fn from_runs(runs: &[TaskRun]) -> Self {
        let run_count = runs.len();

        let parse_at_1 = fraction(runs, |r| initial(r).is_some_and(|t| t.parsed));
        let check_at_1 = fraction(runs, |r| initial(r).is_some_and(|t| t.checked));
        let spec_pass_at_1 = fraction(runs, |r| initial(r).is_some_and(|t| t.specs_passed));

        // TestPass@1 is scored only over runs that actually have a test verdict.
        let testable: Vec<&TaskRun> = runs.iter().filter(|r| r.tests_passed.is_some()).collect();
        let test_pass_at_1 = if testable.is_empty() {
            0.0
        } else {
            let hits = testable
                .iter()
                .filter(|r| r.tests_passed == Some(true))
                .count();
            hits as f64 / testable.len() as f64
        };

        let repair_at_1 = fraction(runs, |r| r.passed_within_repairs(1));

        let average_repair_cycles = mean(runs.iter().map(|r| r.repair_cycles() as f64));

        let all_turns: Vec<&TurnRecord> = runs.iter().flat_map(|r| r.turns.iter()).collect();
        let generated_tokens = all_turns.iter().map(|t| t.generated_tokens).sum();
        let average_feedback_latency_ms =
            mean(all_turns.iter().map(|t| t.feedback_latency_ms as f64));
        let invented_symbols = all_turns
            .iter()
            .map(|t| u64::from(t.invented_symbols))
            .sum();

        // Unrelated-edit rate is over *repair* turns (those carrying the signal).
        let repair_turns: Vec<&TurnRecord> = all_turns
            .iter()
            .copied()
            .filter(|t| t.unrelated_edit.is_some())
            .collect();
        let unrelated_edit_rate = if repair_turns.is_empty() {
            0.0
        } else {
            let hits = repair_turns
                .iter()
                .filter(|t| t.unrelated_edit == Some(true))
                .count();
            hits as f64 / repair_turns.len() as f64
        };

        Self {
            schema_version: crate::SCHEMA_VERSION,
            run_count,
            parse_at_1,
            check_at_1,
            spec_pass_at_1,
            test_pass_at_1,
            testable_runs: testable.len(),
            repair_at_1,
            average_repair_cycles,
            generated_tokens,
            average_feedback_latency_ms,
            invented_symbols,
            unrelated_edit_rate,
        }
    }

    /// Serialize the summary to pretty JSON (the machine-readable report).
    ///
    /// # Errors
    ///
    /// Returns the underlying [`serde_json::Error`] if serialization fails.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

/// The initial turn of a run.
fn initial(run: &TaskRun) -> Option<&TurnRecord> {
    run.turns.first()
}

/// Fraction of `items` for which `predicate` holds; `0.0` when empty.
fn fraction<T>(items: &[T], predicate: impl Fn(&T) -> bool) -> f64 {
    if items.is_empty() {
        return 0.0;
    }
    let hits = items.iter().filter(|x| predicate(x)).count();
    hits as f64 / items.len() as f64
}

/// Arithmetic mean of an iterator; `0.0` when empty.
fn mean(values: impl Iterator<Item = f64>) -> f64 {
    let mut sum = 0.0;
    let mut count = 0usize;
    for v in values {
        sum += v;
        count += 1;
    }
    if count == 0 { 0.0 } else { sum / count as f64 }
}

/// Render a human-readable report for a run and its summary.
///
/// This is a projection of the same numbers [`BenchmarkSummary`] holds — it never
/// computes a metric the machine report does not.
#[must_use]
pub fn render_human(run: &BenchmarkRun, summary: &BenchmarkSummary) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "TDG code-generation benchmark — model `{}`\n",
        run.model.id
    ));
    out.push_str(&format!(
        "  compiler {}, language {}, {} run(s)\n",
        run.compiler_version, run.language_version, summary.run_count
    ));
    out.push_str(&format!("  task set digest: {}\n", run.task_set_digest));
    out.push('\n');

    out.push_str("  metric              value\n");
    out.push_str("  ------------------  -----\n");
    push_pct(&mut out, "Parse@1", summary.parse_at_1);
    push_pct(&mut out, "Check@1", summary.check_at_1);
    push_pct(&mut out, "SpecPass@1", summary.spec_pass_at_1);
    push_pct_or_na(
        &mut out,
        "TestPass@1",
        summary.test_pass_at_1,
        summary.testable_runs > 0,
    );
    push_pct(&mut out, "Repair@1", summary.repair_at_1);
    out.push_str(&format!(
        "  {:<18}  {:.2}\n",
        "avg repair cycles", summary.average_repair_cycles
    ));
    out.push_str(&format!(
        "  {:<18}  {}\n",
        "generated tokens", summary.generated_tokens
    ));
    out.push_str(&format!(
        "  {:<18}  {:.1} ms\n",
        "avg feedback lat.", summary.average_feedback_latency_ms
    ));
    out.push_str(&format!(
        "  {:<18}  {}\n",
        "invented symbols", summary.invented_symbols
    ));
    push_pct(&mut out, "unrelated edits", summary.unrelated_edit_rate);
    out
}

/// Append a percentage-formatted metric row.
fn push_pct(out: &mut String, name: &str, value: f64) {
    out.push_str(&format!("  {name:<18}  {:.1}%\n", value * 100.0));
}

/// Append a percentage row, or `n/a` when the metric does not apply.
fn push_pct_or_na(out: &mut String, name: &str, value: f64, applicable: bool) {
    if applicable {
        push_pct(out, name, value);
    } else {
        out.push_str(&format!("  {name:<18}  n/a\n"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::TurnRecord;
    use crate::model::ModelConfig;

    fn turn(
        turn: usize,
        parsed: bool,
        checked: bool,
        specs: bool,
        invented: u32,
        unrelated: Option<bool>,
    ) -> TurnRecord {
        TurnRecord {
            turn,
            prompt: "p".into(),
            output: "o".into(),
            generated_tokens: 100,
            parsed,
            checked,
            specs_passed: specs,
            invented_symbols: invented,
            feedback_latency_ms: 5,
            unrelated_edit: unrelated,
        }
    }

    #[test]
    fn empty_summary_is_all_zero() {
        let s = BenchmarkSummary::from_runs(&[]);
        assert_eq!(s.run_count, 0);
        assert_eq!(s.parse_at_1, 0.0);
        assert_eq!(s.generated_tokens, 0);
        assert_eq!(s.unrelated_edit_rate, 0.0);
    }

    #[test]
    fn metrics_reflect_runs() {
        let runs = vec![
            // Task A: perfect first attempt, held-out tests pass.
            TaskRun {
                task_id: "a".into(),
                variant: "default".into(),
                turns: vec![turn(0, true, true, true, 0, None)],
                tests_passed: Some(true),
                generation_error: None,
            },
            // Task B: parse fail first, one repair (touching only flagged lines)
            // reaches passing; no held-out tests.
            TaskRun {
                task_id: "b".into(),
                variant: "default".into(),
                turns: vec![
                    turn(0, false, false, false, 1, None),
                    turn(1, true, true, true, 0, Some(false)),
                ],
                tests_passed: None,
                generation_error: None,
            },
        ];
        let s = BenchmarkSummary::from_runs(&runs);
        assert_eq!(s.run_count, 2);
        assert!((s.parse_at_1 - 0.5).abs() < f64::EPSILON);
        assert!((s.check_at_1 - 0.5).abs() < f64::EPSILON);
        assert!((s.spec_pass_at_1 - 0.5).abs() < f64::EPSILON);
        assert!((s.repair_at_1 - 1.0).abs() < f64::EPSILON);
        assert!((s.average_repair_cycles - 0.5).abs() < f64::EPSILON);
        assert_eq!(s.generated_tokens, 300); // 100 + 100 + 100
        assert_eq!(s.invented_symbols, 1);
        assert_eq!(s.testable_runs, 1);
        assert!((s.test_pass_at_1 - 1.0).abs() < f64::EPSILON);
        // One repair turn, not unrelated.
        assert!((s.unrelated_edit_rate - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn human_report_projects_the_summary() {
        let runs = vec![TaskRun {
            task_id: "a".into(),
            variant: "default".into(),
            turns: vec![turn(0, true, true, true, 0, None)],
            tests_passed: None,
            generation_error: None,
        }];
        let run = BenchmarkRun::new(ModelConfig::new("m"), "digest123", runs);
        let summary = BenchmarkSummary::from_run(&run);
        let text = render_human(&run, &summary);
        assert!(text.contains("Parse@1"));
        assert!(text.contains("TestPass@1"));
        assert!(text.contains("n/a")); // no testable runs
        assert!(text.contains("digest123"));
    }
}
