//! Typed views over blocks and statements (grammar section F).

use tuo_syntax::{SyntaxKind, SyntaxNode};

use crate::Ast;
use crate::context::{ast_view, child_nodes};
use crate::expr::Expr;
use crate::item::{ConstDecl, ErrorNode};
use crate::pat::Pattern;
use crate::ty::TypeRef;

ast_view! {
    /// A `{ … }` block: statements followed by an optional tail expression.
    Block from Block
}

impl<'a> Block<'a> {
    /// The statements, in source order (including error islands from
    /// recovery).
    pub fn statements(self) -> impl Iterator<Item = Statement<'a>> {
        let ast = self.ast;
        child_nodes(self.node).filter_map(move |node| Statement::cast(ast, node))
    }

    /// The trailing tail expression, if the block has one.
    #[must_use]
    pub fn tail(self) -> Option<Expr<'a>> {
        let ast = self.ast;
        child_nodes(self.node)
            .find(|node| node.kind.is_expression())
            .and_then(|node| Expr::cast(ast, node))
    }
}

/// A statement inside a block (or a spec body's plain statements).
#[derive(Clone, Copy, Debug)]
pub enum Statement<'a> {
    /// A `let` binding.
    Let(BindingStmt<'a>),
    /// A `var` binding.
    Var(BindingStmt<'a>),
    /// A block-local `const` item.
    Const(ConstDecl<'a>),
    /// An expression statement.
    Expr(ExprStmt<'a>),
    /// A lone `;`.
    Empty(EmptyStmt<'a>),
    /// Material skipped during error recovery.
    Error(ErrorNode<'a>),
}

impl<'a> Statement<'a> {
    /// View `node` as a statement, if it is one.
    #[must_use]
    pub fn cast(ast: Ast<'a>, node: &'a SyntaxNode) -> Option<Self> {
        match node.kind {
            SyntaxKind::LetStatement => BindingStmt::cast(ast, node).map(Self::Let),
            SyntaxKind::VarStatement => BindingStmt::cast(ast, node).map(Self::Var),
            SyntaxKind::ConstItem => ConstDecl::cast(ast, node).map(Self::Const),
            SyntaxKind::ExpressionStatement => ExprStmt::cast(ast, node).map(Self::Expr),
            SyntaxKind::EmptyStatement => EmptyStmt::cast(ast, node).map(Self::Empty),
            SyntaxKind::Error => ErrorNode::cast(ast, node).map(Self::Error),
            _ => None,
        }
    }
}

ast_view! {
    /// A `let` or `var` binding statement.
    BindingStmt from LetStatement | VarStatement
}

impl<'a> BindingStmt<'a> {
    /// Is this a `var` (mutable) binding?
    #[must_use]
    pub fn is_var(self) -> bool {
        self.node.kind == SyntaxKind::VarStatement
    }

    /// The bound pattern.
    #[must_use]
    pub fn pattern(self) -> Option<Pattern<'a>> {
        let ast = self.ast;
        child_nodes(self.node).find_map(move |node| Pattern::cast(ast, node))
    }

    /// The declared type, if annotated.
    #[must_use]
    pub fn ty(self) -> Option<TypeRef<'a>> {
        let ast = self.ast;
        child_nodes(self.node).find_map(move |node| TypeRef::cast(ast, node))
    }

    /// The initializer, if present.
    #[must_use]
    pub fn initializer(self) -> Option<Expr<'a>> {
        let ast = self.ast;
        child_nodes(self.node)
            .find(|node| node.kind.is_expression())
            .and_then(|node| Expr::cast(ast, node))
    }
}

ast_view! {
    /// An expression statement: `expr;` or a block-form expression standing
    /// alone.
    ExprStmt from ExpressionStatement
}

impl<'a> ExprStmt<'a> {
    /// The statement's expression.
    #[must_use]
    pub fn expr(self) -> Option<Expr<'a>> {
        let ast = self.ast;
        child_nodes(self.node).find_map(move |node| Expr::cast(ast, node))
    }
}

ast_view! {
    /// A lone `;`.
    EmptyStmt from EmptyStatement
}
