//! End-to-end test of the evaluation harness over a *real* deterministic model
//! adapter, driving the *real* compiler.
//!
//! No LLM is embedded. Instead, a scripted [`ScriptedModel`] plays back a fixed
//! sequence of generations per task — including a first attempt that fails and a
//! repair that fixes it — so the harness's loop, metrics, and provenance can be
//! exercised deterministically against the actual pipeline. Every `@1` metric
//! asserted here was produced by compiling the scripted output.

use std::cell::RefCell;
use std::collections::HashMap;

use tuo_codegen_bench::{
    BenchTask, BenchmarkRun, BenchmarkSummary, Generation, GenerationError, ModelAdapter,
    ModelConfig, Prompt, RunConfig, SyntaxVariant, TaskSet, render_human, run_task,
};

/// A model that replays a scripted list of generations for each task id, in
/// turn order. Deterministic, offline, and never panics.
struct ScriptedModel {
    id: String,
    /// task_id → the ordered generations to return, one per turn.
    scripts: HashMap<String, Vec<Generation>>,
    /// Per-task turn cursor.
    cursor: RefCell<HashMap<String, usize>>,
    /// The task id currently being run (set before each `run_task`).
    current: RefCell<String>,
}

impl ScriptedModel {
    fn new(id: &str, scripts: HashMap<String, Vec<Generation>>) -> Self {
        Self {
            id: id.to_string(),
            scripts,
            cursor: RefCell::new(HashMap::new()),
            current: RefCell::new(String::new()),
        }
    }

    fn begin(&self, task_id: &str) {
        *self.current.borrow_mut() = task_id.to_string();
        self.cursor.borrow_mut().insert(task_id.to_string(), 0);
    }
}

impl ModelAdapter for ScriptedModel {
    fn config(&self) -> ModelConfig {
        ModelConfig::new(&self.id)
    }

    fn generate(&self, _prompt: &Prompt<'_>) -> Result<Generation, GenerationError> {
        let task_id = self.current.borrow().clone();
        let script = self
            .scripts
            .get(&task_id)
            .ok_or_else(|| GenerationError::new(format!("no script for `{task_id}`")))?;
        let mut cursor = self.cursor.borrow_mut();
        let idx = cursor.entry(task_id.clone()).or_insert(0);
        let produced = script
            .get(*idx)
            .cloned()
            .ok_or_else(|| GenerationError::new("script exhausted"))?;
        *idx += 1;
        Ok(produced)
    }
}

const GOOD_DOUBLE: &str = "fn double(take x: Int) -> Int {\n    x + x\n}\n";
const PARSE_BROKEN: &str = "fn double(take x: Int) -> Int {\n    x +\n}\n";

fn double_task() -> BenchTask {
    BenchTask {
        id: "double".into(),
        instruction: "Write `double`.".into(),
        specs: vec!["spec double {\n    then double(3) == 6;\n}\n".into()],
        tests: vec!["spec double {\n    then double(10) == 20;\n}\n".into()],
        variants: vec![],
        tags: vec![],
    }
}

#[test]
fn a_first_attempt_success_scores_every_at_1_metric() {
    let mut scripts = HashMap::new();
    scripts.insert("double".to_string(), vec![Generation::new(GOOD_DOUBLE, 42)]);
    let model = ScriptedModel::new("scripted", scripts);
    model.begin("double");

    let run = run_task(&model, &double_task(), None, RunConfig::default());
    assert_eq!(run.turns.len(), 1);
    let t = &run.turns[0];
    assert!(t.parsed && t.checked && t.specs_passed);
    assert_eq!(t.generated_tokens, 42);
    assert_eq!(run.repair_cycles(), 0);
    // Held-out tests pass on the correct program.
    assert_eq!(run.tests_passed, Some(true));

    let summary = BenchmarkSummary::from_runs(std::slice::from_ref(&run));
    assert!((summary.parse_at_1 - 1.0).abs() < f64::EPSILON);
    assert!((summary.check_at_1 - 1.0).abs() < f64::EPSILON);
    assert!((summary.spec_pass_at_1 - 1.0).abs() < f64::EPSILON);
    assert!((summary.test_pass_at_1 - 1.0).abs() < f64::EPSILON);
    assert_eq!(summary.generated_tokens, 42);
    assert_eq!(summary.invented_symbols, 0);
}

#[test]
fn a_parse_failure_then_repair_is_measured_end_to_end() {
    let mut scripts = HashMap::new();
    // First attempt fails at parse; the repair fixes it.
    scripts.insert(
        "double".to_string(),
        vec![
            Generation::new(PARSE_BROKEN, 30),
            Generation::new(GOOD_DOUBLE, 20),
        ],
    );
    let model = ScriptedModel::new("scripted", scripts);
    model.begin("double");

    let run = run_task(&model, &double_task(), None, RunConfig::default());
    assert_eq!(run.turns.len(), 2, "one initial + one repair");
    assert!(!run.turns[0].parsed, "first attempt did not parse");
    assert!(run.turns[1].parsed && run.turns[1].checked && run.turns[1].specs_passed);
    assert!(run.eventually_succeeded());
    assert!(run.passed_within_repairs(1));
    assert_eq!(run.repair_cycles(), 1);

    // The repair turn carries an unrelated-edit signal (it rewrote the body line,
    // which the parse error flagged, so it is a *related* edit here).
    assert!(run.turns[1].unrelated_edit.is_some());

    let summary = BenchmarkSummary::from_runs(std::slice::from_ref(&run));
    assert!((summary.parse_at_1 - 0.0).abs() < f64::EPSILON); // first attempt failed
    assert!((summary.repair_at_1 - 1.0).abs() < f64::EPSILON); // passed within 1 repair
    assert!((summary.average_repair_cycles - 1.0).abs() < f64::EPSILON);
    assert_eq!(summary.generated_tokens, 50);
}

