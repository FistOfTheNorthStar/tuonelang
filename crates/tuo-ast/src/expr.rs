//! Typed views over expressions (grammar section G).

use tuo_lexer::TokenKind;
use tuo_syntax::{SyntaxKind, SyntaxNode};

use crate::Ast;
use crate::context::{Name, ast_view, child_nodes, child_tokens, node_of_kind, nth_node};
use crate::pat::Pattern;
use crate::stmt::Block;
use crate::ty::{TypeArgs, TypePath, TypeRef};

/// An expression as written in source.
#[derive(Clone, Copy, Debug)]
pub enum Expr<'a> {
    /// A literal token (`1`, `1.5`, `true`, `'a'`, `"s"`) or the unit
    /// literal `()`.
    Literal(LiteralExpr<'a>),
    /// A (possibly qualified) path, with an optional turbofish.
    Path(PathExpr<'a>),
    /// A struct literal: `Point { x: 1, y }`.
    StructLiteral(StructLiteralExpr<'a>),
    /// A prefix operator application: `-x`, `!x`, `move x`.
    Unary(UnaryExpr<'a>),
    /// An infix operator application.
    Binary(BinaryExpr<'a>),
    /// A range: `a .. b`.
    Range(RangeExpr<'a>),
    /// An assignment: `target = value`.
    Assign(AssignExpr<'a>),
    /// A call: `callee(args)`.
    Call(CallExpr<'a>),
    /// A method call: `receiver.name(args)`.
    MethodCall(MethodCallExpr<'a>),
    /// A field access: `receiver.name` or `tuple.0`.
    Field(FieldExpr<'a>),
    /// An index: `base[index]`.
    Index(IndexExpr<'a>),
    /// Error propagation: `inner?`.
    Try(TryExpr<'a>),
    /// A cast: `inner as Type`.
    Cast(CastExpr<'a>),
    /// A parenthesized expression.
    Group(GroupExpr<'a>),
    /// An `if` expression with optional `else` chain.
    If(IfExpr<'a>),
    /// A `match` expression.
    Match(MatchExpr<'a>),
    /// A `while` loop.
    While(WhileExpr<'a>),
    /// A (possibly labeled) `loop`.
    Loop(LoopExpr<'a>),
    /// A `for` loop.
    For(ForExpr<'a>),
    /// An `unsafe` block.
    Unsafe(UnsafeExpr<'a>),
    /// A `return` with optional value.
    Return(ReturnExpr<'a>),
    /// A `break` with optional label and value.
    Break(BreakExpr<'a>),
    /// A `continue` with optional label.
    Continue(ContinueExpr<'a>),
    /// A block used as an expression.
    Block(Block<'a>),
}

impl<'a> Expr<'a> {
    /// View `node` as an expression, if it is one.
    #[must_use]
    pub fn cast(ast: Ast<'a>, node: &'a SyntaxNode) -> Option<Self> {
        Some(match node.kind {
            SyntaxKind::Literal => Self::Literal(LiteralExpr { ast, node }),
            SyntaxKind::PathExpr => Self::Path(PathExpr { ast, node }),
            SyntaxKind::StructLiteral => Self::StructLiteral(StructLiteralExpr { ast, node }),
            SyntaxKind::UnaryExpr => Self::Unary(UnaryExpr { ast, node }),
            SyntaxKind::BinaryExpr => Self::Binary(BinaryExpr { ast, node }),
            SyntaxKind::RangeExpr => Self::Range(RangeExpr { ast, node }),
            SyntaxKind::AssignExpr => Self::Assign(AssignExpr { ast, node }),
            SyntaxKind::CallExpr => Self::Call(CallExpr { ast, node }),
            SyntaxKind::MethodCallExpr => Self::MethodCall(MethodCallExpr { ast, node }),
            SyntaxKind::FieldExpr => Self::Field(FieldExpr { ast, node }),
            SyntaxKind::IndexExpr => Self::Index(IndexExpr { ast, node }),
            SyntaxKind::TryExpr => Self::Try(TryExpr { ast, node }),
            SyntaxKind::CastExpr => Self::Cast(CastExpr { ast, node }),
            SyntaxKind::GroupExpr => Self::Group(GroupExpr { ast, node }),
            SyntaxKind::IfExpr => Self::If(IfExpr { ast, node }),
            SyntaxKind::MatchExpr => Self::Match(MatchExpr { ast, node }),
            SyntaxKind::WhileExpr => Self::While(WhileExpr { ast, node }),
            SyntaxKind::LoopExpr => Self::Loop(LoopExpr { ast, node }),
            SyntaxKind::ForExpr => Self::For(ForExpr { ast, node }),
            SyntaxKind::UnsafeExpr => Self::Unsafe(UnsafeExpr { ast, node }),
            SyntaxKind::ReturnExpr => Self::Return(ReturnExpr { ast, node }),
            SyntaxKind::BreakExpr => Self::Break(BreakExpr { ast, node }),
            SyntaxKind::ContinueExpr => Self::Continue(ContinueExpr { ast, node }),
            SyntaxKind::Block => Self::Block(Block { ast, node }),
            _ => return None,
        })
    }

    /// The underlying CST node.
    #[must_use]
    pub fn syntax(self) -> &'a SyntaxNode {
        match self {
            Self::Literal(e) => e.node,
            Self::Path(e) => e.node,
            Self::StructLiteral(e) => e.node,
            Self::Unary(e) => e.node,
            Self::Binary(e) => e.node,
            Self::Range(e) => e.node,
            Self::Assign(e) => e.node,
            Self::Call(e) => e.node,
            Self::MethodCall(e) => e.node,
            Self::Field(e) => e.node,
            Self::Index(e) => e.node,
            Self::Try(e) => e.node,
            Self::Cast(e) => e.node,
            Self::Group(e) => e.node,
            Self::If(e) => e.node,
            Self::Match(e) => e.node,
            Self::While(e) => e.node,
            Self::Loop(e) => e.node,
            Self::For(e) => e.node,
            Self::Unsafe(e) => e.node,
            Self::Return(e) => e.node,
            Self::Break(e) => e.node,
            Self::Continue(e) => e.node,
            Self::Block(e) => e.node,
        }
    }
}

/// The `n`-th direct child node viewable as an expression.
fn nth_expr<'a>(ast: Ast<'a>, node: &'a SyntaxNode, n: usize) -> Option<Expr<'a>> {
    child_nodes(node)
        .filter(|child| child.kind.is_expression())
        .nth(n)
        .and_then(|child| Expr::cast(ast, child))
}

ast_view! {
    /// A literal token or the unit literal `()`.
    LiteralExpr from Literal
}

impl<'a> LiteralExpr<'a> {
    /// The kind of the literal token (`IntLiteral`, `StringLiteral`, …);
    /// `OpenParen` for the unit literal.
    #[must_use]
    pub fn token_kind(self) -> Option<TokenKind> {
        child_tokens(self.node)
            .next()
            .map(|index| self.ast.token_kind(index))
    }
}

ast_view! {
    /// A path expression, optionally with a turbofish.
    PathExpr from PathExpr
}

impl<'a> PathExpr<'a> {
    /// The path segments (`self`, `Self`, and identifiers), in source order.
    pub fn segments(self) -> impl Iterator<Item = &'a str> {
        self.segment_names().map(|name| name.text)
    }

    /// The path segments with their exact spans, in source order.
    pub fn segment_names(self) -> impl Iterator<Item = Name<'a>> {
        let ast = self.ast;
        child_tokens(self.node)
            .filter(move |&index| {
                matches!(
                    ast.token_kind(index),
                    TokenKind::Ident | TokenKind::KwSelfType | TokenKind::KwSelfValue
                )
            })
            .map(move |index| ast.token_name(index))
    }

    /// The turbofish type arguments (`::[T]`), if present.
    #[must_use]
    pub fn turbofish(self) -> Option<TypeArgs<'a>> {
        node_of_kind(self.node, SyntaxKind::Turbofish)
            .and_then(|fish| node_of_kind(fish, SyntaxKind::TypeArguments))
            .and_then(|args| TypeArgs::cast(self.ast, args))
    }
}

ast_view! {
    /// A struct literal.
    StructLiteralExpr from StructLiteral
}

impl<'a> StructLiteralExpr<'a> {
    /// The struct's type path.
    #[must_use]
    pub fn path(self) -> Option<TypePath<'a>> {
        node_of_kind(self.node, SyntaxKind::TypePath)
            .and_then(|node| TypePath::cast(self.ast, node))
    }

    /// The explicit type arguments, if any.
    #[must_use]
    pub fn type_args(self) -> Option<TypeArgs<'a>> {
        node_of_kind(self.node, SyntaxKind::TypeArguments)
            .and_then(|node| TypeArgs::cast(self.ast, node))
    }

    /// The field initializers, in source order.
    pub fn inits(self) -> impl Iterator<Item = FieldInit<'a>> {
        let ast = self.ast;
        child_nodes(self.node).filter_map(move |node| FieldInit::cast(ast, node))
    }
}

ast_view! {
    /// One field initializer: `name: expr` or shorthand `name`.
    FieldInit from FieldInit
}

impl<'a> FieldInit<'a> {
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

    /// The initializer value, if not the shorthand form.
    #[must_use]
    pub fn value(self) -> Option<Expr<'a>> {
        nth_expr(self.ast, self.node, 0)
    }
}

ast_view! {
    /// A prefix operator application.
    UnaryExpr from UnaryExpr
}

impl<'a> UnaryExpr<'a> {
    /// The operator as written (`-`, `!`, `move`).
    #[must_use]
    pub fn op(self) -> Option<&'a str> {
        child_tokens(self.node)
            .next()
            .map(|index| self.ast.token_text(index))
    }

    /// The operand.
    #[must_use]
    pub fn operand(self) -> Option<Expr<'a>> {
        nth_expr(self.ast, self.node, 0)
    }
}

/// Implement `lhs`/`op`/`rhs` accessors for the two-operand views.
macro_rules! infix_accessors {
    ($name:ident) => {
        impl<'a> $name<'a> {
            /// The left operand.
            #[must_use]
            pub fn lhs(self) -> Option<Expr<'a>> {
                nth_expr(self.ast, self.node, 0)
            }

            /// The operator as written.
            #[must_use]
            pub fn op(self) -> Option<&'a str> {
                child_tokens(self.node)
                    .next()
                    .map(|index| self.ast.token_text(index))
            }

            /// The right operand.
            #[must_use]
            pub fn rhs(self) -> Option<Expr<'a>> {
                nth_expr(self.ast, self.node, 1)
            }
        }
    };
}

ast_view! {
    /// An infix operator application.
    BinaryExpr from BinaryExpr
}
infix_accessors!(BinaryExpr);

ast_view! {
    /// A range: `a .. b`.
    RangeExpr from RangeExpr
}
infix_accessors!(RangeExpr);

ast_view! {
    /// An assignment: `target = value` (right-associative).
    AssignExpr from AssignExpr
}
infix_accessors!(AssignExpr);

ast_view! {
    /// A call: `callee(args)`.
    CallExpr from CallExpr
}

impl<'a> CallExpr<'a> {
    /// The callee.
    #[must_use]
    pub fn callee(self) -> Option<Expr<'a>> {
        nth_expr(self.ast, self.node, 0)
    }

    /// The arguments, in source order.
    pub fn args(self) -> impl Iterator<Item = Expr<'a>> {
        call_args(self.ast, self.node)
    }
}

ast_view! {
    /// A method call: `receiver.name(args)`.
    MethodCallExpr from MethodCallExpr
}

impl<'a> MethodCallExpr<'a> {
    /// The receiver.
    #[must_use]
    pub fn receiver(self) -> Option<Expr<'a>> {
        nth_expr(self.ast, self.node, 0)
    }

    /// The method name.
    #[must_use]
    pub fn name(self) -> Option<&'a str> {
        member_name(self.ast, self.node)
    }

    /// The arguments, in source order.
    pub fn args(self) -> impl Iterator<Item = Expr<'a>> {
        call_args(self.ast, self.node)
    }
}

ast_view! {
    /// A field access: `receiver.name` or `tuple.0`.
    FieldExpr from FieldExpr
}

impl<'a> FieldExpr<'a> {
    /// The receiver.
    #[must_use]
    pub fn receiver(self) -> Option<Expr<'a>> {
        nth_expr(self.ast, self.node, 0)
    }

    /// The field name (an identifier or a tuple index).
    #[must_use]
    pub fn name(self) -> Option<&'a str> {
        member_name(self.ast, self.node)
    }
}

/// The member name after the `.` (an identifier or an integer index).
fn member_name<'a>(ast: Ast<'a>, node: &'a SyntaxNode) -> Option<&'a str> {
    child_tokens(node)
        .filter(|&index| {
            matches!(
                ast.token_kind(index),
                TokenKind::Ident | TokenKind::IntLiteral
            )
        })
        .map(|index| ast.token_text(index))
        .last()
}

/// The argument expressions inside a node's `CallArgs` child.
fn call_args<'a>(ast: Ast<'a>, node: &'a SyntaxNode) -> impl Iterator<Item = Expr<'a>> {
    node_of_kind(node, SyntaxKind::CallArgs)
        .into_iter()
        .flat_map(move |args| child_nodes(args).filter_map(move |arg| Expr::cast(ast, arg)))
}

ast_view! {
    /// An index: `base[index]`.
    IndexExpr from IndexExpr
}

impl<'a> IndexExpr<'a> {
    /// The indexed expression.
    #[must_use]
    pub fn base(self) -> Option<Expr<'a>> {
        nth_expr(self.ast, self.node, 0)
    }

    /// The index expression.
    #[must_use]
    pub fn index(self) -> Option<Expr<'a>> {
        nth_expr(self.ast, self.node, 1)
    }
}

ast_view! {
    /// Error propagation: `inner?`.
    TryExpr from TryExpr
}

impl<'a> TryExpr<'a> {
    /// The propagated expression.
    #[must_use]
    pub fn inner(self) -> Option<Expr<'a>> {
        nth_expr(self.ast, self.node, 0)
    }
}

