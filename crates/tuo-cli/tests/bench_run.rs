//! End-to-end tests for `tuo bench run`, driven through the real `tuo` binary
//! against a **fake** OpenAI-compatible endpoint served from this test.
//!
//! The harness embeds no model, and neither does this test: a tiny HTTP server
//! on an ephemeral loopback port answers `/v1/chat/completions` with scripted
//! replies, so what is proven is the CLI's side of the contract — the request
//! shape it sends (model, temperature, seed, system + user turns, the brief
//! when primed), how it reads a reply (fenced source, token accounting, an
//! endpoint error), that the recorded run is what `tuo bench report` accepts,
//! and that the metrics come from the real compiler evaluating what the "model"
//! returned. Skips cleanly when `curl` is absent, since the adapter is built on
//! it.

use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::{Value, json};
use tuo_codegen_bench::{BenchTask, BenchmarkRun, SyntaxVariant, TaskSet};

/// A unique scratch directory per test.
fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("bench_run")
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch dir is creatable");
    dir
}

/// Whether the system `curl` the adapter shells out to is available.
fn curl_available() -> bool {
    Command::new("curl")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Run `tuo` with `args`.
fn tuo(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tuo"))
        .args(args)
        .output()
        .expect("the tuo binary runs")
}

/// One scripted reply: the assistant content and the token count to report,
/// or an endpoint error object.
enum Reply {
    Content(&'static str, u64),
    Error(&'static str),
}

/// The requests a fake endpoint received, as parsed JSON bodies.
type Seen = Arc<Mutex<Vec<Value>>>;

/// Serve `replies` in order on an ephemeral loopback port; returns the endpoint
/// base URL and the log of request bodies. Each connection carries exactly one
/// request, which is how `curl` is invoked.
fn fake_endpoint(replies: Vec<Reply>) -> (String, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().expect("local addr").port();
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    thread::spawn(move || {
        for reply in replies {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let body = read_request_body(&mut stream);
            log.lock().expect("log").push(body);
            let payload = match reply {
                Reply::Content(content, tokens) => json!({
                    "choices": [{ "message": { "role": "assistant", "content": content } }],
                    "usage": { "completion_tokens": tokens },
                }),
                Reply::Error(message) => json!({ "error": { "message": message } }),
            }
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{payload}",
                payload.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    (format!("http://127.0.0.1:{port}/v1"), seen)
}

/// Read one HTTP request off `stream` and parse its JSON body.
fn read_request_body(stream: &mut std::net::TcpStream) -> Value {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = stream.read(&mut chunk).expect("read request");
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(split) = find_header_end(&buf) {
            let headers = String::from_utf8_lossy(&buf[..split]).to_ascii_lowercase();
            let length = headers
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if buf.len() >= split + 4 + length {
                let body = &buf[split + 4..split + 4 + length];
                return serde_json::from_slice(body).expect("request body is JSON");
            }
        }
    }
    panic!("request ended before its body");
}

/// The offset of the blank line ending the HTTP headers.
fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

/// Write a pinned task set to `dir` and return its path.
fn write_task_set(dir: &Path, tasks: Vec<BenchTask>) -> PathBuf {
    let set = TaskSet::pinned("bench_run test tasks", tasks);
    let path = dir.join("tasks.json");
    fs::write(&path, set.to_json_pretty().expect("task set json")).expect("write tasks");
    path
}

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

const GOOD: &str = "fn double(take x: Int) -> Int {\n    x + x\n}\n";

#[test]
fn a_live_run_records_what_the_compiler_decided_and_reports_it() {
    if !curl_available() {
        return; // the adapter is built on the system curl; nothing to prove without it
    }
    let dir = scratch("fail_then_repair");
    let tasks = write_task_set(&dir, vec![double_task()]);
    // Turn 0: calls an undefined helper (fails resolution). Turn 1: repaired,
    // fenced in a non-`tuo` info string to prove the fence is still honored.
    let (endpoint, seen) = fake_endpoint(vec![
        Reply::Content(
            "```tuo\nfn double(take x: Int) -> Int {\n    twice(x)\n}\n```",
            11,
        ),
        Reply::Content(
            "Sure:\n```rust\nfn double(take x: Int) -> Int {\n    x + x\n}\n```",
            9,
        ),
    ]);
    let run_path = dir.join("run.json");
    let output = tuo(&[
        "--message-format=json-lines",
        "bench",
        "run",
        tasks.to_str().unwrap(),
        "--endpoint",
        &endpoint,
        "--model",
        "scripted",
        "-o",
        run_path.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    // The requests carried the contract: the model name, deterministic
    // sampling, a system turn, and on the repair turn the compiler's feedback.
    let requests = seen.lock().expect("log").clone();
    assert_eq!(requests.len(), 2, "one initial and one repair request");
    assert_eq!(requests[0]["model"], "scripted");
    assert_eq!(requests[0]["temperature"], 0.0);
    assert!(requests[0]["seed"].is_u64());
    assert_eq!(requests[0]["messages"][0]["role"], "system");
    let repair = requests[1]["messages"][1]["content"].as_str().unwrap();
    assert!(
        repair.contains("R0002"),
        "repair turn carries the diagnostic: {repair}"
    );
    assert!(
        repair.contains("twice(x)"),
        "repair turn carries the prior source: {repair}"
    );

    // The recorded run is the harness's own type, with the verdicts the real
    // compiler produced and the tokens the endpoint reported.
    let run = BenchmarkRun::from_json(&fs::read_to_string(&run_path).unwrap()).unwrap();
    assert_eq!(run.model.id, "scripted@t0");
    assert_eq!(run.runs.len(), 1);
    let turns = &run.runs[0].turns;
    assert_eq!(turns.len(), 2);
    assert!(!turns[0].checked && turns[0].invented_symbols == 1);
    assert!(turns[1].checked && turns[1].specs_passed);
    assert_eq!(turns[1].output, GOOD);
    assert_eq!(
        (turns[0].generated_tokens, turns[1].generated_tokens),
        (11, 9)
    );
    assert_eq!(
        run.runs[0].tests_passed,
        Some(true),
        "held-out tests scored"
    );

    // The same file goes straight back through `bench report`.
    let report = tuo(&[
        "bench",
        "report",
        tasks.to_str().unwrap(),
        run_path.to_str().unwrap(),
    ]);
    assert!(
        report.status.success(),
        "{}",
        String::from_utf8_lossy(&report.stderr)
    );
}

#[test]
fn priming_puts_the_generated_brief_in_the_system_prompt_and_is_recorded() {
    if !curl_available() {
        return; // the adapter is built on the system curl; nothing to prove without it
    }
    let dir = scratch("primed");
    let tasks = write_task_set(&dir, vec![double_task()]);
    let (endpoint, seen) = fake_endpoint(vec![Reply::Content(GOOD, 5)]);
    let run_path = dir.join("run.json");
    let output = tuo(&[
        "bench",
        "run",
        tasks.to_str().unwrap(),
        "--endpoint",
        &endpoint,
        "--model",
        "scripted",
        "--prime",
        "-o",
        run_path.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let brief = String::from_utf8(tuo(&["cheatsheet"]).stdout).unwrap();
    let system = seen.lock().unwrap()[0]["messages"][0]["content"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        system.contains(brief.trim()),
        "the primed system prompt carries the generated brief verbatim"
    );
    let run = BenchmarkRun::from_json(&fs::read_to_string(&run_path).unwrap()).unwrap();
    assert!(run.model.id.ends_with("+primed"));
    assert!(
        run.model
            .extra
            .iter()
            .any(|e| e.key == "primed" && e.value == "true"),
        "priming is recorded in the model config: {:?}",
        run.model.extra
    );
}

#[test]
fn variants_are_each_run_and_an_endpoint_error_is_recorded_not_fatal() {
    if !curl_available() {
        return; // the adapter is built on the system curl; nothing to prove without it
    }
    let dir = scratch("variants_and_error");
    let mut task = double_task();
    task.variants = vec![SyntaxVariant {
        label: "mul".into(),
        note: "x * 2".into(),
        specs: vec![],
    }];
    let tasks = write_task_set(&dir, vec![task]);
    // Default spelling succeeds; the variant's endpoint call fails.
    let (endpoint, seen) = fake_endpoint(vec![
        Reply::Content(GOOD, 4),
        Reply::Error("model is overloaded"),
    ]);
    let run_path = dir.join("run.json");
    let output = tuo(&[
        "bench",
        "run",
        tasks.to_str().unwrap(),
        "--endpoint",
        &endpoint,
        "--model",
        "scripted",
        "-o",
        run_path.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let requests = seen.lock().unwrap().clone();
    assert!(
        requests[1]["messages"][1]["content"]
            .as_str()
            .unwrap()
            .contains("mul"),
        "the variant request names its style"
    );
    let run = BenchmarkRun::from_json(&fs::read_to_string(&run_path).unwrap()).unwrap();
    assert_eq!(run.runs.len(), 2, "default spelling plus one variant");
    assert_eq!(run.runs[0].variant, "default");
    assert_eq!(run.runs[1].variant, "mul");
    let error = run.runs[1]
        .generation_error
        .as_ref()
        .expect("recorded, not fatal");
    assert!(
        error.reason.contains("model is overloaded"),
        "{}",
        error.reason
    );
}

#[test]
fn a_missing_api_key_variable_is_refused_before_any_request() {
    let dir = scratch("missing_key");
    let tasks = write_task_set(&dir, vec![double_task()]);
    let output = tuo(&[
        "bench",
        "run",
        tasks.to_str().unwrap(),
        "--endpoint",
        "http://127.0.0.1:9/v1",
        "--model",
        "any",
        "--api-key-env",
        "TUO_BENCH_RUN_TEST_UNSET_KEY",
        "-o",
        dir.join("run.json").to_str().unwrap(),
    ]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("TUO_BENCH_RUN_TEST_UNSET_KEY"),
        "names the variable"
    );
}
