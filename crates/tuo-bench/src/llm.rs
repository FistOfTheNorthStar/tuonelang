//! The LLM code-generation benchmark schema.
//!
//! This module defines the version-controlled, machine-readable shapes for
//! recording how reliably a model generates tuonelang. It **defines and
//! validates the schema and computes aggregates**; it does not run models. A
//! future harness (or an external agent runner) produces [`TaskAttempt`]
//! records; this crate gives them a stable structure and the summary metrics
//! Prompt 2 requires:
//!
//! - **Parse@1** — fraction of tasks whose first generation parses.
//! - **Check@1** — fraction whose first generation type/ownership-checks.
//! - **SpecPass@1** — fraction whose first generation passes its colocated
//!   specs.
//! - **Repair@1** — fraction that reach passing within one repair cycle.
//! - **average repair cycles** — mean repair iterations across tasks.
//! - **generated tokens** — total tokens the model emitted.
//! - **hallucinated symbol count** — references to symbols that do not exist.
//!
//! The `@1` metrics are "at first attempt"; the schema also stores the repair
//! trajectory so richer metrics can be derived later without a schema change.

use serde::{Deserialize, Serialize};

use crate::SCHEMA_VERSION;

/// A single benchmark *task* (an input): what the model is asked to produce.
///
/// A task is defined by a natural-language prompt and, in the spirit of TDG, the
/// colocated spec(s) the generated program must satisfy. Tasks are stored in
/// version-controlled files so a benchmark is reproducible and reviewable. This
/// is input data; the model's results are recorded as [`TaskAttempt`]s.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSpec {
    /// Stable task identifier, matching the `task_id` of its [`TaskAttempt`].
    pub task_id: String,
    /// Natural-language description of what to generate.
    pub prompt: String,
    /// The colocated spec(s) the generated program must satisfy, as tuonelang
    /// source or a description thereof. Stored verbatim for traceability.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub specs: Vec<String>,
    /// Optional free-form tags (difficulty, category).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

/// A version-controlled set of benchmark tasks (the input side of a benchmark).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSet {
    /// Output/schema version ([`crate::SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// Free-form description of the task set.
    pub description: String,
    /// The tasks.
    pub tasks: Vec<TaskSpec>,
}

impl TaskSet {
    /// Parse a task set from JSON.
    ///
    /// # Errors
    ///
    /// Returns the underlying [`serde_json::Error`] on malformed input.
    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }

    /// Serialize to pretty JSON.
    ///
    /// # Errors
    ///
    /// Returns the underlying [`serde_json::Error`] if serialization fails.
    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

/// The outcome of one compile/spec pipeline evaluation of a generated program.
///
/// Each stage is recorded independently so that partial progress (parses but
/// fails to check, checks but fails specs) is visible rather than collapsed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageOutcome {
    /// Whether the program parsed.
    pub parsed: bool,
    /// Whether the program passed type/ownership checking. Only meaningful when
    /// `parsed` is true.
    pub checked: bool,
    /// Whether the program's colocated specs all passed. Only meaningful when
    /// `checked` is true.
    pub specs_passed: bool,
    /// Count of referenced symbols that do not exist in scope (hallucinations)
    /// observed in this outcome.
    pub hallucinated_symbols: u32,
}

impl StageOutcome {
    /// A fully-passing outcome (parsed, checked, specs passed, no
    /// hallucinations).
    #[must_use]
    pub fn is_success(&self) -> bool {
        self.parsed && self.checked && self.specs_passed
    }
}

/// One repair iteration applied after a failing attempt: the model is shown the
/// diagnostics and produces a revised program, which is re-evaluated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepairStep {
    /// Tokens the model generated for this repair.
    pub generated_tokens: u64,
    /// The outcome after applying this repair.
    pub outcome: StageOutcome,
}

