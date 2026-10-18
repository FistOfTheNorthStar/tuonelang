//! The core task set is authored as one directory per task and assembled into
//! the pinned `benchmarks/llm/codegen/tasks/core-tasks.json`, and **every task
//! is proven solvable** before it can be committed.
//!
//! A benchmark task the compiler cannot accept a solution to measures the
//! benchmark, not the model, so each task directory carries a `reference.tuo`
//! that is never shown to a model: this suite compiles it against the task's
//! shown specs and its held-out tests through the harness's own
//! [`evaluate`]/[`evaluate_tests`], and fails if either does not pass.
//!
//! A task directory under `benchmarks/llm/codegen/sources/core/<id>/` holds:
//!
//! | file              | becomes                                            |
//! |-------------------|----------------------------------------------------|
//! | `instruction.md`  | the instruction — the **only** text a model sees   |
//! | `specs.tuo`       | the specs appended to score SpecPass@1             |
//! | `tests.tuo`       | the held-out specs scoring TestPass@1              |
//! | `tags`            | one tag per line (area, level `L1`–`L5`, kind)     |
//! | `variants`        | optional; one `label: note` per line               |
//! | `reference.tuo`   | a known-good solution, kept out of the task set    |
//!
//! The JSON is generated, never hand-edited; regenerate it (re-pinning every
//! digest) after an intended change with:
//!
//! ```bash
//! TUO_BLESS=1 cargo test -p tuo-codegen-bench --test core_tasks
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use tuo_codegen_bench::{BenchTask, SyntaxVariant, TaskSet, evaluate, evaluate_tests};
use tuo_source::SourceMap;
use tuo_spec::Limits;

const DESCRIPTION: &str = "The core code-generation tasks: single-module tuonelang programs \
across the language's feature areas and five difficulty levels, including repair tasks that \
hand the model a broken program. Generated from benchmarks/llm/codegen/sources/core/ by \
crates/tuo-codegen-bench/tests/core_tasks.rs; every task has a reference solution there that \
passes its specs and held-out tests.";

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("tuo-codegen-bench lives under <workspace>/crates/")
        .to_path_buf()
}

fn sources_dir() -> PathBuf {
    workspace_root().join("benchmarks/llm/codegen/sources/core")
}

fn task_set_path() -> PathBuf {
    workspace_root().join("benchmarks/llm/codegen/tasks/core-tasks.json")
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Every task directory, in name order (the order of the generated set).
fn task_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(sources_dir())
        .expect("the core task sources exist")
        .map(|entry| entry.expect("readable entry").path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    dirs
}

fn load_task(dir: &Path) -> BenchTask {
    let id = dir
        .file_name()
        .and_then(|n| n.to_str())
        .expect("a UTF-8 task directory name")
        .to_owned();
    let variants_path = dir.join("variants");
    let variants = if variants_path.exists() {
        read(&variants_path)
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                let (label, note) = line
                    .split_once(':')
                    .unwrap_or_else(|| panic!("{id}: variant line `{line}` is not `label: note`"));
                SyntaxVariant {
                    label: label.trim().to_owned(),
                    note: note.trim().to_owned(),
                    specs: Vec::new(),
                }
            })
            .collect()
    } else {
        Vec::new()
    };
    BenchTask {
        instruction: read(&dir.join("instruction.md")).trim().to_owned(),
        specs: vec![read(&dir.join("specs.tuo"))],
        tests: vec![read(&dir.join("tests.tuo"))],
        variants,
        tags: read(&dir.join("tags"))
            .lines()
            .map(str::trim)
            .filter(|tag| !tag.is_empty())
            .map(str::to_owned)
            .collect(),
        id,
    }
}

fn assembled() -> TaskSet {
    TaskSet::pinned(
        DESCRIPTION,
        task_dirs().iter().map(|d| load_task(d)).collect(),
    )
}

#[test]
fn the_committed_task_set_is_generated_from_the_sources() {
    let mut expected = assembled().to_json_pretty().expect("a task set serializes");
    expected.push('\n');
    let path = task_set_path();
    if std::env::var_os("TUO_BLESS").is_some() {
        fs::write(&path, &expected).expect("write the task set");
        return;
    }
    let committed = read(&path);
    assert!(
        committed == expected,
        "{} is stale: regenerate it with \
         `TUO_BLESS=1 cargo test -p tuo-codegen-bench --test core_tasks`",
        path.display()
    );
}

#[test]
fn every_task_has_a_reference_solution_that_passes() {
    let mut failures = Vec::new();
    for dir in task_dirs() {
        let task = load_task(&dir);
        let reference = read(&dir.join("reference.tuo"));
        let shown = evaluate(&reference, &task.specs, Limits::default());
        if !shown.is_success() {
            failures.push(format!(
                "{}: the reference fails its specs:\n  {}",
                task.id,
                shown.feedback.join("\n  ")
            ));
        }
        if !evaluate_tests(&reference, &task.tests, Limits::default()) {
            failures.push(format!(
                "{}: the reference fails its held-out tests",
                task.id
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn every_reference_is_canonically_formatted() {
    let mut stale = Vec::new();
    for dir in task_dirs() {
        let path = dir.join("reference.tuo");
        let mut map = SourceMap::new();
        let file = map.intern_file("reference.tuo");
        let id = map
            .add_source(file, read(&path))
            .expect("a storable source");
        let outcome = tuo_fmt::format_source(map.source(id));
        if !outcome.safe || outcome.changed {
            stale.push(path.display().to_string());
        }
    }
    assert!(stale.is_empty(), "run `tuo fmt` on: {}", stale.join(", "));
}

#[test]
fn every_task_is_well_formed() {
    let tasks = assembled();
    assert!(
        tasks.tasks.len() >= 30,
        "the core set is meant to be large enough to gate on ({} tasks)",
        tasks.tasks.len()
    );
    for pinned in &tasks.tasks {
        let task = &pinned.task;
        // Held-out tests must not be a copy of the specs a model is scored on.
        assert_ne!(
            task.specs, task.tests,
            "{}: tests repeat the specs",
            task.id
        );
        let level = task.tags.iter().filter(|t| t.starts_with('L')).count();
        assert_eq!(level, 1, "{}: exactly one difficulty tag L1..L5", task.id);
    }
}
