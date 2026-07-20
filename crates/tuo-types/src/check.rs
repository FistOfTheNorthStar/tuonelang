//! The type checker: declaration collection, then body checking.
//!
//! Collection lowers every module-level signature first (explicit parameter
//! and return types, Constitution §8), so bodies can call forward in any
//! order. Body checking then walks each function, constant, and spec with a
//! fresh [`InferCtx`], unifying where local inference is allowed and
//! reporting structured mismatches everywhere else.
//!
//! Deliberately deferred (poisoned with [`Ty::Error`], never guessed):
//! method calls and interface-member bodies (they need impl/interface
//! dispatch), `Self` types, and interface bounds — those arrive with the
//! trait system, not the core type system.

use std::collections::HashMap;

use tuo_ast::{
    Ast, BindingStmt, Block, ElseBranch, Expr, FnDecl, Item, LiteralPat, Name, Pattern, SpecDecl,
    SpecStatement, Statement, StructLiteralExpr, TypeRef,
};
use tuo_diagnostics::{Diagnostic, DiagnosticCode, Namespace, StructuredValue};
use tuo_lexer::TokenKind;
use tuo_resolve::{Resolution, SymbolId, SymbolKind};
use tuo_source::Span;

use crate::TypeckResult;
use crate::infer::{InferCtx, VarClass};
use crate::ty::{FloatKind, FnTy, IntKind, Ty, WrapperKind};

/// The type checker's diagnostic codes (the reserved `Txxxx` namespace).
///
/// - `T0001` — mismatched types.
/// - `T0002` — wrong number of arguments.
/// - `T0003` — call of a non-function.
/// - `T0004` — unknown field.
/// - `T0005` — struct-literal / struct-pattern field-set error.
/// - `T0006` — operation not supported for a type.
/// - `T0007` — non-exhaustive match.
/// - `T0008` — invalid cast.
/// - `T0009` — invalid `?` propagation.
/// - `T0010` — wrong number of type arguments.
/// - `T0011` — type annotation needed.
/// - `T0012` — expected a value/constructor, found something else.
/// - `T0013` — `break`/`continue` outside a loop.
fn code(number: u16) -> DiagnosticCode {
    DiagnosticCode::new(Namespace::Type, number)
}

#[derive(Clone)]
struct StructDef {
    type_params: Vec<SymbolId>,
    fields: Vec<(String, Ty)>,
}

#[derive(Clone)]
struct EnumDef {
    type_params: Vec<SymbolId>,
    variants: Vec<(SymbolId, Vec<(String, Ty)>)>,
}

#[derive(Clone)]
struct FnSig {
    type_params: Vec<SymbolId>,
    params: Vec<Ty>,
    ret: Ty,
}

/// One enclosing loop during body checking: the type `break` values must
/// meet (`Unit` for `while`/`for`; a fresh variable for `loop`, whose
/// solution becomes the loop's value).
struct Frame {
    break_ty: Ty,
}

pub(crate) struct Checker<'a> {
    resolution: &'a Resolution,
    /// Name-use span → resolved symbol.
    refs: HashMap<Span, SymbolId>,
    /// Declaration name span → declared symbol.
    decls: HashMap<Span, SymbolId>,
    structs: HashMap<SymbolId, StructDef>,
    enums: HashMap<SymbolId, EnumDef>,
    fns: HashMap<SymbolId, FnSig>,
    consts: HashMap<SymbolId, Ty>,
    variant_owner: HashMap<SymbolId, SymbolId>,
    diagnostics: Vec<Diagnostic>,
    symbol_types: HashMap<SymbolId, Ty>,
    // Per-body state.
    icx: InferCtx,
    locals: HashMap<SymbolId, Ty>,
    ret: Ty,
    frames: Vec<Frame>,
    fallback: Span,
}