/// A single task attempted by a model: the first generation plus any repairs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskAttempt {
    /// Stable task identifier (e.g. `"sort-list"`).
    pub task_id: String,
    /// Tokens generated on the first (pre-repair) attempt.
    pub initial_generated_tokens: u64,
    /// The outcome of the first attempt — the basis for all `@1` metrics.
    pub initial_outcome: StageOutcome,
    /// Repair iterations applied after the first attempt, in order. Empty if the
    /// first attempt already succeeded or no repair was attempted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repairs: Vec<RepairStep>,
}

impl TaskAttempt {
    /// The number of repair cycles this task underwent.
    #[must_use]
    pub fn repair_cycles(&self) -> usize {
        self.repairs.len()
    }

    /// Whether the task ever reached a fully-passing outcome (first attempt or
    /// any repair).
    #[must_use]
    pub fn eventually_succeeded(&self) -> bool {
        self.initial_outcome.is_success() || self.repairs.iter().any(|r| r.outcome.is_success())
    }

    /// Whether the task passed within at most `n` repair cycles.
    #[must_use]
    pub fn passed_within_repairs(&self, n: usize) -> bool {
        if self.initial_outcome.is_success() {
            return true;
        }
        self.repairs.iter().take(n).any(|r| r.outcome.is_success())
    }

    /// Total tokens generated across the first attempt and all repairs.
    #[must_use]
    pub fn total_generated_tokens(&self) -> u64 {
        self.initial_generated_tokens + self.repairs.iter().map(|r| r.generated_tokens).sum::<u64>()
    }

    /// Total hallucinated symbols across the first attempt and all repairs.
    #[must_use]
    pub fn total_hallucinated_symbols(&self) -> u64 {
        u64::from(self.initial_outcome.hallucinated_symbols)
            + self
                .repairs
                .iter()
                .map(|r| u64::from(r.outcome.hallucinated_symbols))
                .sum::<u64>()
    }
}

/// The aggregate metrics Prompt 2 requires, computed from a set of attempts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchmarkSummary {
    /// Number of tasks summarized.
    pub task_count: usize,
    /// Fraction of tasks that parsed on the first attempt.
    pub parse_at_1: f64,
    /// Fraction that type/ownership-checked on the first attempt.
    pub check_at_1: f64,
    /// Fraction that passed all specs on the first attempt.
    pub spec_pass_at_1: f64,
    /// Fraction that passed within a single repair cycle.
    pub repair_at_1: f64,
    /// Mean number of repair cycles across all tasks.
    pub average_repair_cycles: f64,
    /// Total tokens generated across all tasks and repairs.
    pub generated_tokens: u64,
    /// Total hallucinated-symbol references across all tasks and repairs.
    pub hallucinated_symbol_count: u64,
}

/// A complete, version-controlled benchmark run: metadata, raw attempts, and
/// the computed summary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchmarkRun {
    /// Output schema version ([`crate::SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// Identifier for the model/config under test (free-form, recorded as data).
    pub model: String,
    /// The individual task attempts.
    pub attempts: Vec<TaskAttempt>,
    /// The aggregate summary derived from `attempts`.
    pub summary: BenchmarkSummary,
}

/// Fraction of `total` for which `predicate` holds; `0.0` when `total == 0`.
fn fraction<T>(items: &[T], predicate: impl Fn(&T) -> bool) -> f64 {
    if items.is_empty() {
        return 0.0;
    }
    let hits = items.iter().filter(|x| predicate(x)).count();
    hits as f64 / items.len() as f64
}

