//! Lexer for tuonelang.
//!
//! Turns one immutable source snapshot ([`tuo_source::SourceText`]) into a
//! complete token stream: [`lex`] → [`LexResult`] of [`Token`]s
//! ([`TokenKind`] + byte-exact [`tuo_source::Span`]) plus structured
//! [`tuo_diagnostics::Diagnostic`]s. It depends only on [`tuo_source`] and
//! [`tuo_diagnostics`] and never on parsing, name resolution, or type
//! checking — a keyword is always a keyword, regardless of syntactic
//! position.
//!
//! The normative reference is `specification/lexical-grammar.md` (and the
//! `(A)` section of `specification/grammar.ebnf`); section references like
//! (§2) below point into `specification/CONSTITUTION.md`.
//!
//! # Design
//!
//! - **TDG-owned boundary.** The public surface is exactly [`Token`],
//!   [`TokenKind`], and [`LexResult`]. The tokenizer is generated with
//!   [Logos](https://crates.io/crates/logos) today, but no Logos type appears
//!   in the API — the engine can be replaced without touching any consumer.
//! - **Lossless.** Nothing is skipped: whitespace and `//` comments are real
//!   tokens ([`TokenKind::Whitespace`], [`TokenKind::LineComment`]), because
//!   the lossless syntax layer ([`tuo-syntax`]) and the formatter need every
//!   byte. The non-EOF tokens of a [`LexResult`] tile the input exactly.
//!   [`LexResult::parser_tokens`] is the trivia-stripped view the parser
//!   consumes; doc comments (`///`, [`TokenKind::DocComment`]) survive that
//!   view because they attach to items (§3).
//! - **Errors are values.** Malformed input produces [`TokenKind::Error`]
//!   tokens plus diagnostics, and lexing continues (§9) — the parser always
//!   receives a full stream, and diagnostics accumulate rather than aborting
//!   at the first problem.
//!
//! [`tuo-syntax`]: ../tuo_syntax/index.html
//!
//! # Lexical diagnostic codes
//!
//! The lexer owns the reserved `Lxxxx` namespace
//! ([`tuo_diagnostics::Namespace::Lexical`]). Codes assigned so far:
//!
//! | Code    | Meaning                                                |
//! |---------|--------------------------------------------------------|
//! | `L0001` | unexpected character                                   |
//! | `L0002` | byte order mark at start of file (§1)                  |
//! | `L0003` | carriage return — source is LF-only (§1)               |
//! | `L0004` | tab character outside a literal (§1)                   |
//! | `L0005` | identifier not in Unicode NFC (§2)                     |
//! | `L0006` | unterminated string literal                            |
//! | `L0007` | empty or unterminated character literal                |
//! | `L0008` | malformed integer literal (missing/invalid digits)     |
//! | `L0009` | invalid escape sequence in a string or char literal    |
//!
//! # Performance
//!
//! Throughput is measured, not assumed: `benches/throughput.rs` is a
//! Criterion benchmark over representative source. No performance claims are
//! made beyond what those numbers show on a given machine.

mod lex;
mod token;

pub use lex::lex;
pub use token::{LexResult, Token, TokenKind};
