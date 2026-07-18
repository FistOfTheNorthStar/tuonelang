//! Fixture + snapshot tests over the shared parser corpus.
//!
//! Fixtures live in the repository's `tests/parser/fixtures/`, split into
//! `ok/` (must parse without diagnostics) and `err/` (must produce
//! diagnostics *and* keep the surrounding constructs). Every fixture's tree
//! and diagnostics are compared byte-for-byte against
//! `tests/parser/snapshots/<name>.snap`; regenerate with:
//!
//! ```sh
//! TUO_BLESS=1 cargo test -p tuo-parser --test snapshots
//! ```
//!
//! Every fixture, of either polarity, must additionally satisfy the
//! losslessness invariants: full token coverage and exact text
//! reconstruction.

use std::path::PathBuf;

use tuo_diagnostics::render::render_all;
use tuo_parser::{ParseResult, parse};
use tuo_source::SourceMap;

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/parser")
}

fn run_fixture(polarity: &str, stem: &str) -> (ParseResult, String) {
    let fixture = corpus_dir().join(format!("fixtures/{polarity}/{stem}.tuo"));
    let text = std::fs::read_to_string(&fixture)
        .unwrap_or_else(|e| panic!("read fixture {}: {e}", fixture.display()));
    let mut map = SourceMap::new();
    let file = map.intern_file(&format!("{stem}.tuo"));
    let source = map.add_source(file, text.as_str()).expect("fixture fits");
    let result = parse(map.source(source));

    // Invariants that hold for every fixture, valid or broken.
    result
        .tree
        .check_coverage()
        .unwrap_or_else(|e| panic!("{stem}: losslessness violated: {e}"));
    assert_eq!(
        result.tree.reconstruct(&text),
        text,
        "{stem}: reconstruction must be byte-identical"
    );

    let mut snapshot = result.tree.render(&text);
    let all = result.all_diagnostics();
    if !all.is_empty() {
        snapshot.push_str("--- diagnostics ---\n");
        snapshot.push_str(&render_all(&all, &map));
    }
    (result, snapshot)
}

fn assert_snapshot(stem: &str, actual: &str) {
    let path = corpus_dir().join(format!("snapshots/{stem}.snap"));
    if std::env::var_os("TUO_BLESS").is_some() {
        std::fs::write(&path, actual).expect("write blessed snapshot");
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("missing snapshot {stem}.snap: {e} (bless with TUO_BLESS=1)"));
    assert_eq!(
        actual, expected,
        "tree diverged from snapshot `{stem}.snap` (bless with TUO_BLESS=1 if intentional)"
    );
}

fn check_ok(stem: &str) {
    let (result, snapshot) = run_fixture("ok", stem);
    assert!(
        !result.has_errors(),
        "{stem}: positive fixture must parse cleanly, got {:#?}",
        result.all_diagnostics()
    );
    assert_snapshot(stem, &snapshot);
}

fn check_err(stem: &str) {
    let (result, snapshot) = run_fixture("err", stem);
    assert!(
        result.has_errors(),
        "{stem}: negative fixture must produce diagnostics"
    );
    assert_snapshot(stem, &snapshot);
}

/// Every fixture in the corpus is exercised by a test below; a new fixture
/// without a matching test fails here.
#[test]
fn corpus_is_fully_covered() {
    let mut stems: Vec<String> = Vec::new();
    for polarity in ["ok", "err"] {
        for entry in std::fs::read_dir(corpus_dir().join("fixtures").join(polarity))
            .expect("corpus dir exists")
        {
            let path = entry.expect("readable dir entry").path();
            if path.extension().is_some_and(|e| e == "tuo") {
                stems.push(format!(
                    "{polarity}/{}",
                    path.file_stem().expect("has stem").to_string_lossy()
                ));
            }
        }
    }
    stems.sort();
    assert_eq!(
        stems,
        [
            "err/broken_items",
            "err/broken_statements",
            "err/unclosed",
            "ok/hello",
            "ok/showcase",
        ],
        "new fixture? add a #[test] fn for it in snapshots.rs"
    );
}

#[test]
fn hello() {
    check_ok("hello");
}

#[test]
fn showcase() {
    check_ok("showcase");
}

#[test]
fn broken_items() {
    check_err("broken_items");
}

#[test]
fn broken_statements() {
    check_err("broken_statements");
}

#[test]
fn unclosed() {
    check_err("unclosed");
}
