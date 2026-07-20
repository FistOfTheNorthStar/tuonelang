//! High-level intermediate representation (HIR) for tuonelang.
//!
//! HIR is the first **semantic** tree: an owned, desugared representation
//! [`lower`]ed from the typed AST views ([`tuo_ast`]) plus the name
//! resolution ([`tuo_resolve::Resolution`]) of one program snapshot. It is
//! the bridge between syntax and analysis:
//!
//! - **Semantic IDs.** Every resolved name is a [`Res`] carrying the
//!   stable [`tuo_resolve::SymbolId`] of its target — never a raw string
//!   and never an AST pointer. Names whose meaning depends on types (field
//!   accesses, method calls) stay as written; the type checker owns them.
//! - **Desugared.** Irrelevant syntax variation is gone: parentheses,
//!   shorthand fields, omitted return types and parameter modes, `where`
//!   versus inline bounds, numeric separators, imports, aliases, and
//!   module qualification all lower to one canonical form (the full list
//!   is on [`lower`]). Canonically equivalent source produces equal HIR.
//! - **Source mappings retained.** Every node carries the exact
//!   [`tuo_source::Span`] it was lowered from.
//! - **Parser-independent.** The HIR types own their data and reference
//!   nothing from the parser or CST — consumers never see a syntax node.
//!
//! No machine-level operations live here: HIR still speaks in expressions,
//! patterns, and blocks. Lowering to executable form is MIR's job
//! (`tuo-mir`).
//!
//! [`render`] is the development pretty-printer behind `tuo debug hir` — a
//! diagnostic aid whose output deliberately omits spans (so equivalent
//! programs render identically), not a stable protocol.

mod hir;
mod lower;
mod print;

pub use hir::{
    Arm, BinOp, BindingDef, Block, ConstDef, EnumDef, Expr, ExprKind, Field, FieldInit, FieldPat,
    Function, GivenBinding, Hir, ImplDef, InterfaceDef, Item, Lit, Param, ParamMode, Pat, PatKind,
    Res, SpecDef, SpecStmt, Stmt, StmtKind, StructDef, Ty, TyKind, TypeParam, UnOp, Variant,
    Wrapper,
};
pub use lower::lower;
pub use print::render;
