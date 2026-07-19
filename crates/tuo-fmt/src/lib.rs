//! Deterministic, canonical formatter for tuonelang source.
//!
//! The formatter defines tuonelang's **canonical source representation**: it
//! parses a snapshot into the lossless CST ([`tuo_syntax`]) and re-emits the
//! text with fixed v0 layout rules. There are **no configuration options**:
//! one brace style, one indentation style (4 spaces), one canonical form.
//!
//! # Guarantees
//!
//! - **Deterministic:** output depends only on the input text.
//! - **Idempotent:** `format(format(s)) == format(s)` — enforced by the
//!   golden and property test suites.
//! - **Comment-preserving:** every `//` and `///` comment survives, with
//!   trailing comments kept on their line and own-line comments on theirs
//!   (blank lines between elements are preserved, capped at one).
//! - **Meaning-preserving:** token lexemes are never rewritten; only
//!   inter-token whitespace and separator commas are normalized. Every
//!   [`format_source`] call **verifies** this before returning: the output
//!   must lex to the same significant token sequence (commas aside), keep
//!   every comment, and parse with the same diagnostics. If verification
//!   ever fails, the input is returned unchanged ([`FormatOutcome::safe`]
//!   turns `false`).
//! - **Conservative on malformed input:** the parser recovers at sync
//!   points, and the formatter reproduces each recovered
//!   [`tuo_syntax::SyntaxKind::Error`] island byte-for-byte while still
//!   formatting the well-formed constructs around it.

mod engine;

use tuo_lexer::TokenKind;
use tuo_source::{SourceMap, SourceText};

/// The result of formatting one source snapshot.
#[derive(Clone, Debug)]
pub struct FormatOutcome {
    /// The canonical text (or the untouched input if `safe` is `false`).
    pub text: String,
    /// Did formatting change anything?
    pub changed: bool,
    /// `false` only if the meaning-preservation self-check failed and the
    /// input was returned unchanged. This indicates a formatter bug worth
    /// reporting; it never silently produces unverified output.
    pub safe: bool,
}

/// Format one source snapshot into tuonelang's canonical representation.
#[must_use]
pub fn format_source(source: &SourceText) -> FormatOutcome {
    let original = source.text();
    let parsed = tuo_parser::parse(source);
    let formatted = engine::format_tree(&parsed.tree, original);
    if formatted == original {
        return FormatOutcome {
            text: formatted,
            changed: false,
            safe: true,
        };
    }

    // Self-check: the canonical text must mean the same thing.
    let mut verify_map = SourceMap::new();
    let file = verify_map.intern_file("fmt-verify.tuo");
    let Ok(id) = verify_map.add_source(file, formatted.as_str()) else {
        return conservative(original);
    };
    let reparsed = tuo_parser::parse(verify_map.source(id));

    let same_tokens = significant_tokens(&parsed.tree.lex, original)
        == significant_tokens(&reparsed.tree.lex, &formatted);
    let same_comments = comment_lexemes(&parsed.tree.lex, original)
        == comment_lexemes(&reparsed.tree.lex, &formatted);
    let same_diagnostics = diagnostic_codes(&parsed) == diagnostic_codes(&reparsed);
    if same_tokens && same_comments && same_diagnostics {
        FormatOutcome {
            text: formatted,
            changed: true,
            safe: true,
        }
    } else {
        conservative(original)
    }
}

fn conservative(original: &str) -> FormatOutcome {
    FormatOutcome {
        text: original.to_owned(),
        changed: false,
        safe: false,
    }
}

/// The `(kind, lexeme)` sequence that carries program meaning: every
/// non-trivia token except EOF and separator commas (which the canonical
/// layout normalizes).
fn significant_tokens<'a>(lex: &tuo_lexer::LexResult, text: &'a str) -> Vec<(TokenKind, &'a str)> {
    lex.tokens
        .iter()
        .filter(|token| {
            !token.is_trivia() && !matches!(token.kind, TokenKind::Eof | TokenKind::Comma)
        })
        .map(|token| {
            let range = token.range();
            (
                token.kind,
                &text[range.start().as_usize()..range.end().as_usize()],
            )
        })
        .collect()
}

/// Every `//` comment lexeme, in source order.
fn comment_lexemes<'a>(lex: &tuo_lexer::LexResult, text: &'a str) -> Vec<&'a str> {
    lex.tokens
        .iter()
        .filter(|token| token.kind == TokenKind::LineComment)
        .map(|token| {
            let range = token.range();
            &text[range.start().as_usize()..range.end().as_usize()]
        })
        .collect()
}

/// The multiset of diagnostic codes (as a sorted list).
fn diagnostic_codes(result: &tuo_parser::ParseResult) -> Vec<String> {
    let mut codes: Vec<String> = result
        .all_diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.code.to_string())
        .collect();
    codes.sort_unstable();
    codes
}
