//! End-to-end tests for `tuo bench report`, driven through the real `tuo`
//! binary.
//!
//! The code-generation evaluation harness embeds no LLM, so the CLI's job is to
//! *score* a recorded run by recompiling the model's outputs. These tests build a
//! pinned task set and a recorded run with the real harness types, write them to
//! disk, and drive the binary to score them — proving that the reported metrics
//! come from actually recompiling the recorded generations, and that a
//! silently-edited task set is refused.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use tuo_codegen_bench::{BenchTask, BenchmarkRun, ModelConfig, TaskRun, TaskSet, TurnRecord};

/// A unique scratch directory per test.
fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("bench_command")
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch dir is creatable");
    dir
}

/// Write `text` to `dir/name` and return the path.
fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, text).expect("write file");
    path
}

/// Run `tuo` with `args`.
fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tuo"))
        .args(args)
        .output()
        .expect("the tuo binary runs")
}

/// Parse the single machine-protocol `item` event out of JSON-lines stdout.
fn item_payload(stdout: &str) -> Value {
    for line in stdout.lines() {
        let value: Value = serde_json::from_str(line).expect("each line is JSON");
        if value["event"] == "item" {
            return value;
        }
    }
    panic!("no item event in output:\n{stdout}");
}

const GOOD_DOUBLE: &str = "fn double(take x: Int) -> Int {\n    x + x\n}\n";

/// One task with a shown spec and a held-out test.
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

/// A recorded run whose one output is the correct program.
fn correct_run(task_set_digest: &str) -> BenchmarkRun {
    let task_run = TaskRun {
        task_id: "double".into(),
        variant: "default".into(),
        turns: vec![TurnRecord {
            turn: 0,
            prompt: "Write `double`.".into(),
            output: GOOD_DOUBLE.into(),
            generated_tokens: 42,
            // These recorded verdicts are what the command RE-PROVES by
            // recompiling; the CLI does not trust them.
            parsed: true,
            checked: true,
            specs_passed: true,
            invented_symbols: 0,
            feedback_latency_ms: 1,
            unrelated_edit: None,
        }],
        tests_passed: Some(true),
        generation_error: None,
    };
    BenchmarkRun::new(
        ModelConfig::new("scripted-v1"),
        task_set_digest,
        vec![task_run],
    )
}

#[test]
fn report_rescoring_a_correct_run_reports_all_metrics_passing() {
    let dir = scratch("correct");
    let set = TaskSet::pinned("demo", vec![double_task()]);
    let digest = set.tasks[0].digest.clone();
    let tasks = write(&dir, "tasks.json", &set.to_json_pretty().unwrap());
    let run_file = write(
        &dir,
        "run.json",
        &correct_run(&digest).to_json_pretty().unwrap(),
    );

    let out = run(&[
        "--message-format=json-lines",
        "bench",
        "report",
        tasks.to_str().unwrap(),
        run_file.to_str().unwrap(),
    ]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let item = item_payload(&stdout);
    assert_eq!(item["kind"], "bench_summary");
    assert_eq!(item["model"], "scripted-v1");
    assert_eq!(item["language_version"], "2024");

    let summary = &item["summary"];
    assert_eq!(summary["parse_at_1"], 1.0);
    assert_eq!(summary["check_at_1"], 1.0);
    assert_eq!(summary["spec_pass_at_1"], 1.0);
    assert_eq!(summary["test_pass_at_1"], 1.0);
    assert_eq!(summary["generated_tokens"], 42);
    assert_eq!(summary["invented_symbols"], 0);
}

#[test]
fn report_recompiles_outputs_so_a_lie_in_the_record_does_not_survive() {
    // The recorded run CLAIMS the program checked, but the actual output is a
    // parse error. Re-scoring recompiles it and reports the truth: Check@1 = 0.
    let dir = scratch("lie");
    let set = TaskSet::pinned("demo", vec![double_task()]);
    let digest = set.tasks[0].digest.clone();
    let tasks = write(&dir, "tasks.json", &set.to_json_pretty().unwrap());

    let mut lying = correct_run(&digest);
    lying.runs[0].turns[0].output = "fn double(take x: Int) -> Int {\n    x +\n}\n".into();
    // The record still says checked = true (a fabricated metric).
    let run_file = write(&dir, "run.json", &lying.to_json_pretty().unwrap());

    let out = run(&[
        "--message-format=json-lines",
        "bench",
        "report",
        tasks.to_str().unwrap(),
        run_file.to_str().unwrap(),
    ]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let summary = &item_payload(&stdout)["summary"];
    // The compiler, not the record, decides: the parse error is exposed.
    assert_eq!(summary["parse_at_1"], 0.0);
    assert_eq!(summary["check_at_1"], 0.0);
}

#[test]
fn report_refuses_a_silently_edited_task_set() {
    // Pin a task set, then tamper with the task without updating its digest.
    let dir = scratch("tampered");
    let mut set = TaskSet::pinned("demo", vec![double_task()]);
    let digest = set.tasks[0].digest.clone();
    set.tasks[0]
        .task
        .instruction
        .push_str(" (secretly changed)");
    let tasks = write(&dir, "tasks.json", &set.to_json_pretty().unwrap());
    let run_file = write(
        &dir,
        "run.json",
        &correct_run(&digest).to_json_pretty().unwrap(),
    );

    let out = run(&[
        "--message-format=json-lines",
        "bench",
        "report",
        tasks.to_str().unwrap(),
        run_file.to_str().unwrap(),
    ]);
    assert!(!out.status.success(), "a silent task edit must be refused");
    let stdout = String::from_utf8_lossy(&out.stdout);
    // The finished event carries the error.
    let finished = stdout
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|v| v["event"] == "finished")
        .expect("a finished event");
    assert_eq!(finished["status"], "error");
    assert!(
        finished["summary"]["error"]
            .as_str()
            .unwrap_or_default()
            .contains("silently"),
        "error explains the silent change: {finished}"
    );
}

#[test]
fn report_human_mode_prints_a_metric_table() {
    let dir = scratch("human");
    let set = TaskSet::pinned("demo", vec![double_task()]);
    let digest = set.tasks[0].digest.clone();
    let tasks = write(&dir, "tasks.json", &set.to_json_pretty().unwrap());
    let run_file = write(
        &dir,
        "run.json",
        &correct_run(&digest).to_json_pretty().unwrap(),
    );

    let out = run(&[
        "bench",
        "report",
        tasks.to_str().unwrap(),
        run_file.to_str().unwrap(),
    ]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Parse@1"),
        "human report has a metric table:\n{stdout}"
    );
    assert!(stdout.contains("TestPass@1"));
    assert!(stdout.contains("scripted-v1"));
}
