//! Property tests for the formatter's guarantees, over the fixture corpora
//! and generated fragment soup:
//!
//! - `format(format(s)) == format(s)` (idempotence);
//! - every comment survives, in order;
//! - parse → format → parse yields a structurally equivalent tree;
//! - the formatter never panics and never returns unverified output.

use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

use tuo_fmt::format_source;
use tuo_lexer::TokenKind;
use tuo_parser::{ParseResult, parse};
use tuo_source::SourceMap;
use tuo_syntax::{SyntaxElement, SyntaxNode};

fn parse_str(text: &str) -> ParseResult {
    let mut map = SourceMap::new();
    let file = map.intern_file("prop.tuo");
    let id = map.add_source(file, text).expect("input fits");
    parse(map.source(id))
}

fn format_str(text: &str) -> tuo_fmt::FormatOutcome {
    let mut map = SourceMap::new();
    let file = map.intern_file("prop.tuo");
    let id = map.add_source(file, text).expect("input fits");
    format_source(map.source(id))
}

/// A structural fingerprint of a parse: node kinds and significant token
/// `(kind, lexeme)` pairs (separator commas excluded — the canonical layout
/// normalizes them), independent of all whitespace.
fn fingerprint(result: &ParseResult, text: &str) -> String {
    fn walk(node: &SyntaxNode, result: &ParseResult, text: &str, out: &mut String) {
        write!(out, "({:?}", node.kind).expect("write to String cannot fail");
        for child in &node.children {
            match child {
                SyntaxElement::Token(index) => {
                    let token = &result.tree.lex.tokens[*index as usize];
                    if token.kind == TokenKind::Comma {
                        continue;
                    }
                    let range = token.range();
                    let lexeme = &text[range.start().as_usize()..range.end().as_usize()];
                    write!(out, " {:?}:{lexeme:?}", token.kind)
                        .expect("write to String cannot fail");
                }
                SyntaxElement::Node(nested) => walk(nested, result, text, out),
            }
        }
        out.push(')');
    }
    let mut out = String::new();
    walk(&result.tree.root, result, text, &mut out);
    out
}

/// Every comment lexeme (`//` and `///`), in source order.
fn comments(text: &str) -> Vec<String> {
    let result = parse_str(text);
    result
        .tree
        .lex
        .tokens
        .iter()
        .filter(|token| matches!(token.kind, TokenKind::LineComment | TokenKind::DocComment))
        .map(|token| {
            let range = token.range();
            text[range.start().as_usize()..range.end().as_usize()].to_owned()
        })
        .collect()
}

/// Check every formatter property on one input.
fn check_properties(text: &str) {
    let outcome = format_str(text);
    if !outcome.safe {
        // The conservative bail-out: allowed only for inputs the formatter
        // cannot verify (e.g. lexically broken byte soup), and it must
        // return the input untouched.
        assert_eq!(outcome.text, text, "unsafe outcome must not modify input");
        assert!(!outcome.changed);
        return;
    }

    // Idempotence.
    let second = format_str(&outcome.text);
    assert!(second.safe, "second-pass self-check failed on {text:?}");
    assert_eq!(second.text, outcome.text, "not idempotent on {text:?}");

    // Comment preservation.
    assert_eq!(
        comments(text),
        comments(&outcome.text),
        "comments changed on {text:?}"
    );

    // parse → format → parse structural equivalence.
    let before = parse_str(text);
    let after = parse_str(&outcome.text);
    assert_eq!(
        fingerprint(&before, text),
        fingerprint(&after, &outcome.text),
        "structure changed on {text:?}"
    );

    // Diagnostics equivalence (same errors, before and after).
    let codes = |result: &ParseResult| {
        let mut codes: Vec<String> = result
            .all_diagnostics()
            .iter()
            .map(|d| d.code.to_string())
            .collect();
        codes.sort_unstable();
        codes
    };
    assert_eq!(
        codes(&before),
        codes(&after),
        "diagnostics changed on {text:?}"
    );
}

#[test]
fn properties_hold_on_the_fmt_fixture_corpus() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fmt/fixtures");
    for entry in fs::read_dir(root).expect("fixture dir exists") {
        let path = entry.expect("readable entry").path();
        if path.extension().is_some_and(|ext| ext == "tuo") {
            check_properties(&fs::read_to_string(&path).expect("fixture is readable"));
        }
    }
}

#[test]
fn properties_hold_on_the_parser_fixture_corpus() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/parser/fixtures");
    for sub in ["ok", "err"] {
        for entry in fs::read_dir(root.join(sub)).expect("fixture dir exists") {
            let path = entry.expect("readable entry").path();
            if path.extension().is_some_and(|ext| ext == "tuo") {
                check_properties(&fs::read_to_string(&path).expect("fixture is readable"));
            }
        }
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
fn properties_hold_on_token_fragment_soup() {
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
        ";",
        " ",
        "\n",
        "\n\n",
        "Box[",
        "]",
        "(",
        ")",
        "+",
        "-",
        "1.5",
        "42",
    ];
    let mut rng = XorShift(0xF0_4A17);
    for _ in 0..400 {
        let picks = (rng.next() % 16) as usize;
        let text: String = (0..picks)
            .map(|_| FRAGMENTS[(rng.next() as usize) % FRAGMENTS.len()])
            .collect();
        check_properties(&text);
    }
}

#[test]
fn properties_hold_on_random_byte_soup() {
    // Byte soup exercises lexical errors and total recovery; the formatter
    // must stay safe (possibly by conservatively returning the input).
    let mut rng = XorShift(0xB17E_5001);
    for _ in 0..300 {
        let len = (rng.next() % 40) as usize;
        let bytes: Vec<u8> = (0..len).map(|_| (rng.next() % 256) as u8).collect();
        let text = String::from_utf8_lossy(&bytes);
        check_properties(&text);
    }
}

#[test]
fn deterministic_across_repeated_runs() {
    let text = fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fmt/fixtures/large.tuo"),
    )
    .expect("fixture is readable");
    let first = format_str(&text).text;
    for _ in 0..3 {
        assert_eq!(format_str(&text).text, first);
    }
}
