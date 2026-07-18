//! Core token tests: keywords, punctuation with maximal munch, exact spans,
//! trivia retention, and the losslessness invariant the syntax layer relies
//! on.

use tuo_lexer::{LexResult, TokenKind, lex};
use tuo_source::SourceMap;

fn lex_str(text: &str) -> LexResult {
    let mut map = SourceMap::new();
    let file = map.intern_file("test.tuo");
    let source = map.add_source(file, text).expect("test source fits");
    lex(map.source(source))
}

/// The kinds of the non-trivia, non-EOF tokens.
fn kinds(text: &str) -> Vec<TokenKind> {
    let result = lex_str(text);
    assert!(
        result.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        result.diagnostics
    );
    result
        .parser_tokens()
        .map(|t| t.kind)
        .filter(|&k| k != TokenKind::Eof)
        .collect()
}

#[test]
fn every_keyword_lexes_as_its_keyword_token() {
    use TokenKind::*;
    let cases: &[(&str, TokenKind)] = &[
        ("module", KwModule),
        ("import", KwImport),
        ("pub", KwPub),
        ("fn", KwFn),
        ("let", KwLet),
        ("var", KwVar),
        ("const", KwConst),
        ("struct", KwStruct),
        ("enum", KwEnum),
        ("interface", KwInterface),
        ("impl", KwImpl),
        ("where", KwWhere),
        ("if", KwIf),
        ("else", KwElse),
        ("match", KwMatch),
        ("for", KwFor),
        ("in", KwIn),
        ("while", KwWhile),
        ("loop", KwLoop),
        ("break", KwBreak),
        ("continue", KwContinue),
        ("return", KwReturn),
        ("take", KwTake),
        ("mut", KwMut),
        ("move", KwMove),
        ("Box", KwBox),
        ("Shared", KwShared),
        ("Weak", KwWeak),
        ("spec", KwSpec),
        ("given", KwGiven),
        ("when", KwWhen),
        ("then", KwThen),
        ("assert", KwAssert),
        ("unsafe", KwUnsafe),
        ("as", KwAs),
        ("self", KwSelfValue),
        ("Self", KwSelfType),
    ];
    for (lexeme, expected) in cases {
        assert_eq!(kinds(lexeme), [*expected], "keyword `{lexeme}`");
        assert!(expected.is_keyword(), "`{lexeme}` must count as a keyword");
    }
}

#[test]
fn true_and_false_surface_as_bool_literals() {
    assert_eq!(kinds("true"), [TokenKind::BoolLiteral]);
    assert_eq!(kinds("false"), [TokenKind::BoolLiteral]);
    assert!(!TokenKind::BoolLiteral.is_keyword());
    assert!(TokenKind::BoolLiteral.is_literal());
}

#[test]
fn keywords_are_reserved_in_every_position_and_prefixes_are_not() {
    use TokenKind::*;
    // `let let` — the second `let` is still the keyword, never an identifier.
    assert_eq!(kinds("let let"), [KwLet, KwLet]);
    // Keyword-prefixed identifiers are ordinary identifiers.
    assert_eq!(kinds("letter iffy selfie"), [Ident, Ident, Ident]);
}

#[test]
fn maximal_munch_resolves_multi_character_operators() {
    use TokenKind::*;
    assert_eq!(
        kinds("-> => :: .. == != <= >= && ||"),
        [
            ThinArrow, FatArrow, ColonColon, DotDot, EqEq, NotEq, LtEq, GtEq, AmpAmp, PipePipe
        ]
    );
    // Adjacent glyphs still munch greedily left-to-right.
    assert_eq!(kinds("a==b"), [Ident, EqEq, Ident]);
    assert_eq!(kinds("0 .. 5"), [IntLiteral, DotDot, IntLiteral]);
    assert_eq!(kinds("!!="), [Bang, NotEq]);
    assert_eq!(kinds("x<=>y"), [Ident, LtEq, Gt, Ident]);
    // `|` (pattern separator, §17) exists alongside `||`; munch prefers `||`.
    assert_eq!(kinds("a | b || c"), [Ident, Pipe, Ident, PipePipe, Ident]);
}

#[test]
fn single_character_punctuation_lexes() {
    use TokenKind::*;
    assert_eq!(
        kinds("< > ! + - * / % = ? . ( ) { } [ ] , ; :"),
        [
            Lt,
            Gt,
            Bang,
            Plus,
            Minus,
            Star,
            Slash,
            Percent,
            Eq,
            Question,
            Dot,
            OpenParen,
            CloseParen,
            OpenBrace,
            CloseBrace,
            OpenBracket,
            CloseBracket,
            Comma,
            Semi,
            Colon
        ]
    );
}

