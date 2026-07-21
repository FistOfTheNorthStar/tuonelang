//! Guard for the ownership fixture corpus (`tests/ownership/fixtures/`).
//!
//! The ownership checker is not implemented yet, so this harness does not
//! run it. It enforces the corpus contract from ADR-0003 and
//! `tests/ownership/README.md` instead, so that the executable examples of
//! `specification/ownership.md` cannot rot before the checker lands:
//!
//! - every fixture — `ok/` and `err/` alike — must pass the front end
//!   (parse → resolve → type-check) with **zero** diagnostics, because
//!   `err/` programs fail only at the ownership stage;
//! - each corpus holds at least 100 cases (functions named `case_*`);
//! - `ok/` fixtures carry no `// ERROR:` annotations;
//! - in `err/` fixtures every case carries exactly one `// ERROR: O00NN …`
//!   annotation citing a diagnostic code defined in
//!   `specification/ownership.md` §15, and helper items carry none.

use std::fs;
use std::path::PathBuf;

use tuo_ast::Ast;
use tuo_source::SourceMap;

/// The ownership diagnostic codes fixed by `specification/ownership.md` §15.
const KNOWN_CODES: [&str; 9] = [
    "O0001", "O0002", "O0003", "O0004", "O0005", "O0006", "O0007", "O0008", "O0009",
];

/// Minimum number of cases each corpus must hold (ADR-0003).
const MIN_CASES: usize = 100;

fn corpus_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/ownership")
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

/// Assert the fixture passes the front end with zero diagnostics.
fn assert_front_end_clean(name: &str, text: &str) {
    let mut map = SourceMap::new();
    let file = map.intern_file("fixture.tuo");
    let id = map.add_source(file, text).expect("fixture fits");
    let parse = tuo_parser::parse(map.source(id));
    assert_eq!(
        parse.diagnostics,
        vec![],
        "{name}: ownership fixtures must parse cleanly"
    );
    let asts = [Ast::new(&parse.tree, text)];
    let resolution = tuo_resolve::resolve(&asts);
    assert_eq!(
        resolution.diagnostics(),
        &[],
        "{name}: ownership fixtures must resolve cleanly"
    );
    let result = tuo_types::check(&asts, &resolution);
    assert_eq!(
        result.diagnostics(),
        &[],
        "{name}: ownership fixtures target *ownership* errors, so they must \
         type-check cleanly"
    );
}

/// Split a fixture into the prelude (before the first case function) and one
/// chunk per case: a chunk runs from one `fn case_` line to the next, so any
/// helper items declared after a case belong to that case's chunk.
fn split_cases(text: &str) -> (String, Vec<String>) {
    let mut prelude = String::new();
    let mut cases: Vec<String> = Vec::new();
    for line in text.lines() {
        if line.starts_with("fn case_") {
            cases.push(String::new());
        }
        let target = cases.last_mut().unwrap_or(&mut prelude);
        target.push_str(line);
        target.push('\n');
    }
    (prelude, cases)
}

/// The `O`-codes cited by `// ERROR:` annotations in a chunk.
fn annotations(chunk: &str) -> Vec<&str> {
    chunk
        .lines()
        .filter_map(|line| line.split("// ERROR: ").nth(1))
        .map(|rest| rest.split_whitespace().next().unwrap_or(""))
        .collect()
}

#[test]
fn ok_fixtures_are_front_end_clean_and_unannotated() {
    let paths = fixture_paths("ok");
    let mut total = 0;
    for path in &paths {
        let name = path.file_name().expect("has name").to_string_lossy();
        let text = fs::read_to_string(path).expect("fixture is readable");
        assert_front_end_clean(&name, &text);
        assert!(
            !text.contains("// ERROR:"),
            "{name}: ok fixtures must not carry ERROR annotations"
        );
        let (_, cases) = split_cases(&text);
        assert!(!cases.is_empty(), "{name}: no `fn case_*` cases found");
        total += cases.len();
    }
    assert!(
        total >= MIN_CASES,
        "positive corpus shrank below {MIN_CASES} cases (found {total})"
    );
}

#[test]
fn err_fixtures_are_front_end_clean_and_fully_annotated() {
    let paths = fixture_paths("err");
    let mut total = 0;
    for path in &paths {
        let name = path.file_name().expect("has name").to_string_lossy();
        let text = fs::read_to_string(path).expect("fixture is readable");
        assert_front_end_clean(&name, &text);

        let (prelude, cases) = split_cases(&text);
        assert_eq!(
            annotations(&prelude),
            Vec::<&str>::new(),
            "{name}: shared helper items must not carry ERROR annotations"
        );
        assert!(!cases.is_empty(), "{name}: no `fn case_*` cases found");
        for chunk in &cases {
            let case_name = chunk
                .lines()
                .next()
                .unwrap_or("")
                .trim_start_matches("fn ")
                .split('(')
                .next()
                .unwrap_or("")
                .to_string();
            let codes = annotations(chunk);
            assert_eq!(
                codes.len(),
                1,
                "{name}: `{case_name}` must carry exactly one ERROR annotation"
            );
            assert!(
                KNOWN_CODES.contains(&codes[0]),
                "{name}: `{case_name}` cites unknown code `{}` — the valid set \
                 is defined in specification/ownership.md §15",
                codes[0]
            );
        }
        total += cases.len();
    }
    assert!(
        total >= MIN_CASES,
        "negative corpus shrank below {MIN_CASES} cases (found {total})"
    );
}

#[test]
fn every_documented_code_is_exercised() {
    let mut cited: Vec<String> = Vec::new();
    for path in fixture_paths("err") {
        let text = fs::read_to_string(&path).expect("fixture is readable");
        for code in annotations(&text) {
            cited.push(code.to_string());
        }
    }
    for code in KNOWN_CODES {
        assert!(
            cited.iter().any(|c| c == code),
            "diagnostic code {code} is documented in specification/ownership.md \
             §15 but no err fixture exercises it"
        );
    }
}
