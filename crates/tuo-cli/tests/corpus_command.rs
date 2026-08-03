//! End-to-end tests for `tuo corpus validate`, driven through the real `tuo`
//! binary.
//!
//! These prove the corpus pipeline works as one system through the CLI — in
//! particular that the CLI injects **native execution** (compile → link → run)
//! into the corpus validator's seam, the one stage the corpus crate cannot
//! perform alone. A correct program with a `main` entry runs natively and the
//! machine protocol reports every stage passing; a repair program is admitted
//! only when it fails at exactly the stage its category names.
//!
//! Like the other native-backend tests in this crate, these assume a working
//! `cc` on the host (used to link the produced object with the runtime shim).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

/// A unique scratch directory per test.
fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("corpus_command")
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch dir is creatable");
    dir
}

/// Write `text` to `dir/name` and return the path.
fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, text).expect("write source");
    path
}

/// Run `tuo` with `args`.
fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tuo"))
        .args(args)
        .output()
        .expect("the tuo binary runs")
}

/// Parse the single machine-protocol `item` event's payload out of JSON-lines
/// stdout.
fn item_payload(stdout: &str) -> Value {
    for line in stdout.lines() {
        let value: Value = serde_json::from_str(line).expect("each line is JSON");
        if value["event"] == "item" {
            return value;
        }
    }
    panic!("no item event in output:\n{stdout}");
}

/// A correct program with a runnable `main` entry and a passing spec.
const CORRECT_MAIN: &str = "\
fn main() -> Int {
    0
}

spec main {
    then main() == 0;
}
";

#[test]
fn validate_admits_a_correct_program_and_runs_it_natively() {
    let dir = scratch("correct_native");
    let file = write(&dir, "main.tuo", CORRECT_MAIN);

    // Human mode: succeeds.
    let human = run(&["corpus", "validate", file.to_str().unwrap()]);
    assert!(
        human.status.success(),
        "human validate failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&human.stdout),
        String::from_utf8_lossy(&human.stderr),
    );

    // Machine mode: every stage, including native execution, passes.
    let machine = run(&[
        "--message-format=json-lines",
        "corpus",
        "validate",
        file.to_str().unwrap(),
    ]);
    assert!(machine.status.success(), "machine validate failed");
    let stdout = String::from_utf8_lossy(&machine.stdout);
    let item = item_payload(&stdout);
    assert_eq!(item["admitted"], Value::Bool(true));
    let validation = &item["metadata"]["validation"];
    assert_eq!(validation["format"], "passed");
    assert_eq!(validation["specs"], "passed");
    // The CLI injects native execution; a program with `main` really ran.
    assert_eq!(
        validation["native_execution"], "passed",
        "native execution should have run and passed: {item}"
    );
}

#[test]
fn validate_records_metadata() {
    let dir = scratch("metadata");
    let file = write(&dir, "main.tuo", CORRECT_MAIN);
    let machine = run(&[
        "--message-format=json-lines",
        "corpus",
        "validate",
        "--origin",
        "llm",
        file.to_str().unwrap(),
    ]);
    assert!(machine.status.success());
    let stdout = String::from_utf8_lossy(&machine.stdout);
    let item = item_payload(&stdout);
    let metadata = &item["metadata"];
    assert_eq!(metadata["origin"], "llm");
    assert_eq!(metadata["language_version"], "2024");
    // Token counts came from the harness's built-in tokenizers.
    assert!(
        metadata["token_counts"]
            .as_array()
            .is_some_and(|a| !a.is_empty()),
        "token counts recorded: {metadata}"
    );
    // Features include `function` and `spec`.
    let features = metadata["features"].as_array().expect("features array");
    let ids: Vec<&str> = features.iter().filter_map(Value::as_str).collect();
    assert!(ids.contains(&"function"), "{ids:?}");
    assert!(ids.contains(&"spec"), "{ids:?}");
}

#[test]
fn validate_rejects_a_broken_program_from_the_correct_corpus() {
    let dir = scratch("reject_broken");
    let file = write(&dir, "broken.tuo", "fn wrong() -> Int {\n    true\n}\n");
    let output = run(&["corpus", "validate", file.to_str().unwrap()]);
    assert!(
        !output.status.success(),
        "a type-broken program must not be admitted to the correct corpus"
    );
}

#[test]
fn validate_admits_a_type_repair_program() {
    let dir = scratch("type_repair");
    // Fails at type check (returns Bool where Int is declared).
    let file = write(&dir, "type.tuo", "fn wrong() -> Int {\n    true\n}\n");
    let output = run(&[
        "--message-format=json-lines",
        "corpus",
        "validate",
        "--category",
        "type-repair",
        file.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "a real type error should be admitted to the type-repair corpus:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let item = item_payload(&stdout);
    assert_eq!(item["admitted"], Value::Bool(true));
    assert_eq!(item["metadata"]["validation"]["type_check"], "failed");
}

#[test]
fn validate_rejects_a_mislabeled_repair() {
    let dir = scratch("mislabeled");
    // A parse error submitted to the *type*-repair corpus is mislabeled.
    let file = write(
        &dir,
        "broken.tuo",
        "fn broken(take x: Int) -> Int {\n    x +\n}\n",
    );
    let output = run(&[
        "corpus",
        "validate",
        "--category",
        "type-repair",
        file.to_str().unwrap(),
    ]);
    assert!(
        !output.status.success(),
        "a parse error is not a type-repair candidate"
    );
}

#[test]
fn validate_handles_a_repository_level_change_across_files() {
    let dir = scratch("repo_change");
    // A two-module program: a library module and a main module that imports it.
    let lib = write(
        &dir,
        "util.tuo",
        "module util;\n\npub fn double(take x: Int) -> Int {\n    x + x\n}\n",
    );
    let app = write(
        &dir,
        "main.tuo",
        "import util::double;\n\nfn main() -> Int {\n    double(0)\n}\n\nspec main {\n    then main() == 0;\n}\n",
    );
    let output = run(&[
        "--message-format=json-lines",
        "corpus",
        "validate",
        "--category",
        "repository-change",
        lib.to_str().unwrap(),
        app.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "a valid multi-file change should be admitted:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let item = item_payload(&stdout);
    assert_eq!(item["admitted"], Value::Bool(true));
    assert_eq!(item["metadata"]["complexity"]["specs"], 1);
}
