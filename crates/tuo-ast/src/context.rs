//! The view context ([`Ast`]) and the low-level CST navigation helpers the
//! typed views are built from.

use tuo_lexer::{Token, TokenKind};
use tuo_source::Span;
use tuo_syntax::{SyntaxElement, SyntaxNode, SyntaxTree};

use crate::item::SourceFile;

/// One spanned name occurrence: the exact identifier text plus where it is.
///
/// This is the currency of tooling that needs to *point at* names — name
/// resolution, rename, and go-to-definition — rather than merely read them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Name<'a> {
    /// The name as written in source.
    pub text: &'a str,
    /// Exactly where it is written.
    pub span: Span,
}

/// The borrowed `(tree, text)` pair every typed view hangs off.
///
/// `text` must be the exact source the tree was parsed from — token and node
/// text accessors slice it by the spans recorded in the lossless stream.
/// The pair is two references, so `Ast` is `Copy` and views stay cheap.
#[derive(Clone, Copy, Debug)]
pub struct Ast<'a> {
    pub(crate) tree: &'a SyntaxTree,
    pub(crate) text: &'a str,
}

impl<'a> Ast<'a> {
    /// View `tree` (parsed from exactly `text`) through typed accessors.
    #[must_use]
    pub fn new(tree: &'a SyntaxTree, text: &'a str) -> Self {
        Self { tree, text }
    }

    /// The typed view of the root node.
    #[must_use]
    pub fn file(self) -> SourceFile<'a> {
        SourceFile::cast(self, &self.tree.root)
            .expect("the parser's root node is always a SourceFile")
    }

    /// The underlying syntax tree.
    #[must_use]
    pub fn tree(self) -> &'a SyntaxTree {
        self.tree
    }

    /// The token at `index` in the lossless stream.
    pub(crate) fn token(self, index: u32) -> &'a Token {
        &self.tree.lex.tokens[index as usize]
    }

    /// The kind of the token at `index`.
    pub(crate) fn token_kind(self, index: u32) -> TokenKind {
        self.token(index).kind
    }

    /// The exact source text of the token at `index`.
    pub(crate) fn token_text(self, index: u32) -> &'a str {
        let range = self.token(index).range();
        &self.text[range.start().as_usize()..range.end().as_usize()]
    }

    /// The exact source text `node` covers (empty for an empty node).
    pub(crate) fn node_text(self, node: &SyntaxNode) -> &'a str {
        node.span(&self.tree.lex.tokens).map_or("", |span| {
            let range = span.range();
            &self.text[range.start().as_usize()..range.end().as_usize()]
        })
    }

    /// The text of the first **direct** child token of `kind`, if any.
    pub(crate) fn direct_token_text(self, node: &SyntaxNode, kind: TokenKind) -> Option<&'a str> {
        child_tokens(node)
            .find(|&index| self.token_kind(index) == kind)
            .map(|index| self.token_text(index))
    }

    /// Does `node` have a direct child token of `kind`?
    pub(crate) fn has_token(self, node: &SyntaxNode, kind: TokenKind) -> bool {
        child_tokens(node).any(|index| self.token_kind(index) == kind)
    }

    /// The spanned name of the token at `index`.
    pub(crate) fn token_name(self, index: u32) -> Name<'a> {
        Name {
            text: self.token_text(index),
            span: self.token(index).span,
        }
    }

    /// The spanned name of the first **direct** child token of `kind`.
    pub(crate) fn direct_token_name(self, node: &SyntaxNode, kind: TokenKind) -> Option<Name<'a>> {
        child_tokens(node)
            .find(|&index| self.token_kind(index) == kind)
            .map(|index| self.token_name(index))
    }
}

/// The direct child nodes of `node`, in source order.
pub(crate) fn child_nodes(node: &SyntaxNode) -> impl Iterator<Item = &SyntaxNode> {
    node.children.iter().filter_map(|child| match child {
        SyntaxElement::Node(nested) => Some(nested),
        SyntaxElement::Token(_) => None,
    })
}

/// The direct child token indices of `node`, in source order.
pub(crate) fn child_tokens(node: &SyntaxNode) -> impl Iterator<Item = u32> + '_ {
    node.children.iter().filter_map(|child| match child {
        SyntaxElement::Token(index) => Some(*index),
        SyntaxElement::Node(_) => None,
    })
}

/// The `n`-th direct child node of `node` (zero-based).
pub(crate) fn nth_node(node: &SyntaxNode, n: usize) -> Option<&SyntaxNode> {
    child_nodes(node).nth(n)
}

/// The first direct child node of `kind`.
pub(crate) fn node_of_kind(node: &SyntaxNode, kind: tuo_syntax::SyntaxKind) -> Option<&SyntaxNode> {
    child_nodes(node).find(|child| child.kind == kind)
}

/// Every direct child node of `kind`, in source order.
pub(crate) fn nodes_of_kind(
    node: &SyntaxNode,
    kind: tuo_syntax::SyntaxKind,
) -> impl Iterator<Item = &SyntaxNode> {
    child_nodes(node).filter(move |child| child.kind == kind)
}

/// Define one typed view struct: a `Copy` wrapper over ([`Ast`], node) that
/// casts from the given [`tuo_syntax::SyntaxKind`]s and exposes the
/// underlying node, its span, and its exact source text.
macro_rules! ast_view {
    ($(#[$doc:meta])* $name:ident from $($kind:ident)|+) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug)]
        pub struct $name<'a> {
            pub(crate) ast: $crate::Ast<'a>,
            pub(crate) node: &'a tuo_syntax::SyntaxNode,
        }

        impl<'a> $name<'a> {
            /// View `node` through this type, if its kind matches.
            #[must_use]
            pub fn cast(ast: $crate::Ast<'a>, node: &'a tuo_syntax::SyntaxNode) -> Option<Self> {
                matches!(node.kind, $(tuo_syntax::SyntaxKind::$kind)|+)
                    .then_some(Self { ast, node })
            }

            /// The underlying CST node.
            #[must_use]
            pub fn syntax(self) -> &'a tuo_syntax::SyntaxNode {
                self.node
            }

            /// The exact source span this construct covers, or `None` if it
            /// is empty.
            #[must_use]
            pub fn span(self) -> Option<tuo_source::Span> {
                self.node.span(&self.ast.tree.lex.tokens)
            }

            /// The exact source text this construct covers.
            #[must_use]
            pub fn text(self) -> &'a str {
                self.ast.node_text(self.node)
            }
        }
    };
}
pub(crate) use ast_view;
