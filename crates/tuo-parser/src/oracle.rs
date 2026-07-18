//! The retained Chumsky engine, kept as a differential-testing oracle.
//!
//! The parser architecture decision gate
//! (`specification/adr/ADR-parser-strategy.md`) selected the handwritten
//! engine ([`crate::parse`]); this module keeps the original Chumsky
//! implementation callable so the `oracle_parity` test suite can compare
//! the two engines tree-for-tree and diagnostic-for-diagnostic on every
//! fixture and on randomized input.
//!
//! **Removal gate:** this oracle (and the `chumsky` dependency with it) is
//! scheduled for removal once the handwritten engine's own test corpus is
//! judged sufficient — see the ADR's follow-up section. Nothing outside
//! differential tests and the decision-gate benchmarks may depend on it.

use chumsky::Parser as _;
use chumsky::error::{Rich, RichReason};

use tuo_diagnostics::Diagnostic;
use tuo_lexer::LexResult;
use tuo_source::SourceText;
use tuo_syntax::{SyntaxElement, SyntaxKind, SyntaxNode, SyntaxTree};

use crate::ParseResult;
use crate::grammar;
use crate::parse::{byte_span, code, depth_guard};
use crate::stream::{Tok, kind_name, parse_stream};

/// Parse one immutable source snapshot with the Chumsky oracle engine.
///
/// Same contract and same output as [`crate::parse`] (enforced by the
/// `oracle_parity` tests); exists only for differential testing and the
/// decision-gate benchmarks.
#[must_use]
pub fn parse(source: &SourceText) -> ParseResult {
    let lex = tuo_lexer::lex(source);
    let toks = parse_stream(&lex);

    // Same adversarial-nesting guard (P0003) as the primary engine.
    let lex = match depth_guard(lex, &toks, source) {
        Ok(lex) => lex,
        Err(result) => return result,
    };

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