ast_view! {
    /// A cast: `inner as Type`.
    CastExpr from CastExpr
}

impl<'a> CastExpr<'a> {
    /// The cast expression.
    #[must_use]
    pub fn inner(self) -> Option<Expr<'a>> {
        nth_expr(self.ast, self.node, 0)
    }

    /// The target type.
    #[must_use]
    pub fn ty(self) -> Option<TypeRef<'a>> {
        let ast = self.ast;
        child_nodes(self.node).find_map(move |node| TypeRef::cast(ast, node))
    }
}

ast_view! {
    /// A parenthesized expression.
    GroupExpr from GroupExpr
}

impl<'a> GroupExpr<'a> {
    /// The inner expression.
    #[must_use]
    pub fn inner(self) -> Option<Expr<'a>> {
        nth_expr(self.ast, self.node, 0)
    }
}

ast_view! {
    /// An `if` expression with optional `else` chain.
    IfExpr from IfExpr
}

/// The alternative of an `if` expression.
#[derive(Clone, Copy, Debug)]
pub enum ElseBranch<'a> {
    /// `else if …`.
    If(IfExpr<'a>),
    /// `else { … }`.
    Block(Block<'a>),
}

impl<'a> IfExpr<'a> {
    /// The condition (child position 0).
    #[must_use]
    pub fn condition(self) -> Option<Expr<'a>> {
        nth_node(self.node, 0).and_then(|node| Expr::cast(self.ast, node))
    }

    /// The `then` block (child position 1).
    #[must_use]
    pub fn then_block(self) -> Option<Block<'a>> {
        nth_node(self.node, 1).and_then(|node| Block::cast(self.ast, node))
    }

    /// The `else` branch, if any (child position 2).
    #[must_use]
    pub fn else_branch(self) -> Option<ElseBranch<'a>> {
        let node = nth_node(self.node, 2)?;
        if let Some(nested) = Self::cast(self.ast, node) {
            return Some(ElseBranch::If(nested));
        }
        Block::cast(self.ast, node).map(ElseBranch::Block)
    }
}

