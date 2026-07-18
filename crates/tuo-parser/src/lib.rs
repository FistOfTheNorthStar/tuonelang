//! Parser for tuonelang.
//!
//! Turns one immutable source snapshot into a lossless concrete syntax tree
//! ([`tuo_syntax::SyntaxTree`]): [`parse`] → [`ParseResult`]. The normative
//! reference is `specification/grammar.ebnf` (sections (B)–(I)) with
//! `specification/syntax.md` as prose companion.
//!
//! # Design
//!
//! - **TDG-owned boundary.** The public surface is [`parse`] and
//!   [`ParseResult`]; everything else is [`tuo_syntax`] and
//!   [`tuo_diagnostics`] data. The grammar is implemented with
//!   [Chumsky](https://crates.io/crates/chumsky) today, but no Chumsky type
//!   appears in the API — the engine is replaceable.
//! - **Complete files, complete recovery.** [`parse`] always consumes the
//!   whole file and never panics on any input. On a syntax error it skips to
//!   the language's deliberate synchronization points — `;`, `}`, and
//!   item-leading keywords (grammar NOTES §2) — keeps the skipped tokens
//!   under [`tuo_syntax::SyntaxKind::Error`] nodes for IDE tooling, records
//!   a diagnostic, and continues. Several broken constructs produce several
//!   diagnostics, and the well-formed constructs around them all survive.
//! - **Lossless.** The tree references every non-trivia token exactly once
//!   ([`tuo_syntax::SyntaxTree::check_coverage`]); together with the lexer's
//!   lossless stream the original text is always reconstructable, malformed
//!   or not.
//!
//! # Parser diagnostic codes
//!
//! The parser owns the reserved `Pxxxx` namespace
//! ([`tuo_diagnostics::Namespace::Parser`]). Codes assigned so far:
//!
//! | Code    | Meaning                                                    |
//! |---------|------------------------------------------------------------|
//! | `P0001` | unexpected token (expected/found, from the grammar)        |
//! | `P0002` | tokens skipped while recovering to a synchronization point |
//! | `P0003` | construct nests deeper than the parser's limit             |

mod grammar;
mod parse;
mod stream;

pub use parse::{ParseResult, parse};
