//! The agent protocol, exercised through the transport-agnostic [`Server`].
//!
//! Each test drives one or more JSON-lines requests through
//! [`Server::handle_line`] and asserts the response shape — proving the agent
//! answers compiler-intelligence queries by reusing the shared compiler engine,
//! not by reimplementing a stage. A stub [`Formatter`] stands in for the
//! canonical formatter (whose real wiring is covered end-to-end by the CLI
//! transport test); the protocol core is what these tests pin.

use serde_json::{Value, json};
use tuo_agent::Server;
use tuo_agent::session::{FormatResult, Formatter};

/// A stub formatter: reports the text canonical and unchanged. Enough to prove
/// the `format` seam is wired; the real `tuo-fmt` path is tested via the CLI.
struct StubFormatter;

impl Formatter for StubFormatter {
    fn format(&self, text: &str) -> FormatResult {
        FormatResult {
            text: text.to_owned(),
            changed: false,
            safe: true,
        }
    }
}

/// A two-function program with a spec attached to `add`.
const LIB: &str = "\
fn add(take a: Int, take b: Int) -> Int {
    a + b
}

fn double(take x: Int) -> Int {
    x * 2
}

spec add {
    given a: Int = 2, b: Int = 3;
    then add(a, b) == 5;
}
";

/// A fresh server with `LIB` opened as `lib.tuo`.
fn server_with_lib() -> Server {
    let mut server = Server::new(Box::new(StubFormatter));
    let response = request(
        &mut server,
        1,
        "set_document",
        json!({ "uri": "lib.tuo", "text": LIB }),
    );
    assert!(response["ok"].as_bool().unwrap(), "document opens");
    server
}

/// Send one request and return its response as a `Value`.
fn request(server: &mut Server, id: u64, method: &str, params: Value) -> Value {
    let line = json!({ "id": id, "method": method, "params": params }).to_string();
    let response = server.handle_line(&line);
    serde_json::to_value(response).expect("response serializes")
}

/// The `result` object of a successful response.
fn result(server: &mut Server, id: u64, method: &str, params: Value) -> Value {
    let response = request(server, id, method, params);
    assert!(
        response["ok"].as_bool().unwrap_or(false),
        "{method} succeeds: {response}"
    );
    assert_eq!(response["id"], id, "the response echoes the request id");
    assert_eq!(response["protocol_version"], 1);
    response["result"].clone()
}

/// A one-based `{line, column}` position for the first occurrence of `needle`.
fn position_of(text: &str, needle: &str) -> Value {
    let byte = text.find(needle).expect("needle present");
    let before = &text[..byte];
    let line = before.matches('\n').count() as u32 + 1;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let column = text[line_start..byte].chars().count() as u32 + 1;
    json!({ "line": line, "column": column })
}

#[test]
fn initialize_reports_the_protocol_and_methods() {
    let mut server = Server::new(Box::new(StubFormatter));
    let result = result(&mut server, 1, "initialize", Value::Null);
    assert_eq!(result["protocol_version"], 1);
    assert_eq!(result["server"], "tuo-agent");
    // It is a compiler-intelligence protocol, not an AI model.
    assert!(
        result["description"]
            .as_str()
            .unwrap()
            .contains("not an AI model")
    );
    let methods = result["methods"].as_array().unwrap();
    for expected in ["check", "type_at", "run_spec", "apply_safe_fix"] {
        assert!(
            methods.iter().any(|m| m == expected),
            "advertises {expected}"
        );
    }
}

#[test]
fn check_reports_a_clean_program() {
    let mut server = server_with_lib();
    let result = result(&mut server, 2, "check", json!({}));
    assert_eq!(result["accepted"], true);
    assert_eq!(result["errors"], 0);
    assert!(result["diagnostics"].as_array().unwrap().is_empty());
}

#[test]
fn check_reports_an_error_with_a_range() {
    let mut server = Server::new(Box::new(StubFormatter));
    request(
        &mut server,
        1,
        "set_document",
        json!({ "uri": "bad.tuo", "text": "fn main() -> Int {\n    undefined()\n}\n" }),
    );
    let result = result(&mut server, 2, "check", json!({}));
    assert_eq!(result["accepted"], false);
    assert!(result["errors"].as_u64().unwrap() >= 1);
    let diag = &result["diagnostics"][0];
    assert_eq!(diag["severity"], "error");
    // The range points at line 2 (one-based).
    assert_eq!(diag["range"]["start"]["line"], 2);
}

#[test]
fn diagnostics_scope_to_one_document() {
    let mut server = server_with_lib();
    let result = result(&mut server, 2, "diagnostics", json!({ "uri": "lib.tuo" }));
    assert_eq!(result["uri"], "lib.tuo");
    assert!(result["diagnostics"].as_array().unwrap().is_empty());
}

