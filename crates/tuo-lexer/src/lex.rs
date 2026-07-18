//! The lexing engine: Logos-generated DFA behind the TDG-owned interface.
//!
//! Nothing Logos-specific escapes this module — [`lex`] takes a
//! [`SourceText`] and returns [`LexResult`] built from [`Token`] and
//! [`Diagnostic`] values only.
//!
//! The token patterns implement `specification/lexical-grammar.md`:
//! maximal munch for multi-character operators, fully reserved keywords
//! (resolved from the identifier lexeme, so the DFA has a single identifier
//! rule), and **no skipped input** — whitespace and comments are real tokens,
//! because the lossless syntax layer and the formatter need them.

use logos::Logos;
use tuo_diagnostics::{Diagnostic, DiagnosticCode, Namespace};
use tuo_source::{ByteOffset, SourceText, Span, TextRange};
use unicode_normalization::is_nfc;

use crate::token::{LexResult, Token, TokenKind};

/// Integer type suffixes (§11). Shared by the decimal and prefixed rules.
const INT_SUFFIXES: [&str; 10] = [
    "isize", "usize", "i16", "i32", "i64", "u16", "u32", "u64", "i8", "u8",
];

/// The raw Logos token set. Private: this enum is the replaceable engine
/// behind the [`TokenKind`] boundary.
///
/// Ordering notes:
/// - Logos picks the longest match, so `->` beats `-`, `1.5` beats `1`, and a
///   terminated string beats its unterminated prefix.
/// - `DocComment` outranks `LineComment` by explicit priority because both
///   patterns match a `///…` line at the same length.
/// - Anything the DFA cannot match at all surfaces as an error slice, which
///   [`lex`] turns into a [`TokenKind::Error`] token plus a diagnostic.
#[derive(Logos, Clone, Copy, PartialEq, Eq, Debug)]
enum Raw {
    #[regex(r"[ \n]+")]
    Whitespace,
    #[regex(r"///[^\n]*", priority = 10)]
    DocComment,
    #[regex(r"//[^\n]*")]
    LineComment,

    #[regex(r"[\p{XID_Start}_]\p{XID_Continue}*")]
    IdentLike,

    #[regex(r"[0-9][0-9_]*(i8|i16|i32|i64|isize|u8|u16|u32|u64|usize)?")]
    Int,
    // Deliberately broad digit class: `0b12` and `0x` must lex as ONE
    // malformed-literal token (diagnosed in `check_prefixed_int`), not
    // shatter into `0b1`, `2`.
    #[regex(r"0[xob][0-9a-fA-F_]*(i8|i16|i32|i64|isize|u8|u16|u32|u64|usize)?")]
    PrefixedInt,
    #[regex(r"[0-9][0-9_]*\.[0-9][0-9_]*([eE][+-]?[0-9][0-9_]*)?(f32|f64)?")]
    Float,
    #[regex(r"[0-9][0-9_]*[eE][+-]?[0-9][0-9_]*(f32|f64)?")]
    FloatExp,

    #[regex(r#""([^"\\\n]|\\[^\n])*""#, priority = 6)]
    String,
    #[regex(r#""([^"\\\n]|\\[^\n])*"#, priority = 3)]
    UnterminatedString,
    #[regex(r"'([^'\\\n]|\\[^\n]|\\u\{[0-9a-fA-F_]+\})'", priority = 6)]
    Char,
    #[regex(r"'[\p{XID_Start}_]\p{XID_Continue}*", priority = 4)]
    Label,
    #[token("'")]
    BareApostrophe,

    #[regex(r"\r\n?")]
    CarriageReturn,
    #[regex(r"\t+")]
    Tab,

