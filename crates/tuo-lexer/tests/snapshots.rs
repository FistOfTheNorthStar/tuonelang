//! Fixture + snapshot tests over the shared lexer corpus.
//!
//! Every `*.tuo` file in the repository's `tests/lexer/fixtures/` is lexed
//! and its full token stream (kinds, byte-exact ranges, lexemes) plus
//! rendered diagnostics are compared byte-for-byte against
//! `tests/lexer/snapshots/<name>.snap`. To regenerate after an intentional
//! change, run:
//!
//! ```sh
//! TUO_BLESS=1 cargo test -p tuo-lexer --test snapshots
//! ```
//!
//! and review the diff like any other code change.

use std::fmt::Write as _;
use std::path::PathBuf;

use tuo_diagnostics::render::render_all;
use tuo_lexer::lex;
use tuo_source::SourceMap;

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/lexer")
}

/// Render one fixture's lex result as the snapshot text.
fn snapshot(name: &str, text: &str) -> String {
    let mut map = SourceMap::new();
    let file = map.intern_file(name);
    let source = map.add_source(file, text).expect("fixture fits");
    let result = lex(map.source(source));

    let mut out = String::new();
    for token in &result.tokens {
        let range = token.range();
        let lexeme = &text[range.start().as_usize()..range.end().as_usize()];
        writeln!(
            out,
            "{:>5}..{:<5} {:<13} {:?}",
            range.start().as_u32(),
            range.end().as_u32(),
            format!("{:?}", token.kind),
            lexeme,
        )
        .expect("write to String cannot fail");
    }
    if !result.diagnostics.is_empty() {
        out.push_str("--- diagnostics ---\n");
        out.push_str(&render_all(&result.diagnostics, &map));
    }
    out
}

fn check_fixture(stem: &str) {
    let fixture = corpus_dir().join(format!("fixtures/{stem}.tuo"));
    let snap_path = corpus_dir().join(format!("snapshots/{stem}.snap"));
    let text = std::fs::read_to_string(&fixture)
        .unwrap_or_else(|e| panic!("read fixture {}: {e}", fixture.display()));
    let actual = snapshot(&format!("{stem}.tuo"), &text);

    if std::env::var_os("TUO_BLESS").is_some() {
        std::fs::write(&snap_path, &actual).expect("write blessed snapshot");
        return;
    }
    let expected = std::fs::read_to_string(&snap_path)
        .unwrap_or_else(|e| panic!("missing snapshot {stem}.snap: {e} (bless with TUO_BLESS=1)"));
    assert_eq!(
        actual, expected,
        "token stream diverged from snapshot `{stem}.snap` (bless with TUO_BLESS=1 if intentional)"
    );
}

/// Every fixture in the corpus has a snapshot, and nothing is silently
/// skipped: a fixture without a matching test below fails here.
#[test]
fn corpus_is_fully_covered() {
    let mut stems: Vec<String> = std::fs::read_dir(corpus_dir().join("fixtures"))
        .expect("corpus fixtures dir exists")
        .map(|entry| entry.expect("readable dir entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "tuo"))
        .map(|p| {
            p.file_stem()
                .expect("fixture has a stem")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    stems.sort();
    assert_eq!(
        stems,
        ["errors", "hello", "showcase", "unicode"],
        "new fixture? add a #[test] fn for it in snapshots.rs"
    );
}

#[test]
fn hello() {
    check_fixture("hello");
}

#[test]
fn showcase() {
    check_fixture("showcase");
}

#[test]
fn unicode() {
    check_fixture("unicode");
}

#[test]
fn errors() {
    check_fixture("errors");
}
