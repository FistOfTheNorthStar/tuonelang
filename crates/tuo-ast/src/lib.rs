//! Typed AST views for tuonelang.
//!
//! This crate is the **typed accessor layer** over the lossless concrete
//! syntax tree ([`tuo_syntax`]): lightweight, `Copy`, borrowed views such as
//! [`FnDecl`], [`StructDecl`], [`EnumDecl`], [`SpecDecl`], [`Expr`],
//! [`Statement`], and [`TypeRef`] that interpret CST structure without
//! copying it. The three representation layers stay separate:
//!
//! - **Concrete syntax** ([`tuo_syntax`]): every token, comment, and source
//!   range, including [`tuo_syntax::SyntaxKind::Error`] nodes where the
//!   parser recovered. Owned by the parser; consumed by formatting/IDE.
//! - **Typed AST (this crate)**: named accessors over that tree. Views are
//!   *lenient* — every accessor returns `Option`s/iterators, so they work on
//!   malformed trees and never panic. **No semantic information lives
//!   here**: no checked types, no resolved names, no ownership facts.
//! - **Semantic IR** ([`tuo_hir`](../tuo_hir) and below): the desugared,
//!   analyzed representation. Type checking attaches its results there,
//!   never to these views.
//!
//! # Using the views
//!
//! Build an [`Ast`] from a parsed tree plus the exact source text it came
//! from, then start at [`Ast::file`]. Every view exposes `syntax()` (the
//! underlying CST node), `span()` (its exact source range), and `text()`
//! (the exact source slice) alongside its typed accessors, so tooling can
//! always drop back down to the concrete layer.
//!
//! [`render`] produces the indented typed-AST dump behind `tuo debug ast` —
//! a developer diagnostic, not a stable protocol.

mod context;
mod expr;
mod item;
mod pat;
mod render;
mod stmt;
mod ty;

pub use context::{Ast, Name};
pub use expr::{
    ArrayLiteralExpr, ArrayLiteralKind, AssignExpr, BinaryExpr, BreakExpr, CallExpr, CastExpr,
    ContinueExpr, ElseBranch, Expr, FieldExpr, FieldInit, ForExpr, GroupExpr, IfExpr, IndexExpr,
    LiteralExpr, LoopExpr, MatchArm, MatchExpr, MethodCallExpr, PathExpr, RangeExpr, ReturnExpr,
    StructLiteralExpr, TryExpr, UnaryExpr, UnsafeExpr, WhileExpr,
};
pub use item::{
    ConstDecl, EnumDecl, ErrorNode, FieldDecl, FnDecl, GenericParam, GenericParams, GivenBinding,
    GivenClause, ImplDecl, ImportDecl, ImportLeaf, InterfaceDecl, Item, ModuleDecl, ParamDecl,
    PathNode, SimpleClause, SourceFile, SpecDecl, SpecStatement, StructDecl, VariantDecl,
    WhenClause, WhereClause, WherePred,
};
pub use pat::{BindingPat, FieldPat, GroupPat, LiteralPat, OrPat, PathPat, Pattern, WildcardPat};
pub use render::render;
pub use stmt::{BindingStmt, Block, EmptyStmt, ExprStmt, Statement};
pub use ty::{FixedArrayType, PathType, TypeArgs, TypePath, TypeRef, UnitType, WrapperType};
