//! Fixture corpus for the type checker (`tests/types/fixtures/`).
//!
//! `ok/` fixtures must resolve and type-check with **zero** diagnostics.
//! `err/` fixtures must resolve cleanly (they target *type* errors) and
//! their type diagnostics — code, span, message, and structured
//! expected/actual values — are snapshotted into `tests/types/snapshots/`.
//!
//! Bless with: `TUO_BLESS=1 cargo test -p tuo-types --test fixtures`

use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

use tuo_ast::Ast;
use tuo_source::SourceMap;

fn corpus_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/types")
}

struct Checked {
    resolution_diagnostics: usize,
    lines: Vec<String>,
}

fn check_fixture(text: &str) -> Checked {
    let mut map = SourceMap::new();
    let file = map.intern_file("fixture.tuo");
    let id = map.add_source(file, text).expect("fixture fits");
    let parse = tuo_parser::parse(map.source(id));
    assert_eq!(
        parse.diagnostics,
        vec![],
        "type fixtures must parse cleanly"
    );
    let asts = [Ast::new(&parse.tree, text)];
    let resolution = tuo_resolve::resolve(&asts);
    let result = tuo_types::check(&asts, &resolution);
    let lines = result
        .diagnostics()
        .iter()
        .map(|diagnostic| {
            let range = diagnostic.primary_span.range();
            let mut line = format!(
                "{}@{}..{} {}",
                diagnostic.code,
                range.start().as_usize(),
                range.end().as_usize(),
                diagnostic.message
            );
            for value in &diagnostic.expected {
                write!(line, " | expected {} {value}", value.kind()).expect("write to String");
            }
            for value in &diagnostic.actual {
                write!(line, " | actual {} {value}", value.kind()).expect("write to String");
            }
            line
        })
        .collect();
    Checked {
        resolution_diagnostics: resolution.diagnostics().len(),
        lines,
    }
}

fn fixture_paths(sub: &str) -> Vec<PathBuf> {
    let mut paths: Vec<_> = fs::read_dir(corpus_root().join("fixtures").join(sub))
        .expect("fixture dir exists")
        .map(|entry| entry.expect("readable entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "tuo"))
        .collect();
    paths.sort();
    paths
}

#[test]
fn ok_fixtures_type_check_cleanly() {
    let paths = fixture_paths("ok");
    assert!(paths.len() >= 9, "ok corpus went missing");
    for path in paths {
        let name = path.file_name().expect("has name").to_string_lossy();
        let text = fs::read_to_string(&path).expect("fixture is readable");
        let checked = check_fixture(&text);
        assert_eq!(
            checked.resolution_diagnostics, 0,
            "{name}: ok fixtures must resolve cleanly"
        );
        assert_eq!(
            checked.lines,
            Vec::<String>::new(),
            "{name}: ok fixtures must type-check cleanly"
        );
    }
}

#[test]
fn err_fixtures_match_their_blessed_snapshots() {
    let root = corpus_root();
    let paths = fixture_paths("err");
    assert!(paths.len() >= 10, "err corpus went missing");
    for path in paths {
        let name = path.file_name().expect("has name").to_string_lossy();
        let stem = path
            .file_stem()
            .expect("has stem")
            .to_string_lossy()
            .into_owned();
        let text = fs::read_to_string(&path).expect("fixture is readable");
        let checked = check_fixture(&text);
        assert_eq!(
            checked.resolution_diagnostics, 0,
            "{name}: err fixtures target type errors, so they must resolve cleanly"
        );
        assert!(
            !checked.lines.is_empty(),
            "{name}: err fixtures must produce type diagnostics"
        );
        let mut rendered = checked.lines.join("\n");
        rendered.push('\n');

        let snapshot_path = root.join("snapshots").join(format!("{stem}.snap"));
        if std::env::var_os("TUO_BLESS").is_some() {
            fs::write(&snapshot_path, &rendered).expect("snapshot is writable");
        }
        let blessed = fs::read_to_string(&snapshot_path).unwrap_or_else(|_| {
            panic!(
                "missing snapshot {} — run with TUO_BLESS=1 to create it",
                snapshot_path.display()
            )
        });
        assert_eq!(
            rendered, blessed,
            "{name}: diagnostics diverged from the blessed snapshot — rerun with \
             TUO_BLESS=1 after verifying the new output"
        );
    }
}

#[test]
fn snapshot_dir_has_no_orphans() {
    let root = corpus_root();
    for entry in fs::read_dir(root.join("snapshots")).expect("snapshot dir exists") {
        let path = entry.expect("readable entry").path();
        let stem = path
            .file_stem()
            .expect("has stem")
            .to_string_lossy()
            .into_owned();
        assert!(
            root.join("fixtures/err")
                .join(format!("{stem}.tuo"))
                .exists(),
            "snapshot {stem}.snap has no err fixture"
        );
    }
}