ast_view! {
    /// A `match` expression.
    MatchExpr from MatchExpr
}

impl<'a> MatchExpr<'a> {
    /// The scrutinee.
    #[must_use]
    pub fn scrutinee(self) -> Option<Expr<'a>> {
        nth_node(self.node, 0).and_then(|node| Expr::cast(self.ast, node))
    }

    /// The arms, in source order.
    pub fn arms(self) -> impl Iterator<Item = MatchArm<'a>> {
        let ast = self.ast;
        child_nodes(self.node).filter_map(move |node| MatchArm::cast(ast, node))
    }
}

ast_view! {
    /// One `pattern (if guard)? => value` arm of a `match`.
    MatchArm from MatchArm
}

impl<'a> MatchArm<'a> {
    /// The arm's pattern.
    #[must_use]
    pub fn pattern(self) -> Option<Pattern<'a>> {
        let ast = self.ast;
        child_nodes(self.node).find_map(move |node| Pattern::cast(ast, node))
    }

    /// The `if` guard, if present.
    #[must_use]
    pub fn guard(self) -> Option<Expr<'a>> {
        self.ast
            .has_token(self.node, TokenKind::KwIf)
            .then(|| nth_expr(self.ast, self.node, 0))
            .flatten()
    }

    /// The arm's value expression.
    #[must_use]
    pub fn value(self) -> Option<Expr<'a>> {
        let n = usize::from(self.ast.has_token(self.node, TokenKind::KwIf));
        nth_expr(self.ast, self.node, n)
    }
}

