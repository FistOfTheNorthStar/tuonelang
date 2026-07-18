//! Engine parity: the handwritten engine behind `tuo_parser::parse` must
//! produce the **same tree and the same diagnostics** as the retained
//! Chumsky oracle (`tuo_parser::oracle`) on the whole fixture corpus, and
//! uphold the same structural invariants on arbitrary input. This is the
//! recovery-quality evidence for `specification/adr/ADR-parser-strategy.md`.

use std::fs;
use std::path::PathBuf;

use tuo_parser::ParseResult;
use tuo_source::SourceMap;

fn parse_both(text: &str) -> (ParseResult, ParseResult) {
    let mut map = SourceMap::new();
    let file = map.intern_file("parity.tuo");
    let source = map.add_source(file, text).expect("input fits");
    (
        tuo_parser::oracle::parse(map.source(source)),
        tuo_parser::parse(map.source(source)),
    )
}

/// `(code, start offset, message)` for every diagnostic, in emitted order.
fn diagnostic_facts(result: &ParseResult) -> Vec<(String, usize, String)> {
    result
        .all_diagnostics()
        .iter()
        .map(|d| {
            (
                d.code.to_string(),
                d.primary_span.range().start().as_usize(),
                d.message.clone(),
            )
        })
        .collect()
}

/// Both engines must agree on the tree (rendered) and on every diagnostic's
/// code, location, and message; the prototype must additionally uphold the
/// losslessness invariants on its own.
fn assert_parity(text: &str, context: &str) {
    let (oracle, hand) = parse_both(text);
    hand.tree
        .check_coverage()
        .unwrap_or_else(|e| panic!("[{context}] prototype coverage violated: {e}"));
    assert_eq!(
        hand.tree.reconstruct(text),
        text,
        "[{context}] prototype reconstruction diverges"
    );
    assert_eq!(
        oracle.tree.render(text),
        hand.tree.render(text),
        "[{context}] trees diverge"
    );
    assert_eq!(
        diagnostic_facts(&oracle),
        diagnostic_facts(&hand),
        "[{context}] diagnostics diverge"
    );
}

fn corpus_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/parser/fixtures")
}

#[test]
fn every_fixture_parses_identically_on_both_engines() {
    let mut checked = 0;
    for sub in ["ok", "err"] {
        let dir = corpus_root().join(sub);
        let mut entries: Vec<_> = fs::read_dir(&dir)
            .expect("fixture dir exists")
            .map(|e| e.expect("readable entry").path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "tuo"))
            .collect();
        entries.sort();
        for path in entries {
            let text = fs::read_to_string(&path).expect("fixture is readable");
            assert_parity(&text, &path.display().to_string());
            checked += 1;
        }
    }
    assert!(
        checked >= 5,
        "fixture corpus went missing ({checked} files)"
    );
}

#[test]
fn the_benchmark_corpus_parses_identically_on_both_engines() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks/compiler/parser");
    let mut entries: Vec<_> = fs::read_dir(&dir)
        .expect("benchmark corpus exists")
        .map(|e| e.expect("readable entry").path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "tuo"))
        .collect();
    entries.sort();
    assert!(!entries.is_empty(), "benchmark corpus is empty");
    for path in entries {
        let text = fs::read_to_string(&path).expect("corpus file is readable");
        assert_parity(&text, &path.display().to_string());
    }
}

/// The recovery scenarios the Chumsky engine's tests pin down, re-checked as
/// engine-agreement properties (code, location, continuation, retention all
/// follow from equality).
#[test]
fn recovery_scenarios_agree() {
    for text in [
        "fn first() -> Int { 1 }\n\nfn broken( {\n\nstruct Point { x: F64 }\n\nfn last() -> Int { 2 }\n",
        "fn a() -> Int { 1 }\n\nfn bad1( {\n\nfn b() -> Int { 2 }\n\nstruct bad2 {{\n\nfn c() -> Int { 3 }\n",
        "fn f() -> Int {\n    let ok = 1;\n    let = broken;\n    var kept = 2;\n    ok + kept\n}\n",
        "fn f() {\n    ) ) )\n}\n\nfn g() -> Int { 4 }\n",
        "fn f() {\n    let x = ;\n}\n",
        "fn f() -> Int {\n    let x = 0b2;\n    7\n}\n",
        "fn ok() -> Int { 1 }\n\nfn open() {\n    let x = 2;\n",
        "fn a() { ! ; }\nfn b() { ) ; }\nfn c() { ] ; }\n",
        "spec s {\n    given x: Int = ;\n    assert 1 == 1;\n}\n",
        "interface I {\n    fn good(in self);\n    fn bad(;\n}\n",
        "impl Display for {\n    fn show(in self) -> Str { \"\" }\n}\n",
        "enum E { A { , B }\n\nfn after() -> Int { 9 }\n",
        "module broken::;\n\nfn f() { }\n",
        "fn f() {\n    match x {\n        1 | => 2,\n        _ => 3,\n    };\n}\n",
    ] {
        assert_parity(text, "recovery scenario");
    }
}

/// Fixed-seed xorshift so failures reproduce exactly.
struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

#[test]
fn random_token_fragments_agree() {
    const FRAGMENTS: &[&str] = &[
        "fn f",
        "() {",
        "}",
        "let x",
        "= 1;",
        "struct S {",
        "x: Int,",
        "spec s {",
        "assert ",
        "match e {",
        "=> 2,",
        "if a",
        "else",
        "loop",
        "'l:",
        "break",
        "|",
        "..",
        "::",
        "->",
        "impl",
        "for",
        "where T:",
        "pub",
        "///doc\n",
        "// c\n",
        "\"s\"",
        "0x2",
        ";",
        " ",
        "\n",
        "Box[",
        "]",
        "(",
        ")",
        "Point { x: 1 }",
        "as Int",
        "?",
        ".f",
        "self",
        "return 1",
    ];
    let mut rng = XorShift(0xADA9_0010);
    for case in 0..500 {
        let picks = (rng.next() % 16) as usize;
        let text: String = (0..picks)
            .map(|_| FRAGMENTS[(rng.next() as usize) % FRAGMENTS.len()])
            .collect();
        assert_parity(&text, &format!("fragment soup case {case}"));
    }
}

#[test]
fn random_byte_soup_agrees() {
    let mut rng = XorShift(0x9A55_0011);
    for case in 0..300 {
        let len = (rng.next() % 48) as usize;
        let bytes: Vec<u8> = (0..len).map(|_| (rng.next() % 256) as u8).collect();
        let text = String::from_utf8_lossy(&bytes);
        assert_parity(&text, &format!("byte soup case {case}"));
    }
}

#[test]
fn pathological_inputs_agree() {
    for pathological in [
        "(".repeat(50_000),
        "{".repeat(50_000),
        format!("fn f() {{ x {} 1; }}", "= x ".repeat(20_000)),
        "let x = 1; ".repeat(2_000),
        "fn f() { } ".repeat(500),
        String::new(),
    ] {
        assert_parity(&pathological, "pathological input");
    }
}