impl BenchmarkSummary {
    /// Compute the summary metrics from a slice of task attempts.
    #[must_use]
    pub fn from_attempts(attempts: &[TaskAttempt]) -> Self {
        let average_repair_cycles = if attempts.is_empty() {
            0.0
        } else {
            let total: usize = attempts.iter().map(TaskAttempt::repair_cycles).sum();
            total as f64 / attempts.len() as f64
        };

        Self {
            task_count: attempts.len(),
            parse_at_1: fraction(attempts, |a| a.initial_outcome.parsed),
            check_at_1: fraction(attempts, |a| a.initial_outcome.checked),
            spec_pass_at_1: fraction(attempts, |a| a.initial_outcome.specs_passed),
            repair_at_1: fraction(attempts, |a| a.passed_within_repairs(1)),
            average_repair_cycles,
            generated_tokens: attempts
                .iter()
                .map(TaskAttempt::total_generated_tokens)
                .sum(),
            hallucinated_symbol_count: attempts
                .iter()
                .map(TaskAttempt::total_hallucinated_symbols)
                .sum(),
        }
    }
}

impl BenchmarkRun {
    /// Build a run from a model label and its attempts, computing the summary.
    #[must_use]
    pub fn new(model: impl Into<String>, attempts: Vec<TaskAttempt>) -> Self {
        let summary = BenchmarkSummary::from_attempts(&attempts);
        Self {
            schema_version: SCHEMA_VERSION,
            model: model.into(),
            attempts,
            summary,
        }
    }

    /// Parse a benchmark run from JSON.
    ///
    /// # Errors
    ///
    /// Returns the underlying [`serde_json::Error`] on malformed input.
    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }

    /// Serialize to pretty JSON for a committed result file.
    ///
    /// # Errors
    ///
    /// Returns the underlying [`serde_json::Error`] if serialization fails.
    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn success() -> StageOutcome {
        StageOutcome {
            parsed: true,
            checked: true,
            specs_passed: true,
            hallucinated_symbols: 0,
        }
    }

    fn parse_fail() -> StageOutcome {
        StageOutcome {
            parsed: false,
            checked: false,
            specs_passed: false,
            hallucinated_symbols: 2,
        }
    }

    #[test]
    fn summary_over_empty_is_all_zero() {
        let s = BenchmarkSummary::from_attempts(&[]);
        assert_eq!(s.task_count, 0);
        assert_eq!(s.parse_at_1, 0.0);
        assert_eq!(s.average_repair_cycles, 0.0);
        assert_eq!(s.generated_tokens, 0);
    }

    #[test]
    fn metrics_reflect_attempts() {
        let attempts = vec![
            // Task A: perfect first attempt.
            TaskAttempt {
                task_id: "a".into(),
                initial_generated_tokens: 100,
                initial_outcome: success(),
                repairs: vec![],
            },
            // Task B: fails first (parse), fixed after one repair.
            TaskAttempt {
                task_id: "b".into(),
                initial_generated_tokens: 120,
                initial_outcome: parse_fail(),
                repairs: vec![RepairStep {
                    generated_tokens: 40,
                    outcome: success(),
                }],
            },
        ];
        let s = BenchmarkSummary::from_attempts(&attempts);

        assert_eq!(s.task_count, 2);
        assert!((s.parse_at_1 - 0.5).abs() < f64::EPSILON); // 1 of 2 parsed first
        assert!((s.check_at_1 - 0.5).abs() < f64::EPSILON);
        assert!((s.spec_pass_at_1 - 0.5).abs() < f64::EPSILON);
        assert!((s.repair_at_1 - 1.0).abs() < f64::EPSILON); // both pass within 1 repair
        assert!((s.average_repair_cycles - 0.5).abs() < f64::EPSILON); // (0 + 1)/2
        assert_eq!(s.generated_tokens, 100 + 120 + 40);
        assert_eq!(s.hallucinated_symbol_count, 2); // from B's failed first attempt
    }

    #[test]
    fn run_round_trips_through_json() {
        let run = BenchmarkRun::new(
            "example-model",
            vec![TaskAttempt {
                task_id: "a".into(),
                initial_generated_tokens: 10,
                initial_outcome: success(),
                repairs: vec![],
            }],
        );
        let json = run.to_json_pretty().unwrap();
        let parsed = BenchmarkRun::from_json(&json).unwrap();
        assert_eq!(run, parsed);
    }
}
