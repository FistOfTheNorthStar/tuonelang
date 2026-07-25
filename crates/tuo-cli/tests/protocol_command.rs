//! Backwards-compatibility tests for the `tuo` machine protocol
//! (`--message-format=json` / `json-lines`), run against the real binary.
//!
//! These pin the **stable wire contract**: the protocol version, the fields
//! every machine message guarantees, and the stream discipline (stdout carries
//! protocol output only; stderr is silent unless `--log`). They are
//! intentionally conservative — they assert that the guaranteed fields are
//! *present* with the right values, not that the object has *exactly* those
//! fields, because the protocol permits additive growth without a version
//! bump. A change that drops or renames a guaranteed field, or that alters the
//! version, breaks a test here and must be paired with a `PROTOCOL_VERSION`
//! bump and an update to `tests/cli/protocol/protocol-v1.schema.json`.

use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::Value;

/// The protocol version this test suite pins. Must equal
/// `tuo_cli::protocol::PROTOCOL_VERSION` (asserted indirectly: every message
/// carries it) and the `protocol_version` in the committed schema fixture.
const PROTOCOL_VERSION: u64 = 1;

fn fixture(name: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/cli/protocol/fixtures")
        .join(name)
        .to_str()
        .expect("utf-8 path")
        .to_owned()
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tuo"))
        .args(args)
        .output()
        .expect("the tuo binary runs")
}

/// Parse stdout as one JSON envelope (the `json` format).
fn envelope(output: &Output) -> Value {
    let stdout = std::str::from_utf8(&output.stdout).expect("utf-8 stdout");
    serde_json::from_str(stdout.trim()).unwrap_or_else(|error| {
        panic!("stdout is not a single JSON object: {error}\n---\n{stdout}");
    })
}

/// Parse stdout as a sequence of JSON lines (the `json-lines` format).
fn lines(output: &Output) -> Vec<Value> {
    let stdout = std::str::from_utf8(&output.stdout).expect("utf-8 stdout");
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("each line is a JSON object"))
        .collect()
}

/// The events array of a `json` envelope.
fn events(envelope: &Value) -> &Vec<Value> {
    envelope["events"].as_array().expect("events is an array")
}

/// Assert the guaranteed fields of one event object are present and typed.
fn assert_event_shape(event: &Value) {
    assert!(event["event"].is_string(), "event carries a kind: {event}");
    assert!(
        event["status"].is_string(),
        "event carries a status: {event}"
    );
    let status = event["status"].as_str().unwrap();
    assert!(
        matches!(status, "ok" | "error" | "running"),
        "status is a stable value, got {status:?}"
    );
}

#[test]
fn json_envelope_carries_the_versioned_header() {
    let output = run(&["--message-format=json", "check", &fixture("passing.tuo")]);
    let envelope = envelope(&output);
    assert_eq!(
        envelope["protocol_version"].as_u64(),
        Some(PROTOCOL_VERSION),
        "the envelope is versioned"
    );
    assert_eq!(
        envelope["command"].as_str(),
        Some("check"),
        "the envelope names the command"
    );
    assert!(!events(&envelope).is_empty(), "the envelope carries events");
    for event in events(&envelope) {
        assert_event_shape(event);
    }
}

#[test]
fn every_stream_starts_with_started_and_ends_with_finished() {
    let output = run(&["--message-format=json", "spec", &fixture("passing.tuo")]);
    let envelope = envelope(&output);
    let events = events(&envelope);
    assert_eq!(events.first().unwrap()["event"], "started");
    let last = events.last().unwrap();
    assert_eq!(last["event"], "finished");
    // The terminal event is never `running`.
    assert_ne!(last["status"], "running");
}

#[test]
fn json_lines_tags_every_line_and_streams_events() {
    let output = run(&[
        "--message-format=json-lines",
        "spec",
        &fixture("passing.tuo"),
    ]);
    let lines = lines(&output);
    assert!(lines.len() >= 3, "started + at least one item + finished");
    for line in &lines {
        // Each line is self-describing: version + command + the event fields.
        assert_eq!(line["protocol_version"].as_u64(), Some(PROTOCOL_VERSION));
        assert_eq!(line["command"].as_str(), Some("spec"));
        assert_event_shape(line);
    }
    assert_eq!(lines.first().unwrap()["event"], "started");
    assert_eq!(lines.last().unwrap()["event"], "finished");
}

#[test]
fn a_diagnostic_event_carries_stable_id_and_source_range() {
    let output = run(&["--message-format=json", "check", &fixture("undefined.tuo")]);
    assert!(!output.status.success(), "an undefined name is an error");
    let envelope = envelope(&output);
    let diagnostic = events(&envelope)
        .iter()
        .find(|event| event["event"] == "diagnostic")
        .expect("an error program emits a diagnostic event");
    // Stable id (the diagnostic code) and the independently-versioned
    // diagnostic schema travel together.
    assert!(
        diagnostic["id"].as_str().is_some_and(|id| !id.is_empty()),
        "the diagnostic has a stable id"
    );
    assert!(diagnostic["diagnostic_schema_version"].is_number());
    // The source range is the versioned span object with canonical offsets.
    let span = &diagnostic["diagnostic"]["primary_span"];
    assert!(span["start"].is_number(), "range has a start offset");
    assert!(span["end"].is_number(), "range has an end offset");
    assert_eq!(diagnostic["status"], "error");
}

