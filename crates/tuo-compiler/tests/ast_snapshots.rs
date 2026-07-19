//! Golden snapshots of the typed-AST debug rendering (`tuo debug ast`'s
//! output) over the shared parser fixture corpus in `tests/parser/fixtures/`.
//! Snapshots live next to the CST snapshots as `<name>.ast.snap`.
//!
//! Bless with: `TUO_BLESS=1 cargo test -p tuo-compiler --test ast_snapshots`

use std::fs;
use std::path::PathBuf;

use tuo_compiler::ast::{self, Ast};
use tuo_compiler::parser::parse;
use tuo_compiler::source::SourceMap;

fn corpus_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/parser")
}

fn rendered_ast(text: &str) -> String {
    let mut map = SourceMap::new();
    let file = map.intern_file("snapshot.tuo");
    let source = map.add_source(file, text).expect("fixture fits");
    let result = parse(map.source(source));
    ast::render(Ast::new(&result.tree, text))
}

#[test]
fn ast_snapshots_match_the_corpus() {
    let root = corpus_root();
    let mut checked = 0;
    for sub in ["ok", "err"] {
        let dir = root.join("fixtures").join(sub);
        let mut entries: Vec<_> = fs::read_dir(&dir)
            .expect("fixture dir exists")
            .map(|entry| entry.expect("readable entry").path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "tuo"))
            .collect();
        entries.sort();
        for path in entries {
            let stem = path.file_stem().expect("has stem").to_string_lossy();
            let text = fs::read_to_string(&path).expect("fixture is readable");
            let actual = rendered_ast(&text);
            let snap_path = root
                .join("snapshots")
                .join(format!("{sub}--{stem}.ast.snap"));
            if std::env::var_os("TUO_BLESS").is_some() {
                fs::write(&snap_path, &actual).expect("snapshot is writable");
            }
            let expected = fs::read_to_string(&snap_path).unwrap_or_else(|_| {
                panic!(
                    "missing snapshot {} — run with TUO_BLESS=1 to create it",
                    snap_path.display()
                )
            });
            assert_eq!(
                actual,
                expected,
                "AST snapshot diverged for {} — rerun with TUO_BLESS=1 after verifying",
                path.display()
            );
            checked += 1;
        }
    }
    assert!(
        checked >= 5,
        "fixture corpus went missing ({checked} files)"
    );
}