ast_view! {
    /// A `while` loop.
    WhileExpr from WhileExpr
}

impl<'a> WhileExpr<'a> {
    /// The loop condition (child position 0).
    #[must_use]
    pub fn condition(self) -> Option<Expr<'a>> {
        nth_node(self.node, 0).and_then(|node| Expr::cast(self.ast, node))
    }

    /// The loop body (child position 1).
    #[must_use]
    pub fn body(self) -> Option<Block<'a>> {
        nth_node(self.node, 1).and_then(|node| Block::cast(self.ast, node))
    }
}

ast_view! {
    /// A (possibly labeled) `loop`.
    LoopExpr from LoopExpr
}

impl<'a> LoopExpr<'a> {
    /// The loop label, if any (including the `'`).
    #[must_use]
    pub fn label(self) -> Option<&'a str> {
        self.ast
            .direct_token_text(self.node, TokenKind::LifetimeLabel)
    }

    /// The loop body.
    #[must_use]
    pub fn body(self) -> Option<Block<'a>> {
        node_of_kind(self.node, SyntaxKind::Block).and_then(|node| Block::cast(self.ast, node))
    }
}

ast_view! {
    /// A `for pattern in iterable { … }` loop.
    ForExpr from ForExpr
}

impl<'a> ForExpr<'a> {
    /// The binding pattern (child position 0).
    #[must_use]
    pub fn pattern(self) -> Option<Pattern<'a>> {
        nth_node(self.node, 0).and_then(|node| Pattern::cast(self.ast, node))
    }

    /// The iterated expression (child position 1).
    #[must_use]
    pub fn iterable(self) -> Option<Expr<'a>> {
        nth_node(self.node, 1).and_then(|node| Expr::cast(self.ast, node))
    }

    /// The loop body (child position 2).
    #[must_use]
    pub fn body(self) -> Option<Block<'a>> {
        nth_node(self.node, 2).and_then(|node| Block::cast(self.ast, node))
    }
}

