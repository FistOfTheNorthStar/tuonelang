//! Malformed-input tests: every lexical error produces a synthetic
//! [`TokenKind::Error`] token plus an `Lxxxx` diagnostic, and the lexer keeps
//! going (§9) — nothing silently succeeds and nothing aborts the stream.

use tuo_lexer::{LexResult, TokenKind, lex};
use tuo_source::SourceMap;

fn lex_str(text: &str) -> LexResult {
    let mut map = SourceMap::new();
    let file = map.intern_file("test.tuo");
    let source = map.add_source(file, text).expect("test source fits");
    lex(map.source(source))
}

/// Assert one diagnostic with `code`, one `Error` token, and a still-complete
/// stream (tokens tile the whole input).
fn assert_single_error(text: &str, code: &str) -> LexResult {
    let result = lex_str(text);
    let codes: Vec<_> = result
        .diagnostics
        .iter()
        .map(|d| d.code.to_string())
        .collect();
    assert_eq!(codes, [code], "diagnostics for {text:?}");
    assert_eq!(
        result
            .tokens
            .iter()
            .filter(|t| t.kind == TokenKind::Error)
            .count(),
        1,
        "error tokens for {text:?}"
    );
    let mut cursor = 0;
    for token in result.tokens.iter().filter(|t| t.kind != TokenKind::Eof) {
        assert_eq!(token.range().start().as_usize(), cursor);
        cursor = token.range().end().as_usize();
    }
    assert_eq!(cursor, text.len(), "recovery must cover all of {text:?}");
    result
}

#[test]
fn unterminated_string_at_end_of_input() {
    assert_single_error(r#"let s = "no end"#, "L0006");
}

#[test]
fn a_newline_ends_an_unterminated_string_and_lexing_resumes() {
    let result = assert_single_error("let s = \"broken\nlet t = 1;", "L0006");
    // The error token stops before the newline; the next line lexes fully.
    let after: Vec<_> = result.parser_tokens().map(|t| t.kind).collect();
    use TokenKind::*;
    assert_eq!(
        after,
        [
            KwLet, Ident, Eq, Error, KwLet, Ident, Eq, IntLiteral, Semi, Eof
        ]
    );
}

#[test]
fn empty_char_literal() {
    // `''` is two bare apostrophes: two diagnostics, two error tokens.
    let result = lex_str("''");
    let codes: Vec<_> = result
        .diagnostics
        .iter()
        .map(|d| d.code.to_string())
        .collect();
    assert_eq!(codes, ["L0007", "L0007"]);
}

#[test]
fn a_bare_apostrophe_is_an_unterminated_char_literal() {
    assert_single_error("let c = ' ;", "L0007");
}

#[test]
fn invalid_escape_in_string() {
    let result = assert_single_error(r#""bad \q escape""#, "L0009");
    // The diagnostic points at the escape itself, not the whole literal.
    let span = result.diagnostics[0].primary_span.range();
    assert_eq!(span.start().as_u32(), 5);
    assert_eq!(span.len(), 2);
}

#[test]
fn invalid_escape_in_char() {
    assert_single_error(r"'\q'", "L0009");
    assert_single_error(r"'\u'", "L0009");
}

#[test]
fn malformed_unicode_escapes() {
    assert_single_error(r#""\u{}""#, "L0009"); // no digits
    assert_single_error(r#""\u41""#, "L0009"); // missing braces
    assert_single_error(r#""\u{41""#, "L0009"); // unclosed brace
}

#[test]
fn hex_literal_without_digits() {
    let result = assert_single_error("let n = 0x;", "L0008");
    assert!(result.diagnostics[0].message.contains("missing digits"));
    // `0x` is ONE malformed token, not `0` then an identifier `x`.
    let error = result
        .tokens
        .iter()
        .find(|t| t.kind == TokenKind::Error)
        .expect("has error token");
    assert_eq!(error.range().len(), 2);
}

#[test]
fn binary_literal_with_an_out_of_base_digit() {
    let result = assert_single_error("0b102", "L0008");
    assert!(result.diagnostics[0].message.contains('2'));
    // Also one token — no reinterpretation as `0b10` + `2`.
    assert_eq!(result.tokens[0].range().len(), 5);
}

#[test]
fn octal_literal_with_an_out_of_base_digit() {
    assert_single_error("0o18", "L0008");
    assert_single_error("0x_", "L0008"); // separator only, no digits
}

#[test]
fn carriage_returns_are_errors_not_normalized() {
    let result = assert_single_error("let a = 1;\r\nlet b = 2;", "L0003");
    // Both lines still lex; the CRLF is one error token.
    use TokenKind::*;
    let after: Vec<_> = result.parser_tokens().map(|t| t.kind).collect();
    assert_eq!(
        after,
        [
            KwLet, Ident, Eq, IntLiteral, Semi, Error, KwLet, Ident, Eq, IntLiteral, Semi, Eof
        ]
    );
    assert_single_error("lone\rcr", "L0003");
}

#[test]
fn tabs_are_errors_outside_literals() {
    assert_single_error("\tfn f() {}", "L0004");
    // …but fine inside a string literal (§1).
    let result = lex_str("\"a\tb\"");
    assert!(!result.has_errors());
    assert_eq!(result.tokens[0].kind, TokenKind::StringLiteral);
}

#[test]
fn diagnostics_accumulate_across_multiple_errors() {
    let result = lex_str("0b2 \t \"open");
    let codes: Vec<_> = result
        .diagnostics
        .iter()
        .map(|d| d.code.to_string())
        .collect();
    assert_eq!(codes, ["L0008", "L0004", "L0006"]);
}