/// Type-check every body in `files`, using `resolution`'s stable symbols.
pub(crate) fn run(files: &[Ast<'_>], resolution: &Resolution) -> TypeckResult {
    let refs = resolution
        .references()
        .iter()
        .map(|reference| (reference.span, reference.symbol))
        .collect();
    let decls = resolution
        .symbols()
        .filter_map(|(id, symbol)| symbol.declaration.map(|span| (span, id)))
        .collect();
    let fallback = resolution
        .references()
        .first()
        .map_or_else(dummy_span, |reference| reference.span);
    let mut checker = Checker {
        resolution,
        refs,
        decls,
        structs: HashMap::new(),
        enums: HashMap::new(),
        fns: HashMap::new(),
        consts: HashMap::new(),
        variant_owner: HashMap::new(),
        diagnostics: Vec::new(),
        symbol_types: HashMap::new(),
        icx: InferCtx::default(),
        locals: HashMap::new(),
        ret: Ty::Unit,
        frames: Vec::new(),
        fallback,
    };
    for ast in files {
        checker.collect_headers(*ast);
    }
    for ast in files {
        checker.collect_signatures(*ast);
    }
    for ast in files {
        checker.check_file(*ast);
    }
    TypeckResult {
        diagnostics: checker.diagnostics,
        symbol_types: checker.symbol_types,
    }
}

/// A span for constructs whose own tokens are missing (malformed trees).
fn dummy_span() -> Span {
    Span::new(
        tuo_source::SourceId::from_raw(0),
        tuo_source::TextRange::new(0u32, 0u32).expect("empty range is valid"),
    )
}

impl<'a> Checker<'a> {
    // ------------------------------------------------------------------
    // Small helpers
    // ------------------------------------------------------------------

    fn at(&self, span: Option<Span>) -> Span {
        span.unwrap_or(self.fallback)
    }

    fn symbol_at(&self, name: Option<Name<'_>>) -> Option<SymbolId> {
        name.and_then(|name| self.refs.get(&name.span).copied())
    }

    fn declared_at(&self, name: Option<Name<'_>>) -> Option<SymbolId> {
        name.and_then(|name| self.decls.get(&name.span).copied())
    }

    fn render(&self, ty: &Ty) -> String {
        self.icx.apply(ty).render(self.resolution)
    }

    fn prelude(&self, name: &str) -> Option<SymbolId> {
        self.resolution.prelude_symbol(name)
    }

    fn push(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    /// Report a `T0001` mismatch with structured expected/actual types.
    fn mismatch(&mut self, expected: &Ty, actual: &Ty, span: Span) {
        let expected_text = self.render(expected);
        let actual_text = self.render(actual);
        self.push(
            Diagnostic::error(code(1), "mismatched types", span)
                .with_primary_label(format!("expected `{expected_text}`, found `{actual_text}`"))
                .with_expected(StructuredValue::Type(expected_text))
                .with_actual(StructuredValue::Type(actual_text)),
        );
    }

    /// Unify, reporting a `T0001` at `span` on failure.
    fn expect_ty(&mut self, expected: &Ty, actual: &Ty, span: Span) {
        if let Err(mismatch) = self.icx.unify(expected, actual) {
            self.mismatch(&mismatch.expected, &mismatch.actual, span);
        }
    }

    /// Join two branch types (`if`/`match` arms): `Never` yields to the
    /// other side; anything else must unify.
    fn join(&mut self, acc: Ty, next: Ty, span: Span) -> Ty {
        if matches!(self.icx.apply(&acc), Ty::Never) {
            return next;
        }
        if matches!(self.icx.apply(&next), Ty::Never) {
            return acc;
        }
        self.expect_ty(&acc, &next, span);
        acc
    }

    fn fresh(&mut self, span: Span) -> Ty {
        self.icx.fresh(VarClass::General, span)
    }

    // ------------------------------------------------------------------
    // Collection: headers (type-parameter lists), then full signatures
    // ------------------------------------------------------------------

    fn generic_symbols(&self, generics: Option<tuo_ast::GenericParams<'_>>) -> Vec<SymbolId> {
        generics
            .into_iter()
            .flat_map(|list| list.params())
            .filter_map(|param| self.declared_at(param.name_ref()))
            .collect()
    }

    /// Register every type-declaring item's *shape* (its type parameters)
    /// so signature lowering can arity-check generic references in any
    /// order.
    fn collect_headers(&mut self, ast: Ast<'_>) {
        for item in ast.file().items() {
            match item {
                Item::Struct(decl) => {
                    if let Some(id) = self.declared_at(decl.name_ref()) {
                        let type_params = self.generic_symbols(decl.generics());
                        self.structs.insert(
                            id,
                            StructDef {
                                type_params,
                                fields: Vec::new(),
                            },
                        );
                    }
                }
                Item::Enum(decl) => {
                    if let Some(id) = self.declared_at(decl.name_ref()) {
                        let type_params = self.generic_symbols(decl.generics());
                        self.enums.insert(
                            id,
                            EnumDef {
                                type_params,
                                variants: Vec::new(),
                            },
                        );
                        for variant in self.resolution.variants_of(id) {
                            self.variant_owner.insert(*variant, id);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn collect_signatures(&mut self, ast: Ast<'_>) {
        for item in ast.file().items() {
            match item {
                Item::Fn(decl) => {
                    if let Some(id) = self.declared_at(decl.name_ref()) {
                        let sig = self.lower_fn_sig(decl);
                        self.symbol_types.insert(
                            id,
                            Ty::Fn(Box::new(FnTy {
                                params: sig.params.clone(),
                                ret: sig.ret.clone(),
                            })),
                        );
                        self.fns.insert(id, sig);
                    }
                }
                Item::Struct(decl) => {
                    if let Some(id) = self.declared_at(decl.name_ref()) {
                        let fields = decl
                            .fields()
                            .filter_map(|field| {
                                let name = field.name()?.to_owned();
                                let ty = field.ty().map_or(Ty::Error, |ty| self.lower_type(ty));
                                Some((name, ty))
                            })
                            .collect();
                        if let Some(def) = self.structs.get_mut(&id) {
                            def.fields = fields;
                        }
                    }
                }
                Item::Enum(decl) => {
                    if let Some(id) = self.declared_at(decl.name_ref()) {
                        let variants = decl
                            .variants()
                            .filter_map(|variant| {
                                let symbol = self.declared_at(variant.name_ref())?;
                                let fields = variant
                                    .fields()
                                    .filter_map(|field| {
                                        let name = field.name()?.to_owned();
                                        let ty =
                                            field.ty().map_or(Ty::Error, |ty| self.lower_type(ty));
                                        Some((name, ty))
                                    })
                                    .collect();
                                Some((symbol, fields))
                            })
                            .collect();
                        if let Some(def) = self.enums.get_mut(&id) {
                            def.variants = variants;
                        }
                    }
                }
                Item::Const(decl) => {
                    if let Some(id) = self.declared_at(decl.name_ref()) {
                        let ty = decl.ty().map_or(Ty::Error, |ty| self.lower_type(ty));
                        self.symbol_types.insert(id, ty.clone());
                        self.consts.insert(id, ty);
                    }
                }
                _ => {}
            }
        }
    }

    fn lower_fn_sig(&mut self, decl: FnDecl<'_>) -> FnSig {
        let type_params = self.generic_symbols(decl.generics());
        let params = decl
            .params()
            .map(|param| param.ty().map_or(Ty::Error, |ty| self.lower_type(ty)))
            .collect();
        let ret = decl
            .return_type()
            .map_or(Ty::Unit, |ty| self.lower_type(ty));
        FnSig {
            type_params,
            params,
            ret,
        }
    }

    // ------------------------------------------------------------------
    // Type lowering (declared types are always fully explicit)
    // ------------------------------------------------------------------

    fn check_type_arity(&mut self, name: &str, expected: usize, found: usize, span: Span) -> bool {
        if expected == found {
            return true;
        }
        self.push(
            Diagnostic::error(
                code(10),
                format!(
                    "wrong number of type arguments for `{name}`: expected {expected}, found {found}"
                ),
                span,
            )
            .with_expected(StructuredValue::Count(expected as u64))
            .with_actual(StructuredValue::Count(found as u64)),
        );
        false
    }

    fn builtin_ty(&mut self, name: &str, args: Vec<Ty>, span: Span) -> Option<Ty> {
        let nullary = |kind: Ty| Some((kind, 0));
        let (ty, arity) = match name {
            "I8" => nullary(Ty::Int(IntKind::I8))?,
            "I16" => nullary(Ty::Int(IntKind::I16))?,
            "I32" => nullary(Ty::Int(IntKind::I32))?,
            "I64" | "Int" => nullary(Ty::Int(IntKind::I64))?,
            "Isize" => nullary(Ty::Int(IntKind::Isize))?,
            "U8" => nullary(Ty::Int(IntKind::U8))?,
            "U16" => nullary(Ty::Int(IntKind::U16))?,
            "U32" => nullary(Ty::Int(IntKind::U32))?,
            "U64" => nullary(Ty::Int(IntKind::U64))?,
            "Usize" => nullary(Ty::Int(IntKind::Usize))?,
            "F32" => nullary(Ty::Float(FloatKind::F32))?,
            "F64" | "Float" => nullary(Ty::Float(FloatKind::F64))?,
            "Bool" => nullary(Ty::Bool)?,
            "Char" => nullary(Ty::Char)?,
            "String" => nullary(Ty::String)?,
            "Str" => nullary(Ty::Str)?,
            "Array" => {
                if !self.check_type_arity("Array", 1, args.len(), span) {
                    return Some(Ty::Error);
                }
                return Some(Ty::Array(Box::new(
                    args.into_iter().next().unwrap_or(Ty::Error),
                )));
            }
            _ => return None,
        };
        if !self.check_type_arity(name, arity, args.len(), span) {
            return Some(Ty::Error);
        }
        Some(ty)
    }

    /// Lower a written type to a [`Ty`]. Declared types are strict: generic
    /// types take exactly their declared number of arguments.
    fn lower_type(&mut self, ty: TypeRef<'_>) -> Ty {
        match ty {
            TypeRef::Unit(_) => Ty::Unit,
            TypeRef::Wrapper(wrapper) => {
                let kind = match wrapper.wrapper() {
                    Some("Box") => WrapperKind::Box,
                    Some("Shared") => WrapperKind::Shared,
                    Some("Weak") => WrapperKind::Weak,
                    _ => return Ty::Error,
                };
                let args: Vec<Ty> = wrapper
                    .args()
                    .into_iter()
                    .flat_map(|list| list.types())
                    .map(|ty| self.lower_type(ty))
                    .collect();
                let span = self.at(wrapper.span());
                if !self.check_type_arity(kind.name(), 1, args.len(), span) {
                    return Ty::Error;
                }
                Ty::Wrapper(kind, Box::new(args.into_iter().next().unwrap_or(Ty::Error)))
            }
            TypeRef::Path(path) => {
                let args: Vec<Ty> = path
                    .args()
                    .into_iter()
                    .flat_map(|list| list.types())
                    .map(|ty| self.lower_type(ty))
                    .collect();
                let Some(last) = path.path().and_then(|p| p.segment_names().last()) else {
                    return Ty::Error;
                };
                if last.text == "Self" {
                    return Ty::Error;
                }
                self.lower_named(last, args)
            }
        }
    }

    fn lower_named(&mut self, name: Name<'_>, args: Vec<Ty>) -> Ty {
        if let Some(symbol) = self.refs.get(&name.span).copied() {
            return self.lower_symbol_ty(symbol, name, args);
        }
        self.builtin_ty(name.text, args, name.span)
            .unwrap_or(Ty::Error)
    }

    fn lower_symbol_ty(&mut self, symbol: SymbolId, name: Name<'_>, args: Vec<Ty>) -> Ty {
        match self.resolution.symbol(symbol).kind {
            SymbolKind::Struct => {
                let params = self
                    .structs
                    .get(&symbol)
                    .map_or(0, |def| def.type_params.len());
                if !self.check_type_arity(name.text, params, args.len(), name.span) {
                    return Ty::Error;
                }
                Ty::Struct(symbol, args)
            }
            SymbolKind::Enum => {
                if Some(symbol) == self.prelude("Option") {
                    if !self.check_type_arity("Option", 1, args.len(), name.span) {
                        return Ty::Error;
                    }
                    return Ty::Option(Box::new(args.into_iter().next().unwrap_or(Ty::Error)));
                }
                if Some(symbol) == self.prelude("Result") {
                    if !self.check_type_arity("Result", 2, args.len(), name.span) {
                        return Ty::Error;
                    }
                    let mut args = args.into_iter();
                    return Ty::Result(
                        Box::new(args.next().unwrap_or(Ty::Error)),
                        Box::new(args.next().unwrap_or(Ty::Error)),
                    );
                }
                let params = self
                    .enums
                    .get(&symbol)
                    .map_or(0, |def| def.type_params.len());
                if !self.check_type_arity(name.text, params, args.len(), name.span) {
                    return Ty::Error;
                }
                Ty::Enum(symbol, args)
            }
            SymbolKind::TypeParam => {
                if !args.is_empty() {
                    self.check_type_arity(name.text, 0, args.len(), name.span);
                    return Ty::Error;
                }
                Ty::Param(symbol)
            }
            // Interfaces-as-types (and anything else) belong to later
            // pipeline stages; poison quietly rather than guess.
            _ => Ty::Error,
        }
    }

    // ------------------------------------------------------------------
    // Body checking
    // ------------------------------------------------------------------

    fn check_file(&mut self, ast: Ast<'_>) {
        for item in ast.file().items() {
            match item {
                Item::Fn(decl) => self.check_fn(decl, None),
                Item::Impl(decl) => {
                    let target = decl.target().map(|ty| self.lower_type(ty));
                    for function in decl.functions() {
                        self.check_fn(function, target.clone());
                    }
                }
                Item::Const(decl) => {
                    let Some(id) = self.declared_at(decl.name_ref()) else {
                        continue;
                    };
                    let fallback = self.at(decl.name_ref().map(|name| name.span));
                    self.begin_body(Ty::Unit, fallback);
                    let declared = self.consts.get(&id).cloned().unwrap_or(Ty::Error);
                    if let Some(value) = decl.value() {
                        let span = self.at(value_span(&value));
                        let actual = self.expr(value);
                        self.expect_ty(&declared, &actual, span);
                    }
                    self.finish_body();
                }
                Item::Spec(decl) => self.check_spec(decl),
                // Interface member bodies need `Self`; deferred with the
                // trait system.
                _ => {}
            }
        }
    }

    fn begin_body(&mut self, ret: Ty, fallback: Span) {
        self.icx = InferCtx::default();
        self.locals = HashMap::new();
        self.ret = ret;
        self.frames = Vec::new();
        self.fallback = fallback;
    }

    /// Default remaining literal variables, report unsolved ones, and
    /// publish the body's local types.
    fn finish_body(&mut self) {
        for span in self.icx.finalize() {
            self.push(
                Diagnostic::error(code(11), "type annotation needed", span)
                    .with_primary_label("cannot infer the type here"),
            );
        }
        let locals = std::mem::take(&mut self.locals);
        for (symbol, ty) in locals {
            let ty = self.icx.apply(&ty);
            self.symbol_types.insert(symbol, ty);
        }
    }

    fn check_fn(&mut self, decl: FnDecl<'_>, self_ty: Option<Ty>) {
        let Some(body) = decl.body() else {
            return;
        };
        // Top-level functions were lowered during collection — reuse that
        // signature so its diagnostics are not repeated. Impl functions are
        // not module-callable and lower here.
        let collected = self
            .declared_at(decl.name_ref())
            .and_then(|id| self.fns.get(&id).cloned());
        let fallback = self.at(decl
            .name_ref()
            .map(|name| name.span)
            .or_else(|| decl.span()));
        let ret = match &collected {
            Some(sig) => sig.ret.clone(),
            None => decl
                .return_type()
                .map_or(Ty::Unit, |ty| self.lower_type(ty)),
        };
        self.begin_body(ret.clone(), fallback);
        for (index, param) in decl.params().enumerate() {
            let ty = if param.is_receiver() {
                self_ty.clone().unwrap_or(Ty::Error)
            } else if let Some(sig) = &collected {
                sig.params.get(index).cloned().unwrap_or(Ty::Error)
            } else {
                param.ty().map_or(Ty::Error, |ty| self.lower_type(ty))
            };
            if let Some(symbol) = self.declared_at(param.name_ref()) {
                self.locals.insert(symbol, ty);
            }
        }
        let body_span = self.at(body.span());
        let actual = self.block(body);
        self.expect_ty(&ret, &actual, body_span);
        self.finish_body();
    }

    fn check_spec(&mut self, decl: SpecDecl<'_>) {
        self.begin_body(Ty::Unit, self.at(decl.span()));
        for statement in decl.statements() {
            match statement {
                SpecStatement::Given(clause) => {
                    for binding in clause.bindings() {
                        let declared = binding.ty().map_or(Ty::Error, |ty| self.lower_type(ty));
                        if let Some(init) = binding.initializer() {
                            let span = self.at(value_span(&init));
                            let actual = self.expr(init);
                            self.expect_ty(&declared, &actual, span);
                        }
                        if let Some(symbol) = self.declared_at(binding.name_ref()) {
                            self.locals.insert(symbol, declared);
                        }
                    }
                }
                SpecStatement::When(clause) => {
                    if let Some(binding) = clause.binding() {
                        self.binding_stmt(binding);
                    } else if let Some(expr) = clause.expr() {
                        self.expr(expr);
                    }
                }
                SpecStatement::Then(clause) | SpecStatement::Assert(clause) => {
                    if let Some(expr) = clause.expr() {
                        let span = self.at(value_span(&expr));
                        let actual = self.expr(expr);
                        self.expect_ty(&Ty::Bool, &actual, span);
                    }
                }
                SpecStatement::Let(binding) | SpecStatement::Var(binding) => {
                    self.binding_stmt(binding);
                }
                SpecStatement::Expr(statement) => {
                    if let Some(expr) = statement.expr() {
                        self.expr(expr);
                    }
                }
                SpecStatement::Error(_) => {}
            }
        }
        self.finish_body();
    }

    // ------------------------------------------------------------------
    // Statements and blocks
    // ------------------------------------------------------------------

    fn block(&mut self, block: Block<'_>) -> Ty {
        let mut diverges = false;
        // Block-form expressions standing alone parse as expression
        // statements, never as the syntactic tail — so a final,
        // semicolon-less expression statement is this block's value.
        let statements: Vec<Statement<'_>> = block.statements().collect();
        let tail_statement = match (block.tail(), statements.last()) {
            (None, Some(Statement::Expr(statement))) if !statement.has_semicolon() => {
                statement.expr()
            }
            _ => None,
        };
        let statement_count = statements.len() - usize::from(tail_statement.is_some());
        for statement in &statements[..statement_count] {
            match *statement {
                Statement::Let(binding) | Statement::Var(binding) => self.binding_stmt(binding),
                Statement::Const(decl) => {
                    let declared = decl.ty().map_or(Ty::Error, |ty| self.lower_type(ty));
                    if let Some(value) = decl.value() {
                        let span = self.at(value_span(&value));
                        let actual = self.expr(value);
                        self.expect_ty(&declared, &actual, span);
                    }
                    if let Some(symbol) = self.declared_at(decl.name_ref()) {
                        self.locals.insert(symbol, declared);
                    }
                }
                Statement::Expr(statement) => {
                    if let Some(expr) = statement.expr() {
                        let ty = self.expr(expr);
                        if matches!(self.icx.apply(&ty), Ty::Never) {
                            diverges = true;
                        }
                    }
                }
                Statement::Empty(_) | Statement::Error(_) => {}
            }
        }
        if let Some(tail) = block.tail().or(tail_statement) {
            self.expr(tail)
        } else if diverges {
            Ty::Never
        } else {
            Ty::Unit
        }
    }

    fn binding_stmt(&mut self, binding: BindingStmt<'_>) {
        let declared = binding.ty().map(|ty| self.lower_type(ty));
        let init = binding.initializer().map(|init| {
            let span = self.at(value_span(&init));
            (self.expr(init), span)
        });
        let ty = match (declared, init) {
            (Some(declared), Some((actual, span))) => {
                self.expect_ty(&declared, &actual, span);
                declared
            }
            (Some(declared), None) => declared,
            (None, Some((actual, _))) => actual,
            (None, None) => {
                let span = self.at(binding.span());
                self.fresh(span)
            }
        };
        if let Some(pattern) = binding.pattern() {
            self.pattern(pattern, &ty);
        }
    }

    // ------------------------------------------------------------------
    // Patterns
    // ------------------------------------------------------------------

    fn pattern(&mut self, pattern: Pattern<'_>, expected: &Ty) {
        match pattern {
            Pattern::Wildcard(_) => {}
            Pattern::Binding(binding) => {
                if let Some(symbol) = self.declared_at(binding.name_ref()) {
                    self.locals.insert(symbol, expected.clone());
                }
            }
            Pattern::Literal(literal) => {
                let span = self.at(literal.span());
                let ty = self.literal_pattern_ty(literal, span);
                self.expect_ty(expected, &ty, span);
            }
            Pattern::Path(path) => self.path_pattern(path, expected),
            Pattern::Or(or) => {
                for alternative in or.alternatives() {
                    self.pattern(alternative, expected);
                }
            }
            Pattern::Group(group) => {
                if let Some(inner) = group.inner() {
                    self.pattern(inner, expected);
                }
            }
        }
    }

    fn literal_pattern_ty(&mut self, literal: LiteralPat<'_>, span: Span) -> Ty {
        match literal.token_kind() {
            Some(TokenKind::IntLiteral) => self.icx.fresh(VarClass::Integer, span),
            Some(TokenKind::FloatLiteral) => self.icx.fresh(VarClass::Float, span),
            Some(TokenKind::BoolLiteral) => Ty::Bool,
            Some(TokenKind::CharLiteral) => Ty::Char,
            Some(TokenKind::StringLiteral) => Ty::Str,
            Some(TokenKind::OpenParen) => Ty::Unit,
            _ => Ty::Error,
        }
    }

    fn path_pattern(&mut self, path: tuo_ast::PathPat<'_>, expected: &Ty) {
        let span = self.at(path.span());
        let Some(symbol) = self.symbol_at(path.segment_names().last()) else {
            return;
        };
        let fields: Vec<tuo_ast::FieldPat<'_>> = path.fields().collect();
        let shape = self.constructor_shape(symbol, None, span);
        let Some(shape) = shape else {
            return;
        };
        self.expect_ty(expected, &shape.ty, span);
        // Check the written field patterns against the constructor's
        // fields.
        let mut seen: Vec<&str> = Vec::new();
        for field in &fields {
            let Some(name) = field.name_ref() else {
                continue;
            };
            seen.push(name.text);
            let Some((_, field_ty)) = shape
                .fields
                .iter()
                .find(|(field_name, _)| field_name == name.text)
            else {
                self.unknown_field(name.text, &shape.ty, name.span);
                continue;
            };
            let field_ty = field_ty.clone();
            if let Some(sub) = field.pattern() {
                self.pattern(sub, &field_ty);
            } else if let Some(symbol) = self.declared_at(field.name_ref()) {
                self.locals.insert(symbol, field_ty);
            }
        }
        if !path.has_rest() {
            let missing: Vec<String> = shape
                .fields
                .iter()
                .map(|(name, _)| name.clone())
                .filter(|name| !seen.iter().any(|seen| seen == name))
                .collect();
            if !missing.is_empty() {
                let rendered = self.render(&shape.ty);
                let mut diagnostic = Diagnostic::error(
                    code(5),
                    format!(
                        "pattern for `{rendered}` does not mention fields {}",
                        missing
                            .iter()
                            .map(|name| format!("`{name}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    span,
                )
                .with_help("add the missing fields, or `..` to ignore the rest");
                for name in missing {
                    diagnostic = diagnostic.with_expected(StructuredValue::Name(name));
                }
                self.push(diagnostic);
            }
        }
    }

    // ------------------------------------------------------------------
    // Constructors (struct literals and variant patterns share this)
    // ------------------------------------------------------------------

    fn unknown_field(&mut self, field: &str, on: &Ty, span: Span) {
        let rendered = self.render(on);
        self.push(
            Diagnostic::error(
                code(4),
                format!("no field `{field}` on type `{rendered}`"),
                span,
            )
            .with_actual(StructuredValue::Name(field.to_owned()))
            .with_expected(StructuredValue::Type(rendered)),
        );
    }

    /// What constructing `symbol` produces: the constructed type (with
    /// fresh variables for its type arguments unless `given_args` pins
    /// them) and its field list.
    fn constructor_shape(
        &mut self,
        symbol: SymbolId,
        given_args: Option<Vec<Ty>>,
        span: Span,
    ) -> Option<Shape> {
        let kind = self.resolution.symbol(symbol).kind;
        match kind {
            SymbolKind::Struct => {
                let def = self.structs.get(&symbol).cloned()?;
                let args = self.instantiation_args(&def.type_params, given_args, span)?;
                let substitution = substitution(&def.type_params, &args);
                let fields = def
                    .fields
                    .iter()
                    .map(|(name, ty)| (name.clone(), substitute(ty, &substitution)))
                    .collect();
                Some(Shape {
                    ty: Ty::Struct(symbol, args),
                    fields,
                })
            }
            SymbolKind::Variant => self.variant_shape(symbol, given_args, span),
            _ => {
                let noun = kind.noun();
                let name = self.resolution.symbol(symbol).name.clone();
                self.push(
                    Diagnostic::error(
                        code(12),
                        format!("expected a struct or enum variant, found {noun} `{name}`"),
                        span,
                    )
                    .with_actual(StructuredValue::Name(name)),
                );
                None
            }
        }
    }

    fn variant_shape(
        &mut self,
        symbol: SymbolId,
        given_args: Option<Vec<Ty>>,
        span: Span,
    ) -> Option<Shape> {
        // The canonical Option/Result variants (Constitution §14, §15).
        if Some(symbol) == self.prelude("Some") {
            let value = self.given_or_fresh(given_args, span);
            return Some(Shape {
                ty: Ty::Option(Box::new(value.clone())),
                fields: vec![("value".to_owned(), value)],
            });
        }
        if Some(symbol) == self.prelude("None") {
            let value = self.given_or_fresh(given_args, span);
            return Some(Shape {
                ty: Ty::Option(Box::new(value)),
                fields: Vec::new(),
            });
        }
        if Some(symbol) == self.prelude("Ok") {
            let (ok, err) = self.given_pair_or_fresh(given_args, span);
            return Some(Shape {
                ty: Ty::Result(Box::new(ok.clone()), Box::new(err)),
                fields: vec![("value".to_owned(), ok)],
            });
        }
        if Some(symbol) == self.prelude("Err") {
            let (ok, err) = self.given_pair_or_fresh(given_args, span);
            return Some(Shape {
                ty: Ty::Result(Box::new(ok), Box::new(err.clone())),
                fields: vec![("error".to_owned(), err)],
            });
        }
        let owner = self.variant_owner.get(&symbol).copied()?;
        let def = self.enums.get(&owner).cloned()?;
        let args = self.instantiation_args(&def.type_params, given_args, span)?;
        let map = substitution(&def.type_params, &args);
        let fields = def
            .variants
            .iter()
            .find(|(variant, _)| *variant == symbol)
            .map(|(_, fields)| {
                fields
                    .iter()
                    .map(|(name, ty)| (name.clone(), substitute(ty, &map)))
                    .collect()
            })?;
        Some(Shape {
            ty: Ty::Enum(owner, args),
            fields,
        })
    }

    fn given_or_fresh(&mut self, given: Option<Vec<Ty>>, span: Span) -> Ty {
        match given {
            Some(args) if args.len() == 1 => args.into_iter().next().unwrap_or(Ty::Error),
            Some(args) => {
                self.check_type_arity("Option", 1, args.len(), span);
                Ty::Error
            }
            None => self.fresh(span),
        }
    }

    fn given_pair_or_fresh(&mut self, given: Option<Vec<Ty>>, span: Span) -> (Ty, Ty) {
        match given {
            Some(args) if args.len() == 2 => {
                let mut args = args.into_iter();
                (
                    args.next().unwrap_or(Ty::Error),
                    args.next().unwrap_or(Ty::Error),
                )
            }
            Some(args) => {
                self.check_type_arity("Result", 2, args.len(), span);
                (Ty::Error, Ty::Error)
            }
            None => (self.fresh(span), self.fresh(span)),
        }
    }

    fn instantiation_args(
        &mut self,
        type_params: &[SymbolId],
        given: Option<Vec<Ty>>,
        span: Span,
    ) -> Option<Vec<Ty>> {
        match given {
            Some(args) => {
                if !self.check_type_arity("this type", type_params.len(), args.len(), span) {
                    return None;
                }
                Some(args)
            }
            None => Some(type_params.iter().map(|_| self.fresh(span)).collect()),
        }
    }

    fn struct_literal(&mut self, literal: StructLiteralExpr<'_>) -> Ty {
        let span = self.at(literal.span());
        let given_args: Option<Vec<Ty>> = literal
            .type_args()
            .map(|args| args.types().map(|ty| self.lower_type(ty)).collect());
        let Some(name) = literal.path().and_then(|path| path.segment_names().last()) else {
            return Ty::Error;
        };
        let Some(symbol) = self.refs.get(&name.span).copied() else {
            // The resolver stays silent on builtin type names; a literal of
            // one is a type error, not a name error.
            if self.builtin_ty(name.text, Vec::new(), name.span).is_some() {
                self.push(
                    Diagnostic::error(
                        code(12),
                        format!("`{}` is not a struct", name.text),
                        name.span,
                    )
                    .with_actual(StructuredValue::Type(name.text.to_owned())),
                );
            }
            // Also check the initializer expressions for their own errors.
            for init in literal.inits() {
                if let Some(value) = init.value() {
                    self.expr(value);
                }
            }
            return Ty::Error;
        };
        let Some(shape) = self.constructor_shape(symbol, given_args, span) else {
            for init in literal.inits() {
                if let Some(value) = init.value() {
                    self.expr(value);
                }
            }
            return Ty::Error;
        };
        let mut seen: Vec<String> = Vec::new();
        for init in literal.inits() {
            let Some(name) = init.name_ref() else {
                continue;
            };
            if seen.iter().any(|seen| seen == name.text) {
                self.push(
                    Diagnostic::error(
                        code(5),
                        format!("field `{}` specified more than once", name.text),
                        name.span,
                    )
                    .with_actual(StructuredValue::Name(name.text.to_owned())),
                );
                continue;
            }
            seen.push(name.text.to_owned());
            let field_ty = shape
                .fields
                .iter()
                .find(|(field, _)| field == name.text)
                .map(|(_, ty)| ty.clone());
            let Some(field_ty) = field_ty else {
                self.unknown_field(name.text, &shape.ty, name.span);
                if let Some(value) = init.value() {
                    self.expr(value);
                }
                continue;
            };
            if let Some(value) = init.value() {
                let value_at = self.at(value_span(&value));
                let actual = self.expr(value);
                self.expect_ty(&field_ty, &actual, value_at);
            } else {
                // Shorthand `{ y }` reads the local `y`.
                let actual = self
                    .symbol_at(init.name_ref())
                    .and_then(|symbol| self.locals.get(&symbol).cloned())
                    .unwrap_or(Ty::Error);
                self.expect_ty(&field_ty, &actual, name.span);
            }
        }
        let missing: Vec<String> = shape
            .fields
            .iter()
            .map(|(name, _)| name.clone())
            .filter(|name| !seen.contains(name))
            .collect();
        if !missing.is_empty() {
            let rendered = self.render(&shape.ty);
            let mut diagnostic = Diagnostic::error(
                code(5),
                format!(
                    "missing fields {} in literal of `{rendered}`",
                    missing
                        .iter()
                        .map(|name| format!("`{name}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                span,
            );
            for name in missing {
                diagnostic = diagnostic.with_expected(StructuredValue::Name(name));
            }
            self.push(diagnostic);
        }
        shape.ty
    }

    // ------------------------------------------------------------------
    // Expressions
    // ------------------------------------------------------------------

    #[expect(
        clippy::too_many_lines,
        reason = "one arm per expression form; the dispatch reads best in one place"
    )]
    fn expr(&mut self, expr: Expr<'_>) -> Ty {
        let span = self.at(expr.span());
        match expr {
            Expr::Literal(literal) => match literal.token_kind() {
                Some(TokenKind::IntLiteral) => self.icx.fresh(VarClass::Integer, span),
                Some(TokenKind::FloatLiteral) => self.icx.fresh(VarClass::Float, span),
                Some(TokenKind::BoolLiteral) => Ty::Bool,
                Some(TokenKind::CharLiteral) => Ty::Char,
                Some(TokenKind::StringLiteral) => Ty::Str,
                Some(TokenKind::OpenParen) => Ty::Unit,
                _ => Ty::Error,
            },
            Expr::Path(path) => self.path_value(path),
            Expr::StructLiteral(literal) => self.struct_literal(literal),
            Expr::Unary(unary) => {
                let operand = unary.operand().map_or(Ty::Error, |inner| self.expr(inner));
                match unary.op() {
                    Some("-") => {
                        let applied = self.icx.apply(&operand);
                        match applied {
                            Ty::Int(kind) if !kind.is_signed() => {
                                self.unsupported_op("-", &applied, span);
                                Ty::Error
                            }
                            Ty::Int(_) | Ty::Float(_) | Ty::Var(_) | Ty::Error | Ty::Never => {
                                operand
                            }
                            other => {
                                self.unsupported_op("-", &other, span);
                                Ty::Error
                            }
                        }
                    }
                    Some("!") => {
                        self.expect_ty(&Ty::Bool, &operand, span);
                        Ty::Bool
                    }
                    // `move` is an ownership operator; the type passes
                    // through.
                    _ => operand,
                }
            }
            Expr::Binary(binary) => {
                let lhs = binary.lhs().map_or(Ty::Error, |inner| self.expr(inner));
                let rhs_span = binary
                    .rhs()
                    .map_or(span, |inner| self.at(value_span(&inner)));
                let rhs = binary.rhs().map_or(Ty::Error, |inner| self.expr(inner));
                self.binary_op(binary.op().unwrap_or(""), &lhs, &rhs, span, rhs_span)
            }
            Expr::Range(range) => {
                let lhs = range.lhs().map_or(Ty::Error, |inner| self.expr(inner));
                let rhs_span = range
                    .rhs()
                    .map_or(span, |inner| self.at(value_span(&inner)));
                let rhs = range.rhs().map_or(Ty::Error, |inner| self.expr(inner));
                self.expect_ty(&lhs, &rhs, rhs_span);
                let elem = self.icx.fresh(VarClass::Integer, span);
                if let Err(mismatch) = self.icx.unify(&elem, &lhs) {
                    let actual = self.render(&mismatch.actual);
                    self.push(
                        Diagnostic::error(
                            code(6),
                            format!("range endpoints must be integers, found `{actual}`"),
                            span,
                        )
                        .with_actual(StructuredValue::Type(actual)),
                    );
                    return Ty::Error;
                }
                Ty::Range(Box::new(elem))
            }
            Expr::Assign(assign) => {
                let lhs = assign.lhs().map_or(Ty::Error, |inner| self.expr(inner));
                let rhs_span = assign
                    .rhs()
                    .map_or(span, |inner| self.at(value_span(&inner)));
                let rhs = assign.rhs().map_or(Ty::Error, |inner| self.expr(inner));
                self.expect_ty(&lhs, &rhs, rhs_span);
                Ty::Unit
            }
            Expr::Call(call) => self.call(call, span),
            Expr::MethodCall(call) => {
                // Method dispatch needs impl resolution: check the receiver
                // and arguments for their own errors, poison the result.
                if let Some(receiver) = call.receiver() {
                    self.expr(receiver);
                }
                for arg in call.args() {
                    self.expr(arg);
                }
                Ty::Error
            }
            Expr::Field(field) => {
                let receiver = field.receiver().map_or(Ty::Error, |inner| self.expr(inner));
                let Some(name) = field.name() else {
                    return Ty::Error;
                };
                self.field_ty(&receiver, name, span)
            }
            Expr::Index(index) => {
                let base = index.base().map_or(Ty::Error, |inner| self.expr(inner));
                let index_span = index
                    .index()
                    .map_or(span, |inner| self.at(value_span(&inner)));
                let index_ty = index.index().map_or(Ty::Error, |inner| self.expr(inner));
                match self.icx.apply(&base) {
                    Ty::Array(item) => {
                        self.expect_ty(&Ty::Int(IntKind::Usize), &index_ty, index_span);
                        *item
                    }
                    Ty::Error | Ty::Var(_) | Ty::Never => Ty::Error,
                    other => {
                        self.unsupported_op("index", &other, span);
                        Ty::Error
                    }
                }
            }
            Expr::Try(inner) => {
                let operand = inner.inner().map_or(Ty::Error, |e| self.expr(e));
                self.try_ty(&operand, span)
            }
            Expr::Cast(cast) => {
                let inner = cast.inner().map_or(Ty::Error, |e| self.expr(e));
                let target = cast.ty().map_or(Ty::Error, |ty| self.lower_type(ty));
                let applied = self.icx.apply(&inner);
                let valid = matches!(applied, Ty::Var(_) | Ty::Error | Ty::Never)
                    || (applied.is_numeric() && target.is_numeric())
                    || applied == target;
                if !valid {
                    let from = self.render(&inner);
                    let to = self.render(&target);
                    self.push(
                        Diagnostic::error(
                            code(8),
                            format!("invalid cast from `{from}` to `{to}`"),
                            span,
                        )
                        .with_note("`as` converts only between numeric types")
                        .with_expected(StructuredValue::Type(to))
                        .with_actual(StructuredValue::Type(from)),
                    );
                    return Ty::Error;
                }
                target
            }
            Expr::Group(group) => group.inner().map_or(Ty::Unit, |inner| self.expr(inner)),
            Expr::If(if_expr) => self.if_expr(if_expr, span),
            Expr::Match(match_expr) => self.match_expr(match_expr, span),
            Expr::While(while_expr) => {
                if let Some(condition) = while_expr.condition() {
                    let condition_span = self.at(value_span(&condition));
                    let ty = self.expr(condition);
                    self.expect_ty(&Ty::Bool, &ty, condition_span);
                }
                self.frames.push(Frame { break_ty: Ty::Unit });
                if let Some(body) = while_expr.body() {
                    let body_span = self.at(body.span());
                    let ty = self.block(body);
                    self.expect_ty(&Ty::Unit, &ty, body_span);
                }
                self.frames.pop();
                Ty::Unit
            }
            Expr::Loop(loop_expr) => {
                let break_ty = self.fresh(span);
                self.frames.push(Frame {
                    break_ty: break_ty.clone(),
                });
                if let Some(body) = loop_expr.body() {
                    self.block(body);
                }
                self.frames.pop();
                // A loop's value is what its breaks yield; with no `break`
                // the variable stays free and the loop diverges — but that
                // free variable must not demand an annotation.
                let applied = self.icx.apply(&break_ty);
                if matches!(applied, Ty::Var(_)) {
                    let _ = self.icx.unify(&break_ty, &Ty::Never);
                    Ty::Never
                } else {
                    break_ty
                }
            }
            Expr::For(for_expr) => {
                let elem = for_expr.iterable().map_or(Ty::Error, |iterable| {
                    let iterable_span = self.at(value_span(&iterable));
                    let ty = self.expr(iterable);
                    self.element_ty(&ty, iterable_span)
                });
                if let Some(pattern) = for_expr.pattern() {
                    self.pattern(pattern, &elem);
                }
                self.frames.push(Frame { break_ty: Ty::Unit });
                if let Some(body) = for_expr.body() {
                    let body_span = self.at(body.span());
                    let ty = self.block(body);
                    self.expect_ty(&Ty::Unit, &ty, body_span);
                }
                self.frames.pop();
                Ty::Unit
            }
            Expr::Unsafe(unsafe_expr) => {
                unsafe_expr.body().map_or(Ty::Unit, |body| self.block(body))
            }
            Expr::Return(ret_expr) => {
                let value_at = ret_expr
                    .value()
                    .map_or(span, |value| self.at(value_span(&value)));
                let actual = ret_expr.value().map_or(Ty::Unit, |value| self.expr(value));
                let expected = self.ret.clone();
                self.expect_ty(&expected, &actual, value_at);
                Ty::Never
            }
            Expr::Break(break_expr) => {
                let value_at = break_expr
                    .value()
                    .map_or(span, |value| self.at(value_span(&value)));
                let actual = break_expr
                    .value()
                    .map_or(Ty::Unit, |value| self.expr(value));
                match self.frames.last() {
                    Some(frame) => {
                        let break_ty = frame.break_ty.clone();
                        self.expect_ty(&break_ty, &actual, value_at);
                    }
                    None => self.outside_loop("break", span),
                }
                Ty::Never
            }
            Expr::Continue(_) => {
                if self.frames.is_empty() {
                    self.outside_loop("continue", span);
                }
                Ty::Never
            }
            Expr::Block(block) => self.block(block),
        }
    }

    fn outside_loop(&mut self, what: &str, span: Span) {
        self.push(
            Diagnostic::error(code(13), format!("`{what}` outside of a loop"), span)
                .with_primary_label("no enclosing loop"),
        );
    }

    fn unsupported_op(&mut self, op: &str, ty: &Ty, span: Span) {
        let rendered = self.render(ty);
        self.push(
            Diagnostic::error(
                code(6),
                format!("operator `{op}` is not supported for type `{rendered}`"),
                span,
            )
            .with_actual(StructuredValue::Type(rendered)),
        );
    }

    fn binary_op(&mut self, op: &str, lhs: &Ty, rhs: &Ty, span: Span, rhs_span: Span) -> Ty {
        match op {
            "+" | "-" | "*" | "/" | "%" => {
                self.expect_ty(lhs, rhs, rhs_span);
                let applied = self.icx.apply(lhs);
                match applied {
                    Ty::Int(_) | Ty::Float(_) | Ty::Var(_) | Ty::Error | Ty::Never => {
                        // Constrain a free variable to "some integer" only
                        // when nothing else has: arithmetic defaults follow
                        // the literal rules.
                        lhs.clone()
                    }
                    other => {
                        self.unsupported_op(op, &other, span);
                        Ty::Error
                    }
                }
            }
            "==" | "!=" => {
                self.expect_ty(lhs, rhs, rhs_span);
                Ty::Bool
            }
            "<" | "<=" | ">" | ">=" => {
                self.expect_ty(lhs, rhs, rhs_span);
                let applied = self.icx.apply(lhs);
                match applied {
                    Ty::Int(_) | Ty::Float(_) | Ty::Char | Ty::Var(_) | Ty::Error | Ty::Never => {
                        Ty::Bool
                    }
                    other => {
                        self.unsupported_op(op, &other, span);
                        Ty::Bool
                    }
                }
            }
            "&&" | "||" => {
                self.expect_ty(&Ty::Bool, lhs, span);
                self.expect_ty(&Ty::Bool, rhs, rhs_span);
                Ty::Bool
            }
            _ => Ty::Error,
        }
    }

    fn field_ty(&mut self, receiver: &Ty, field: &str, span: Span) -> Ty {
        let applied = self.icx.apply(receiver);
        match &applied {
            Ty::Struct(symbol, args) => {
                let Some(def) = self.structs.get(symbol).cloned() else {
                    return Ty::Error;
                };
                let map = substitution(&def.type_params, args);
                match def.fields.iter().find(|(name, _)| name == field) {
                    Some((_, ty)) => substitute(ty, &map),
                    None => {
                        self.unknown_field(field, &applied, span);
                        Ty::Error
                    }
                }
            }
            Ty::Tuple(items) => match field.parse::<usize>().ok().and_then(|i| items.get(i)) {
                Some(ty) => ty.clone(),
                None => {
                    self.unknown_field(field, &applied, span);
                    Ty::Error
                }
            },
            Ty::Error | Ty::Var(_) | Ty::Never => Ty::Error,
            _ => {
                self.unknown_field(field, &applied, span);
                Ty::Error
            }
        }
    }

    fn try_ty(&mut self, operand: &Ty, span: Span) -> Ty {
        // Return types are always written explicitly, so the enclosing
        // function's fallibility can be checked by shape.
        match self.icx.apply(operand) {
            Ty::Option(item) => {
                if !matches!(self.icx.apply(&self.ret.clone()), Ty::Option(_)) {
                    let ret_rendered = self.render(&self.ret.clone());
                    self.push(
                        Diagnostic::error(
                            code(9),
                            "`?` on an `Option` requires the function to return `Option`",
                            span,
                        )
                        .with_actual(StructuredValue::Type(ret_rendered)),
                    );
                }
                *item
            }
            Ty::Result(ok, err) => {
                match self.icx.apply(&self.ret.clone()) {
                    Ty::Result(_, ret_err) => self.expect_ty(&ret_err, &err, span),
                    _ => {
                        let ret_rendered = self.render(&self.ret.clone());
                        self.push(
                            Diagnostic::error(
                                code(9),
                                "`?` on a `Result` requires the function to return `Result` \
                                 with the same error type",
                                span,
                            )
                            .with_actual(StructuredValue::Type(ret_rendered)),
                        );
                    }
                }
                *ok
            }
            Ty::Error | Ty::Var(_) | Ty::Never => Ty::Error,
            other => {
                let rendered = other.render(self.resolution);
                self.push(
                    Diagnostic::error(
                        code(9),
                        format!(
                            "`?` applied to `{rendered}`, which is neither `Option` nor `Result`"
                        ),
                        span,
                    )
                    .with_actual(StructuredValue::Type(rendered)),
                );
                Ty::Error
            }
        }
    }

    fn element_ty(&mut self, iterable: &Ty, span: Span) -> Ty {
        match self.icx.apply(iterable) {
            Ty::Range(item) | Ty::Array(item) => *item,
            Ty::Error | Ty::Var(_) | Ty::Never => Ty::Error,
            other => {
                let rendered = other.render(self.resolution);
                self.push(
                    Diagnostic::error(code(6), format!("type `{rendered}` is not iterable"), span)
                        .with_actual(StructuredValue::Type(rendered))
                        .with_note("`for` iterates ranges (`a .. b`) and arrays in v0"),
                );
                Ty::Error
            }
        }
    }

    fn if_expr(&mut self, if_expr: tuo_ast::IfExpr<'_>, span: Span) -> Ty {
        if let Some(condition) = if_expr.condition() {
            let condition_span = self.at(value_span(&condition));
            let ty = self.expr(condition);
            self.expect_ty(&Ty::Bool, &ty, condition_span);
        }
        let then_ty = if_expr
            .then_block()
            .map_or(Ty::Unit, |block| self.block(block));
        match if_expr.else_branch() {
            None => {
                // Without an `else`, the whole expression is `()`, so the
                // `then` block must be too (or diverge).
                let then_span = if_expr
                    .then_block()
                    .and_then(|block| block.span())
                    .unwrap_or(span);
                self.expect_ty(&Ty::Unit, &then_ty, then_span);
                Ty::Unit
            }
            Some(branch) => {
                let (else_ty, else_span) = match branch {
                    ElseBranch::If(nested) => {
                        let nested_span = self.at(nested.span());
                        (self.if_expr(nested, nested_span), nested_span)
                    }
                    ElseBranch::Block(block) => {
                        let block_span = self.at(block.span());
                        (self.block(block), block_span)
                    }
                };
                self.join(then_ty, else_ty, else_span)
            }
        }
    }

    fn match_expr(&mut self, match_expr: tuo_ast::MatchExpr<'_>, span: Span) -> Ty {
        let scrutinee_span = match_expr
            .scrutinee()
            .map_or(span, |scrutinee| self.at(value_span(&scrutinee)));
        let scrutinee = match_expr
            .scrutinee()
            .map_or(Ty::Error, |scrutinee| self.expr(scrutinee));
        let mut result = Ty::Never;
        let mut coverage = Coverage::default();
        for arm in match_expr.arms() {
            if let Some(pattern) = arm.pattern() {
                self.pattern(pattern, &scrutinee);
                if arm.guard().is_none() {
                    self.cover(&pattern, &mut coverage);
                }
            }
            if let Some(guard) = arm.guard() {
                let guard_span = self.at(value_span(&guard));
                let ty = self.expr(guard);
                self.expect_ty(&Ty::Bool, &ty, guard_span);
            }
            if let Some(value) = arm.value() {
                let value_at = self.at(value_span(&value));
                let ty = self.expr(value);
                result = self.join(result, ty, value_at);
            }
        }
        self.check_exhaustiveness(&scrutinee, &coverage, scrutinee_span);
        result
    }

    /// Record what an unguarded arm's pattern covers.
    fn cover(&self, pattern: &Pattern<'_>, coverage: &mut Coverage) {
        match pattern {
            Pattern::Wildcard(_) | Pattern::Binding(_) => coverage.catchall = true,
            Pattern::Literal(literal) => {
                let text = literal.text();
                if text == "true" {
                    coverage.true_seen = true;
                } else if text == "false" {
                    coverage.false_seen = true;
                }
            }
            Pattern::Path(path) => {
                if let Some(symbol) = self.symbol_at(path.segment_names().last()) {
                    coverage.variants.push(symbol);
                }
            }
            Pattern::Or(or) => {
                for alternative in or.alternatives() {
                    self.cover(&alternative, coverage);
                }
            }
            Pattern::Group(group) => {
                if let Some(inner) = group.inner() {
                    self.cover(&inner, coverage);
                }
            }
        }
    }

    fn check_exhaustiveness(&mut self, scrutinee: &Ty, coverage: &Coverage, span: Span) {
        if coverage.catchall {
            return;
        }
        let applied = self.icx.apply(scrutinee);
        let missing: Vec<String> = match &applied {
            Ty::Enum(symbol, _) => self
                .resolution
                .variants_of(*symbol)
                .iter()
                .filter(|variant| !coverage.variants.contains(variant))
                .map(|variant| self.resolution.symbol(*variant).name.clone())
                .collect(),
            Ty::Option(_) | Ty::Result(_, _) => {
                let names: &[&str] = if matches!(applied, Ty::Option(_)) {
                    &["Some", "None"]
                } else {
                    &["Ok", "Err"]
                };
                names
                    .iter()
                    .copied()
                    .filter(|name| {
                        self.prelude(name)
                            .is_none_or(|symbol| !coverage.variants.contains(&symbol))
                    })
                    .map(ToOwned::to_owned)
                    .collect()
            }
            Ty::Bool => {
                let mut missing = Vec::new();
                if !coverage.true_seen {
                    missing.push("true".to_owned());
                }
                if !coverage.false_seen {
                    missing.push("false".to_owned());
                }
                missing
            }
            Ty::Error | Ty::Var(_) | Ty::Never => return,
            _ => {
                let rendered = self.render(&applied);
                self.push(
                    Diagnostic::error(
                        code(7),
                        format!("non-exhaustive match on `{rendered}`"),
                        span,
                    )
                    .with_primary_label("not all values are covered")
                    .with_help("add a `_` arm to cover the remaining values"),
                );
                return;
            }
        };
        if missing.is_empty() {
            return;
        }
        let rendered = self.render(&applied);
        let mut diagnostic = Diagnostic::error(
            code(7),
            format!(
                "non-exhaustive match on `{rendered}`: missing {}",
                missing
                    .iter()
                    .map(|name| format!("`{name}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            span,
        )
        .with_help("cover every variant, or add a `_` arm");
        for name in missing {
            diagnostic = diagnostic.with_expected(StructuredValue::Name(name));
        }
        self.push(diagnostic);
    }

    // ------------------------------------------------------------------
    // Paths and calls
    // ------------------------------------------------------------------

    fn path_value(&mut self, path: tuo_ast::PathExpr<'_>) -> Ty {
        let span = self.at(path.span());
        let Some(symbol) = self.symbol_at(path.segment_names().last()) else {
            return Ty::Error;
        };
        let given_args: Option<Vec<Ty>> = path
            .turbofish()
            .map(|args| args.types().map(|ty| self.lower_type(ty)).collect());
        match self.resolution.symbol(symbol).kind {
            SymbolKind::Param | SymbolKind::Local => {
                self.locals.get(&symbol).cloned().unwrap_or(Ty::Error)
            }
            SymbolKind::Const => self.consts.get(&symbol).cloned().unwrap_or(Ty::Error),
            SymbolKind::Function => {
                let (params, ret) = self.instantiate_fn(symbol, given_args, span);
                Ty::Fn(Box::new(FnTy { params, ret }))
            }
            SymbolKind::Variant => {
                let Some(shape) = self.constructor_shape(symbol, given_args, span) else {
                    return Ty::Error;
                };
                if shape.fields.is_empty() {
                    shape.ty
                } else {
                    let name = self.resolution.symbol(symbol).name.clone();
                    self.push(
                        Diagnostic::error(
                            code(12),
                            format!(
                                "variant `{name}` has a payload; construct it with `{name} {{ … }}`"
                            ),
                            span,
                        )
                        .with_actual(StructuredValue::Name(name)),
                    );
                    Ty::Error
                }
            }
            kind => {
                let noun = kind.noun();
                let name = self.resolution.symbol(symbol).name.clone();
                self.push(
                    Diagnostic::error(
                        code(12),
                        format!("expected a value, found {noun} `{name}`"),
                        span,
                    )
                    .with_actual(StructuredValue::Name(name)),
                );
                Ty::Error
            }
        }
    }

    /// Instantiate a function signature: explicit turbofish arguments pin
    /// the type parameters, otherwise fresh variables are solved from the
    /// call.
    fn instantiate_fn(
        &mut self,
        symbol: SymbolId,
        given_args: Option<Vec<Ty>>,
        span: Span,
    ) -> (Vec<Ty>, Ty) {
        let Some(sig) = self.fns.get(&symbol).cloned() else {
            return (Vec::new(), Ty::Error);
        };
        let args = match given_args {
            Some(args) => {
                if !self.check_type_arity(
                    &self.resolution.symbol(symbol).name.clone(),
                    sig.type_params.len(),
                    args.len(),
                    span,
                ) {
                    return (sig.params.clone(), Ty::Error);
                }
                args
            }
            None => sig.type_params.iter().map(|_| self.fresh(span)).collect(),
        };
        let map = substitution(&sig.type_params, &args);
        (
            sig.params.iter().map(|ty| substitute(ty, &map)).collect(),
            substitute(&sig.ret, &map),
        )
    }

    fn call(&mut self, call: tuo_ast::CallExpr<'_>, span: Span) -> Ty {
        let args: Vec<Expr<'_>> = call.args().collect();
        // Direct calls of a named function keep the callee out of
        // value position (no function-type detour, better diagnostics).
        if let Some(Expr::Path(path)) = call.callee() {
            if let Some(symbol) = self.symbol_at(path.segment_names().last()) {
                if self.resolution.symbol(symbol).kind == SymbolKind::Function {
                    let given_args: Option<Vec<Ty>> = path
                        .turbofish()
                        .map(|list| list.types().map(|ty| self.lower_type(ty)).collect());
                    let (params, ret) = self.instantiate_fn(symbol, given_args, span);
                    self.check_args(&params, &args, span);
                    return ret;
                }
            }
        }
        let callee_ty = call.callee().map_or(Ty::Error, |callee| self.expr(callee));
        match self.icx.apply(&callee_ty) {
            Ty::Fn(fn_ty) => {
                self.check_args(&fn_ty.params, &args, span);
                fn_ty.ret
            }
            Ty::Var(_) if self.icx.literal_class(&callee_ty).is_some() => {
                // A literal variable is known to be numeric — never a
                // function — even before it defaults.
                let rendered = match self.icx.literal_class(&callee_ty) {
                    Some(VarClass::Float) => "{float}",
                    _ => "{integer}",
                };
                self.push(
                    Diagnostic::error(
                        code(3),
                        format!("expected a function, found `{rendered}`"),
                        span,
                    )
                    .with_actual(StructuredValue::Type(rendered.to_owned())),
                );
                for arg in args {
                    self.expr(arg);
                }
                Ty::Error
            }
            Ty::Error | Ty::Var(_) | Ty::Never => {
                for arg in args {
                    self.expr(arg);
                }
                Ty::Error
            }
            other => {
                let rendered = self.render(&other);
                self.push(
                    Diagnostic::error(
                        code(3),
                        format!("expected a function, found `{rendered}`"),
                        span,
                    )
                    .with_actual(StructuredValue::Type(rendered)),
                );
                for arg in args {
                    self.expr(arg);
                }
                Ty::Error
            }
        }
    }

    fn check_args(&mut self, params: &[Ty], args: &[Expr<'_>], span: Span) {
        if params.len() != args.len() {
            self.push(
                Diagnostic::error(
                    code(2),
                    format!(
                        "this call takes {} argument{} but {} {} supplied",
                        params.len(),
                        if params.len() == 1 { "" } else { "s" },
                        args.len(),
                        if args.len() == 1 { "was" } else { "were" },
                    ),
                    span,
                )
                .with_expected(StructuredValue::Count(params.len() as u64))
                .with_actual(StructuredValue::Count(args.len() as u64)),
            );
        }
        for (param, arg) in params.iter().zip(args.iter()) {
            let arg_span = self.at(value_span(arg));
            let actual = self.expr(*arg);
            self.expect_ty(param, &actual, arg_span);
        }
        // Extra arguments still get checked for their own errors.
        for arg in args.iter().skip(params.len()) {
            self.expr(*arg);
        }
    }
}

/// The constructed type and field list of a struct or variant constructor.
struct Shape {
    ty: Ty,
    fields: Vec<(String, Ty)>,
}

/// Coverage gathered from a match's unguarded arms.
#[derive(Default)]
struct Coverage {
    catchall: bool,
    variants: Vec<SymbolId>,
    true_seen: bool,
    false_seen: bool,
}

fn substitution(params: &[SymbolId], args: &[Ty]) -> HashMap<SymbolId, Ty> {
    params.iter().copied().zip(args.iter().cloned()).collect()
}

/// Replace `Ty::Param`s per `map` (identity for unmapped params).
fn substitute(ty: &Ty, map: &HashMap<SymbolId, Ty>) -> Ty {
    match ty {
        Ty::Param(symbol) => map.get(symbol).cloned().unwrap_or_else(|| ty.clone()),
        Ty::Tuple(items) => Ty::Tuple(items.iter().map(|item| substitute(item, map)).collect()),
        Ty::Array(item) => Ty::Array(Box::new(substitute(item, map))),
        Ty::Range(item) => Ty::Range(Box::new(substitute(item, map))),
        Ty::Fn(fn_ty) => Ty::Fn(Box::new(FnTy {
            params: fn_ty
                .params
                .iter()
                .map(|param| substitute(param, map))
                .collect(),
            ret: substitute(&fn_ty.ret, map),
        })),
        Ty::Option(item) => Ty::Option(Box::new(substitute(item, map))),
        Ty::Result(ok, err) => Ty::Result(
            Box::new(substitute(ok, map)),
            Box::new(substitute(err, map)),
        ),
        Ty::Struct(symbol, args) => Ty::Struct(
            *symbol,
            args.iter().map(|arg| substitute(arg, map)).collect(),
        ),
        Ty::Enum(symbol, args) => Ty::Enum(
            *symbol,
            args.iter().map(|arg| substitute(arg, map)).collect(),
        ),
        Ty::Wrapper(kind, item) => Ty::Wrapper(*kind, Box::new(substitute(item, map))),
        other => other.clone(),
    }
}

/// The span of an expression view (every well-formed expression covers at
/// least one token).
fn value_span(expr: &Expr<'_>) -> Option<Span> {
    expr.span()
}
