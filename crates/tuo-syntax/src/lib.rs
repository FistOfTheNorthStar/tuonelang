//! Lossless syntax representation for tuonelang.
//!
//! This crate defines the concrete syntax tree the parser produces and the
//! formatter/IDE layers consume: [`SyntaxNode`]s of [`SyntaxKind`] whose
//! leaves are **indices into the lexer's lossless token stream**
//! ([`SyntaxElement::Token`]). It sits between [`tuo_lexer`] and the parser
//! and must not depend on parsing, semantic analysis, or code generation.
//!
//! # Design: structure over a lossless stream
//!
//! Rather than storing text, the tree references the [`tuo_lexer::LexResult`]
//! it was parsed from ([`SyntaxTree::lex`]). Because that stream tiles the
//! source byte-exactly (trivia included), the pair (tree, stream) is fully
//! lossless: [`SyntaxTree::reconstruct`] rebuilds the original text, and
//! [`SyntaxTree::check_coverage`] verifies that the tree accounts for every
//! non-trivia token exactly once — even in the presence of syntax errors,
//! whose material is retained under [`SyntaxKind::Error`] nodes rather than
//! discarded.

mod kind;
mod tree;

pub use kind::SyntaxKind;
pub use tree::{SyntaxElement, SyntaxNode, SyntaxTree};
