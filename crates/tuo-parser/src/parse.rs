//! The public parse entry point and the helpers both engines share.

use tuo_diagnostics::{Confidence, Diagnostic, DiagnosticCode, Edit, Namespace};
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
    const BINARY: [K; 12] = [
        K::Plus,
        K::Minus,
        K::Star,
        K::Slash,
        K::Percent,
        K::AmpAmp,
        K::PipePipe,
        // ADR-0019's bitwise levels chain left-associatively exactly as the
        // arithmetic ones do, so they deepen the tree the same way and must
        // be counted here too — otherwise `x & x & ... & 0` rebuilds the
        // stack overflow the fuzz sweep found on `+` chains.
        K::Amp,
        K::Pipe,
        K::Caret,
        K::LtLt,
        K::GtGt,
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

/// The targeted diagnostic for a parameter mode written at a call site —
/// `f(mut x)` — where `primary` spans the `in`/`mut`/`take` keyword.
///
/// Before it, the keyword sent the statement into generic recovery ("skipped
/// N tokens") and the dropped call then surfaced as an unrelated type error.
/// A mode belongs to the callee's parameter, and a borrow is taken at the
/// call because the signature says so, so the fix is always to delete the
/// keyword: a machine-applicable edit removing it and the whitespace after it.
/// Both engines build the diagnostic here, so they cannot drift.
pub(crate) fn call_arg_mode(primary: Span, source: &SourceText) -> Diagnostic {
    let text = source.text();
    let range = primary.range();
    let start = range.start().as_usize();
    let keyword = &text[start..range.end().as_usize()];
    let mut end = range.end().as_usize();
    while text
        .as_bytes()
        .get(end)
        .is_some_and(u8::is_ascii_whitespace)
    {
        end += 1;
    }
    let removal = TextRange::new(
        ByteOffset::new(u32::try_from(start).unwrap_or(u32::MAX)),
        ByteOffset::new(u32::try_from(end).unwrap_or(u32::MAX)),
    )
    .map_or(primary, |range| Span::new(primary.source(), range));
    Diagnostic::error(
        code(1),
        format!("`{keyword}` is a parameter mode, not part of a call argument"),
        primary,
    )
    .with_primary_label(format!("remove `{keyword}` here"))
    .with_help(
        "modes are written once, on the callee's parameters; a call passes the \
         argument plainly and the signature decides whether it is borrowed or moved",
    )
    .with_suggestion(
        format!("remove `{keyword}`"),
        vec![Edit {
            span: removal,
            replacement: String::new(),
        }],
        Confidence::MachineApplicable,
    )
}

/// The targeted diagnostic for a mutable binding spelled `let mut x`, where
/// `let_kw` and `mut_kw` span the two keywords; the diagnostic spans both.
///
/// Before it, the `mut` sent the statement into generic recovery ("skipped N
/// tokens"), and the dropped binding then surfaced as an unresolved name at
/// every use. tuonelang writes a mutable binding with its own keyword, so the
/// statement now parses as the `var` binding it means, and the fix is always
/// to write `var` in place of `let mut`: a machine-applicable edit. Both
/// engines build the diagnostic here, so they cannot drift.
pub(crate) fn let_mut(let_kw: Span, mut_kw: Span) -> Diagnostic {
    let primary = TextRange::new(let_kw.range().start(), mut_kw.range().end())
        .map_or(let_kw, |range| Span::new(let_kw.source(), range));
    Diagnostic::error(
        code(1),
        "mutable bindings use `var`, not `let mut`",
        primary,
    )
    .with_primary_label("write `var` here")
    .with_help(
        "`let` binds immutably and `var` mutably; there is no `mut` modifier \
             on a binding",
    )
    .with_suggestion(
        "replace `let mut` with `var`",
        vec![Edit {
            span: primary,
            replacement: "var".to_owned(),
        }],
        Confidence::MachineApplicable,
    )
}

/// The targeted diagnostic for an import path whose segments are separated
/// by `.` (`import std.array;`), where `path` spans the whole path.
///
/// Before it, the `.` sent the import into item-level recovery ("skipped N
/// tokens"), and the dropped module then surfaced as an unresolved name at
/// every use. The path now parses as the `::` path it means, and the fix is
/// always to write `::` for each `.`: a machine-applicable edit per
/// separator. Both engines build the diagnostic here, so they cannot drift.
pub(crate) fn import_dotted_path(path: Span, source: &SourceText) -> Diagnostic {
    let range = path.range();
    let start = range.start().as_usize();
    let text = &source.text()[start..range.end().as_usize()];
    let edits = text
        .match_indices('.')
        .filter_map(|(offset, _)| {
            let at = u32::try_from(start + offset).ok()?;
            let dot = TextRange::new(ByteOffset::new(at), ByteOffset::new(at + 1)).ok()?;
            Some(Edit {
                span: Span::new(path.source(), dot),
                replacement: "::".to_owned(),
            })
        })
        .collect();
    Diagnostic::error(
        code(1),
        "import paths separate segments with `::`, not `.`",
        path,
    )
    .with_primary_label("write `::` between these segments")
    .with_help(format!(
        "`.` reaches a field; a module path is written with `::`, as in `{}`",
        text.replace('.', "::"),
    ))
    .with_suggestion(
        "replace `.` with `::`",
        edits,
        Confidence::MachineApplicable,
    )
}

/// The targeted diagnostic for an import written without its closing `;`,
/// where `last` spans the import's last token. It is reported only when the
/// next token starts another item (or the file ends), so the `;` is all that
/// is missing.
///
/// Before it, the import and everything up to the next item keyword fell into
/// item-level recovery ("skipped N tokens"), and the dropped module then
/// surfaced as an unresolved name at every use. The import now parses as if
/// the `;` were there, and the fix is always to insert one after `last`: a
/// machine-applicable edit. Both engines build the diagnostic here, so they
/// cannot drift.
pub(crate) fn import_semi(last: Span) -> Diagnostic {
    let after = Span::new(last.source(), TextRange::empty(last.range().end()));
    Diagnostic::error(code(1), "expected `;` after the import", last)
        .with_primary_label("add `;` after this import")
        .with_help("an import is an item, and ends with `;` like a `const` or a statement")
        .with_suggestion(
            "insert `;`",
            vec![Edit {
                span: after,
                replacement: ";".to_owned(),
            }],
            Confidence::MachineApplicable,
        )
}

/// The targeted diagnostic for a match arm written without the comma that
/// separates it from the one before — most often after a block-bodied arm
/// (`0 => { … }` followed by `_ => …`). `previous` spans the earlier arm's
/// last token and `next` this arm's first.
///
/// Before it, the missing comma sent the whole `match` into generic recovery
/// ("skipped N tokens"), and the dropped statement then surfaced as unrelated
/// resolution and type errors. The arm now parses as if the comma were
/// there, and the fix is always to insert one after `previous`: a
/// machine-applicable edit. Both engines build the diagnostic here, so they
/// cannot drift.
pub(crate) fn match_arm_comma(previous: Span, next: Span) -> Diagnostic {
    let after = Span::new(previous.source(), TextRange::empty(previous.range().end()));
    Diagnostic::error(code(1), "expected `,` between match arms", previous)
        .with_primary_label("add `,` after this arm")
        .with_secondary_label(next, "the next arm starts here")
        .with_help("every match arm but the last is followed by `,`, a block-bodied arm included")
        .with_suggestion(
            "insert `,`",
            vec![Edit {
                span: after,
                replacement: ",".to_owned(),
            }],
            Confidence::MachineApplicable,
        )
}
