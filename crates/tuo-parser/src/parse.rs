//! The public parse entry point and error conversion.

use chumsky::Parser as _;
use chumsky::error::{Rich, RichReason};

use tuo_diagnostics::{Diagnostic, DiagnosticCode, Namespace};
use tuo_lexer::LexResult;
use tuo_source::{ByteOffset, SourceText, Span, TextRange};
use tuo_syntax::{SyntaxElement, SyntaxKind, SyntaxNode, SyntaxTree};

use crate::grammar;
use crate::stream::{Tok, kind_name, parse_stream};

/// Everything the parser produced for one source snapshot.
///
/// Parsing never fails and never panics: malformed input yields a tree with
/// [`SyntaxKind::Error`] nodes plus diagnostics, and the tree remains fully
/// reconstructable ([`SyntaxTree::reconstruct`]).
#[derive(Clone, Debug)]
pub struct ParseResult {
    /// The concrete syntax tree, including the lossless token stream.
    pub tree: SyntaxTree,
    /// Parser diagnostics (`Pxxxx` codes), in source order. Lexical
    /// diagnostics live in `tree.lex.diagnostics`.
    pub diagnostics: Vec<Diagnostic>,
}

impl ParseResult {
    /// Lexical and parser diagnostics together, in source order.
    #[must_use]
    pub fn all_diagnostics(&self) -> Vec<Diagnostic> {
        let mut all = self.tree.lex.diagnostics.clone();
        all.extend(self.diagnostics.iter().cloned());
        all.sort_by_key(|d| d.primary_span.range().start());
        all
    }

    /// Did lexing or parsing produce any diagnostics?
    #[must_use]
    pub fn has_errors(&self) -> bool {
        !self.diagnostics.is_empty() || !self.tree.lex.diagnostics.is_empty()
    }
}

/// Parse one immutable source snapshot into a concrete syntax tree.
///
/// Lexes internally, then parses the trivia-stripped stream. On syntax
/// errors the parser recovers at the language's synchronization points
/// (`;`, `}`, item-leading keywords), keeps the skipped material under
/// [`SyntaxKind::Error`] nodes, and accumulates one diagnostic per skip —
/// so a file with several broken constructs reports several errors and
/// still yields every well-formed construct around them.
#[must_use]
pub fn parse(source: &SourceText) -> ParseResult {
    let lex = tuo_lexer::lex(source);
    let toks = parse_stream(&lex);

    // Guard the recursive-descent engine against adversarial nesting depth
    // (P0003) *before* parsing: a stack overflow is not a diagnostic.
    if let Some(pos) = nests_too_deeply(&toks) {
        let primary = byte_span(pos, &toks, &lex, source);
        let els = toks
            .iter()
            .map(|tk| SyntaxElement::Token(tk.index))
            .collect();
        let root = SyntaxNode::new(
            SyntaxKind::SourceFile,
            vec![SyntaxElement::Node(SyntaxNode::new(SyntaxKind::Error, els))],
        );
        let diagnostic = Diagnostic::error(
            code(3),
            format!("construct nests deeper than the parser's limit ({MAX_NEST})"),
            primary,
        )
        .with_note("deeply nested delimiters, prefix operators, or chains are rejected before parsing to keep the parser total");
        return ParseResult {
            tree: SyntaxTree { root, lex },
            diagnostics: vec![diagnostic],
        };
    }

    let (output, errors) = grammar::parser().parse(&toks[..]).into_output_errors();

    let mut diagnostics: Vec<Diagnostic> = errors
        .into_iter()
        .map(|error| to_diagnostic(&error, &toks, &lex, source))
        .collect();
    diagnostics.sort_by_key(|d| d.primary_span.range().start());

    let root = output.unwrap_or_else(|| {
        // Unrecoverable parse (should be rare: recovery is total by
        // construction). Keep losslessness: every token goes under one
        // Error node. The failure itself has already produced diagnostics.
        let els = toks
            .iter()
            .map(|tk| SyntaxElement::Token(tk.index))
            .collect();
        SyntaxNode::new(
            SyntaxKind::SourceFile,
            vec![SyntaxElement::Node(SyntaxNode::new(SyntaxKind::Error, els))],
        )
    });

    ParseResult {
        tree: SyntaxTree { root, lex },
        diagnostics,
    }
}

