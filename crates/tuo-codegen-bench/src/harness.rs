//! The evaluation loop and the provenance it keeps.
//!
//! [`run_task`] drives one task through a [`ModelAdapter`]: it asks the model for
//! an initial generation, compiles it (via [`crate::evaluate`]), and — while the
//! program still fails and a repair budget remains — feeds the compiler's
//! diagnostics back to the model and re-compiles the revised program. Every turn
//! is recorded, so the resulting [`TaskRun`] is a complete, replayable account of
//! what the model produced and what the compiler said about it.
//!
//! The harness keeps everything a reviewer needs to trust and reproduce a result:
//! the exact **prompts** (in each [`TurnRecord`]), the **model configuration**
//! (in the [`BenchmarkRun`]), the **compiler and language versions**, the model's
//! **outputs** (the generated source of every turn), and the compiler's
//! **results** (per-turn evaluation). Nothing is summarized away.

use serde::{Deserialize, Serialize};
use tuo_spec::Limits;

use crate::evaluate::{Evaluation, evaluate, evaluate_tests};
use crate::model::{GenerationError, ModelAdapter, ModelConfig, Prompt};
use crate::task::{BenchTask, SyntaxVariant};

/// The compiler version a run was produced against (this workspace's version).
#[must_use]
pub fn compiler_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// The language/edition version a run targets (v0 has one edition).
#[must_use]
pub fn language_version() -> String {
    "2024".to_string()
}

/// How much repair to allow and how to bound spec execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunConfig {
    /// Maximum number of repair turns after the initial generation. `0` scores
    /// only the first attempt (all `@1` metrics still make sense).
    pub max_repairs: usize,
    /// The spec sandbox limits.
    pub limits: Limits,
}

impl Default for RunConfig {
    fn default() -> Self {
        Self {
            max_repairs: 2,
            limits: Limits::default(),
        }
    }
}

/// One turn of a task: the prompt sent, the model's output, and the compiler's
/// evaluation of it. Repair turns additionally record the unrelated-edit signal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnRecord {
    /// `0` for the initial generation, `1..` for repair turns.
    pub turn: usize,
    /// The instruction the model was given this turn (verbatim, for
    /// reproducibility). Repair turns include the appended diagnostics.
    pub prompt: String,
    /// The tuonelang source the model produced this turn.
    pub output: String,
    /// The token count the model reported for this turn.
    pub generated_tokens: u64,
    /// Whether the program parsed.
    pub parsed: bool,
    /// Whether the program passed the whole front end.
    pub checked: bool,
    /// Whether the colocated specs all passed.
    pub specs_passed: bool,
    /// Invented-symbol (undefined-name) count the compiler reported.
    pub invented_symbols: u32,
    /// Wall-clock latency of the compiler feedback for this turn, in
    /// milliseconds (measured, not promised).
    pub feedback_latency_ms: u64,
    /// For a repair turn: whether this repair edited lines the previous turn's
    /// diagnostics did **not** flag (an "unrelated edit"). `None` on the initial
    /// turn, which has no prior feedback to be related to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unrelated_edit: Option<bool>,
}

/// The result of one (task, variant) evaluation: the ordered turns, the held-out
/// test verdict, and a note on any generation failure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskRun {
    /// The task id.
    pub task_id: String,
    /// The syntax variant evaluated, or `"default"` for the task's default
    /// spelling.
    pub variant: String,
    /// The turns, in order (initial generation then repairs).
    pub turns: Vec<TurnRecord>,
    /// Whether the task's held-out tests passed on the final accepted program.
    /// `None` when the task defines no held-out tests, or the program never
    /// checked (so tests could not be scored).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tests_passed: Option<bool>,
    /// A generation failure that ended the run early, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_error: Option<GenerationError>,
}

impl TaskRun {
    /// The initial turn, if the model produced one.
    #[must_use]
    pub fn initial(&self) -> Option<&TurnRecord> {
        self.turns.first()
    }

    /// The final turn (the last generation produced).
    #[must_use]
    pub fn final_turn(&self) -> Option<&TurnRecord> {
        self.turns.last()
    }

    /// The number of repair turns (turns after the initial generation).
    #[must_use]
    pub fn repair_cycles(&self) -> usize {
        self.turns.len().saturating_sub(1)
    }

    /// Whether any turn reached a fully-passing evaluation.
    #[must_use]
    pub fn eventually_succeeded(&self) -> bool {
        self.turns
            .iter()
            .any(|t| t.parsed && t.checked && t.specs_passed)
    }

