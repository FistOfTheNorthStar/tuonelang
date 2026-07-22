//! Golden MIR suite over `tests/mir/golden/`: each fixture's lowered MIR
//! is blessed into a `.mir` sibling and every run checks output equality
//! and lowering determinism. Fixtures must be front-end clean — MIR is
//! only defined for accepted programs.
//!
//! Bless with: `TUO_BLESS=1 cargo test -p tuo-mir --test golden`

use std::fs;
use std::path::{Path, PathBuf};

use tuo_ast::Ast;
use tuo_source::SourceMap;

fn corpus_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/mir/golden")
}

/// Run the full front end and MIR lowering; panics on any front-end
/// diagnostic (fixtures must be accepted programs).
fn lower_to_text(name: &str, text: &str) -> String {
    let mut map = SourceMap::new();
    let file = map.intern_file(name);
    let id = map.add_source(file, text).expect("fixture fits");
    let parse = tuo_parser::parse(map.source(id));
    assert_eq!(
        parse.diagnostics,
        vec![],
        "{name}: fixture has parse errors"
    );
    let asts = [Ast::new(&parse.tree, text)];
    let resolution = tuo_resolve::resolve(&asts);
    assert_eq!(
        resolution.diagnostics(),
        &[],
        "{name}: fixture has resolution errors"
    );
    let types = tuo_types::check(&asts, &resolution);
    assert_eq!(types.diagnostics(), &[], "{name}: fixture has type errors");
    let ownership = tuo_ownership::check(&asts, &resolution, &types);
    assert_eq!(
        ownership.diagnostics(),
        &[],
        "{name}: fixture has ownership errors"
    );
    let hir = tuo_hir::lower(&asts, &resolution);
    let program = tuo_mir::lower(&hir, &resolution, &types);
    tuo_mir::render(&program, &resolution)
}

fn fixtures(root: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = fs::read_dir(root)
        .expect("golden dir exists")
        .map(|entry| entry.expect("readable entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "tuo"))
        .collect();
    entries.sort();
    entries
}

#[test]
fn golden_corpus_matches_blessed_mir() {
    let root = corpus_root();
    let entries = fixtures(&root);
    assert!(entries.len() >= 5, "golden corpus went missing");

    for path in entries {
        let name = path.file_name().expect("has name").to_string_lossy();
        let text = fs::read_to_string(&path).expect("fixture is readable");
        let rendered = lower_to_text(&name, &text);

        // Determinism: lowering the same program twice renders equally.
        assert_eq!(
            rendered,
            lower_to_text(&name, &text),
            "{name}: MIR lowering is nondeterministic"
        );

        let golden_path = path.with_extension("mir");
        if std::env::var_os("TUO_BLESS").is_some() {
            fs::write(&golden_path, &rendered).expect("golden is writable");
        }
        let golden = fs::read_to_string(&golden_path).unwrap_or_else(|_| {
            panic!(
                "missing golden {} — run with TUO_BLESS=1 to create it",
                golden_path.display()
            )
        });
        assert_eq!(
            rendered, golden,
            "{name}: lowered MIR diverged — rerun with TUO_BLESS=1 after verifying"
        );
    }
}

#[test]
fn golden_dir_has_no_orphans() {
    let root = corpus_root();
    for entry in fs::read_dir(&root).expect("golden dir exists") {
        let path = entry.expect("readable entry").path();
        if path.extension().is_some_and(|ext| ext == "mir") {
            assert!(
                path.with_extension("tuo").exists(),
                "golden file {:?} has no fixture",
                path.file_name()
            );
        }
    }
}

/// Every accepted program of the ownership corpus must lower — or be
/// skipped for one of the *documented* v0 reasons. Nothing may panic,
/// and no reason outside the documented set may appear.
#[test]
fn ownership_ok_corpus_lowers_or_skips_for_documented_reasons() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/ownership/fixtures/ok");
    let documented = [
        "method calls are not lowered in v0",
        "calls through function-typed values are not lowered in v0",
        "function-typed values are not lowered in v0",
        "`const` references are lowered only for literal initializers in v0",
        "destructuring `let` patterns are not lowered in v0",
        "destructuring `for` patterns are not lowered in v0",
        "or-patterns that bind are not lowered in v0",
        "match guards on arms that bind non-`Copy` values are not lowered in v0",
        "iterating an array of non-`Copy` elements is not lowered in v0",
        "impl functions are not lowered in v0",
    ];
    let mut lowered = 0usize;
    let mut skipped = 0usize;
    for path in fixtures(&root) {
        let name = path.file_name().expect("has name").to_string_lossy();
        let text = fs::read_to_string(&path).expect("fixture is readable");
        let mut map = SourceMap::new();
        let file = map.intern_file(&name);
        let id = map.add_source(file, text.as_str()).expect("fixture fits");
        let parse = tuo_parser::parse(map.source(id));
        let asts = [Ast::new(&parse.tree, text.as_str())];
        let resolution = tuo_resolve::resolve(&asts);
        let types = tuo_types::check(&asts, &resolution);
        let hir = tuo_hir::lower(&asts, &resolution);
        let program = tuo_mir::lower(&hir, &resolution, &types);
        lowered += program.functions.len();
        skipped += program.skipped.len();
        for skip in &program.skipped {
            assert!(
                documented
                    .iter()
                    .any(|reason| skip.reason.starts_with(reason)),
                "{name}: fn {} skipped for an undocumented reason: {}",
                skip.name,
                skip.reason
            );
        }
    }
    assert!(
        lowered > 120,
        "suspiciously few lowered functions ({lowered})"
    );
    // The v0 subset covers the whole accepted ownership corpus; only a
    // small tail of constructs (method calls, indirect calls, …) skips.
    assert!(
        skipped <= lowered / 10,
        "suspiciously many skipped functions ({skipped} of {lowered})"
    );
}
