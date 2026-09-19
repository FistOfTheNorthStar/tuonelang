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
use tuo_source::{SourceText, Span};
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

/// The marker a targeted recovery prefixes to its custom message, so
/// [`to_diagnostic`] can tell it from an ordinary skipped-token report.
pub(crate) const TARGETED: &str = "\u{1}targeted:";

/// Build the targeted-recovery diagnostic named by `message`, or `None` if
/// `message` is an ordinary recovery report.
///
/// The grammar is built over token *kinds* and has no access to the source
/// text, so it cannot spell the parameter's name itself; it emits the marker
/// alone and the name is read here, off the same span the diagnostic points
/// at. Targeted recoveries must match the handwritten engine's diagnostic
/// exactly — same code, message, label, and help — because `oracle_parity`
/// compares them.
fn targeted_recovery(message: &str, primary: Span, source: &SourceText) -> Option<Diagnostic> {
    let kind = message.strip_prefix(TARGETED)?;
    if kind == "generic-params-empty" {
        return Some(
            Diagnostic::error(code(1), "empty generic parameter list".to_owned(), primary)
                .with_primary_label("a generic parameter list needs at least one parameter")
                .with_help(concat!(
                    "name a type parameter (`[T]`), or remove the brackets — a ",
                    "declaration with no type parameters is written without a list",
                )),
        );
    }
    if kind != "param-missing-mode" {
        return None;
    }
    let range = primary.range();
    let name = &source.text()[range.start().as_usize()..range.end().as_usize()];
    Some(
        Diagnostic::error(
            code(1),
            format!("parameter `{name}` is missing its passing mode"),
            primary,
        )
        .with_primary_label("expected `in`, `mut`, or `take` before this name")
        .with_help(concat!(
            "every parameter states how it takes its argument: `in` borrows it ",
            "read-only, `mut` borrows it mutably, `take` moves it — there is no ",
            "default, so the choice is always written",
        )),
    )
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
        // A custom message is a recovery report by default (`P0002`). The one
        // exception is a *targeted* recovery, which diagnoses a specific
        // mistake rather than reporting skipped tokens: it carries the
        // `TARGETED` sentinel so it keeps its own code, label, and help
        // instead of being dressed as generic resynchronization. The
        // handwritten engine produces the identical diagnostic directly;
        // parity over the `invalid/` corpus pins the two together.
        RichReason::Custom(message) => match targeted_recovery(message, primary, source) {
            Some(diagnostic) => diagnostic,
            None => Diagnostic::error(code(2), message.clone(), primary)
                .with_primary_label("skipped during error recovery")
                .with_note(
                    "the parser resynchronized at the next `;`, `}`, or item keyword and continued",
                ),
        },
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