    /// Whether a fully-passing evaluation was reached within `n` repair turns.
    #[must_use]
    pub fn passed_within_repairs(&self, n: usize) -> bool {
        self.turns
            .iter()
            .take(n + 1)
            .any(|t| t.parsed && t.checked && t.specs_passed)
    }
}

/// Drive one (task, variant) through `model`, recording every turn.
///
/// The loop stops when the program fully passes, the repair budget is exhausted,
/// or the model fails to generate. Held-out tests are scored against the final
/// checked program.
#[must_use]
pub fn run_task(
    model: &dyn ModelAdapter,
    task: &BenchTask,
    variant: Option<&SyntaxVariant>,
    config: RunConfig,
) -> TaskRun {
    let variant_label = variant.map_or("default", |v| v.label.as_str());
    let specs = task.specs_for(variant);
    let mut turns: Vec<TurnRecord> = Vec::new();
    let mut previous: Option<(String, Evaluation)> = None;
    let mut generation_error = None;

    let total_turns = config.max_repairs + 1;
    for turn in 0..total_turns {
        // Build the prompt. The initial turn carries only the instruction; a
        // repair turn also carries the prior output and the compiler's feedback.
        let feedback: Vec<String> = previous
            .as_ref()
            .map(|(_, e)| e.feedback.clone())
            .unwrap_or_default();
        let previous_source = previous.as_ref().map(|(src, _)| src.clone());
        let prompt = Prompt {
            instruction: &task.instruction,
            variant: variant.map(|v| v.label.as_str()),
            previous_source: previous_source.as_deref(),
            feedback: &feedback,
        };

        let generation = match model.generate(&prompt) {
            Ok(g) => g,
            Err(err) => {
                generation_error = Some(err);
                break;
            }
        };

        let evaluation = evaluate(&generation.source, specs, config.limits);

        // The unrelated-edit signal compares this repair against the previous
        // turn's flagged lines.
        let unrelated_edit = previous.as_ref().map(|(prev_src, prev_eval)| {
            is_unrelated_edit(prev_src, &generation.source, prev_eval)
        });

        turns.push(TurnRecord {
            turn,
            prompt: render_prompt(&prompt),
            output: generation.source.clone(),
            generated_tokens: generation.generated_tokens,
            parsed: evaluation.parsed,
            checked: evaluation.checked,
            specs_passed: evaluation.specs_passed,
            invented_symbols: evaluation.invented_symbols,
            feedback_latency_ms: duration_ms(evaluation.latency),
            unrelated_edit,
        });

        if evaluation.is_success() {
            previous = Some((generation.source, evaluation));
            break;
        }
        previous = Some((generation.source, evaluation));
    }

    // Score held-out tests against the final checked program, if there are any.
    let tests_passed = score_tests(task, &previous, config.limits);

    TaskRun {
        task_id: task.id.clone(),
        variant: variant_label.to_string(),
        turns,
        tests_passed,
        generation_error,
    }
}

/// Score the task's held-out tests against the final generation, when the task
/// has tests and the final program checked (an unchecked program cannot have its
/// tests meaningfully run).
fn score_tests(
    task: &BenchTask,
    final_state: &Option<(String, Evaluation)>,
    limits: Limits,
) -> Option<bool> {
    if task.tests.is_empty() {
        return None;
    }
    let (source, eval) = final_state.as_ref()?;
    if !eval.checked {
        // The program never checked, so held-out tests could not be scored.
        return None;
    }
    Some(evaluate_tests(source, &task.tests, limits))
}

/// Whether a repair edited a line the previous turn's diagnostics did not flag.
///
/// The measure is deliberately simple and honest: compare the two generations
/// line by line; an edit is *unrelated* when a line that changed (or was added)
/// is outside the set of lines the compiler pointed an error at last turn. When
/// the previous turn flagged no lines at all (e.g. a spec-only failure), any edit
/// is unrelated by definition, which is the conservative reading.
fn is_unrelated_edit(previous: &str, current: &str, previous_eval: &Evaluation) -> bool {
    let prev_lines: Vec<&str> = previous.lines().collect();
    let curr_lines: Vec<&str> = current.lines().collect();
    let flagged = &previous_eval.error_lines;

    let max_len = prev_lines.len().max(curr_lines.len());
    for i in 0..max_len {
        let before = prev_lines.get(i);
        let after = curr_lines.get(i);
        if before != after {
            // 1-based line number of the change.
            let line = u32::try_from(i + 1).unwrap_or(u32::MAX);
            if !flagged.contains(&line) {
                return true;
            }
        }
    }
    false
}

