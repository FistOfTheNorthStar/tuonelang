//! Lowering: typed AST views + name resolution → owned HIR.
//!
//! Lowering is **total** — it never fails and never diagnoses. Malformed or
//! unresolved input becomes poison nodes (`Res::Err`, `ExprKind::Err`, …)
//! that were already diagnosed by the parser or resolver.
//!
//! Irrelevant syntax variation is eliminated here, so canonically
//! equivalent spellings produce equal HIR:
//!
//! - parenthesized groups (expressions and patterns) are unwrapped;
//! - a final semicolon-less block-form expression statement becomes the
//!   block's tail; lone `;` statements disappear;
//! - shorthand struct-literal fields and field patterns are expanded to
//!   their explicit `name: …` form;
//! - an omitted `-> ()` return type is made explicit;
//! - an omitted parameter mode becomes the explicit default `in`;
//! - `where`-clause bounds on a plain type parameter are merged into that
//!   parameter's inline bound list;
//! - numeric literals lose their `_` separators;
//! - `import` items and `module` declarations are resolved away entirely,
//!   so a name used via a module path, an import, or an import alias
//!   lowers to the same resolved symbol.

use std::collections::HashMap;

use tuo_ast::{self as ast, Ast, Name};
use tuo_lexer::TokenKind;
use tuo_resolve::{Resolution, SymbolId, is_builtin_type};
use tuo_source::Span;

use crate::hir::{
    Arm, BinOp, BindingDef, Block, ConstDef, EnumDef, Expr, ExprKind, Field, FieldInit, FieldPat,
    Function, GivenBinding, Hir, ImplDef, InterfaceDef, Item, Lit, Param, ParamMode, Pat, PatKind,
    Res, SpecDef, SpecStmt, Stmt, StmtKind, StructDef, Ty, TyKind, TypeParam, UnOp, Variant,
    Wrapper,
};