    #[token("->")]
    ThinArrow,
    #[token("=>")]
    FatArrow,
    #[token("::")]
    ColonColon,
    #[token("..")]
    DotDot,
    #[token("==")]
    EqEq,
    #[token("!=")]
    NotEq,
    #[token("<=")]
    LtEq,
    #[token(">=")]
    GtEq,
    #[token("&&")]
    AmpAmp,
    #[token("||")]
    PipePipe,
    #[token("|")]
    Pipe,
    #[token("<")]
    Lt,
    #[token(">")]
    Gt,
    #[token("!")]
    Bang,
    #[token("+")]
    Plus,
    #[token("-")]
    Minus,
    #[token("*")]
    Star,
    #[token("/")]
    Slash,
    #[token("%")]
    Percent,
    #[token("=")]
    Eq,
    #[token("?")]
    Question,
    #[token(".")]
    Dot,
    #[token("(")]
    OpenParen,
    #[token(")")]
    CloseParen,
    #[token("{")]
    OpenBrace,
    #[token("}")]
    CloseBrace,
    #[token("[")]
    OpenBracket,
    #[token("]")]
    CloseBracket,
    #[token(",")]
    Comma,
    #[token(";")]
    Semi,
    #[token(":")]
    Colon,
}

/// A lexical diagnostic code (the reserved `Lxxxx` namespace).
fn code(number: u16) -> DiagnosticCode {
    DiagnosticCode::new(Namespace::Lexical, number)
}

/// Lex one immutable source snapshot into a complete token stream.
///
/// Every byte of the input is covered by exactly one token (trivia included),
/// in order, followed by a zero-width [`TokenKind::Eof`]. Malformed input
/// becomes [`TokenKind::Error`] tokens with accompanying diagnostics; lexing
/// itself never fails (§9).
#[must_use]
pub fn lex(source: &SourceText) -> LexResult {
    let text = source.text();
    let mut tokens = Vec::new();
    let mut diagnostics = Vec::new();

    for (result, span) in Raw::lexer(text).spanned() {
        let slice = &text[span.clone()];
        let start = u32::try_from(span.start).expect("SourceText length fits in u32");
        let len = u32::try_from(slice.len()).expect("SourceText length fits in u32");
        let range = TextRange::at(ByteOffset::new(start), len);
        let span = Span::new(source.source(), range);

        let kind = match result {
            Ok(raw) => classify(raw, slice, span, &mut diagnostics),
            Err(()) => {
                diagnostics.push(unexpected_input(slice, start, span));
                TokenKind::Error
            }
        };
        tokens.push(Token { kind, span });
    }

    tokens.push(Token {
        kind: TokenKind::Eof,
        span: Span::new(
            source.source(),
            TextRange::empty(ByteOffset::new(source.len())),
        ),
    });
    LexResult {
        tokens,
        diagnostics,
    }
}

