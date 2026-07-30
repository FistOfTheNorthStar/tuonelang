//! The `tuo agent --stdio` transport, run against the real binary.
//!
//! The protocol *core* is pinned in `tuo-agent`'s own tests; this suite pins the
//! **transport**: that `tuo agent --stdio` speaks JSON-lines over the process's
//! standard streams (one response per request line), that stdout carries
//! protocol output only, that one long-lived process reuses its compiler
//! database across many requests, that the real canonical formatter (`tuo-fmt`)
//! backs `format`, and that `--stdio` is required.

use std::io::Write;
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

/// Drive `tuo agent --stdio`: write each request line to stdin, close it, and
/// collect the process output.
fn drive(requests: &[Value]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_tuo"))
        .args(["agent", "--stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the tuo binary spawns");
    {
        let mut stdin = child.stdin.take().expect("stdin is piped");
        for request in requests {
            writeln!(stdin, "{request}").expect("write request line");
        }
        // Dropping stdin closes it, so the server reaches EOF and exits.
    }
    child.wait_with_output().expect("the process completes")
}

/// The response lines on stdout, parsed as JSON objects.
fn responses(output: &Output) -> Vec<Value> {
    let stdout = std::str::from_utf8(&output.stdout).expect("utf-8 stdout");
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("each stdout line is a JSON response"))
        .collect()
}

/// A request object.
fn req(id: u64, method: &str, params: Value) -> Value {
    json!({ "id": id, "method": method, "params": params })
}

const LIB: &str = "\
fn add(take a: Int, take b: Int) -> Int {
    a + b
}

spec add {
    then add(2, 3) == 5;
}
";

#[test]
fn one_response_per_request_line_in_order() {
    let output = drive(&[
        req(1, "initialize", json!({})),
        req(2, "set_document", json!({ "uri": "lib.tuo", "text": LIB })),
        req(3, "check", json!({})),
    ]);
    assert!(output.status.success(), "clean EOF is a success exit");
    let responses = responses(&output);
    assert_eq!(responses.len(), 3, "one response per request");
    // Responses arrive in request order, each echoing its id.
    for (index, response) in responses.iter().enumerate() {
        assert_eq!(response["id"], (index + 1) as u64);
        assert_eq!(response["protocol_version"], 1);
    }
    assert_eq!(responses[2]["result"]["accepted"], true);
}

#[test]
fn stdout_carries_protocol_output_only() {
    let output = drive(&[
        req(1, "set_document", json!({ "uri": "lib.tuo", "text": LIB })),
        req(2, "check", json!({})),
    ]);
    // Every stdout line parses as a JSON response — nothing else is written.
    let stdout = std::str::from_utf8(&output.stdout).expect("utf-8 stdout");
    for line in stdout.lines().filter(|l| !l.trim().is_empty()) {
        let value: Value = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("non-protocol line on stdout: {e}\n{line}"));
        assert!(value["id"].is_number(), "each line is a response");
    }
    // stderr is silent without --log.
    assert!(
        output.stderr.is_empty(),
        "stderr is silent in the agent protocol without --log"
    );
}

#[test]
fn one_process_reuses_the_database_across_edits() {
    // A single agent process handles an edit that first breaks then fixes the
    // program, proving the compiler is not restarted per request.
    let broken = "fn main() -> Int {\n    gone()\n}\n";
    let fixed = "fn helper() -> Int {\n    1\n}\n\nfn main() -> Int {\n    helper()\n}\n";
    let output = drive(&[
        req(1, "set_document", json!({ "uri": "m.tuo", "text": broken })),
        req(2, "check", json!({})),
        req(3, "set_document", json!({ "uri": "m.tuo", "text": fixed })),
        req(4, "check", json!({})),
    ]);
    let responses = responses(&output);
    assert_eq!(responses[1]["result"]["accepted"], false, "first is broken");
    assert_eq!(responses[3]["result"]["accepted"], true, "then it is fixed");
}

#[test]
fn format_uses_the_real_canonical_formatter() {
    // The transport injects `tuo-fmt`, so `format` returns genuinely canonical
    // text: a badly-spaced function is reformatted.
    let output = drive(&[req(1, "format", json!({ "text": "fn   a( ) ->Int{1}" }))]);
    let responses = responses(&output);
    let result = &responses[0]["result"];
    assert_eq!(result["safe"], true, "the formatter verified the result");
    assert_eq!(result["changed"], true, "the messy input is reformatted");
    let text = result["text"].as_str().unwrap();
    // Canonical form uses single spacing and 4-space indentation.
    assert!(text.contains("fn a("), "canonical spacing: {text}");
}

#[test]
fn run_spec_executes_over_the_transport() {
    let output = drive(&[
        req(1, "set_document", json!({ "uri": "lib.tuo", "text": LIB })),
        req(2, "run_spec", json!({})),
    ]);
    let responses = responses(&output);
    let result = &responses[1]["result"];
    assert_eq!(result["passed"], true);
    assert_eq!(result["summary"]["ran"], 1);
}

#[test]
fn a_blank_line_is_skipped() {
    // Blank lines between requests are ignored (not a parse error).
    let mut child = Command::new(env!("CARGO_BIN_EXE_tuo"))
        .args(["agent", "--stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    {
        let mut stdin = child.stdin.take().unwrap();
        writeln!(stdin, "{}", req(1, "initialize", json!({}))).unwrap();
        writeln!(stdin).unwrap();
        writeln!(stdin, "{}", req(2, "check", json!({}))).unwrap();
    }
    let output = child.wait_with_output().unwrap();
    let responses = responses(&output);
    assert_eq!(responses.len(), 2, "the blank line produced no response");
}

#[test]
fn stdio_is_required() {
    // `tuo agent` without `--stdio` is a usage error: the CLI never advertises a
    // transport it does not have.
    let output = Command::new(env!("CARGO_BIN_EXE_tuo"))
        .args(["agent"])
        .output()
        .expect("runs");
    assert!(!output.status.success(), "missing --stdio is rejected");
}
