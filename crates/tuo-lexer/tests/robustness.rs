//! Deterministic robustness sweep: the lexer's invariants must hold on
//! arbitrary input, not just well-formed programs.
//!
//! This is the stable-toolchain companion to the coverage-guided fuzz target
//! in `fuzz/fuzz_targets/lex.rs` (which needs nightly + `cargo fuzz`). It
//! drives the same invariants over a few thousand pseudo-random inputs from
//! a fixed-seed generator, so it runs — reproducibly — in ordinary CI.

use tuo_lexer::{LexResult, TokenKind, lex};
use tuo_source::SourceMap;

fn lex_str(text: &str) -> LexResult {
    let mut map = SourceMap::new();
    let file = map.intern_file("fuzz.tuo");
    let source = map.add_source(file, text).expect("input fits");
    lex(map.source(source))
}

/// The invariants any input must satisfy. Mirrors the fuzz target.
fn check_invariants(text: &str) {
    let result = lex_str(text);

    // 1. The stream is never empty and always ends with a zero-width EOF at
    //    the end of input.
    let (eof, rest) = result.tokens.split_last().expect("never empty");
    assert_eq!(eof.kind, TokenKind::Eof, "input {text:?}");
    assert_eq!(eof.range().start().as_usize(), text.len());
    assert!(eof.range().is_empty());

    // 2. Losslessness: non-EOF tokens tile the input exactly, in order, with
    //    no gaps, overlaps, or zero-width tokens.
    let mut cursor = 0;
    for token in rest {
        assert_eq!(token.range().start().as_usize(), cursor, "input {text:?}");
        assert!(!token.range().is_empty(), "input {text:?}");
        cursor = token.range().end().as_usize();
        // 3. Every range lies on char boundaries (slicing must not panic).
        let _ = &text[token.range().start().as_usize()..token.range().end().as_usize()];
    }
    assert_eq!(cursor, text.len(), "input {text:?}");

    // 4. Diagnostics and Error tokens appear together or not at all.
    let has_error_token = rest.iter().any(|t| t.kind == TokenKind::Error);
    assert_eq!(
        has_error_token,
        result.has_errors(),
        "error tokens and diagnostics must accompany each other: {text:?}"
    );
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
    let mut rng = XorShift(0x5EED_0001);
    for _ in 0..2_000 {
        let len = (rng.next() % 64) as usize;
        let bytes: Vec<u8> = (0..len).map(|_| (rng.next() % 256) as u8).collect();
        // Arbitrary bytes; keep only the valid-UTF-8 prefix mixes via lossy
        // conversion (the compiler only ever lexes `str`).
        let text = String::from_utf8_lossy(&bytes);
        check_invariants(&text);
    }
}

#[test]
fn random_token_fragments_uphold_the_invariants() {
    const FRAGMENTS: &[&str] = &[
        "fn",
        "let",
        "spec",
        "0x",
        "0b102",
        "1.5e",
        "..",
        "->",
        "'",
        "\"",
        "\\u{",
        "🦀",
        "ä",
        "a\u{0308}",
        "_",
        "//",
        "///",
        "\r\n",
        "\t",
        "'a'",
        "\"s\"",
        "'lbl",
        "1_000",
        "e5",
        "}",
        "{",
        " ",
        "\n",
    ];
    let mut rng = XorShift(0xF00D_CAFE);
    for _ in 0..2_000 {
        let picks = (rng.next() % 12) as usize;
        let text: String = (0..picks)
            .map(|_| FRAGMENTS[(rng.next() as usize) % FRAGMENTS.len()])
            .collect();
        check_invariants(&text);
    }
}

#[test]
fn pathological_inputs_terminate_and_uphold_the_invariants() {
    check_invariants("");
    check_invariants(&"\"".repeat(500));
    check_invariants(&"'".repeat(500));
    check_invariants(&"/".repeat(500));
    check_invariants(&"0x".repeat(300));
    check_invariants(&"ä".repeat(300));
    check_invariants(&"\u{FEFF}".repeat(10));
    check_invariants(&format!("\"{}", "\\\"".repeat(400)));
    check_invariants(&"9".repeat(1_000));
}