#[test]
fn held_out_tests_can_disagree_with_shown_specs() {
    // A generation that satisfies the shown spec `double(3) == 6`? No — the wrong
    // impl `x + 1` gives 4, failing even the shown spec. Use a task whose shown
    // spec the wrong impl passes but whose held-out test it fails.
    let task = BenchTask {
        id: "id".into(),
        instruction: "Return the input.".into(),
        // Shown spec only checks the zero case, which `x + 0`-like bugs pass.
        specs: vec!["spec identity {\n    then identity(0) == 0;\n}\n".into()],
        // Held-out test checks a non-zero case.
        tests: vec!["spec identity {\n    then identity(5) == 5;\n}\n".into()],
        variants: vec![],
        tags: vec![],
    };
    // Buggy: always returns 0. Passes the shown spec, fails the held-out test.
    let buggy = "fn identity(take x: Int) -> Int {\n    x - x\n}\n";
    let mut scripts = HashMap::new();
    scripts.insert("id".to_string(), vec![Generation::new(buggy, 15)]);
    let model = ScriptedModel::new("scripted", scripts);
    model.begin("id");

    let run = run_task(
        &model,
        &task,
        None,
        RunConfig {
            max_repairs: 0,
            ..RunConfig::default()
        },
    );
    assert!(
        run.turns[0].specs_passed,
        "shown spec (identity(0)==0) passes"
    );
    assert_eq!(
        run.tests_passed,
        Some(false),
        "held-out test (identity(5)==5) fails on the buggy program"
    );
}

#[test]
fn a_generation_error_ends_the_run_and_is_recorded() {
    // No script entry → the adapter errors on the first generate.
    let model = ScriptedModel::new("scripted", HashMap::new());
    model.begin("missing");
    let task = BenchTask {
        id: "missing".into(),
        instruction: "x".into(),
        specs: vec![],
        tests: vec![],
        variants: vec![],
        tags: vec![],
    };
    let run = run_task(&model, &task, None, RunConfig::default());
    assert!(run.turns.is_empty());
    assert!(run.generation_error.is_some());
}

#[test]
fn syntax_variants_are_evaluated_and_labeled() {
    // Two spellings of the same task; both compile. The run records which variant
    // it evaluated, so their metrics are comparable.
    let task = BenchTask {
        id: "double".into(),
        instruction: "Write double.".into(),
        specs: vec!["spec double {\n    then double(2) == 4;\n}\n".into()],
        tests: vec![],
        variants: vec![
            SyntaxVariant {
                label: "add".into(),
                note: "x + x".into(),
                specs: vec![],
            },
            SyntaxVariant {
                label: "mul".into(),
                note: "x * 2".into(),
                specs: vec![],
            },
        ],
        tags: vec![],
    };
    let add_impl = "fn double(take x: Int) -> Int {\n    x + x\n}\n";
    let mul_impl = "fn double(take x: Int) -> Int {\n    x * 2\n}\n";

    let mut scripts = HashMap::new();
    scripts.insert("double".to_string(), vec![Generation::new(add_impl, 10)]);
    let add_model = ScriptedModel::new("scripted", scripts);
    add_model.begin("double");
    let add_run = run_task(
        &add_model,
        &task,
        Some(&task.variants[0]),
        RunConfig::default(),
    );
    assert_eq!(add_run.variant, "add");
    assert!(add_run.turns[0].specs_passed);

    let mut scripts = HashMap::new();
    scripts.insert("double".to_string(), vec![Generation::new(mul_impl, 12)]);
    let mul_model = ScriptedModel::new("scripted", scripts);
    mul_model.begin("double");
    let mul_run = run_task(
        &mul_model,
        &task,
        Some(&task.variants[1]),
        RunConfig::default(),
    );
    assert_eq!(mul_run.variant, "mul");
    assert!(mul_run.turns[0].specs_passed);
}

#[test]
fn a_benchmark_run_keeps_full_provenance_and_reports_both_ways() {
    let mut scripts = HashMap::new();
    scripts.insert("double".to_string(), vec![Generation::new(GOOD_DOUBLE, 42)]);
    let model = ScriptedModel::new("scripted-v1", scripts);
    model.begin("double");

    let task = double_task();
    let set = TaskSet::pinned("demo", vec![task.clone()]);
    let run = run_task(&model, &task, None, RunConfig::default());

    let bench = BenchmarkRun::new(model.config(), set.tasks[0].digest.clone(), vec![run]);
    // Provenance is present.
    assert_eq!(bench.model.id, "scripted-v1");
    assert!(!bench.compiler_version.is_empty());
    assert_eq!(bench.language_version, "2024");
    assert_eq!(bench.task_set_digest, set.tasks[0].digest);
    // The raw run keeps outputs (the generated source of the turn).
    assert_eq!(bench.runs[0].turns[0].output, GOOD_DOUBLE);

    // Both reports come from the same summary.
    let summary = BenchmarkSummary::from_run(&bench);
    let machine = summary.to_json().unwrap();
    assert!(machine.contains("\"parse_at_1\""));
    let human = render_human(&bench, &summary);
    assert!(human.contains("scripted-v1"));
    assert!(human.contains("Parse@1"));

    // The whole run round-trips through JSON (a committed result file).
    let json = bench.to_json_pretty().unwrap();
    let parsed = BenchmarkRun::from_json(&json).unwrap();
    assert_eq!(bench, parsed);
}
