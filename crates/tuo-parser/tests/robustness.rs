//! Deterministic robustness sweep: `parse` must never panic and must uphold
//! its structural invariants on arbitrary input — byte soup, token
//! fragments, and pathological nesting alike.

use tuo_parser::{ParseResult, parse};
use tuo_source::SourceMap;

fn parse_str(text: &str) -> ParseResult {
    let mut map = SourceMap::new();
    let file = map.intern_file("fuzz.tuo");
    let source = map.add_source(file, text).expect("input fits");
    parse(map.source(source))
}

fn check_invariants(text: &str) {
    let result = parse_str(text);
    // Losslessness: full coverage and byte-identical reconstruction, no
    // matter how broken the input is.
    result
        .tree
        .check_coverage()
        .unwrap_or_else(|e| panic!("coverage violated on {text:?}: {e}"));
    assert_eq!(result.tree.reconstruct(text), text, "input {text:?}");
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
fn random_byte_soup_upholds_the_invariants() {
    let mut rng = XorShift(0x9A55_0002);
    for _ in 0..500 {
        let len = (rng.next() % 48) as usize;
        let bytes: Vec<u8> = (0..len).map(|_| (rng.next() % 256) as u8).collect();
        let text = String::from_utf8_lossy(&bytes);
        check_invariants(&text);
    }
}

#[test]
fn random_token_fragments_uphold_the_invariants() {
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
    ];
    let mut rng = XorShift(0xC0FF_EE03);
    for _ in 0..500 {
        let picks = (rng.next() % 16) as usize;
        let text: String = (0..picks)
            .map(|_| FRAGMENTS[(rng.next() as usize) % FRAGMENTS.len()])
            .collect();
        check_invariants(&text);
    }
}

#[test]
fn pathological_nesting_is_rejected_not_overflowed() {
    for pathological in [
        "(".repeat(50_000),
        "[".repeat(50_000),
        "{".repeat(50_000),
        "!".repeat(50_000),
        format!("fn f() {{ x {} 1; }}", "= x ".repeat(20_000)),
        format!("fn f() {{ {} x; }}", "return ".repeat(20_000)),
    ] {
        let result = parse_str(&pathological);
        assert!(result.has_errors(), "pathological input must diagnose");
        check_invariants(&pathological);
    }
}

#[test]
fn pathological_flat_inputs_terminate() {
    // Wide, non-nesting inputs must parse (or recover) without blowing up.
    check_invariants(&"let x = 1; ".repeat(2_000));
    check_invariants(&"fn f() { } ".repeat(500));
    check_invariants(&"1 + ".repeat(2_000));
    check_invariants(&";".repeat(5_000));
    check_invariants("");
}