#[test]
fn numeric_literals_lex_with_bases_suffixes_and_separators() {
    use TokenKind::*;
    assert_eq!(
        kinds("0 42 1_000_000 255u8 0xFF 0xFFi32 0o17 0b1010usize"),
        [IntLiteral; 8]
    );
    assert_eq!(
        kinds("1.0 0.5 2.5e10 1_0.5_0e+3f64 7e-2 3f? 1.5f32"),
        [
            FloatLiteral,
            FloatLiteral,
            FloatLiteral,
            FloatLiteral,
            FloatLiteral,
            IntLiteral,
            Ident,
            Question,
            FloatLiteral
        ]
    );
}

#[test]
fn a_trailing_or_leading_dot_is_not_a_float() {
    use TokenKind::*;
    // `1.` and `.5` are member/range syntax, not float literals (§10 note).
    assert_eq!(kinds("1."), [IntLiteral, Dot]);
    assert_eq!(kinds(".5"), [Dot, IntLiteral]);
    assert_eq!(kinds("x.0"), [Ident, Dot, IntLiteral]);
}

#[test]
fn strings_chars_and_labels_lex() {
    use TokenKind::*;
    assert_eq!(
        kinds(r#""hello" "with \"escapes\" \u{1F600}" 'a' '\n' '\u{41}'"#),
        [
            StringLiteral,
            StringLiteral,
            CharLiteral,
            CharLiteral,
            CharLiteral
        ]
    );
    assert_eq!(
        kinds("'outer: loop { break 'outer; }"),
        [
            LifetimeLabel,
            Colon,
            KwLoop,
            OpenBrace,
            KwBreak,
            LifetimeLabel,
            Semi,
            CloseBrace
        ]
    );
}

#[test]
fn underscore_is_the_wildcard_but_underscore_prefixed_names_are_idents() {
    use TokenKind::*;
    assert_eq!(kinds("_ _x _1"), [Underscore, Ident, Ident]);
}

#[test]
fn comments_are_kept_and_doc_comments_reach_the_parser() {
    let result = lex_str("// trivia\n/// docs\nfn f() {}\n");
    let all: Vec<_> = result.tokens.iter().map(|t| t.kind).collect();
    assert!(all.contains(&TokenKind::LineComment));
    assert!(all.contains(&TokenKind::DocComment));
    // The parser view drops the line comment but keeps the doc comment.
    let parser: Vec<_> = result.parser_tokens().map(|t| t.kind).collect();
    assert!(!parser.contains(&TokenKind::LineComment));
    assert!(parser.contains(&TokenKind::DocComment));
    // `////…` and `///…` are both doc comments; `//` alone is a line comment.
    let result = lex_str("//\n////x\n");
    let all: Vec<_> = result.tokens.iter().map(|t| t.kind).collect();
    assert!(all.contains(&TokenKind::LineComment));
    assert!(all.contains(&TokenKind::DocComment));
}

#[test]
fn tokens_tile_the_input_exactly_and_end_with_eof() {
    let text = "fn main() -> Int { // answer\n    return 42;\n}\n";
    let result = lex_str(text);
    let (eof, rest) = result.tokens.split_last().expect("stream never empty");
    assert_eq!(eof.kind, TokenKind::Eof);
    assert_eq!(eof.range().start().as_usize(), text.len());
    assert!(eof.range().is_empty());
    let mut cursor = 0;
    for token in rest {
        assert_eq!(
            token.range().start().as_usize(),
            cursor,
            "gap or overlap before {token:?}"
        );
        cursor = token.range().end().as_usize();
        assert!(!token.range().is_empty(), "zero-width non-EOF token");
    }
    assert_eq!(cursor, text.len(), "input not fully covered");
}

#[test]
fn spans_are_byte_exact() {
    use TokenKind::*;
    let text = "let x = 42;";
    let result = lex_str(text);
    let lexemes: Vec<(TokenKind, &str)> = result
        .tokens
        .iter()
        .filter(|t| t.kind != Eof)
        .map(|t| {
            let r = t.range();
            (t.kind, &text[r.start().as_usize()..r.end().as_usize()])
        })
        .collect();
    assert_eq!(
        lexemes,
        [
            (KwLet, "let"),
            (Whitespace, " "),
            (Ident, "x"),
            (Whitespace, " "),
            (Eq, "="),
            (Whitespace, " "),
            (IntLiteral, "42"),
            (Semi, ";"),
        ]
    );
}

#[test]
fn empty_input_lexes_to_just_eof() {
    let result = lex_str("");
    assert_eq!(result.tokens.len(), 1);
    assert_eq!(result.tokens[0].kind, TokenKind::Eof);
    assert!(!result.has_errors());
}