/// The parser's diagnostic codes (the reserved `Pxxxx` namespace).
fn code(number: u16) -> DiagnosticCode {
    DiagnosticCode::new(Namespace::Parser, number)
}

/// The maximum depth for every construct that recurses in the grammar.
const MAX_NEST: usize = 128;

/// Linear pre-scan for input that would recurse beyond [`MAX_NEST`] in the
/// recursive-descent engine: delimiter nesting, runs of prefix
/// operators/expression-heading keywords, and `=` chains within one
/// statement. Returns the parse-stream position of the first offending
/// token.
fn nests_too_deeply(toks: &[Tok]) -> Option<usize> {
    use tuo_lexer::TokenKind as K;
    /// Tokens that begin a nested expression without a delimiter.
    const PREFIX: [K; 8] = [
        K::Minus,
        K::Bang,
        K::KwMove,
        K::KwReturn,
        K::KwBreak,
        K::KwIf,
        K::KwWhile,
        K::KwMatch,
    ];
    let mut delims = 0usize;
    let mut prefix_run = 0usize;
    let mut assigns = 0usize;
    for (pos, tk) in toks.iter().enumerate() {
        match tk.kind {
            K::OpenParen | K::OpenBracket | K::OpenBrace => delims += 1,
            K::CloseParen | K::CloseBracket | K::CloseBrace => delims = delims.saturating_sub(1),
            _ => {}
        }
        prefix_run = if PREFIX.contains(&tk.kind) {
            prefix_run + 1
        } else {
            0
        };
        match tk.kind {
            K::Eq => assigns += 1,
            K::Semi | K::OpenBrace | K::CloseBrace => assigns = 0,
            _ => {}
        }
        if delims > MAX_NEST || prefix_run > MAX_NEST || assigns > MAX_NEST {
            return Some(pos);
        }
    }
    None
}

/// Convert one chumsky error to a diagnostic.
///
/// - `P0001`: unexpected token (expected/found, from the grammar itself).
/// - `P0002`: tokens skipped during recovery ("malformed statement/item:
///   skipped …"), pointing at the first skipped token.
fn to_diagnostic(
    error: &Rich<'_, Tok>,
    toks: &[Tok],
    lex: &LexResult,
    source: &SourceText,
) -> Diagnostic {
    let span = error.span();
    let primary = byte_span(span.start, toks, lex, source);

    match error.reason() {
        RichReason::Custom(message) => Diagnostic::error(code(2), message.clone(), primary)
            .with_primary_label("skipped during error recovery")
            .with_note(
                "the parser resynchronized at the next `;`, `}`, or item keyword and continued",
            ),
        RichReason::ExpectedFound { .. } => {
            let mut expected: Vec<String> = error.expected().map(ToString::to_string).collect();
            expected.sort_unstable();
            expected.dedup();
            if expected.len() > 8 {
                expected.truncate(8);
                expected.push("…".to_owned());
            }
            let expected = if expected.is_empty() {
                "a different token".to_owned()
            } else {
                expected.join(" or ")
            };
            let found = error.found().map_or("end of file", |tk| kind_name(tk.kind));
            Diagnostic::error(
                code(1),
                format!("expected {expected}, found {found}"),
                primary,
            )
            .with_primary_label(format!("expected {expected}"))
        }
    }
}

/// The byte-level span of the token at parse-stream position `pos`, or an
/// empty span at end of file.
fn byte_span(pos: usize, toks: &[Tok], lex: &LexResult, source: &SourceText) -> Span {
    let range = toks
        .get(pos)
        .and_then(|tk| lex.tokens.get(tk.index as usize))
        .map_or_else(
            || TextRange::empty(ByteOffset::new(source.len())),
            tuo_lexer::Token::range,
        );
    Span::new(source.source(), range)
}