ast_view! {
    /// An `unsafe { … }` block expression.
    UnsafeExpr from UnsafeExpr
}

impl<'a> UnsafeExpr<'a> {
    /// The unsafe block.
    #[must_use]
    pub fn body(self) -> Option<Block<'a>> {
        node_of_kind(self.node, SyntaxKind::Block).and_then(|node| Block::cast(self.ast, node))
    }
}

ast_view! {
    /// A `return` with optional value.
    ReturnExpr from ReturnExpr
}

impl<'a> ReturnExpr<'a> {
    /// The returned value, if any.
    #[must_use]
    pub fn value(self) -> Option<Expr<'a>> {
        nth_expr(self.ast, self.node, 0)
    }
}

ast_view! {
    /// A `break` with optional label and value.
    BreakExpr from BreakExpr
}

impl<'a> BreakExpr<'a> {
    /// The target label, if any (including the `'`).
    #[must_use]
    pub fn label(self) -> Option<&'a str> {
        self.ast
            .direct_token_text(self.node, TokenKind::LifetimeLabel)
    }

    /// The break value, if any.
    #[must_use]
    pub fn value(self) -> Option<Expr<'a>> {
        nth_expr(self.ast, self.node, 0)
    }
}

ast_view! {
    /// A `continue` with optional label.
    ContinueExpr from ContinueExpr
}

impl<'a> ContinueExpr<'a> {
    /// The target label, if any (including the `'`).
    #[must_use]
    pub fn label(self) -> Option<&'a str> {
        self.ast
            .direct_token_text(self.node, TokenKind::LifetimeLabel)
    }
}
