//! Typed views over patterns (grammar section H).

use tuo_lexer::TokenKind;
use tuo_syntax::{SyntaxKind, SyntaxNode};

use crate::Ast;
use crate::context::{Name, ast_view, child_nodes, node_of_kind};

/// A pattern as written in source.
#[derive(Clone, Copy, Debug)]
pub enum Pattern<'a> {
    /// A literal pattern (`1`, `"s"`, `true`, `()`, …).
    Literal(LiteralPat<'a>),
    /// The wildcard pattern `_`.
    Wildcard(WildcardPat<'a>),
    /// A binding pattern: a single identifier.
    Binding(BindingPat<'a>),
    /// A path pattern, optionally with a field block:
    /// `Shape::Circle { radius, .. }`.
    Path(PathPat<'a>),
    /// An or-pattern: `A | B | C`.
    Or(OrPat<'a>),
    /// A parenthesized pattern.
    Group(GroupPat<'a>),
}

impl<'a> Pattern<'a> {
    /// View `node` as a pattern, if it is one.
    #[must_use]
    pub fn cast(ast: Ast<'a>, node: &'a SyntaxNode) -> Option<Self> {
        match node.kind {
            SyntaxKind::LiteralPattern => LiteralPat::cast(ast, node).map(Self::Literal),
            SyntaxKind::WildcardPattern => WildcardPat::cast(ast, node).map(Self::Wildcard),
            SyntaxKind::BindingPattern => BindingPat::cast(ast, node).map(Self::Binding),
            SyntaxKind::PathPattern => PathPat::cast(ast, node).map(Self::Path),
            SyntaxKind::OrPattern => OrPat::cast(ast, node).map(Self::Or),
            SyntaxKind::GroupPattern => GroupPat::cast(ast, node).map(Self::Group),
            _ => None,
        }
    }

    /// The exact source text of the pattern.
    #[must_use]
    pub fn text(self) -> &'a str {
        match self {
            Self::Literal(pat) => pat.text(),
            Self::Wildcard(pat) => pat.text(),
            Self::Binding(pat) => pat.text(),
            Self::Path(pat) => pat.text(),
            Self::Or(pat) => pat.text(),
            Self::Group(pat) => pat.text(),
        }
    }

    /// The underlying CST node.
    #[must_use]
    pub fn syntax(self) -> &'a SyntaxNode {
        match self {
            Self::Literal(pat) => pat.syntax(),
            Self::Wildcard(pat) => pat.syntax(),
            Self::Binding(pat) => pat.syntax(),
            Self::Path(pat) => pat.syntax(),
            Self::Or(pat) => pat.syntax(),
            Self::Group(pat) => pat.syntax(),
        }
    }
}

ast_view! {
    /// A literal pattern.
    LiteralPat from LiteralPattern
}

impl LiteralPat<'_> {
    /// The kind of the literal token (`IntLiteral`, `BoolLiteral`, …);
    /// `OpenParen` for the unit literal `()`.
    #[must_use]
    pub fn token_kind(self) -> Option<TokenKind> {
        crate::context::child_tokens(self.node)
            .next()
            .map(|index| self.ast.token_kind(index))
    }
}

ast_view! {
    /// The wildcard pattern `_`.
    WildcardPat from WildcardPattern
}

ast_view! {
    /// A binding pattern: one identifier introducing a name.
    BindingPat from BindingPattern
}

impl<'a> BindingPat<'a> {
    /// The bound name.
    #[must_use]
    pub fn name(self) -> Option<&'a str> {
        self.ast.direct_token_text(self.node, TokenKind::Ident)
    }

    /// The bound name with its exact span.
    #[must_use]
    pub fn name_ref(self) -> Option<Name<'a>> {
        self.ast.direct_token_name(self.node, TokenKind::Ident)
    }
}

ast_view! {
    /// A path pattern, optionally destructuring fields.
    PathPat from PathPattern
}

impl<'a> PathPat<'a> {
    /// The path segments with their exact spans, in source order. (With a
    /// field block the parser nests the path in a `TypePath` child;
    /// otherwise the segment tokens are direct children.)
    pub fn segment_names(self) -> impl Iterator<Item = Name<'a>> {
        let ast = self.ast;
        let path = node_of_kind(self.node, SyntaxKind::TypePath).unwrap_or(self.node);
        crate::context::child_tokens(path)
            .filter(move |&index| ast.token_kind(index) == TokenKind::Ident)
            .map(move |index| ast.token_name(index))
    }

    /// The destructured fields, if the pattern has a field block.
    pub fn fields(self) -> impl Iterator<Item = FieldPat<'a>> {
        let ast = self.ast;
        node_of_kind(self.node, SyntaxKind::FieldPatternBlock)
            .into_iter()
            .flat_map(move |block| {
                child_nodes(block).filter_map(move |node| FieldPat::cast(ast, node))
            })
    }

    /// Does the field block end in a `..` rest pattern?
    #[must_use]
    pub fn has_rest(self) -> bool {
        node_of_kind(self.node, SyntaxKind::FieldPatternBlock)
            .is_some_and(|block| node_of_kind(block, SyntaxKind::RestPattern).is_some())
    }
}

ast_view! {
    /// One field inside a path pattern's field block: `name` or
    /// `name: pattern`.
    FieldPat from FieldPattern
}

impl<'a> FieldPat<'a> {
    /// The field name.
    #[must_use]
    pub fn name(self) -> Option<&'a str> {
        self.ast.direct_token_text(self.node, TokenKind::Ident)
    }

    /// The field name with its exact span.
    #[must_use]
    pub fn name_ref(self) -> Option<Name<'a>> {
        self.ast.direct_token_name(self.node, TokenKind::Ident)
    }

    /// The sub-pattern, if not the shorthand form.
    #[must_use]
    pub fn pattern(self) -> Option<Pattern<'a>> {
        let ast = self.ast;
        child_nodes(self.node).find_map(move |node| Pattern::cast(ast, node))
    }
}

ast_view! {
    /// An or-pattern: two or more alternatives separated by `|`.
    OrPat from OrPattern
}

impl<'a> OrPat<'a> {
    /// The alternatives, in source order.
    pub fn alternatives(self) -> impl Iterator<Item = Pattern<'a>> {
        let ast = self.ast;
        child_nodes(self.node).filter_map(move |node| Pattern::cast(ast, node))
    }
}

ast_view! {
    /// A parenthesized pattern.
    GroupPat from GroupPattern
}

impl<'a> GroupPat<'a> {
    /// The inner pattern.
    #[must_use]
    pub fn inner(self) -> Option<Pattern<'a>> {
        let ast = self.ast;
        child_nodes(self.node).find_map(move |node| Pattern::cast(ast, node))
    }
}
