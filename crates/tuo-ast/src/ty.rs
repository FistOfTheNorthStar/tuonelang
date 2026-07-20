//! Typed views over type references (grammar section D).

use tuo_lexer::TokenKind;
use tuo_syntax::{SyntaxKind, SyntaxNode};

use crate::Ast;
use crate::context::{Name, ast_view, child_nodes, node_of_kind};

/// A reference to a type as written in source: `()`, a wrapper
/// (`Box[T]`, `Shared[T]`, `Weak[T]`), or a (possibly generic) path.
#[derive(Clone, Copy, Debug)]
pub enum TypeRef<'a> {
    /// The unit type `()`.
    Unit(UnitType<'a>),
    /// A memory wrapper type: `Box[…]`, `Shared[…]`, or `Weak[…]`.
    Wrapper(WrapperType<'a>),
    /// A path type such as `Int` or `collections::Map[K, V]`.
    Path(PathType<'a>),
}

impl<'a> TypeRef<'a> {
    /// View `node` as a type reference, if it is one.
    #[must_use]
    pub fn cast(ast: Ast<'a>, node: &'a SyntaxNode) -> Option<Self> {
        match node.kind {
            SyntaxKind::UnitType => UnitType::cast(ast, node).map(Self::Unit),
            SyntaxKind::WrapperType => WrapperType::cast(ast, node).map(Self::Wrapper),
            SyntaxKind::PathType => PathType::cast(ast, node).map(Self::Path),
            _ => None,
        }
    }

    /// The exact source span this type reference covers, or `None` if it
    /// is empty.
    #[must_use]
    pub fn span(self) -> Option<tuo_source::Span> {
        match self {
            Self::Unit(ty) => ty.span(),
            Self::Wrapper(ty) => ty.span(),
            Self::Path(ty) => ty.span(),
        }
    }

    /// The exact source text of the type reference.
    #[must_use]
    pub fn text(self) -> &'a str {
        match self {
            Self::Unit(ty) => ty.text(),
            Self::Wrapper(ty) => ty.text(),
            Self::Path(ty) => ty.text(),
        }
    }

    /// The underlying CST node.
    #[must_use]
    pub fn syntax(self) -> &'a SyntaxNode {
        match self {
            Self::Unit(ty) => ty.syntax(),
            Self::Wrapper(ty) => ty.syntax(),
            Self::Path(ty) => ty.syntax(),
        }
    }
}

ast_view! {
    /// The unit type `()`.
    UnitType from UnitType
}

ast_view! {
    /// A wrapper type: `Box[…]`, `Shared[…]`, or `Weak[…]`.
    WrapperType from WrapperType
}

impl<'a> WrapperType<'a> {
    /// The wrapper keyword as written (`Box`, `Shared`, or `Weak`).
    #[must_use]
    pub fn wrapper(self) -> Option<&'a str> {
        crate::context::child_tokens(self.node)
            .next()
            .map(|index| self.ast.token_text(index))
    }

    /// The wrapped type arguments.
    #[must_use]
    pub fn args(self) -> Option<TypeArgs<'a>> {
        node_of_kind(self.node, SyntaxKind::TypeArguments)
            .and_then(|node| TypeArgs::cast(self.ast, node))
    }
}

ast_view! {
    /// A path type such as `Int`, `Self`, or `collections::Map[K, V]`.
    PathType from PathType
}

impl<'a> PathType<'a> {
    /// The path part of the type.
    #[must_use]
    pub fn path(self) -> Option<TypePath<'a>> {
        node_of_kind(self.node, SyntaxKind::TypePath)
            .and_then(|node| TypePath::cast(self.ast, node))
    }

    /// The type arguments, if the path is generic.
    #[must_use]
    pub fn args(self) -> Option<TypeArgs<'a>> {
        node_of_kind(self.node, SyntaxKind::TypeArguments)
            .and_then(|node| TypeArgs::cast(self.ast, node))
    }
}

ast_view! {
    /// A type path: `Self` or `IDENT (:: IDENT)*`.
    TypePath from TypePath
}

impl<'a> TypePath<'a> {
    /// The path segments, in source order.
    pub fn segments(self) -> impl Iterator<Item = &'a str> {
        self.segment_names().map(|name| name.text)
    }

    /// The path segments with their exact spans, in source order.
    pub fn segment_names(self) -> impl Iterator<Item = Name<'a>> {
        let ast = self.ast;
        crate::context::child_tokens(self.node)
            .filter(move |&index| {
                matches!(
                    ast.token_kind(index),
                    TokenKind::Ident | TokenKind::KwSelfType
                )
            })
            .map(move |index| ast.token_name(index))
    }
}

ast_view! {
    /// A bracketed type-argument list: `[T, U]`.
    TypeArgs from TypeArguments
}

impl<'a> TypeArgs<'a> {
    /// The argument types, in source order.
    pub fn types(self) -> impl Iterator<Item = TypeRef<'a>> {
        let ast = self.ast;
        child_nodes(self.node).filter_map(move |node| TypeRef::cast(ast, node))
    }
}
