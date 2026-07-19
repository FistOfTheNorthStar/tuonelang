//! Golden formatting suite over `tests/fmt/fixtures/`: each fixture's
//! canonical form is blessed into `tests/fmt/golden/` and every run checks
//! output equality, idempotence, and the formatter's safety guarantees.
//!
//! Bless with: `TUO_BLESS=1 cargo test -p tuo-fmt --test golden`

use std::fs;
use std::path::PathBuf;

use tuo_fmt::format_source;
use tuo_source::SourceMap;

fn corpus_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fmt")
}

fn format_str(text: &str) -> tuo_fmt::FormatOutcome {
    let mut map = SourceMap::new();
    let file = map.intern_file("golden.tuo");
    let id = map.add_source(file, text).expect("fixture fits");
    format_source(map.source(id))
}

#[test]
fn golden_corpus_is_canonical_and_idempotent() {
    let root = corpus_root();
    let mut entries: Vec<_> = fs::read_dir(root.join("fixtures"))
        .expect("fixture dir exists")
        .map(|entry| entry.expect("readable entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "tuo"))
        .collect();
    entries.sort();
    assert!(entries.len() >= 12, "fixture corpus went missing");

    for path in entries {
        let name = path.file_name().expect("has name").to_string_lossy();
        let text = fs::read_to_string(&path).expect("fixture is readable");
        let outcome = format_str(&text);
        assert!(outcome.safe, "{name}: formatter failed its own self-check");

        let golden_path = root.join("golden").join(name.as_ref());
        if std::env::var_os("TUO_BLESS").is_some() {
            fs::write(&golden_path, &outcome.text).expect("golden is writable");
        }
        let golden = fs::read_to_string(&golden_path).unwrap_or_else(|_| {
            panic!(
                "missing golden {} — run with TUO_BLESS=1 to create it",
                golden_path.display()
            )
        });
        assert_eq!(
            outcome.text, golden,
            "{name}: canonical output diverged — rerun with TUO_BLESS=1 after verifying"
        );

        // The required invariant: format(format(source)) == format(source).
        let second = format_str(&outcome.text);
        assert!(second.safe, "{name}: second pass failed the self-check");
        assert_eq!(
            second.text, outcome.text,
            "{name}: formatting is not idempotent"
        );
        assert!(!second.changed, "{name}: second pass reported changes");
    }
}

#[test]
fn golden_dir_has_no_orphans() {
    let root = corpus_root();
    for entry in fs::read_dir(root.join("golden")).expect("golden dir exists") {
        let path = entry.expect("readable entry").path();
        let name = path.file_name().expect("has name").to_owned();
        assert!(
            root.join("fixtures").join(&name).exists(),
            "golden file {name:?} has no fixture"
        );
    }
}