/// Turn a raw DFA match into a public [`TokenKind`], running the checks the
/// DFA cannot express (NFC identifiers, digit/base agreement, escape
/// validity) and recording diagnostics for §1/§2/§9 violations.
fn classify(raw: Raw, slice: &str, span: Span, diagnostics: &mut Vec<Diagnostic>) -> TokenKind {
    match raw {
        Raw::Whitespace => TokenKind::Whitespace,
        Raw::DocComment => TokenKind::DocComment,
        Raw::LineComment => TokenKind::LineComment,
        Raw::IdentLike => classify_ident(slice, span, diagnostics),
        Raw::Int => TokenKind::IntLiteral,
        Raw::PrefixedInt => match check_prefixed_int(slice) {
            Ok(()) => TokenKind::IntLiteral,
            Err(message) => {
                diagnostics.push(
                    Diagnostic::error(code(8), message, span)
                        .with_note("integer literals are decimal, `0x` hex, `0o` octal, or `0b` binary, with `_` separators allowed between digits"),
                );
                TokenKind::Error
            }
        },
        Raw::Float | Raw::FloatExp => TokenKind::FloatLiteral,
        Raw::String => match check_escapes(&slice[1..slice.len() - 1], span, 1) {
            Ok(()) => TokenKind::StringLiteral,
            Err(diagnostic) => {
                diagnostics.push(*diagnostic);
                TokenKind::Error
            }
        },
        Raw::UnterminatedString => {
            diagnostics.push(
                Diagnostic::error(code(6), "unterminated string literal", span)
                    .with_note("string literals cannot span lines; write `\\n` for a newline"),
            );
            TokenKind::Error
        }
        Raw::Char => match check_escapes(&slice[1..slice.len() - 1], span, 1) {
            Ok(()) => TokenKind::CharLiteral,
            Err(diagnostic) => {
                diagnostics.push(*diagnostic);
                TokenKind::Error
            }
        },
        Raw::Label => {
            if is_nfc(&slice[1..]) {
                TokenKind::LifetimeLabel
            } else {
                diagnostics.push(not_nfc(slice, span));
                TokenKind::Error
            }
        }
        Raw::BareApostrophe => {
            diagnostics.push(
                Diagnostic::error(code(7), "empty or unterminated character literal", span)
                    .with_note(
                        "a character literal is one Unicode scalar or escape between `'` quotes",
                    ),
            );
            TokenKind::Error
        }
        Raw::CarriageReturn => {
            diagnostics.push(
                Diagnostic::error(code(3), "carriage return in source", span)
                    .with_note("tuonelang source uses LF (`\\n`) line endings only (§1)")
                    .with_help("convert the file's line endings from CRLF to LF"),
            );
            TokenKind::Error
        }
        Raw::Tab => {
            diagnostics.push(
                Diagnostic::error(code(4), "tab character in source", span)
                    .with_note("tabs are not allowed outside string and character literals (§1)")
                    .with_help("indent with spaces"),
            );
            TokenKind::Error
        }
        Raw::ThinArrow => TokenKind::ThinArrow,
        Raw::FatArrow => TokenKind::FatArrow,
        Raw::ColonColon => TokenKind::ColonColon,
        Raw::DotDot => TokenKind::DotDot,
        Raw::EqEq => TokenKind::EqEq,
        Raw::NotEq => TokenKind::NotEq,
        Raw::LtEq => TokenKind::LtEq,
        Raw::GtEq => TokenKind::GtEq,
        Raw::AmpAmp => TokenKind::AmpAmp,
        Raw::PipePipe => TokenKind::PipePipe,
        Raw::Pipe => TokenKind::Pipe,
        Raw::Lt => TokenKind::Lt,
        Raw::Gt => TokenKind::Gt,
        Raw::Bang => TokenKind::Bang,
        Raw::Plus => TokenKind::Plus,
        Raw::Minus => TokenKind::Minus,
        Raw::Star => TokenKind::Star,
        Raw::Slash => TokenKind::Slash,
        Raw::Percent => TokenKind::Percent,
        Raw::Eq => TokenKind::Eq,
        Raw::Question => TokenKind::Question,
        Raw::Dot => TokenKind::Dot,
        Raw::OpenParen => TokenKind::OpenParen,
        Raw::CloseParen => TokenKind::CloseParen,
        Raw::OpenBrace => TokenKind::OpenBrace,
        Raw::CloseBrace => TokenKind::CloseBrace,
        Raw::OpenBracket => TokenKind::OpenBracket,
        Raw::CloseBracket => TokenKind::CloseBracket,
        Raw::Comma => TokenKind::Comma,
        Raw::Semi => TokenKind::Semi,
        Raw::Colon => TokenKind::Colon,
    }
}

/// An identifier-shaped lexeme: keyword, wildcard, or `IDENT` (checked NFC).
fn classify_ident(slice: &str, span: Span, diagnostics: &mut Vec<Diagnostic>) -> TokenKind {
    if slice == "_" {
        return TokenKind::Underscore;
    }
    if let Some(keyword) = TokenKind::from_keyword(slice) {
        return keyword;
    }
    if is_nfc(slice) {
        TokenKind::Ident
    } else {
        diagnostics.push(not_nfc(slice, span));
        TokenKind::Error
    }
}

/// §2: a source identifier that is not already in NFC is a lexical error —
/// the lexer never silently normalizes.
fn not_nfc(slice: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        code(5),
        format!("identifier `{slice}` is not in Unicode NFC"),
        span,
    )
    .with_note("identifiers must already be in NFC; the lexer never normalizes silently (§2)")
    .with_help("re-enter the identifier in composed (NFC) form")
}

