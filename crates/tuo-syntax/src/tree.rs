//! The tree structure: nodes over token indices.

use std::fmt::Write as _;

use tuo_lexer::{LexResult, Token, TokenKind};
use tuo_source::Span;

use crate::SyntaxKind;

/// One child of a [`SyntaxNode`]: either a nested node or a reference to a
/// token in the lexed stream (by index into `LexResult::tokens`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SyntaxElement {
    /// A token, identified by its index in the lexed token stream.
    Token(u32),
    /// A nested syntax node.
    Node(SyntaxNode),
}

/// An interior node of the concrete syntax tree.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SyntaxNode {
    /// What production this node represents.
    pub kind: SyntaxKind,
    /// The node's children, in source order.
    pub children: Vec<SyntaxElement>,
}

impl SyntaxNode {
    /// A new node over `children`.
    #[must_use]
    pub fn new(kind: SyntaxKind, children: Vec<SyntaxElement>) -> Self {
        Self { kind, children }
    }

    /// The index of the first token under this node, in tree order.
    #[must_use]
    pub fn first_token(&self) -> Option<u32> {
        self.children.iter().find_map(|child| match child {
            SyntaxElement::Token(index) => Some(*index),
            SyntaxElement::Node(node) => node.first_token(),
        })
    }

    /// The index of the last token under this node, in tree order.
    #[must_use]
    pub fn last_token(&self) -> Option<u32> {
        self.children.iter().rev().find_map(|child| match child {
            SyntaxElement::Token(index) => Some(*index),
            SyntaxElement::Node(node) => node.last_token(),
        })
    }

    /// The source span this node covers, or `None` for an empty node.
    #[must_use]
    pub fn span(&self, tokens: &[Token]) -> Option<Span> {
        let first = tokens.get(self.first_token()? as usize)?;
        let last = tokens.get(self.last_token()? as usize)?;
        first.span.try_join(last.span)
    }

    /// Every node of `kind` in this subtree, in source order (including
    /// `self`).
    #[must_use]
    pub fn descendants_of_kind(&self, kind: SyntaxKind) -> Vec<&Self> {
        let mut found = Vec::new();
        self.collect_kind(kind, &mut found);
        found
    }

    fn collect_kind<'a>(&'a self, kind: SyntaxKind, found: &mut Vec<&'a Self>) {
        if self.kind == kind {
            found.push(self);
        }
        for child in &self.children {
            if let SyntaxElement::Node(node) = child {
                node.collect_kind(kind, found);
            }
        }
    }

    fn collect_token_indices(&self, out: &mut Vec<u32>) {
        for child in &self.children {
            match child {
                SyntaxElement::Token(index) => out.push(*index),
                SyntaxElement::Node(node) => node.collect_token_indices(out),
            }
        }
    }
}

/// A parsed file: the root [`SyntaxNode`] plus the lossless token stream it
/// indexes into.
///
/// # Losslessness
///
/// The tree holds every **non-trivia** token of the stream exactly once, in
/// source order ([`Self::check_coverage`] verifies this). Trivia (whitespace,
/// `//` comments) stays in [`Self::lex`], which itself tiles the source text
/// byte-exactly — so the original text is always reconstructable
/// ([`Self::reconstruct`]), no matter how malformed the input was. That is
/// the property formatting and IDE tooling build on.
#[derive(Clone, Debug)]
pub struct SyntaxTree {
    /// The root node; its kind is [`SyntaxKind::SourceFile`].
    pub root: SyntaxNode,
    /// The lexer output the tree's token indices point into.
    pub lex: LexResult,
}

impl SyntaxTree {
    /// Reconstruct the exact original source text from the token stream.
    ///
    /// `text` must be the source the tree was parsed from; the result is
    /// byte-identical to it (this is asserted by tests and the fuzz target,
    /// not trusted).
    #[must_use]
    pub fn reconstruct(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for token in &self.lex.tokens {
            let range = token.range();
            out.push_str(&text[range.start().as_usize()..range.end().as_usize()]);
        }
        out
    }

    /// Verify the losslessness invariant: the tree references every
    /// non-trivia, non-EOF token of the stream exactly once, in order.
    ///
    /// Returns a description of the first violation, or `Ok(())`.
    ///
    /// # Errors
    ///
    /// When a token is missing from, duplicated in, or out of order in the
    /// tree.
    pub fn check_coverage(&self) -> Result<(), String> {
        let mut in_tree = Vec::new();
        self.root.collect_token_indices(&mut in_tree);
        let expected: Vec<u32> = self
            .lex
            .tokens
            .iter()
            .enumerate()
            .filter(|(_, t)| !t.is_trivia() && t.kind != TokenKind::Eof)
            .map(|(i, _)| u32::try_from(i).expect("token count fits in u32"))
            .collect();
        if in_tree == expected {
            return Ok(());
        }
        Err(format!(
            "tree tokens diverge from stream: tree holds {in_tree:?}, stream has {expected:?}"
        ))
    }

    /// Render the tree structure for snapshots and debugging: one line per
    /// node/token, indented by depth, with each token's kind and lexeme.
    ///
    /// `text` must be the source the tree was parsed from.
    #[must_use]
    pub fn render(&self, text: &str) -> String {
        let mut out = String::new();
        self.render_node(&self.root, text, 0, &mut out);
        out
    }

    fn render_node(&self, node: &SyntaxNode, text: &str, depth: usize, out: &mut String) {
        let indent = "  ".repeat(depth);
        writeln!(out, "{indent}{:?}", node.kind).expect("write to String cannot fail");
        for child in &node.children {
            match child {
                SyntaxElement::Node(nested) => self.render_node(nested, text, depth + 1, out),
                SyntaxElement::Token(index) => {
                    let token = &self.lex.tokens[*index as usize];
                    let range = token.range();
                    let lexeme = &text[range.start().as_usize()..range.end().as_usize()];
                    let indent = "  ".repeat(depth + 1);
                    writeln!(out, "{indent}{:?} {lexeme:?}", token.kind)
                        .expect("write to String cannot fail");
                }
            }
        }
    }
}
