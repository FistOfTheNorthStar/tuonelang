//! ADR-0028's `std::json` oracle: the document tree `std::json` stores as an
//! index arena, re-expressed as a recursive type over `Box`, must render
//! byte-identically to `std::json::render` and answer `std::json`'s own spec
//! questions the same way.
//!
//! The program is `tests/codegen/oracles/box_json.tuo`, compiled together with
//! the exact `std::json` source `tuo-stdlib` embeds (which this suite does not
//! modify). It converts each document's arena into a `Box` tree, renders the
//! tree — once through a deep copy taken with `std::boxed::get`, once by moving
//! it out with `std::boxed::into` — and compares both renderings with
//! `std::json::render`'s bytes. The corpus is every document `std::json`'s
//! specs parse, plus the json-parse workload's document; `main` returns the
//! number of documents that agree (27), or 100 + the index of the first that
//! does not.
//!
//! The check runs on all three engines: the module's specs and `main` on the
//! reference interpreter, and `main` natively through `tuo run` (Cranelift) and
//! `tuo run --release` (LLVM).

use std::path::PathBuf;
use std::process::{Command, Output};

use tuo_compiler::source::SourceMap;
use tuo_mir_interp::{Interpreter, Value};

/// What `main` returns when every corpus document renders alike.
const ALL_DOCUMENTS_AGREE: i128 = 27;

/// The oracle program's committed source.
const BOX_JSON: &str = include_str!("../../../tests/codegen/oracles/box_json.tuo");

/// A fresh per-test directory holding `std_json.tuo` (the embedded catalog
/// source) and `box_json.tuo`; returns the two paths in compile order.
fn workspace(name: &str) -> [PathBuf; 2] {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("box_json_oracle")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch workspace is creatable");
    let json = dir.join("std_json.tuo");
    std::fs::write(&json, tuo_stdlib::JSON.source).expect("std::json source is writable");
    let oracle = dir.join("box_json.tuo");
    std::fs::write(&oracle, BOX_JSON).expect("oracle source is writable");
    [json, oracle]
}

/// Run the `tuo` binary with `args` followed by the two source files.
fn tuo(args: &[&str], files: &[PathBuf; 2]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tuo"))
        .args(args)
        .args(files.iter().map(PathBuf::as_path))
        .output()
        .expect("the tuo binary runs")
}

/// `main`'s value on the reference interpreter, for the program made of
/// `std::json` and the oracle.
fn interpret_main(files: &[PathBuf; 2]) -> i128 {
    let mut map = SourceMap::new();
    let ids: Vec<_> = files
        .iter()
        .map(|path: &PathBuf| {
            let text = std::fs::read_to_string(path).expect("source is readable");
            let file = map.intern_file(&path.display().to_string());
            map.add_source(file, text.as_str())
                .expect("source is not too large")
        })
        .collect();
    let check = tuo_compiler::check_sources(&map, &ids);
    assert!(
        !check.has_errors(),
        "std::json plus the oracle must be accepted"
    );
    let parses: Vec<_> = ids
        .iter()
        .map(|&id| tuo_compiler::parser::parse(map.source(id)))
        .collect();
    let asts: Vec<_> = ids
        .iter()
        .zip(&parses)
        .map(|(&id, parse)| tuo_compiler::ast::Ast::new(&parse.tree, map.source(id).text()))
        .collect();
    let hir = tuo_compiler::hir::lower(&asts, &check.resolution);
    let program = tuo_compiler::mir::lower(&hir, &check.resolution, &check.types);
    let interpreter =
        Interpreter::new(&program, &check.types).expect("lowered MIR verifies for the interpreter");
    match interpreter.run("main", Vec::new()) {
        Ok(Value::Int(value, _)) => value,
        Ok(other) => panic!("main returned a non-integer value: {}", other.render()),
        Err(error) => panic!("main trapped on the interpreter: {error:?}"),
    }
}

/// Every spec in the oracle module (and every one of `std::json`'s own, which
/// the same program carries) passes on the reference interpreter, none skipped.
#[test]
fn the_box_tree_specs_pass_on_the_interpreter() {
    let files = workspace("spec");
    let output = tuo(&["spec"], &files);
    // The human report goes to stderr; stdout is reserved for protocol output.
    let report = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "`tuo spec` over std::json and the oracle must pass:\n{report}"
    );
    let oracle_specs = BOX_JSON.matches("\nspec ").count();
    let json_specs = tuo_stdlib::JSON.source.matches("\nspec ").count();
    let summary = format!("{} passed, 0 failed", oracle_specs + json_specs);
    assert!(
        report.contains(&summary),
        "expected `{summary}` (no spec skipped or failed):\n{report}"
    );
}

/// The Box tree renders every corpus document byte-identically to
/// `std::json::render` on all three engines.
#[test]
fn the_box_tree_renders_like_std_json_on_all_three_engines() {
    let files = workspace("engines");
    assert_eq!(
        interpret_main(&files),
        ALL_DOCUMENTS_AGREE,
        "interpreter: a value of 100 + i names corpus document i as the first mismatch"
    );
    for (backend, args) in [
        ("cranelift", &["run"][..]),
        ("llvm (--release)", &["run", "--release"][..]),
    ] {
        let output = tuo(args, &files);
        assert_eq!(
            output.status.code(),
            Some(i32::try_from(ALL_DOCUMENTS_AGREE).expect("small")),
            "{backend}: every corpus document renders alike natively (exit 100 + i names \
             document i as the first mismatch); stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
