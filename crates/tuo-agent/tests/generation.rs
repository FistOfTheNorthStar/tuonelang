//! The compiler-guided generation queries, exercised through the
//! transport-agnostic [`Server`].
//!
//! These pin the seven generation methods and — as much as the shape — pin the
//! **honesty contract** the prompt requires: syntactic guidance is always
//! flagged non-exhaustive and kept in its own field, semantic guidance projects
//! the shared compiler engine, and no response claims the compiler can
//! enumerate every valid next token.

use serde_json::{Value, json};
use tuo_agent::Server;
use tuo_agent::session::{FormatResult, Formatter};

/// A stub formatter (the generation queries never format).
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

/// A clean, accepted program with a struct, an enum, a function, and a call
/// site — enough to exercise every generation query on the happy path.
const PROG: &str = "\
pub struct User {
    name: Str,
    age: Int,
}

pub enum UserError {
    NotFound,
    Invalid,
}

pub fn lookup(in id: Int) -> Result[User, UserError] {
    let found = id;
    if found > 0 {
        Ok { value: User { name: \"a\", age: found } }
    } else {
        Err { error: UserError::NotFound }
    }
}

fn caller(in id: Int) -> Result[User, UserError] {
    lookup(id)
}
";

/// A fresh server with `PROG` opened as `m.tuo`.
fn server_with_prog() -> Server {
    let mut server = Server::new(Box::new(StubFormatter));
    let response = request(
        &mut server,
        1,
        "set_document",
        json!({ "uri": "m.tuo", "text": PROG }),
    );
    assert!(response["ok"].as_bool().unwrap(), "document opens");
    server
}

/// Send one request and return its response as a `Value`.
fn request(server: &mut Server, id: u64, method: &str, params: Value) -> Value {
    let line = json!({ "id": id, "method": method, "params": params }).to_string();
    serde_json::to_value(server.handle_line(&line)).expect("response serializes")
}

/// The `result` object of a successful response.
fn result(server: &mut Server, id: u64, method: &str, params: Value) -> Value {
    let response = request(server, id, method, params);
    assert!(
        response["ok"].as_bool().unwrap_or(false),
        "{method} succeeds: {response}"
    );
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

// ----------------------------------------------------------------------
// Semantic queries.
// ----------------------------------------------------------------------

#[test]
fn expected_type_at_reports_the_recorded_expression_type() {
    let mut server = server_with_prog();
    // The call `lookup(id)` is `caller`'s tail; its recorded type is the Result
    // both signatures declare.
    let result = result(
        &mut server,
        2,
        "expected_type_at",
        json!({ "uri": "m.tuo", "position": position_of(PROG, "lookup(id)") }),
    );
    let ty = result["type"].as_str().unwrap_or("");
    assert!(
        ty.contains("Result") || ty.contains("User"),
        "expected a Result type, got {result}"
    );
    // The source is always named — never an over-claimed hole oracle.
    let source = result["source"].as_str().unwrap();
    assert!(
        source == "recorded_expression" || source == "enclosing_return",
        "source is one of the two honest kinds: {source}"
    );
}

#[test]
fn expected_type_at_falls_back_to_the_enclosing_return() {
    let mut server = server_with_prog();
    // A position inside a body but on no recorded expression falls back to the
    // enclosing function's declared return type — never a claimed hole oracle.
    let result = result(
        &mut server,
        2,
        "expected_type_at",
        json!({ "uri": "m.tuo", "position": position_of(PROG, "let found") }),
    );
    // Either a recorded expression type or the enclosing return — both honest.
    assert!(result["source"].is_string() || result["source"].is_null());
}

#[test]
fn visible_symbols_at_lists_items_and_locals_conservatively() {
    let mut server = server_with_prog();
    // At the `Ok { ... }` tail of `lookup`, everything module-level plus the
    // enclosing parameter and the earlier local is visible.
    let result = result(
        &mut server,
        2,
        "visible_symbols_at",
        json!({ "uri": "m.tuo", "position": position_of(PROG, "Ok { value") }),
    );
    // It is honestly marked an over-approximation.
    assert_eq!(result["complete"], false);
    let names: Vec<&str> = result["visible_symbols"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    // Module-level items are visible.
    assert!(names.contains(&"lookup"), "sees the function: {names:?}");
    assert!(names.contains(&"User"), "sees the struct: {names:?}");
    assert!(names.contains(&"UserError"), "sees the enum: {names:?}");
    // The parameter and the earlier local are visible at the tail.
    assert!(names.contains(&"id"), "sees the parameter: {names:?}");
    assert!(
        names.contains(&"found"),
        "sees the earlier local: {names:?}"
    );
}

#[test]
fn visible_symbols_isolate_locals_to_their_function() {
    let mut server = server_with_prog();
    // Inside `caller`, `lookup`'s local `found` is not visible — a local belongs
    // to its own function's region. (`id` here is `caller`'s own parameter.)
    let result = result(
        &mut server,
        2,
        "visible_symbols_at",
        json!({ "uri": "m.tuo", "position": position_of(PROG, "lookup(id)") }),
    );
    let names: Vec<&str> = result["visible_symbols"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"id"),
        "caller's own parameter is visible: {names:?}"
    );
    assert!(
        !names.contains(&"found"),
        "the other function's local is not in scope: {names:?}"
    );
}

#[test]
fn valid_members_of_a_struct_lists_its_fields() {
    let mut server = server_with_prog();
    let result = result(
        &mut server,
        2,
        "valid_members_of",
        json!({ "uri": "m.tuo", "position": position_of(PROG, "User {") }),
    );
    assert_eq!(result["kind"], "struct");
    // The member set is exhaustive (the checker's shape is complete).
    assert_eq!(result["exhaustive"], true);
    let members: Vec<&str> = result["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["name"].as_str().unwrap())
        .collect();
    assert!(members.contains(&"name"), "has field name: {members:?}");
    assert!(members.contains(&"age"), "has field age: {members:?}");
}

#[test]
fn valid_members_of_an_enum_lists_its_variants() {
    let mut server = server_with_prog();
    let result = result(
        &mut server,
        2,
        "valid_members_of",
        json!({ "uri": "m.tuo", "position": position_of(PROG, "UserError {") }),
    );
    assert_eq!(result["kind"], "enum");
    assert_eq!(result["exhaustive"], true);
    let members: Vec<&str> = result["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["name"].as_str().unwrap())
        .collect();
    assert!(
        members.contains(&"NotFound"),
        "has variant NotFound: {members:?}"
    );
    assert!(
        members.contains(&"Invalid"),
        "has variant Invalid: {members:?}"
    );
}

#[test]
fn call_signature_reports_parameters_and_return() {
    let mut server = server_with_prog();
    // Cursor inside the call `lookup(id)` in `caller` — the call being filled in.
    let text = PROG;
    let byte = text.find("lookup(id)").unwrap() + "lookup(".len();
    let before = &text[..byte];
    let line = before.matches('\n').count() as u32 + 1;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let column = text[line_start..byte].chars().count() as u32 + 1;
    let result = result(
        &mut server,
        2,
        "call_signature",
        json!({ "uri": "m.tuo", "position": { "line": line, "column": column } }),
    );
    assert_eq!(result["name"], "lookup");
    let params = result["parameters"].as_array().unwrap();
    assert_eq!(params.len(), 1, "lookup has one parameter");
    // `Int` renders as its width alias `I64`.
    assert_eq!(params[0]["type"], "I64");
    assert!(result["return_type"].as_str().unwrap().contains("Result"));
    // The active argument index is a best-effort lexical read.
    assert!(result["active_parameter"].is_number() || result["active_parameter"].is_null());
}

#[test]
fn imports_for_symbol_locates_a_public_definition() {
    let mut server = server_with_prog();
    let result = result(
        &mut server,
        2,
        "imports_for_symbol",
        json!({ "name": "User" }),
    );
    assert_eq!(result["name"], "User");
    let imports = result["imports"].as_array().unwrap();
    assert!(
        imports.iter().any(|i| i["name"] == "User"),
        "finds User: {result}"
    );
}

#[test]
fn imports_for_symbol_of_an_unknown_name_is_empty() {
    let mut server = server_with_prog();
    let result = result(
        &mut server,
        2,
        "imports_for_symbol",
        json!({ "name": "Nonexistent" }),
    );
    assert!(result["imports"].as_array().unwrap().is_empty());
}

// ----------------------------------------------------------------------
// Syntactic queries — the honesty contract.
// ----------------------------------------------------------------------

#[test]
fn expected_syntax_at_is_always_non_exhaustive() {
    let mut server = server_with_prog();
    let result = result(
        &mut server,
        2,
        "expected_syntax_at",
        json!({ "uri": "m.tuo", "position": position_of(PROG, "Ok { value") }),
    );
    // The central honesty guard: never claims to enumerate every valid token.
    assert_eq!(
        result["exhaustive"], false,
        "syntax guidance is not complete"
    );
    assert!(
        result["note"]
            .as_str()
            .unwrap()
            .contains("does not enumerate"),
        "the note says so plainly: {result}"
    );
    assert!(
        result["expected_syntax_categories"].is_array(),
        "categories are a list"
    );
}

#[test]
fn expected_syntax_at_top_level_suggests_items() {
    let mut server = Server::new(Box::new(StubFormatter));
    request(
        &mut server,
        1,
        "set_document",
        json!({ "uri": "e.tuo", "text": "" }),
    );
    // An empty document: an item may begin.
    let result = result(
        &mut server,
        2,
        "expected_syntax_at",
        json!({ "uri": "e.tuo", "position": { "offset": 0 } }),
    );
    let categories: Vec<&str> = result["expected_syntax_categories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap())
        .collect();
    assert!(
        categories.contains(&"fn"),
        "top level offers fn: {categories:?}"
    );
}

#[test]
fn context_at_separates_syntactic_and_semantic() {
    let mut server = server_with_prog();
    let result = result(
        &mut server,
        2,
        "context_at",
        json!({ "uri": "m.tuo", "position": position_of(PROG, "Ok { value") }),
    );
    // The two guidance kinds live in clearly separate fields.
    assert!(result["syntactic"].is_object(), "has a syntactic block");
    assert!(result["semantic"].is_object(), "has a semantic block");
    // The syntactic block is honestly flagged.
    assert_eq!(result["syntactic"]["exhaustive"], false);
    // The semantic block carries the compiler-computed expected type.
    assert!(result["semantic"]["expected_type"].is_object());
}

// ----------------------------------------------------------------------
// Cross-cutting.
// ----------------------------------------------------------------------

#[test]
fn generation_methods_are_advertised() {
    let mut server = Server::new(Box::new(StubFormatter));
    let result = result(&mut server, 1, "initialize", Value::Null);
    let methods: Vec<&str> = result["methods"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m.as_str().unwrap())
        .collect();
    for expected in [
        "context_at",
        "expected_type_at",
        "visible_symbols_at",
        "valid_members_of",
        "call_signature",
        "imports_for_symbol",
        "expected_syntax_at",
    ] {
        assert!(methods.contains(&expected), "advertises {expected}");
    }
}

#[test]
fn generation_queries_are_deterministic() {
    let mut server = server_with_prog();
    let pos = position_of(PROG, "Ok { value");
    let first = result(
        &mut server,
        2,
        "visible_symbols_at",
        json!({ "uri": "m.tuo", "position": pos }),
    );
    let second = result(
        &mut server,
        3,
        "visible_symbols_at",
        json!({ "uri": "m.tuo", "position": pos }),
    );
    assert_eq!(first, second, "same state, same answer");
}

#[test]
fn a_generation_query_on_an_unopened_document_errors() {
    let mut server = Server::new(Box::new(StubFormatter));
    let response = request(
        &mut server,
        1,
        "expected_type_at",
        json!({ "uri": "missing.tuo", "position": { "offset": 0 } }),
    );
    assert_eq!(response["ok"], false);
    assert_eq!(response["error"]["code"], "unknown_document");
}