/// Lower one resolved program snapshot into HIR.
///
/// `files` must be the same slice `resolution` was produced from — the
/// lowering is keyed by the exact name spans resolution recorded.
#[must_use]
pub fn lower(files: &[Ast<'_>], resolution: &Resolution) -> Hir {
    let mut refs = HashMap::new();
    for reference in resolution.references() {
        refs.insert(reference.span, reference.symbol);
    }
    let mut decls = HashMap::new();
    for (id, symbol) in resolution.symbols() {
        if let Some(span) = symbol.declaration {
            decls.insert(span, id);
        }
    }
    let mut spec_targets = HashMap::new();
    for attachment in resolution.spec_targets() {
        spec_targets.insert(attachment.spec, attachment.target);
    }
    let spec_symbols: HashMap<Span, SymbolId> = resolution.spec_symbols().collect();
    let cx = Cx {
        refs,
        decls,
        spec_targets,
        spec_symbols,
    };
    let mut items = Vec::new();
    for ast in files {
        let file = ast.file();
        let fallback = file.span().unwrap_or_else(dummy_span);
        for item in file.items() {
            if let Some(lowered) = cx.item(item, fallback) {
                items.push(lowered);
            }
        }
    }
    Hir { items }
}

/// A span for constructs whose own tokens are missing (malformed trees).
fn dummy_span() -> Span {
    Span::new(
        tuo_source::SourceId::from_raw(0),
        tuo_source::TextRange::new(0u32, 0u32).expect("empty range is valid"),
    )
}

/// The lowering context: resolution's facts, keyed for span lookup.
struct Cx {
    /// Every resolved name use, by its exact span.
    refs: HashMap<Span, SymbolId>,
    /// Every symbol, by its declaring name token's span.
    decls: HashMap<Span, SymbolId>,
    /// Spec symbol → targeted function symbol.
    spec_targets: HashMap<SymbolId, SymbolId>,
    /// Spec block span → the spec's own symbol.
    spec_symbols: HashMap<Span, SymbolId>,
}

impl Cx {
    // ------------------------------------------------------------------
    // Name lookup
    // ------------------------------------------------------------------

    /// The symbol declared by `name`, as a [`Res`].
    fn declared(&self, name: Option<Name<'_>>) -> Res {
        name.and_then(|name| self.decls.get(&name.span))
            .map_or(Res::Err, |&id| Res::Symbol(id))
    }

    /// The resolved target of the *use* at `name`, honoring builtins and
    /// `Self`.
    fn used(&self, name: Name<'_>) -> Res {
        if let Some(&id) = self.refs.get(&name.span) {
            return Res::Symbol(id);
        }
        if name.text == "Self" {
            return Res::SelfType;
        }
        if is_builtin_type(name.text) {
            return Res::Builtin(name.text.to_owned());
        }
        Res::Err
    }

    /// The resolved target of a multi-segment path: resolution records the
    /// reference on the *last* segment it resolved. Builtins and `Self`
    /// only count for single-segment paths.
    fn path_target(&self, segments: &[Name<'_>]) -> Res {
        let Some(last) = segments.last() else {
            return Res::Err;
        };
        if let Some(&id) = self.refs.get(&last.span) {
            return Res::Symbol(id);
        }
        if segments.len() == 1 {
            return self.used(*last);
        }
        Res::Err
    }

    // ------------------------------------------------------------------
    // Items
    // ------------------------------------------------------------------

    /// Lower one top-level item; imports, module declarations, and error
    /// islands are resolved away (`None`).
    fn item(&self, item: ast::Item<'_>, fallback: Span) -> Option<Item> {
        match item {
            ast::Item::Import(_) | ast::Item::Error(_) => None,
            ast::Item::Fn(decl) => Some(Item::Fn(self.function(decl, fallback))),
            ast::Item::Struct(decl) => Some(Item::Struct(self.struct_def(decl, fallback))),
            ast::Item::Enum(decl) => Some(Item::Enum(self.enum_def(decl, fallback))),
            ast::Item::Interface(decl) => Some(Item::Interface(self.interface_def(decl, fallback))),
            ast::Item::Impl(decl) => Some(Item::Impl(self.impl_def(decl, fallback))),
            ast::Item::Const(decl) => Some(Item::Const(self.const_def(decl, fallback))),
            ast::Item::Spec(decl) => Some(Item::Spec(self.spec_def(decl, fallback))),
        }
    }

    fn function(&self, decl: ast::FnDecl<'_>, fallback: Span) -> Function {
        let span = decl.span().unwrap_or(fallback);
        let mut type_params = self.type_params(decl.generics(), span);
        self.merge_where(&mut type_params, decl.where_clause());
        let ret = decl.return_type().map_or(
            Ty {
                kind: TyKind::Unit,
                span,
            },
            |ty| self.ty(ty, span),
        );
        Function {
            symbol: self.declared(decl.name_ref()),
            span,
            type_params,
            params: decl.params().map(|param| self.param(param, span)).collect(),
            ret,
            body: decl.body().map(|body| self.block(body, span)),
        }
    }

    fn param(&self, decl: ast::ParamDecl<'_>, fallback: Span) -> Param {
        let span = decl.span().unwrap_or(fallback);
        let mode = match decl.mode() {
            Some("mut") => ParamMode::Mut,
            Some("take") => ParamMode::Take,
            // Any other first token is the name itself: the mode was
            // omitted, and the default is `in`.
            _ => ParamMode::In,
        };
        Param {
            symbol: self.declared(decl.name_ref()),
            span,
            mode,
            is_receiver: decl.is_receiver(),
            ty: decl.ty().map(|ty| self.ty(ty, span)),
        }
    }

    fn type_params(
        &self,
        generics: Option<ast::GenericParams<'_>>,
        fallback: Span,
    ) -> Vec<TypeParam> {
        generics
            .into_iter()
            .flat_map(|list| list.params())
            .map(|param| {
                let span = param.span().unwrap_or(fallback);
                TypeParam {
                    symbol: self.declared(param.name_ref()),
                    span,
                    bounds: param.bounds().map(|bound| self.type_path(bound)).collect(),
                }
            })
            .collect()
    }

    /// Merge `where T: Bound` predicates into the bound list of the type
    /// parameter they constrain — the inline and `where` spellings are the
    /// same declaration. Predicates over anything but a plain type
    /// parameter have no v0 semantics and are dropped here.
    fn merge_where(&self, type_params: &mut [TypeParam], clause: Option<ast::WhereClause<'_>>) {
        for pred in clause.into_iter().flat_map(|clause| clause.preds()) {
            let Some(ast::TypeRef::Path(path)) = pred.ty() else {
                continue;
            };
            if path.args().is_some() {
                continue;
            }
            let Some(type_path) = path.path() else {
                continue;
            };
            let segments: Vec<Name<'_>> = type_path.segment_names().collect();
            let target = self.path_target(&segments);
            let Some(param) = type_params
                .iter_mut()
                .find(|param| param.symbol == target && target != Res::Err)
            else {
                continue;
            };
            param
                .bounds
                .extend(pred.bounds().map(|bound| self.type_path(bound)));
        }
    }

    fn struct_def(&self, decl: ast::StructDecl<'_>, fallback: Span) -> StructDef {
        let span = decl.span().unwrap_or(fallback);
        let mut type_params = self.type_params(decl.generics(), span);
        self.merge_where(&mut type_params, decl.where_clause());
        StructDef {
            symbol: self.declared(decl.name_ref()),
            span,
            type_params,
            fields: decl.fields().map(|field| self.field(field, span)).collect(),
        }
    }

    fn field(&self, decl: ast::FieldDecl<'_>, fallback: Span) -> Field {
        let name = decl.name_ref();
        let span = name.map_or_else(|| decl.span().unwrap_or(fallback), |name| name.span);
        Field {
            name: name.map(|name| name.text.to_owned()).unwrap_or_default(),
            span,
            ty: decl.ty().map(|ty| self.ty(ty, span)),
        }
    }

    fn enum_def(&self, decl: ast::EnumDecl<'_>, fallback: Span) -> EnumDef {
        let span = decl.span().unwrap_or(fallback);
        EnumDef {
            symbol: self.declared(decl.name_ref()),
            span,
            type_params: self.type_params(decl.generics(), span),
            variants: decl
                .variants()
                .map(|variant| {
                    let variant_span = variant.span().unwrap_or(span);
                    Variant {
                        symbol: self.declared(variant.name_ref()),
                        span: variant_span,
                        fields: variant
                            .fields()
                            .map(|field| self.field(field, variant_span))
                            .collect(),
                    }
                })
                .collect(),
        }
    }

    fn interface_def(&self, decl: ast::InterfaceDecl<'_>, fallback: Span) -> InterfaceDef {
        let span = decl.span().unwrap_or(fallback);
        InterfaceDef {
            symbol: self.declared(decl.name_ref()),
            span,
            type_params: self.type_params(decl.generics(), span),
            members: decl
                .members()
                .map(|member| self.function(member, span))
                .collect(),
        }
    }

    fn impl_def(&self, decl: ast::ImplDecl<'_>, fallback: Span) -> ImplDef {
        let span = decl.span().unwrap_or(fallback);
        let mut type_params = self.type_params(decl.generics(), span);
        self.merge_where(&mut type_params, decl.where_clause());
        ImplDef {
            span,
            type_params,
            interface: decl
                .interface()
                .map_or(Res::Err, |path| self.type_path(path)),
            interface_args: decl
                .interface_args()
                .into_iter()
                .flat_map(|args| args.types())
                .map(|ty| self.ty(ty, span))
                .collect(),
            target: decl.target().map(|ty| self.ty(ty, span)),
            functions: decl
                .functions()
                .map(|function| self.function(function, span))
                .collect(),
        }
    }

    fn const_def(&self, decl: ast::ConstDecl<'_>, fallback: Span) -> ConstDef {
        let span = decl.span().unwrap_or(fallback);
        ConstDef {
            symbol: self.declared(decl.name_ref()),
            span,
            ty: decl.ty().map(|ty| self.ty(ty, span)),
            value: decl.value().map(|value| self.expr(value, span)),
        }
    }

    fn spec_def(&self, decl: ast::SpecDecl<'_>, fallback: Span) -> SpecDef {
        let span = decl.span().unwrap_or(fallback);
        // The spec's own identity comes from its block span; its target
        // (identifier-named specs only) from resolving the target name.
        let spec_symbol = decl
            .span()
            .and_then(|span| self.spec_symbols.get(&span).copied());
        let target = match self.declared(decl.target_name()) {
            Res::Symbol(id) => self.spec_targets.get(&id).copied(),
            _ => None,
        };
        SpecDef {
            spec_symbol,
            span,
            // A string-named spec's name token includes its quotes; the
            // HIR name is the contents either way.
            name: strip_quotes(decl.name().unwrap_or_default(), '"'),
            target,
            statements: decl
                .statements()
                .filter_map(|statement| self.spec_stmt(statement, span))
                .collect(),
        }
    }

    fn spec_stmt(&self, statement: ast::SpecStatement<'_>, fallback: Span) -> Option<SpecStmt> {
        Some(match statement {
            ast::SpecStatement::Given(clause) => SpecStmt::Given(
                clause
                    .bindings()
                    .map(|binding| {
                        let span = binding.span().unwrap_or(fallback);
                        GivenBinding {
                            symbol: self.declared(binding.name_ref()),
                            span,
                            ty: binding.ty().map(|ty| self.ty(ty, span)),
                            init: binding.initializer().map(|init| self.expr(init, span)),
                        }
                    })
                    .collect(),
            ),
            ast::SpecStatement::When(clause) => {
                let span = clause.span().unwrap_or(fallback);
                let stmt = if let Some(binding) = clause.binding() {
                    self.binding_stmt(binding, span)
                } else if let Some(expr) = clause.expr() {
                    Stmt {
                        kind: StmtKind::Expr(self.expr(expr, span)),
                        span,
                    }
                } else {
                    Stmt {
                        kind: StmtKind::Err,
                        span,
                    }
                };
                SpecStmt::When(stmt)
            }
            ast::SpecStatement::Then(clause) => {
                let span = clause.span().unwrap_or(fallback);
                SpecStmt::Then(
                    clause
                        .expr()
                        .map_or(err_expr(span), |expr| self.expr(expr, span)),
                )
            }
            ast::SpecStatement::Assert(clause) => {
                let span = clause.span().unwrap_or(fallback);
                SpecStmt::Assert(
                    clause
                        .expr()
                        .map_or(err_expr(span), |expr| self.expr(expr, span)),
                )
            }
            ast::SpecStatement::Let(binding) | ast::SpecStatement::Var(binding) => {
                let span = binding.span().unwrap_or(fallback);
                SpecStmt::Stmt(self.binding_stmt(binding, span))
            }
            ast::SpecStatement::Expr(statement) => {
                let span = statement.span().unwrap_or(fallback);
                SpecStmt::Stmt(Stmt {
                    kind: statement
                        .expr()
                        .map_or(StmtKind::Err, |expr| StmtKind::Expr(self.expr(expr, span))),
                    span,
                })
            }
            ast::SpecStatement::Error(_) => return None,
        })
    }

    // ------------------------------------------------------------------
    // Types
    // ------------------------------------------------------------------

    fn ty(&self, ty: ast::TypeRef<'_>, fallback: Span) -> Ty {
        let span = ty.span().unwrap_or(fallback);
        match ty {
            ast::TypeRef::Unit(_) => Ty {
                kind: TyKind::Unit,
                span,
            },
            ast::TypeRef::Wrapper(wrapper) => {
                let kind = match wrapper.wrapper() {
                    Some("Box") => Some(Wrapper::Box),
                    Some("Shared") => Some(Wrapper::Shared),
                    Some("Weak") => Some(Wrapper::Weak),
                    _ => None,
                };
                Ty {
                    kind: kind.map_or(TyKind::Err, |kind| TyKind::Wrapper {
                        wrapper: kind,
                        args: wrapper
                            .args()
                            .into_iter()
                            .flat_map(|args| args.types())
                            .map(|arg| self.ty(arg, span))
                            .collect(),
                    }),
                    span,
                }
            }
            ast::TypeRef::Path(path) => {
                let res = path
                    .path()
                    .map_or(Res::Err, |type_path| self.type_path(type_path));
                let kind = if res == Res::SelfType {
                    TyKind::SelfType
                } else {
                    TyKind::Path {
                        res,
                        args: path
                            .args()
                            .into_iter()
                            .flat_map(|args| args.types())
                            .map(|arg| self.ty(arg, span))
                            .collect(),
                    }
                };
                Ty { kind, span }
            }
        }
    }

    /// Resolve a type path to its target.
    fn type_path(&self, path: ast::TypePath<'_>) -> Res {
        let segments: Vec<Name<'_>> = path.segment_names().collect();
        self.path_target(&segments)
    }

    // ------------------------------------------------------------------
    // Blocks and statements
    // ------------------------------------------------------------------

    fn block(&self, block: ast::Block<'_>, fallback: Span) -> Block {
        let span = block.span().unwrap_or(fallback);
        let statements: Vec<ast::Statement<'_>> = block.statements().collect();
        let mut tail = block.tail().map(|expr| Box::new(self.expr(expr, span)));
        let mut end = statements.len();
        // A final semicolon-less block-form expression statement is this
        // block's tail value — the grammar just parses it as a statement.
        if tail.is_none()
            && let Some(ast::Statement::Expr(statement)) = statements.last()
            && !statement.has_semicolon()
            && let Some(expr) = statement.expr()
        {
            tail = Some(Box::new(self.expr(expr, span)));
            end -= 1;
        }
        let stmts = statements[..end]
            .iter()
            .filter_map(|statement| self.stmt(*statement, span))
            .collect();
        Block { stmts, tail, span }
    }

    /// Lower one statement; lone `;` statements disappear.
    fn stmt(&self, statement: ast::Statement<'_>, fallback: Span) -> Option<Stmt> {
        Some(match statement {
            ast::Statement::Let(binding) | ast::Statement::Var(binding) => {
                let span = binding.span().unwrap_or(fallback);
                self.binding_stmt(binding, span)
            }
            ast::Statement::Const(decl) => {
                let span = decl.span().unwrap_or(fallback);
                Stmt {
                    kind: StmtKind::Const(self.const_def(decl, span)),
                    span,
                }
            }
            ast::Statement::Expr(statement) => {
                let span = statement.span().unwrap_or(fallback);
                Stmt {
                    kind: statement
                        .expr()
                        .map_or(StmtKind::Err, |expr| StmtKind::Expr(self.expr(expr, span))),
                    span,
                }
            }
            ast::Statement::Empty(_) => return None,
            ast::Statement::Error(node) => Stmt {
                kind: StmtKind::Err,
                span: node.span().unwrap_or(fallback),
            },
        })
    }

    fn binding_stmt(&self, binding: ast::BindingStmt<'_>, span: Span) -> Stmt {
        Stmt {
            kind: StmtKind::Binding(BindingDef {
                mutable: binding.is_var(),
                pat: binding
                    .pattern()
                    .map_or(err_pat(span), |pattern| self.pat(pattern, span)),
                ty: binding.ty().map(|ty| self.ty(ty, span)),
                init: binding.initializer().map(|init| self.expr(init, span)),
            }),
            span,
        }
    }

    // ------------------------------------------------------------------
    // Patterns
    // ------------------------------------------------------------------

    fn pat(&self, pattern: ast::Pattern<'_>, fallback: Span) -> Pat {
        let span = pattern.span().unwrap_or(fallback);
        match pattern {
            ast::Pattern::Wildcard(_) => Pat {
                kind: PatKind::Wildcard,
                span,
            },
            ast::Pattern::Literal(literal) => Pat {
                kind: literal.token_kind().map_or(PatKind::Err, |kind| {
                    match lit(kind, literal.text()) {
                        Some(value) => PatKind::Lit(value),
                        None => PatKind::Err,
                    }
                }),
                span,
            },
            ast::Pattern::Binding(binding) => {
                // Resolution decides whether a bare identifier binds a fresh
                // name or refers to a unit enum variant (`None`, `Empty`, …).
                // When it recorded a *reference* (not a declaration) here, the
                // name is that variant — lower it as a payload-less constructor
                // pattern so matching tests the discriminant, exactly like a
                // qualified `Shape::Empty`.
                let name = binding.name_ref();
                let referenced = name.and_then(|name| self.refs.get(&name.span).copied());
                let kind = if let Some(variant) = referenced {
                    PatKind::Ctor {
                        ctor: Res::Symbol(variant),
                        fields: Vec::new(),
                        rest: false,
                    }
                } else {
                    PatKind::Binding(self.declared(name))
                };
                Pat { kind, span }
            }
            ast::Pattern::Path(path) => {
                let segments: Vec<Name<'_>> = path.segment_names().collect();
                Pat {
                    kind: PatKind::Ctor {
                        ctor: self.path_target(&segments),
                        fields: path
                            .fields()
                            .filter_map(|field| self.field_pat(field))
                            .collect(),
                        rest: path.has_rest(),
                    },
                    span,
                }
            }
            ast::Pattern::Or(or) => Pat {
                kind: PatKind::Or(
                    or.alternatives()
                        .map(|alternative| self.pat(alternative, span))
                        .collect(),
                ),
                span,
            },
            // Parentheses are irrelevant syntax: unwrap them.
            ast::Pattern::Group(group) => group
                .inner()
                .map_or(err_pat(span), |inner| self.pat(inner, span)),
        }
    }

    /// Lower one field of a constructor pattern, expanding shorthand
    /// `name` to explicit `name: binding`.
    fn field_pat(&self, field: ast::FieldPat<'_>) -> Option<FieldPat> {
        let name = field.name_ref()?;
        let pat = match field.pattern() {
            Some(pattern) => self.pat(pattern, name.span),
            None => Pat {
                kind: PatKind::Binding(self.declared(Some(name))),
                span: name.span,
            },
        };
        Some(FieldPat {
            name: name.text.to_owned(),
            span: name.span,
            pat,
        })
    }

    // ------------------------------------------------------------------
    // Expressions
    // ------------------------------------------------------------------

    #[expect(
        clippy::too_many_lines,
        reason = "one arm per expression form; splitting would obscure the dispatch"
    )]
    fn expr(&self, expr: ast::Expr<'_>, fallback: Span) -> Expr {
        let span = expr.span().unwrap_or(fallback);
        let kind = match expr {
            ast::Expr::Literal(literal) => literal
                .token_kind()
                .and_then(|kind| lit(kind, literal.text()))
                .map_or(ExprKind::Err, ExprKind::Lit),
            ast::Expr::Path(path) => {
                let segments: Vec<Name<'_>> = path.segment_names().collect();
                ExprKind::Path {
                    res: self.path_target(&segments),
                    args: path
                        .turbofish()
                        .into_iter()
                        .flat_map(|args| args.types())
                        .map(|ty| self.ty(ty, span))
                        .collect(),
                }
            }
            ast::Expr::StructLiteral(literal) => ExprKind::StructLit {
                ctor: literal.path().map_or(Res::Err, |path| self.type_path(path)),
                args: literal
                    .type_args()
                    .into_iter()
                    .flat_map(|args| args.types())
                    .map(|ty| self.ty(ty, span))
                    .collect(),
                fields: literal
                    .inits()
                    .filter_map(|init| self.field_init(init))
                    .collect(),
            },
            ast::Expr::Unary(unary) => {
                let op = match unary.op() {
                    Some("-") => Some(UnOp::Neg),
                    Some("!") => Some(UnOp::Not),
                    Some("move") => Some(UnOp::Move),
                    _ => None,
                };
                match (op, unary.operand()) {
                    (Some(op), Some(operand)) => ExprKind::Unary {
                        op,
                        operand: Box::new(self.expr(operand, span)),
                    },
                    _ => ExprKind::Err,
                }
            }
            ast::Expr::Binary(binary) => {
                let op = binary.op().and_then(bin_op);
                match (op, binary.lhs(), binary.rhs()) {
                    (Some(op), Some(lhs), Some(rhs)) => ExprKind::Binary {
                        op,
                        lhs: Box::new(self.expr(lhs, span)),
                        rhs: Box::new(self.expr(rhs, span)),
                    },
                    _ => ExprKind::Err,
                }
            }
            ast::Expr::Range(range) => match (range.lhs(), range.rhs()) {
                (Some(lo), Some(hi)) => ExprKind::Range {
                    lo: Box::new(self.expr(lo, span)),
                    hi: Box::new(self.expr(hi, span)),
                },
                _ => ExprKind::Err,
            },
            ast::Expr::Assign(assign) => match (assign.lhs(), assign.rhs()) {
                (Some(target), Some(value)) => ExprKind::Assign {
                    target: Box::new(self.expr(target, span)),
                    value: Box::new(self.expr(value, span)),
                },
                _ => ExprKind::Err,
            },
            ast::Expr::Call(call) => ExprKind::Call {
                callee: Box::new(
                    call.callee()
                        .map_or(err_expr(span), |callee| self.expr(callee, span)),
                ),
                args: call.args().map(|arg| self.expr(arg, span)).collect(),
            },
            ast::Expr::MethodCall(call) => ExprKind::MethodCall {
                receiver: Box::new(
                    call.receiver()
                        .map_or(err_expr(span), |receiver| self.expr(receiver, span)),
                ),
                name: call.name().unwrap_or_default().to_owned(),
                args: call.args().map(|arg| self.expr(arg, span)).collect(),
            },
            ast::Expr::Field(field) => ExprKind::Field {
                receiver: Box::new(
                    field
                        .receiver()
                        .map_or(err_expr(span), |receiver| self.expr(receiver, span)),
                ),
                name: field.name().unwrap_or_default().to_owned(),
            },
            ast::Expr::Index(index) => match (index.base(), index.index()) {
                (Some(base), Some(idx)) => ExprKind::Index {
                    base: Box::new(self.expr(base, span)),
                    index: Box::new(self.expr(idx, span)),
                },
                _ => ExprKind::Err,
            },
            ast::Expr::Try(inner) => inner.inner().map_or(ExprKind::Err, |inner| {
                ExprKind::Try(Box::new(self.expr(inner, span)))
            }),
            ast::Expr::Cast(cast) => match (cast.inner(), cast.ty()) {
                (Some(value), Some(ty)) => ExprKind::Cast {
                    value: Box::new(self.expr(value, span)),
                    ty: self.ty(ty, span),
                },
                _ => ExprKind::Err,
            },
            // Parentheses are irrelevant syntax: unwrap them.
            ast::Expr::Group(group) => {
                return group
                    .inner()
                    .map_or(err_expr(span), |inner| self.expr(inner, span));
            }
            ast::Expr::If(if_expr) => self.if_expr(if_expr, span),
            ast::Expr::Match(match_expr) => ExprKind::Match {
                scrutinee: Box::new(
                    match_expr
                        .scrutinee()
                        .map_or(err_expr(span), |scrutinee| self.expr(scrutinee, span)),
                ),
                arms: match_expr
                    .arms()
                    .map(|arm| {
                        let arm_span = arm.span().unwrap_or(span);
                        Arm {
                            pat: arm
                                .pattern()
                                .map_or(err_pat(arm_span), |pattern| self.pat(pattern, arm_span)),
                            guard: arm.guard().map(|guard| self.expr(guard, arm_span)),
                            value: arm
                                .value()
                                .map_or(err_expr(arm_span), |value| self.expr(value, arm_span)),
                            span: arm_span,
                        }
                    })
                    .collect(),
            },
            ast::Expr::While(while_expr) => ExprKind::While {
                cond: Box::new(
                    while_expr
                        .condition()
                        .map_or(err_expr(span), |cond| self.expr(cond, span)),
                ),
                body: while_expr
                    .body()
                    .map_or_else(|| err_block(span), |body| self.block(body, span)),
            },
            ast::Expr::Loop(loop_expr) => ExprKind::Loop {
                label: loop_expr.label().map(str::to_owned),
                body: loop_expr
                    .body()
                    .map_or_else(|| err_block(span), |body| self.block(body, span)),
            },
            ast::Expr::For(for_expr) => ExprKind::For {
                pat: for_expr
                    .pattern()
                    .map_or(err_pat(span), |pattern| self.pat(pattern, span)),
                iter: Box::new(
                    for_expr
                        .iterable()
                        .map_or(err_expr(span), |iter| self.expr(iter, span)),
                ),
                body: for_expr
                    .body()
                    .map_or_else(|| err_block(span), |body| self.block(body, span)),
            },
            ast::Expr::Unsafe(unsafe_expr) => ExprKind::Unsafe(
                unsafe_expr
                    .body()
                    .map_or_else(|| err_block(span), |body| self.block(body, span)),
            ),
            ast::Expr::Return(ret) => {
                ExprKind::Return(ret.value().map(|value| Box::new(self.expr(value, span))))
            }
            ast::Expr::Break(brk) => ExprKind::Break {
                label: brk.label().map(str::to_owned),
                value: brk.value().map(|value| Box::new(self.expr(value, span))),
            },
            ast::Expr::Continue(cont) => ExprKind::Continue {
                label: cont.label().map(str::to_owned),
            },
            ast::Expr::Block(block) => ExprKind::Block(self.block(block, span)),
        };
        Expr { kind, span }
    }

    fn if_expr(&self, if_expr: ast::IfExpr<'_>, span: Span) -> ExprKind {
        ExprKind::If {
            cond: Box::new(
                if_expr
                    .condition()
                    .map_or(err_expr(span), |cond| self.expr(cond, span)),
            ),
            then: if_expr
                .then_block()
                .map_or_else(|| err_block(span), |then| self.block(then, span)),
            els: if_expr.else_branch().map(|branch| {
                Box::new(match branch {
                    ast::ElseBranch::If(nested) => {
                        let nested_span = nested.span().unwrap_or(span);
                        Expr {
                            kind: self.if_expr(nested, nested_span),
                            span: nested_span,
                        }
                    }
                    ast::ElseBranch::Block(block) => {
                        let block_span = block.span().unwrap_or(span);
                        Expr {
                            kind: ExprKind::Block(self.block(block, block_span)),
                            span: block_span,
                        }
                    }
                })
            }),
        }
    }

    /// Lower one struct-literal field, expanding shorthand `name` to the
    /// explicit `name: name` form (the shorthand *reads* the name, and
    /// resolution recorded what it read).
    fn field_init(&self, init: ast::FieldInit<'_>) -> Option<FieldInit> {
        let name = init.name_ref()?;
        let value = match init.value() {
            Some(value) => self.expr(value, name.span),
            None => Expr {
                kind: ExprKind::Path {
                    res: self.used(name),
                    args: Vec::new(),
                },
                span: name.span,
            },
        };
        Some(FieldInit {
            name: name.text.to_owned(),
            span: name.span,
            value,
        })
    }
}

