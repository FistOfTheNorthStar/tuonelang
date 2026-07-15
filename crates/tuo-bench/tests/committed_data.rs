//! Validation tests for the version-controlled harness data files.
//!
//! These guard the committed fixtures and results under `tools/tokenizer-lab`
//! and `benchmarks/llm`: they must always parse against the current schema, and
//! the example benchmark run's stored `summary` must equal the summary
//! recomputed from its `attempts`. This keeps the machine-readable files honest
//! as the schema evolves.

use std::path::{Path, PathBuf};

use tuo_bench::SCHEMA_VERSION;
use tuo_bench::fixtures::FixtureFile;
use tuo_bench::llm::{BenchmarkRun, BenchmarkSummary, TaskSet};
use tuo_bench::measure;
use tuo_bench::tokenizer::Registry;

/// Repository root (two levels up from this crate's manifest dir).
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent() // crates/
        .and_then(Path::parent) // repo root
        .expect("tuo-bench should live under <root>/crates/")
        .to_path_buf()
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()))
}

#[test]
fn syntax_fixture_parses_and_measures() {
    let path = repo_root().join("tools/tokenizer-lab/fixtures/syntax-comparisons.json");
    let fixtures = FixtureFile::from_json(&read(&path))
        .unwrap_or_else(|e| panic!("fixture did not parse: {e}"));

    // Every required comparison from Prompt 2 must be present.
    let ids: Vec<&str> = fixtures.groups.iter().map(|g| g.id.as_str()).collect();
    for required in [
        "function-keyword",
        "spec-keyword",
        "signed-int-type",
        "string-type",
        "binding-keywords",
        "generic-syntax",
        "import-syntax",
    ] {
        assert!(
            ids.contains(&required),
            "missing comparison group `{required}`"
        );
    }

    // The engine runs cleanly over the committed fixture across all adapters.
    let registry = Registry::with_builtin_adapters();
    let report = measure::run(&registry, &fixtures);
    assert_eq!(report.tokenizers.len(), registry.len());
    assert_eq!(report.groups.len(), fixtures.groups.len());
}

#[test]
fn committed_measurement_report_matches_fresh_run() {
    let root = repo_root();
    let fixtures = FixtureFile::from_json(&read(
        &root.join("tools/tokenizer-lab/fixtures/syntax-comparisons.json"),
    ))
    .expect("fixtures parse");
    let committed = read(&root.join("tools/tokenizer-lab/results/syntax-comparisons.report.json"));

    let fresh = measure::run(&Registry::with_builtin_adapters(), &fixtures)
        .to_json_pretty()
        .expect("serialize");

    assert_eq!(
        fresh.trim(),
        committed.trim(),
        "committed tokenizer report is stale; regenerate it with `tokenizer-lab run`",
    );
}

#[test]
fn llm_task_set_parses() {
    let path = repo_root().join("benchmarks/llm/tasks/starter-tasks.json");
    let tasks =
        TaskSet::from_json(&read(&path)).unwrap_or_else(|e| panic!("task set did not parse: {e}"));
    assert_eq!(tasks.schema_version, SCHEMA_VERSION);
    assert!(!tasks.tasks.is_empty());
}

#[test]
fn llm_example_run_summary_is_consistent() {
    let path = repo_root().join("benchmarks/llm/results/example-run.json");
    let run = BenchmarkRun::from_json(&read(&path))
        .unwrap_or_else(|e| panic!("example run did not parse: {e}"));

    assert_eq!(run.schema_version, SCHEMA_VERSION);

    // The stored summary must match one recomputed from the raw attempts.
    let recomputed = BenchmarkSummary::from_attempts(&run.attempts);
    assert_eq!(
        run.summary, recomputed,
        "example-run.json summary is inconsistent with its attempts",
    );
}
