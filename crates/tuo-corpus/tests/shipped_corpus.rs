//! The trust guarantee, enforced: every program shipped under the top-level
//! `corpus/` directory must be **admissible to the category its directory
//! names**. A fixture that stopped meeting its contract — a "correct" program
//! that no longer type-checks, a "type_repair" program that now fails at parse —
//! fails this test, so the corpus can never silently drift into dishonesty.
//!
//! Native execution is not exercised here (it is skipped): this test proves the
//! *semantic* contract of each category over the shipped sources, hermetically
//! and without a linker. The CLI's `corpus_command` test proves native execution
//! end to end.

use std::fs;
use std::path::{Path, PathBuf};

use tuo_corpus::{Candidate, Category, Config, Origin, SourceFile, admit};

/// The workspace-root `corpus/` directory. This test lives at
/// `crates/tuo-corpus/tests/`, so the workspace root is three levels up.
fn corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent() // crates/
        .and_then(Path::parent) // workspace root
        .expect("tuo-corpus lives under <workspace>/crates/")
        .join("corpus")
}

/// Every category and the directory it is stored under.
fn categories() -> [(Category, &'static str); 6] {
    [
        (Category::Correct, "correct"),
        (Category::SyntaxRepair, "syntax_repair"),
        (Category::TypeRepair, "type_repair"),
        (Category::OwnershipRepair, "ownership_repair"),
        (Category::SpecRepair, "spec_repair"),
        (Category::RepositoryChange, "repository_change"),
    ]
}

/// Read `.tuo` files directly under `dir` (one file = one single-file
/// candidate), and one candidate per immediate subdirectory (all its `.tuo`
/// files together = one multi-file candidate). Returns `(label, files)` pairs.
fn read_candidates(dir: &Path) -> Vec<(String, Vec<SourceFile>)> {
    let mut candidates = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return candidates;
    };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            // A multi-file candidate: every `.tuo` under this subdirectory.
            let mut files = read_tuo_files(&path);
            files.sort_by(|a, b| a.name.cmp(&b.name));
            if !files.is_empty() {
                candidates.push((path.display().to_string(), files));
            }
        } else if path.extension().is_some_and(|e| e == "tuo") {
            let text = fs::read_to_string(&path).expect("read fixture");
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            candidates.push((
                path.display().to_string(),
                vec![SourceFile::new(name, text)],
            ));
        }
    }
    candidates
}

/// Every `.tuo` file directly under `dir` as a `SourceFile`.
fn read_tuo_files(dir: &Path) -> Vec<SourceFile> {
    let mut files = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return files;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "tuo") {
            let text = fs::read_to_string(&path).expect("read fixture");
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            files.push(SourceFile::new(name, text));
        }
    }
    files
}

#[test]
fn every_shipped_fixture_is_admissible_to_its_category() {
    let root = corpus_root();
    let mut total = 0;
    for (category, dir_name) in categories() {
        let dir = root.join(dir_name);
        let candidates = read_candidates(&dir);
        for (label, files) in candidates {
            let candidate = Candidate {
                category,
                origin: Origin::Human,
                files,
                fixed: None,
            };
            // Native execution is intentionally not injected here (skipped).
            let result = admit(&candidate, Config::default(), None);
            assert!(
                result.is_ok(),
                "fixture `{label}` is not admissible to the `{}` corpus: {:?}",
                category.id(),
                result.err(),
            );
            total += 1;
        }
    }
    // Guard against a silently-empty corpus directory tree.
    assert!(
        total >= 6,
        "expected the shipped corpus to hold fixtures across categories, found {total}",
    );
}
