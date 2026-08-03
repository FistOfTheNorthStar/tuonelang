//! The public parse entry point and the helpers both engines share.

use tuo_diagnostics::{Diagnostic, DiagnosticCode, Namespace};
use tuo_lexer::LexResult;
use tuo_source::{ByteOffset, SourceText, Span, TextRange};
use tuo_syntax::{SyntaxElement, SyntaxKind, SyntaxNode, SyntaxTree};

use crate::stream::Tok;

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
    // The handwritten recursive-descent + Pratt engine, chosen at the
    // parser architecture decision gate (ADR-parser-strategy).
    crate::handwritten::parse(source)
}

/// The parser's diagnostic codes (the reserved `Pxxxx` namespace).
pub(crate) fn code(number: u16) -> DiagnosticCode {
    DiagnosticCode::new(Namespace::Parser, number)
}

/// The maximum depth for every construct that recurses in the grammar.
const MAX_NEST: usize = 128;

/// Run the [`nests_too_deeply`] pre-scan; on violation, build the flat
/// depth-limited `ParseResult` (one `Error` node over every token, one
/// `P0003`). Shared by both parsing engines so the guarded path is
/// byte-for-byte identical.
pub(crate) fn depth_guard(
    lex: LexResult,
    toks: &[Tok],
    source: &SourceText,
) -> Result<LexResult, ParseResult> {
    let Some(pos) = nests_too_deeply(toks) else {
        return Ok(lex);
    };
    let primary = byte_span(pos, toks, &lex, source);
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
    Err(ParseResult {
        tree: SyntaxTree { root, lex },
        diagnostics: vec![diagnostic],
    })
}

/// Linear pre-scan for input that would recurse beyond [`MAX_NEST`] in the
/// recursive-descent engine, **or** build an expression tree deeper than
/// [`MAX_NEST`] that a later recursive tree-walk (resolution, typing, lowering)
/// would overflow on: delimiter nesting, runs of prefix operators/expression-
/// heading keywords, `=` chains within one statement, and left-associative
/// binary-operator chains (`+ - * / % && ||`) within one statement. Returns the
/// parse-stream position of the first offending token.
///
/// The binary-chain bound is the subtle one. Pratt precedence-climbing parses
/// `a + a + … + a` iteratively, so a long chain does not overflow the *parser* —
/// but it produces a left-nested `BinaryExpr` tree whose depth equals the chain
/// length, and the downstream stages walk that tree recursively. Bounding the
/// chain here keeps the whole pipeline total on adversarial input, honoring this
/// guard's charter (keep constructs shallow enough that no stage recurses past
/// the limit) rather than pushing a depth guard into every later walker.
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
    /// Left-associative binary operators that chain (each one deepens the
    /// expression tree by one level). Comparison and range are excluded — the
    /// grammar makes them non-associative, so they cannot chain.
    const BINARY: [K; 7] = [
        K::Plus,
        K::Minus,
        K::Star,
        K::Slash,
        K::Percent,
        K::AmpAmp,
        K::PipePipe,
    ];
    let mut delims = 0usize;
    let mut prefix_run = 0usize;
    let mut assigns = 0usize;
    let mut binary_run = 0usize;
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
        // Count binary operators within the current statement; a statement
        // boundary or block delimiter resets the chain. `Minus` is both a
        // prefix and a binary operator; counting it here is conservative (it
        // can only *raise* the depth estimate), which is the safe direction.
        match tk.kind {
            k if BINARY.contains(&k) => binary_run += 1,
            K::Semi | K::OpenBrace | K::CloseBrace => binary_run = 0,
            _ => {}
        }
        if delims > MAX_NEST || prefix_run > MAX_NEST || assigns > MAX_NEST || binary_run > MAX_NEST
        {
            return Some(pos);
        }
    }
    None
}

/// The byte-level span of the token at parse-stream position `pos`, or an
/// empty span at end of file.
pub(crate) fn byte_span(pos: usize, toks: &[Tok], lex: &LexResult, source: &SourceText) -> Span {
    let range = toks
        .get(pos)
        .and_then(|tk| lex.tokens.get(tk.index as usize))
        .map_or_else(
            || TextRange::empty(ByteOffset::new(source.len())),
            tuo_lexer::Token::range,
        );
    Span::new(source.source(), range)
}