/// Validate a `0x`/`0o`/`0b` literal the broad DFA rule accepted: strip any
/// type suffix, then require at least one digit and only digits of the base.
fn check_prefixed_int(slice: &str) -> Result<(), String> {
    let (radix, base_name) = match &slice[..2] {
        "0x" => (16, "hexadecimal"),
        "0o" => (8, "octal"),
        "0b" => (2, "binary"),
        prefix => unreachable!("DFA only matches 0x/0o/0b, got {prefix:?}"),
    };
    let mut digits = &slice[2..];
    for suffix in INT_SUFFIXES {
        if let Some(stripped) = digits.strip_suffix(suffix) {
            digits = stripped;
            break;
        }
    }
    if !digits.chars().any(|c| c != '_') {
        return Err(format!("{base_name} literal is missing digits"));
    }
    if let Some(bad) = digits.chars().find(|&c| c != '_' && !c.is_digit(radix)) {
        return Err(format!("invalid digit `{bad}` in {base_name} literal"));
    }
    Ok(())
}

/// Validate every escape sequence in a string/char literal body. `offset` is
/// the body's byte offset within the token (1: past the opening quote), used
/// to point the diagnostic at the exact escape.
fn check_escapes(body: &str, token_span: Span, offset: u32) -> Result<(), Box<Diagnostic>> {
    let mut chars = body.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        if c != '\\' {
            continue;
        }
        let Some((_, escaped)) = chars.next() else {
            unreachable!("DFA never ends a literal body on a bare backslash");
        };
        let valid = match escaped {
            '"' | '\'' | '\\' | 'n' | 'r' | 't' | '0' => true,
            'u' => {
                // \u{HEX+}
                let mut ok = chars.next_with(|&(_, c)| c == '{');
                let mut digits = 0usize;
                while chars.next_with(|&(_, c)| c.is_ascii_hexdigit()) {
                    digits += 1;
                }
                ok = ok && digits > 0 && chars.next_with(|&(_, c)| c == '}');
                ok
            }
            _ => false,
        };
        if !valid {
            let escape_start = token_span.range().start()
                + (offset + u32::try_from(at).expect("literal fits in u32"));
            let escape_len = 1 + u32::try_from(escaped.len_utf8()).expect("scalar fits in u32");
            let span = Span::new(token_span.source(), TextRange::at(escape_start, escape_len));
            return Err(Box::new(
                Diagnostic::error(
                    code(9),
                    format!("invalid escape sequence `\\{escaped}`"),
                    span,
                )
                .with_note(
                    r#"valid escapes are `\"`, `\'`, `\\`, `\n`, `\r`, `\t`, `\0`, and `\u{HEX}`"#,
                ),
            ));
        }
    }
    Ok(())
}

/// A diagnostic for input the DFA could not match at all: a BOM (§1) or an
/// unexpected character.
fn unexpected_input(slice: &str, start: u32, span: Span) -> Diagnostic {
    if start == 0 && slice.starts_with('\u{FEFF}') {
        return Diagnostic::error(code(2), "byte order mark at start of file", span)
            .with_note("tuonelang source is UTF-8 without a BOM (§1)")
            .with_help("save the file without a byte order mark");
    }
    let display = slice.chars().next().map_or_else(
        || "<empty>".to_owned(),
        |c| {
            if c.is_control() || c == '\u{FEFF}' {
                format!("U+{:04X}", u32::from(c))
            } else {
                format!("`{c}`")
            }
        },
    );
    Diagnostic::error(
        code(1),
        format!("unexpected character {display} in source"),
        span,
    )
}

/// Tiny peek-and-advance helper for escape scanning.
trait NextWith<T> {
    /// Consume and return `true` if the next element satisfies `pred`.
    fn next_with(&mut self, pred: impl Fn(&T) -> bool) -> bool;
}

impl<I: Iterator> NextWith<I::Item> for std::iter::Peekable<I> {
    fn next_with(&mut self, pred: impl Fn(&I::Item) -> bool) -> bool {
        if self.peek().is_some_and(pred) {
            self.next();
            true
        } else {
            false
        }
    }
}