#[test]
fn type_at_reports_a_function_signature() {
    let mut server = server_with_lib();
    let result = result(
        &mut server,
        2,
        "type_at",
        json!({ "uri": "lib.tuo", "position": position_of(LIB, "add") }),
    );
    assert_eq!(result["name"], "add");
    assert_eq!(result["kind"], "function");
    let ty = result["type"].as_str().unwrap();
    assert!(ty.contains("fn(") && ty.contains("->"), "signature: {ty}");
}

#[test]
fn definition_jumps_to_the_declaration() {
    let mut server = server_with_lib();
    // The call `add(a, b)` in the spec jumps to `add`'s declaration on line 1.
    let result = result(
        &mut server,
        2,
        "definition",
        json!({ "uri": "lib.tuo", "position": position_of(LIB, "add(a, b)") }),
    );
    assert_eq!(result["uri"], "lib.tuo");
    assert_eq!(result["range"]["start"]["line"], 1);
}

#[test]
fn references_list_every_use() {
    let mut server = server_with_lib();
    let result = result(
        &mut server,
        2,
        "references",
        json!({ "uri": "lib.tuo", "position": position_of(LIB, "add"), "include_declaration": true }),
    );
    let refs = result["references"].as_array().unwrap();
    assert!(
        refs.len() >= 2,
        "add is used more than once: {}",
        refs.len()
    );
    assert!(refs.iter().all(|r| r["uri"] == "lib.tuo"));
}

