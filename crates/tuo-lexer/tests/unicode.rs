//! Unicode tests: multibyte identifiers and literals with byte-exact spans,
//! the §2 NFC requirement, and §1 byte-level preconditions (BOM).

use tuo_lexer::{LexResult, TokenKind, lex};
use tuo_source::SourceMap;

fn lex_str(text: &str) -> LexResult {
    let mut map = SourceMap::new();
    let file = map.intern_file("test.tuo");
    let source = map.add_source(file, text).expect("test source fits");
    lex(map.source(source))
}

fn lexeme<'t>(text: &'t str, result: &LexResult, index: usize) -> &'t str {
    let range = result.tokens[index].range();
    &text[range.start().as_usize()..range.end().as_usize()]
}

#[test]
fn multibyte_identifiers_lex_with_byte_exact_spans() {
    let text = "let tervehdys = päivää;";
    let result = lex_str(text);
    assert!(!result.has_errors());
    let kinds: Vec<_> = result.tokens.iter().map(|t| t.kind).collect();
    use TokenKind::*;
    assert_eq!(
        kinds,
        [
            KwLet, Whitespace, Ident, Whitespace, Eq, Whitespace, Ident, Semi, Eof
        ]
    );
    // `päivää` is 9 bytes (three two-byte `ä`s), and the span reflects bytes,
    // not characters.
    assert_eq!(lexeme(text, &result, 6), "päivää");
    assert_eq!(result.tokens[6].range().len(), 9);
    assert_eq!(lexeme(text, &result, 7), ";");
}

#[test]
fn non_latin_scripts_are_valid_identifiers() {
    for ident in ["δ", "числo", "変数", "मान"] {
        let result = lex_str(ident);
        assert_eq!(
            result.tokens[0].kind,
            TokenKind::Ident,
            "`{ident}` must lex as an identifier"
        );
        assert!(!result.has_errors(), "`{ident}` must not diagnose");
    }
}

#[test]
fn emoji_are_not_identifiers() {
    // Emoji lack XID_Start: unexpected-character diagnostic, lexing continues.
    let result = lex_str("let 🦀 = 1;");
    use TokenKind::*;
    assert!(result.tokens.iter().any(|t| t.kind == Error));
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.diagnostics[0].code.to_string(), "L0001");
    // Recovery: the rest of the statement still lexes.
    let kinds: Vec<_> = result.parser_tokens().map(|t| t.kind).collect();
    assert_eq!(kinds, [KwLet, Error, Eq, IntLiteral, Semi, Eof]);
}

#[test]
fn non_nfc_identifiers_are_rejected_not_normalized() {
    // "ä" spelled as 'a' + U+0308 COMBINING DIAERESIS — NFD, not NFC.
    let decomposed = "pa\u{0308}iva\u{0308}a\u{0308}";
    let result = lex_str(decomposed);
    assert_eq!(result.tokens[0].kind, TokenKind::Error);
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.diagnostics[0].code.to_string(), "L0005");
    // The whole lexeme is one error token — no silent split or normalize.
    assert_eq!(
        result.tokens[0].range().len(),
        u32::try_from(decomposed.len()).expect("fits")
    );

    // The same word in NFC is a plain identifier.
    let composed = lex_str("päivää");
    assert_eq!(composed.tokens[0].kind, TokenKind::Ident);
    assert!(!composed.has_errors());
}

#[test]
fn non_nfc_labels_are_rejected_too() {
    let result = lex_str("'a\u{0308}: loop {}");
    assert_eq!(result.tokens[0].kind, TokenKind::Error);
    assert_eq!(result.diagnostics[0].code.to_string(), "L0005");
    assert_eq!(
        lex_str("'ä: loop {}").tokens[0].kind,
        TokenKind::LifetimeLabel
    );
}

#[test]
fn multibyte_content_in_string_and_char_literals() {
    let text = "\"terve 🦀 маілма\" '🦀'";
    let result = lex_str(text);
    assert!(!result.has_errors());
    assert_eq!(result.tokens[0].kind, TokenKind::StringLiteral);
    assert_eq!(result.tokens[2].kind, TokenKind::CharLiteral);
    // The char literal spans quote + 4-byte scalar + quote = 6 bytes.
    assert_eq!(result.tokens[2].range().len(), 6);
}

#[test]
fn a_byte_order_mark_is_diagnosed_not_swallowed() {
    let result = lex_str("\u{FEFF}fn main() {}");
    assert_eq!(result.tokens[0].kind, TokenKind::Error);
    assert_eq!(result.diagnostics[0].code.to_string(), "L0002");
    // Everything after the BOM still lexes normally.
    assert_eq!(result.tokens[1].kind, TokenKind::KwFn);
}