/// A poison expression at `span`.
fn err_expr(span: Span) -> Expr {
    Expr {
        kind: ExprKind::Err,
        span,
    }
}

/// A poison pattern at `span`.
fn err_pat(span: Span) -> Pat {
    Pat {
        kind: PatKind::Err,
        span,
    }
}

/// An empty block standing in for a missing one.
fn err_block(span: Span) -> Block {
    Block {
        stmts: Vec::new(),
        tail: None,
        span,
    }
}

/// Interpret a literal token as a [`Lit`], normalizing numeric separators
/// and stripping quotes.
fn lit(kind: TokenKind, text: &str) -> Option<Lit> {
    Some(match kind {
        TokenKind::OpenParen => Lit::Unit,
        TokenKind::BoolLiteral => Lit::Bool(text == "true"),
        TokenKind::IntLiteral => Lit::Int(text.replace('_', "")),
        TokenKind::FloatLiteral => Lit::Float(text.replace('_', "")),
        TokenKind::CharLiteral => Lit::Char(strip_quotes(text, '\'')),
        TokenKind::StringLiteral => Lit::Str(strip_quotes(text, '"')),
        _ => return None,
    })
}

/// The contents between a literal's quotes (the raw text when malformed).
fn strip_quotes(text: &str, quote: char) -> String {
    text.strip_prefix(quote)
        .and_then(|rest| rest.strip_suffix(quote))
        .unwrap_or(text)
        .to_owned()
}

/// The infix operator behind its written form.
fn bin_op(op: &str) -> Option<BinOp> {
    Some(match op {
        "+" => BinOp::Add,
        "-" => BinOp::Sub,
        "*" => BinOp::Mul,
        "/" => BinOp::Div,
        "%" => BinOp::Rem,
        "==" => BinOp::Eq,
        "!=" => BinOp::Ne,
        "<" => BinOp::Lt,
        "<=" => BinOp::Le,
        ">" => BinOp::Gt,
        ">=" => BinOp::Ge,
        "&&" => BinOp::And,
        "||" => BinOp::Or,
        _ => return None,
    })
}