/// Render a prompt to the exact text the model was shown, for the turn record.
fn render_prompt(prompt: &Prompt<'_>) -> String {
    let mut out = String::new();
    out.push_str(prompt.instruction);
    if let Some(variant) = prompt.variant {
        out.push_str(&format!("\n\n[variant: {variant}]"));
    }
    if let Some(previous) = prompt.previous_source {
        out.push_str("\n\n[previous attempt]\n");
        out.push_str(previous);
    }
    if !prompt.feedback.is_empty() {
        out.push_str("\n\n[compiler feedback]\n");
        out.push_str(&prompt.feedback.join("\n"));
    }
    out
}

/// A duration in whole milliseconds, saturating.
fn duration_ms(d: std::time::Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// Re-score a recorded run's outputs by compiling them again — a *verification*
/// of the recorded metrics rather than a fresh generation.
///
/// A [`BenchmarkRun`] keeps the model's outputs (every turn's generated source),
/// so a host that also has the pinned [`crate::TaskSet`] can prove the recorded
/// per-turn verdicts: this recompiles each turn's output against its task's specs
/// (and re-scores held-out tests) and returns fresh [`TaskRun`]s. Because the
/// compiler is deterministic on a fixed input, the re-scored verdicts must match
/// the recorded ones for a run that was not fabricated. `tasks` maps a task id to
/// its task definition (from the verified task set).
///
/// Turns whose task id is not in `tasks` are returned unchanged (the caller
/// cannot re-score what it does not have the specs for).
#[must_use]
pub fn rescore(run: &BenchmarkRun, tasks: &[&BenchTask], limits: Limits) -> Vec<TaskRun> {
    run.runs
        .iter()
        .map(|task_run| rescore_one(task_run, tasks, limits))
        .collect()
}

/// Re-score one recorded task run.
fn rescore_one(task_run: &TaskRun, tasks: &[&BenchTask], limits: Limits) -> TaskRun {
    let Some(task) = tasks.iter().find(|t| t.id == task_run.task_id) else {
        // No task definition available: return the record unchanged.
        return task_run.clone();
    };
    // Resolve the variant by label, if this run targeted one.
    let variant = task.variants.iter().find(|v| v.label == task_run.variant);
    let specs = task.specs_for(variant);

    let mut turns = Vec::with_capacity(task_run.turns.len());
    let mut previous: Option<(String, Evaluation)> = None;
    let mut final_state: Option<(String, Evaluation)> = None;
    for record in &task_run.turns {
        let evaluation = evaluate(&record.output, specs, limits);
        let unrelated_edit = previous
            .as_ref()
            .map(|(prev_src, prev_eval)| is_unrelated_edit(prev_src, &record.output, prev_eval));
        turns.push(TurnRecord {
            turn: record.turn,
            prompt: record.prompt.clone(),
            output: record.output.clone(),
            // Generated tokens are the model's own accounting, not recomputed.
            generated_tokens: record.generated_tokens,
            parsed: evaluation.parsed,
            checked: evaluation.checked,
            specs_passed: evaluation.specs_passed,
            invented_symbols: evaluation.invented_symbols,
            feedback_latency_ms: duration_ms(evaluation.latency),
            unrelated_edit,
        });
        previous = Some((record.output.clone(), evaluation.clone()));
        final_state = Some((record.output.clone(), evaluation));
    }

    let tests_passed = score_tests(task, &final_state, limits);

    TaskRun {
        task_id: task_run.task_id.clone(),
        variant: task_run.variant.clone(),
        turns,
        tests_passed,
        generation_error: task_run.generation_error.clone(),
    }
}

/// A complete benchmark run: the model under test, the provenance versions, and
/// every task run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchmarkRun {
    /// The harness schema version ([`crate::SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// The model configuration under test (provenance).
    pub model: ModelConfig,
    /// The compiler version this run was produced against (provenance).
    pub compiler_version: String,
    /// The language/edition version this run targets (provenance).
    pub language_version: String,
    /// The digest of the task set this run evaluated, so a result names exactly
    /// which (pinned) benchmark produced it.
    pub task_set_digest: String,
    /// Every (task, variant) run.
    pub runs: Vec<TaskRun>,
}

impl BenchmarkRun {
    /// Assemble a run from its model config, the pinned task-set digest, and the
    /// per-task results, stamping the compiler/language versions.
    #[must_use]
    pub fn new(model: ModelConfig, task_set_digest: impl Into<String>, runs: Vec<TaskRun>) -> Self {
        Self {
            schema_version: crate::SCHEMA_VERSION,
            model,
            compiler_version: compiler_version(),
            language_version: language_version(),
            task_set_digest: task_set_digest.into(),
            runs,
        }
    }

    /// Parse a run from JSON.
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