#[test]
fn a_spec_item_carries_identity_range_and_assertions() {
    let output = run(&["--message-format=json", "spec", &fixture("passing.tuo")]);
    let envelope = envelope(&output);
    let item = events(&envelope)
        .iter()
        .find(|event| event["event"] == "item" && event["kind"] == "spec_result")
        .expect("a spec run emits a spec_result item");
    assert!(item["id"].as_str().is_some(), "the spec has a stable id");
    assert_eq!(item["name"], "add");
    assert!(item["range"]["start"].is_number(), "the spec has a range");
    assert!(item["duration_micros"].is_number(), "timing is reported");
    let assertion = &item["assertions"].as_array().expect("assertions")[0];
    assert_eq!(assertion["assertion"], "then");
    assert!(assertion["range"]["start"].is_number());
    assert_eq!(assertion["outcome"], "passed");
}

#[test]
fn a_failing_spec_item_reports_expected_and_actual() {
    let output = run(&["--message-format=json", "spec", &fixture("failing.tuo")]);
    assert!(!output.status.success(), "a failing spec → failure exit");
    let envelope = envelope(&output);
    let item = events(&envelope)
        .iter()
        .find(|event| event["kind"] == "spec_result")
        .expect("a spec_result item");
    assert_eq!(item["passed"], false);
    let detail = &item["assertions"][0]["detail"];
    assert_eq!(detail["expected"], "999I64");
    assert_eq!(detail["actual"], "20I64");
    // The finished summary reflects the failure.
    let finished = events(&envelope).last().unwrap();
    assert_eq!(finished["status"], "error");
    assert_eq!(finished["summary"]["failed"], 1);
}

#[test]
fn stdout_is_protocol_only_and_stderr_is_silent_in_machine_mode() {
    // An error program still emits protocol output on stdout and — without
    // --log — nothing on stderr, so a consumer parsing stdout is undisturbed.
    let output = run(&["--message-format=json", "check", &fixture("undefined.tuo")]);
    assert!(
        output.stderr.is_empty(),
        "stderr must be silent in machine mode without --log, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // stdout parses cleanly as the sole protocol object.
    let _ = envelope(&output);
}

#[test]
fn log_flag_enables_stderr_logging_only_when_stdout_write_is_fine() {
    // With --log, stderr *may* carry logging; stdout stays protocol-only. We
    // assert the weaker, stable guarantee: stdout is still a clean envelope.
    let output = run(&[
        "--message-format=json",
        "--log",
        "check",
        &fixture("passing.tuo"),
    ]);
    let _ = envelope(&output);
}

#[test]
fn debug_has_no_machine_protocol() {
    // `tuo debug` is an unstable developer aid, not a protocol; a machine
    // format is a usage error, not unversioned JSON on stdout.
    let output = run(&[
        "--message-format=json",
        "debug",
        "syntax",
        &fixture("passing.tuo"),
    ]);
    assert!(!output.status.success(), "machine debug is refused");
    assert!(
        output.stdout.is_empty(),
        "no protocol output for a refused debug dump"
    );
}

#[test]
fn fmt_emits_a_file_item_per_input() {
    // `passing.tuo` is already canonical → an `unchanged` fmt_file item.
    let output = run(&[
        "--message-format=json",
        "fmt",
        "--check",
        &fixture("passing.tuo"),
    ]);
    let envelope = envelope(&output);
    let item = events(&envelope)
        .iter()
        .find(|event| event["kind"] == "fmt_file")
        .expect("a fmt run emits a fmt_file item");
    assert!(item["file"].as_str().is_some());
    assert!(
        matches!(
            item["outcome"].as_str(),
            Some("unchanged" | "reformatted" | "would-reformat" | "unsafe" | "error")
        ),
        "the outcome is a stable value, got {:?}",
        item["outcome"]
    );
}

#[test]
fn the_committed_schema_pins_the_current_protocol_version() {
    // The schema fixture and the binary must agree on the version, so a bump
    // is never silent: this fails until both move together.
    let schema_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/cli/protocol/protocol-v1.schema.json");
    let text = std::fs::read_to_string(&schema_path).expect("schema fixture exists");
    let schema: Value = serde_json::from_str(&text).expect("schema fixture is valid JSON");
    assert_eq!(
        schema["protocol_version"].as_u64(),
        Some(PROTOCOL_VERSION),
        "the committed schema pins the protocol version the tests assert"
    );
    // And a live message carries that same version.
    let output = run(&["--message-format=json", "check", &fixture("passing.tuo")]);
    assert_eq!(
        envelope(&output)["protocol_version"].as_u64(),
        Some(PROTOCOL_VERSION),
        "the binary emits the version the schema pins"
    );
}
