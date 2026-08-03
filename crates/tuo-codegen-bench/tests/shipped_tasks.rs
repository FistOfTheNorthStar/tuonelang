//! The shipped benchmark tasks must stay honest: every task under
//! `benchmarks/llm/codegen/tasks/` must load, and its content-digest pin must
//! still match its content. A task edited without re-pinning fails this test, so
//! a committed benchmark can never drift silently.

use std::path::PathBuf;

use tuo_codegen_bench::TaskSet;

/// The workspace-root `benchmarks/llm/codegen/tasks/` directory. This test lives
/// at `crates/tuo-codegen-bench/tests/`, so the workspace root is three levels
/// up.
fn tasks_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent() // crates/
        .and_then(|p| p.parent()) // workspace root
        .expect("tuo-codegen-bench lives under <workspace>/crates/")
        .join("benchmarks/llm/codegen/tasks")
}

#[test]
fn every_shipped_task_set_verifies_its_pins() {
    let dir = tasks_dir();
    let mut total = 0;
    for entry in std::fs::read_dir(&dir).expect("tasks directory exists") {
        let path = entry.expect("readable entry").path();
        if path.extension().is_some_and(|e| e == "json") {
            let text = std::fs::read_to_string(&path).expect("read task set");
            let set = TaskSet::from_json(&text)
                .unwrap_or_else(|e| panic!("{} is not a valid task set: {e}", path.display()));
            // The load-time honesty check: no task may have drifted from its pin.
            set.verify_digests().unwrap_or_else(|m| {
                panic!(
                    "{}: task `{}` was edited without re-pinning (recorded {}, actual {})",
                    path.display(),
                    m.task_id,
                    m.recorded,
                    m.actual
                )
            });
            total += set.tasks.len();
        }
    }
    assert!(total >= 1, "expected at least one shipped benchmark task");
}

#[test]
fn the_starter_set_has_the_expected_shape() {
    let path = tasks_dir().join("starter-tasks.json");
    let text = std::fs::read_to_string(&path).expect("starter set exists");
    let set = TaskSet::from_json(&text).expect("valid");
    let tasks = set.tasks().expect("pins verify");
    // Every shipped task carries specs and held-out tests, so all @1 metrics and
    // TestPass@1 are exercisable.
    for task in &tasks {
        assert!(!task.specs.is_empty(), "task `{}` has shown specs", task.id);
        assert!(
            !task.tests.is_empty(),
            "task `{}` has held-out tests",
            task.id
        );
    }
    // At least one task offers comparable syntax variants for language-design
    // evaluation.
    assert!(
        tasks.iter().any(|t| !t.variants.is_empty()),
        "the starter set exercises syntax variants"
    );
}