#[test]
fn symbols_outline_the_document() {
    let mut server = server_with_lib();
    let result = result(&mut server, 2, "symbols", json!({ "uri": "lib.tuo" }));
    let names: Vec<&str> = result["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"add"));
    assert!(names.contains(&"double"));
    // A function symbol carries its signature.
    let add = result["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "add")
        .unwrap();
    assert!(add["signature"].as_str().unwrap().contains("fn("));
}

#[test]
fn signature_shows_the_called_function() {
    let mut server = server_with_lib();
    // Just inside the argument list of `add(a, b)`.
    let call = LIB.find("add(a, b)").unwrap();
    let before = &LIB[..call + 4];
    let line = before.matches('\n').count() as u32 + 1;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let column = LIB[line_start..call + 4].chars().count() as u32 + 1;
    let result = result(
        &mut server,
        2,
        "signature",
        json!({ "uri": "lib.tuo", "position": { "line": line, "column": column } }),
    );
    assert!(result["label"].as_str().unwrap().contains("add"));
    assert_eq!(result["parameters"].as_array().unwrap().len(), 2);
}

#[test]
fn members_list_enum_variants() {
    let mut server = Server::new(Box::new(StubFormatter));
    let src = "\
enum Color {
    Red,
    Green,
    Blue,
}

fn pick() -> Color {
    Color::Red
}
";
    request(
        &mut server,
        1,
        "set_document",
        json!({ "uri": "e.tuo", "text": src }),
    );
    let result = result(
        &mut server,
        2,
        "members",
        json!({ "uri": "e.tuo", "position": position_of(src, "Color {") }),
    );
    let members: Vec<&str> = result["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["name"].as_str().unwrap())
        .collect();
    assert!(members.contains(&"Red"), "variants: {members:?}");
    assert!(members.contains(&"Green"));
    assert!(members.contains(&"Blue"));
}

#[test]
fn available_imports_list_public_items() {
    let mut server = server_with_lib();
    let result = result(&mut server, 2, "available_imports", Value::Null);
    let imports = result["imports"].as_array().unwrap();
    // Every entry names a public, module-level item with a kind.
    assert!(
        imports
            .iter()
            .all(|i| i["name"].is_string() && i["kind"].is_string())
    );
}

#[test]
fn specs_for_navigates_from_function_to_spec() {
    let mut server = server_with_lib();
    let result = result(
        &mut server,
        2,
        "specs_for",
        json!({ "uri": "lib.tuo", "position": position_of(LIB, "add") }),
    );
    assert_eq!(result["function"], "add");
    let specs = result["specs"].as_array().unwrap();
    assert_eq!(specs.len(), 1, "add has one attached spec");
}

#[test]
fn run_spec_executes_the_specs() {
    let mut server = server_with_lib();
    let result = result(&mut server, 2, "run_spec", json!({}));
    assert_eq!(result["passed"], true);
    assert_eq!(result["summary"]["ran"], 1);
    let run = &result["runs"][0];
    assert_eq!(run["name"], "add");
    assert_eq!(run["passed"], true);
    // The measured duration is reported (an observation, not a promise).
    assert!(run["duration_micros"].is_number());
}

#[test]
fn run_spec_refuses_a_broken_program() {
    let mut server = Server::new(Box::new(StubFormatter));
    request(
        &mut server,
        1,
        "set_document",
        json!({ "uri": "bad.tuo", "text": "fn main() -> Int {\n    undefined()\n}\n" }),
    );
    let response = request(&mut server, 2, "run_spec", json!({}));
    assert_eq!(response["ok"], false);
    assert_eq!(response["error"]["code"], "unavailable");
    // The front-end errors are attached so the agent can act on them.
    assert!(response["error"]["data"]["diagnostics"].is_array());
}

#[test]
fn verify_affected_runs_a_subset() {
    let mut server = server_with_lib();
    // Verify affected by an edit to lib.tuo — every spec whose deps touch a
    // symbol in lib.tuo. For a single-file program that is the whole set, but
    // the point is the selection path executes and yields a valid report.
    let result = result(
        &mut server,
        2,
        "verify",
        json!({ "affected_by": "lib.tuo" }),
    );
    assert_eq!(result["passed"], true);
}

#[test]
fn apply_safe_fix_offers_only_compiler_authored_fixes() {
    // A clean program has no fixes: the agent never invents one.
    let mut server = server_with_lib();
    let result = result(
        &mut server,
        2,
        "apply_safe_fix",
        json!({ "uri": "lib.tuo" }),
    );
    assert!(result["fixes"].as_array().unwrap().is_empty());
}

#[test]
fn format_uses_the_injected_formatter() {
    let mut server = server_with_lib();
    let result = result(&mut server, 2, "format", json!({ "text": "fn a(){}" }));
    // The stub reports canonical-unchanged; the real formatter is tested e2e.
    assert_eq!(result["text"], "fn a(){}");
    assert_eq!(result["safe"], true);
}

#[test]
fn an_edit_flows_through_the_shared_database() {
    // Prove database reuse: one server, successive edits, each query reads the
    // latest program. A first version with an error, then a fix.
    let mut server = Server::new(Box::new(StubFormatter));
    request(
        &mut server,
        1,
        "set_document",
        json!({ "uri": "m.tuo", "text": "fn main() -> Int {\n    gone()\n}\n" }),
    );
    let broken = result(&mut server, 2, "check", json!({}));
    assert_eq!(broken["accepted"], false);

    let fixed = "fn helper() -> Int {\n    1\n}\n\nfn main() -> Int {\n    helper()\n}\n";
    request(
        &mut server,
        3,
        "set_document",
        json!({ "uri": "m.tuo", "text": fixed }),
    );
    let clean = result(&mut server, 4, "check", json!({}));
    assert_eq!(clean["accepted"], true, "the fixed program is accepted");
    // The now-defined helper resolves after the edit.
    let def = result(
        &mut server,
        5,
        "definition",
        json!({ "uri": "m.tuo", "position": position_of(fixed, "helper()") }),
    );
    assert_eq!(def["range"]["start"]["line"], 1);
}

#[test]
fn responses_are_deterministic() {
    // The same request against the same state yields the same result (modulo
    // the measured spec duration, which lives in its own field). Two identical
    // `check` calls must be byte-identical.
    let mut server = server_with_lib();
    let first = result(&mut server, 2, "check", json!({}));
    let second = result(&mut server, 3, "check", json!({}));
    assert_eq!(first, second, "check is deterministic");

    // type_at is likewise deterministic.
    let pos = position_of(LIB, "double");
    let a = result(
        &mut server,
        4,
        "type_at",
        json!({ "uri": "lib.tuo", "position": pos }),
    );
    let b = result(
        &mut server,
        5,
        "type_at",
        json!({ "uri": "lib.tuo", "position": pos }),
    );
    assert_eq!(a, b, "type_at is deterministic");
}

#[test]
fn unknown_method_is_a_structured_error() {
    let mut server = server_with_lib();
    let response = request(&mut server, 2, "nonexistent", json!({}));
    assert_eq!(response["ok"], false);
    assert_eq!(response["error"]["code"], "unknown_method");
    assert_eq!(response["error"]["data"]["method"], "nonexistent");
}

#[test]
fn a_malformed_line_is_a_parse_error() {
    let mut server = server_with_lib();
    let response = server.handle_line("this is not json");
    let value = serde_json::to_value(response).unwrap();
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["code"], "parse_error");
}

#[test]
fn a_query_on_an_unopened_document_errors() {
    let mut server = Server::new(Box::new(StubFormatter));
    let response = request(
        &mut server,
        1,
        "diagnostics",
        json!({ "uri": "absent.tuo" }),
    );
    assert_eq!(response["ok"], false);
    assert_eq!(response["error"]["code"], "unknown_document");
}
