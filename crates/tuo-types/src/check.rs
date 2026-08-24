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

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use tuo_ast::{
    ArrayLiteralExpr, ArrayLiteralKind, Ast, BindingStmt, Block, ElseBranch, Expr, FnDecl, Item,
    LiteralPat, Name, Pattern, SpecDecl, SpecStatement, Statement, StructLiteralExpr, TypeRef,
};
use tuo_diagnostics::{Diagnostic, DiagnosticCode, Namespace, StructuredValue};
use tuo_lexer::TokenKind;
use tuo_resolve::{Builtin, BuiltinParamMode, Resolution, SymbolId, SymbolKind};
use tuo_source::Span;

use crate::TypeckResult;
use crate::infer::{InferCtx, VarClass};
use crate::ty::{
    FloatKind, FnParam, FnTy, IntKind, MAX_FIXED_ARRAY_LEN, ParamMode, Ty, WrapperKind,
};

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
/// - `T0014` — invalid fixed-size array length (suffixed, unparsable,
///   or over [`MAX_FIXED_ARRAY_LEN`]).
/// - `T0015` — not first-class: a builtin or generic function used as a
///   value (ADR-0008 Tier 1). Only ordinary top-level user `fn`s are
///   first-class function values.
/// - `T0016` — recursive type without a heap-wrapper indirection: a struct
///   or enum reaches itself through its own fields/payloads (by value or
///   through `Array`/`Map`/`Option`/`Result`/tuple/fixed-array components),
///   which v0 cannot represent — only a `Box`/`Shared`/`Weak` indirection
///   breaks a cycle (ADR-0016).
fn code(number: u16) -> DiagnosticCode {
    DiagnosticCode::new(Namespace::Type, number)
}

/// The spec-purity diagnostic (ADR-0006 Stage A): `R0007`, a spec whose
/// executed closure reaches an effectful function. It lives in the `Rxxxx`
/// namespace because it belongs to the spec-semantics rule family `R0006`
/// opened (what a `spec` may name or reach), even though the effect
/// discipline it depends on is computed here in the type-checking stage —
/// see `specification/static-semantics.md` §2.3 and §3.6.
fn spec_purity_code() -> DiagnosticCode {
    DiagnosticCode::new(Namespace::Resolution, 7)
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
    /// Per-parameter passing modes (ADR-0008 Tier 1), parallel to `params` —
    /// carried so a function's value type (`Ty::Fn`) spells its modes, which
    /// are part of exact function-type equality and drive an indirect call's
    /// borrows.
    modes: Vec<ParamMode>,
    ret: Ty,
}

impl FnSig {
    /// This signature as a function type: mode-carrying params + return.
    fn as_fn_ty(&self) -> FnTy {
        FnTy {
            params: self
                .params
                .iter()
                .cloned()
                .zip(
                    self.modes
                        .iter()
                        .copied()
                        .chain(std::iter::repeat(ParamMode::In)),
                )
                .map(|(ty, mode)| FnParam { mode, ty })
                .collect(),
            ret: self.ret.clone(),
        }
    }
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
    expr_types: HashMap<Span, Ty>,
    /// Call-graph edges for the purity computation (ADR-0006): enclosing
    /// function → every function it calls *or references as a value*
    /// (conservative — a reference taints like a call). `BTreeMap`/`BTreeSet`
    /// keep the fixed point and the spec gate deterministic.
    call_edges: BTreeMap<SymbolId, BTreeSet<SymbolId>>,
    // Per-body state.
    icx: InferCtx,
    /// Types of the current body's expressions, recorded raw (inference
    /// variables unresolved) and published — applied — by [`finish_body`].
    body_exprs: Vec<(Span, Ty)>,
    locals: HashMap<SymbolId, Ty>,
    ret: Ty,
    frames: Vec<Frame>,
    fallback: Span,
    /// The module-level function whose body is being checked (`None` in
    /// consts, specs, and impl bodies) — the source node of recorded
    /// [`Checker::call_edges`].
    current_fn: Option<SymbolId>,
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
        expr_types: HashMap::new(),
        icx: InferCtx::default(),
        body_exprs: Vec::new(),
        locals: HashMap::new(),
        ret: Ty::Unit,
        frames: Vec::new(),
        fallback,
        current_fn: None,
        call_edges: BTreeMap::new(),
    };
    checker.install_builtin_signatures();
    for ast in files {
        checker.collect_headers(*ast);
    }
    for ast in files {
        checker.collect_signatures(*ast);
    }
    checker.check_recursion_boundary();
    for ast in files {
        checker.check_file(*ast);
    }
    let effectful = checker.compute_effectful();
    checker.check_spec_purity(&effectful);
    let struct_shapes = checker
        .structs
        .iter()
        .map(|(&symbol, def)| {
            (
                symbol,
                crate::StructShape {
                    type_params: def.type_params.clone(),
                    fields: def.fields.clone(),
                },
            )
        })
        .collect();
    let enum_shapes = checker
        .enums
        .iter()
        .map(|(&symbol, def)| {
            (
                symbol,
                crate::EnumShape {
                    type_params: def.type_params.clone(),
                    variants: def.variants.clone(),
                },
            )
        })
        .collect();
    TypeckResult {
        diagnostics: checker.diagnostics,
        symbol_types: checker.symbol_types,
        expr_types: checker.expr_types,
        struct_shapes,
        enum_shapes,
        effectful,
    }
}

/// The fixed signature `(params, ret)` of a builtin function
/// (`specification/static-semantics.md` §3.6–§3.7). `Int` is `I64`.
///
/// The growable-`Array` builtins are **generic over the element type** since
/// ADR-0012 and are checked by [`Checker::check_array_builtin_call`], which
/// intercepts them before the fixed path. The `Array[Int]` entries kept here are
/// only the seed installed into `symbol_types`/`fns` (arity + a monomorphic
/// fallback); every real call resolves its element type from the receiver, so
/// these `Int` shapes are never the type a widened call is checked against.
fn builtin_signature(builtin: Builtin) -> (Vec<Ty>, Ty) {
    let int_array = || Ty::Array(Box::new(Ty::int()));
    let int_map = || Ty::Map(Box::new(Ty::int()), Box::new(Ty::int()));
    match builtin {
        Builtin::RtWrite => (vec![Ty::int(), Ty::Str], Ty::int()),
        Builtin::RtReadByte | Builtin::RtExit => (vec![Ty::int()], Ty::int()),
        Builtin::RtWriteString => (vec![Ty::int(), Ty::String], Ty::int()),
        // ADR-0007: the structured fork-join primitive. The task function is
        // a non-capturing `fn(take Int) -> Int` value (ADR-0008 Tier 1) — the
        // only callable that may cross a thread boundary in v0.
        Builtin::RtParMap => (
            vec![
                Ty::Fn(Box::new(FnTy {
                    params: vec![FnParam {
                        mode: ParamMode::Take,
                        ty: Ty::int(),
                    }],
                    ret: Ty::int(),
                })),
                Ty::Array(Box::new(Ty::int())),
                Ty::int(),
            ],
            Ty::Array(Box::new(Ty::int())),
        ),
        // ADR-0013: the OS effect boundary — clock, argv, and file
        // open/close/remove. All scalar-or-`Str` in, `Int` out; errors are
        // negative return values, never traps.
        Builtin::RtNowNanos | Builtin::RtArgCount => (Vec::new(), Ty::int()),
        Builtin::RtArgByte => (vec![Ty::int(), Ty::int()], Ty::int()),
        Builtin::RtOpen => (vec![Ty::Str, Ty::int()], Ty::int()),
        Builtin::RtClose => (vec![Ty::int()], Ty::int()),
        Builtin::RtRemoveFile => (vec![Ty::Str], Ty::int()),
        // ADR-0014: socket effects — descriptor producers over the same
        // seam; `write`/`read_byte`/`close` move the bytes and release them.
        Builtin::RtListen | Builtin::RtBoundPort | Builtin::RtAccept => {
            (vec![Ty::int()], Ty::int())
        }
        Builtin::RtConnect => (vec![Ty::Str, Ty::int()], Ty::int()),
        // ADR-0015: channels and mutexes — runtime-owned objects behind
        // opaque `Int` handles, the same shape as a descriptor.
        Builtin::RtChanNew | Builtin::RtMutexNew => (Vec::new(), Ty::int()),
        Builtin::RtChanSend => (vec![Ty::int(), Ty::int()], Ty::int()),
        Builtin::RtChanRecv
        | Builtin::RtChanClose
        | Builtin::RtMutexLock
        | Builtin::RtMutexUnlock => (vec![Ty::int()], Ty::int()),
        Builtin::StrLen => (vec![Ty::Str], Ty::int()),
        Builtin::StrByteAt => (vec![Ty::Str, Ty::int()], Ty::int()),
        Builtin::StrSlice => (vec![Ty::Str, Ty::int(), Ty::int()], Ty::Str),
        Builtin::StringEmpty => (Vec::new(), Ty::String),
        Builtin::StringFromStr => (vec![Ty::Str], Ty::String),
        Builtin::StringPushByte => (vec![Ty::String, Ty::int()], Ty::Unit),
        Builtin::StringAppend => (vec![Ty::String, Ty::Str], Ty::Unit),
        Builtin::StringConcat => (vec![Ty::Str, Ty::Str], Ty::String),
        Builtin::StringLen => (vec![Ty::String], Ty::int()),
        Builtin::StringByteAt => (vec![Ty::String, Ty::int()], Ty::int()),
        Builtin::StringSlice => (vec![Ty::String, Ty::int(), Ty::int()], Ty::String),
        Builtin::StringAsStr => (vec![Ty::String], Ty::Str),
        Builtin::ArrayEmpty => (Vec::new(), int_array()),
        Builtin::ArrayPush => (vec![int_array(), Ty::int()], Ty::Unit),
        Builtin::ArraySet => (vec![int_array(), Ty::int(), Ty::int()], Ty::Unit),
        Builtin::ArrayPop => (vec![int_array()], Ty::Option(Box::new(Ty::int()))),
        Builtin::ArrayLen => (vec![int_array()], Ty::int()),
        Builtin::ArrayGet => (vec![int_array(), Ty::int()], Ty::int()),
        // The `std::map` builtins are key/value-parametric (ADR-0011) and are
        // checked by `check_map_builtin_call`; these monomorphic shapes are
        // only the installed seed, exactly like the `Array[Int]` entries above.
        Builtin::MapEmpty => (Vec::new(), int_map()),
        Builtin::MapInsert => (
            vec![int_map(), Ty::int(), Ty::int()],
            Ty::Option(Box::new(Ty::int())),
        ),
        Builtin::MapGet => (vec![int_map(), Ty::int()], Ty::Option(Box::new(Ty::int()))),
        Builtin::MapContainsKey => (vec![int_map(), Ty::int()], Ty::Bool),
        Builtin::MapRemove => (vec![int_map(), Ty::int()], Ty::Option(Box::new(Ty::int()))),
        Builtin::MapLen => (vec![int_map()], Ty::int()),
        Builtin::MapKeys => (vec![int_map()], Ty::Array(Box::new(Ty::int()))),
    }
}

/// The fully qualified display name of a module-level symbol
/// (`std::rt::write`; a root-module symbol is just its name).
fn qualified_name(resolution: &Resolution, symbol: SymbolId) -> String {
    let data = resolution.symbol(symbol);
    let path = &resolution.module(data.module).path;
    if path.is_empty() {
        data.name.clone()
    } else {
        format!("{}::{}", path.join("::"), data.name)
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
    // Builtin functions and the effect system (ADR-0006 Stage A)
    // ------------------------------------------------------------------

    /// Seed the fixed signatures of the six builtin functions the resolver
    /// installed (`std::rt`/`std::str`, `specification/static-semantics.md`
    /// §3.6), so calls to them are checked exactly like calls to declared
    /// functions (arity `T0002`, argument types `T0001`).
    fn install_builtin_signatures(&mut self) {
        for (builtin, symbol) in self.resolution.builtins() {
            let (params, ret) = builtin_signature(builtin);
            let modes = builtin
                .param_modes()
                .iter()
                .map(|mode| match mode {
                    BuiltinParamMode::Take => ParamMode::Take,
                    BuiltinParamMode::In => ParamMode::In,
                    BuiltinParamMode::Mut => ParamMode::Mut,
                })
                .collect();
            let sig = FnSig {
                type_params: Vec::new(),
                params,
                modes,
                ret,
            };
            self.symbol_types
                .insert(symbol, Ty::Fn(Box::new(sig.as_fn_ty())));
            self.fns.insert(symbol, sig);
        }
    }

    /// Record a call-graph edge from the enclosing function to `callee`,
    /// for the transitive purity computation. Called for direct calls and
    /// for value references to a function alike — a reference taints
    /// conservatively, which can never wrongly accept a spec.
    fn record_call_edge(&mut self, callee: SymbolId) {
        if let Some(caller) = self.current_fn {
            self.call_edges.entry(caller).or_default().insert(callee);
        }
    }

    /// Compute the set of **effectful** functions: the `std::rt` effect
    /// builtins plus every function that transitively reaches one through
    /// [`Self::call_edges`] — a fixed point over the call graph (cycles
    /// converge: a recursive cluster is effectful iff some member reaches
    /// an effect).
    fn compute_effectful(&self) -> BTreeSet<SymbolId> {
        let mut effectful: BTreeSet<SymbolId> = self
            .resolution
            .builtins()
            .filter(|(builtin, _)| builtin.is_effect())
            .map(|(_, symbol)| symbol)
            .collect();
        loop {
            let mut changed = false;
            for (&caller, callees) in &self.call_edges {
                if !effectful.contains(&caller)
                    && callees.iter().any(|callee| effectful.contains(callee))
                {
                    effectful.insert(caller);
                    changed = true;
                }
            }
            if !changed {
                return effectful;
            }
        }
    }

    /// The spec-purity gate (`R0007`, ADR-0006): a spec whose executed
    /// closure — its resolved dependencies transitively closed over the
    /// call graph — includes an effectful function is a front-end error,
    /// so `tuo check`/`spec`/`verify` refuse it before the interpreter is
    /// ever involved. `main` and ordinary functions may be effectful; only
    /// specs are restricted.
    fn check_spec_purity(&mut self, effectful: &BTreeSet<SymbolId>) {
        let mut specs: Vec<(Span, SymbolId)> = self.resolution.spec_symbols().collect();
        specs.sort_by_key(|(span, _)| (span.source(), span.range().start()));
        for (block_span, spec) in specs {
            let Some(reached) = self.first_effectful_reached(spec, effectful) else {
                continue;
            };
            let spec_name = self.resolution.symbol(spec).name.clone();
            let target = qualified_name(self.resolution, reached);
            let span = self
                .resolution
                .symbol(spec)
                .declaration
                .unwrap_or(block_span);
            let mut diagnostic = Diagnostic::error(
                spec_purity_code(),
                format!(
                    "spec `{spec_name}` reaches the effectful function `{target}`; specs \
                     execute only the pure core in the deterministic sandbox"
                ),
                span,
            )
            .with_primary_label("this spec's executed closure performs an effect")
            .with_help(
                "effects (`std::rt::write`/`read_byte`/`exit`/`write_string`) may run in \
                 `main` and ordinary functions, never in a spec",
            )
            .with_actual(StructuredValue::Name(target));
            if let Some(declaration) = self.resolution.symbol(reached).declaration {
                diagnostic = diagnostic
                    .with_secondary_label(declaration, "effectful function declared here");
            }
            self.push(diagnostic);
        }
    }

    /// Breadth-first search from `spec`'s resolved dependencies through the
    /// call graph for the first effectful function, in deterministic
    /// (first-use, then symbol-order) order.
    fn first_effectful_reached(
        &self,
        spec: SymbolId,
        effectful: &BTreeSet<SymbolId>,
    ) -> Option<SymbolId> {
        let mut queue: Vec<SymbolId> = self.resolution.dependencies_of(spec).to_vec();
        let mut seen: BTreeSet<SymbolId> = queue.iter().copied().collect();
        let mut cursor = 0;
        while let Some(&symbol) = queue.get(cursor) {
            cursor += 1;
            if effectful.contains(&symbol) {
                return Some(symbol);
            }
            if let Some(callees) = self.call_edges.get(&symbol) {
                for &callee in callees {
                    if seen.insert(callee) {
                        queue.push(callee);
                    }
                }
            }
        }
        None
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
                        self.symbol_types
                            .insert(id, Ty::Fn(Box::new(sig.as_fn_ty())));
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
        let modes = decl
            .params()
            .map(|param| mode_from_text(param.mode()))
            .collect();
        let ret = decl
            .return_type()
            .map_or(Ty::Unit, |ty| self.lower_type(ty));
        FnSig {
            type_params,
            params,
            modes,
            ret,
        }
    }

    // ------------------------------------------------------------------
    // The recursion boundary (ADR-0016, `T0016`)
    // ------------------------------------------------------------------

    /// Reject every struct/enum that reaches itself through its own fields
    /// or variant payloads without a heap-wrapper indirection (`T0016`).
    ///
    /// v0 cannot represent such a type: a by-value cycle has infinite size,
    /// and a cycle through `Array`/`Map` (heap-indirected but glue-walked)
    /// would recurse the native backends' inline clone/drop emission
    /// forever. Only `Box`/`Shared`/`Weak` break a cycle — their layout is
    /// a bare pointer and the backends refuse their *values* cleanly — so
    /// `Weak` back-edges (the shipped cyclic-shape declarations) stay
    /// legal. A successor ADR that gives the backends runtime-recursive
    /// glue relaxes this boundary; until then the check keeps an accepted
    /// program from ever hanging the compiler.
    fn check_recursion_boundary(&mut self) {
        let mut aggregates: Vec<SymbolId> = self
            .structs
            .keys()
            .copied()
            .chain(self.enums.keys().copied())
            .collect();
        aggregates.sort();
        for symbol in aggregates {
            let mut visited = HashSet::new();
            if self.reaches_by_value(symbol, symbol, &mut visited) {
                let (name, span) = self
                    .resolution
                    .symbols()
                    .find(|(id, _)| *id == symbol)
                    .map_or_else(
                        || ("<unknown>".to_string(), self.fallback),
                        |(_, declared)| {
                            (
                                declared.name.to_string(),
                                declared.declaration.unwrap_or(self.fallback),
                            )
                        },
                    );
                self.push(Diagnostic::error(
                    code(16),
                    format!(
                        "recursive type: `{name}` reaches itself through its own \
                         fields without a heap-wrapper indirection; v0 cannot \
                         represent this — break the cycle with a `Box`/`Shared`/\
                         `Weak` indirection, or flatten the recursion into an \
                         index arena (how `std::json` stores its tree)"
                    ),
                    span,
                ));
            }
        }
    }

    /// Does `from`'s field graph reach `target` without passing through a
    /// heap wrapper? `visited` keeps the walk linear (and terminating) over
    /// arbitrary declaration graphs.
    fn reaches_by_value(
        &self,
        from: SymbolId,
        target: SymbolId,
        visited: &mut HashSet<SymbolId>,
    ) -> bool {
        if !visited.insert(from) {
            return false;
        }
        let mut field_tys: Vec<Ty> = Vec::new();
        if let Some(def) = self.structs.get(&from) {
            field_tys.extend(def.fields.iter().map(|(_, ty)| ty.clone()));
        }
        if let Some(def) = self.enums.get(&from) {
            field_tys.extend(
                def.variants
                    .iter()
                    .flat_map(|(_, fields)| fields.iter().map(|(_, ty)| ty.clone())),
            );
        }
        field_tys
            .iter()
            .any(|ty| self.ty_reaches(ty, target, visited))
    }

    /// Does `ty` structurally contain `target` (or an aggregate whose field
    /// graph reaches it) without a heap-wrapper indirection in between?
    /// Type arguments are followed too, so a generic self-application
    /// (`struct A { w: Wrap[A] }`) is caught at the use edge.
    fn ty_reaches(&self, ty: &Ty, target: SymbolId, visited: &mut HashSet<SymbolId>) -> bool {
        match ty {
            Ty::Struct(symbol, targs) | Ty::Enum(symbol, targs) => {
                *symbol == target
                    || self.reaches_by_value(*symbol, target, visited)
                    || targs
                        .iter()
                        .any(|arg| self.ty_reaches(arg, target, visited))
            }
            // The one legal indirection: a wrapper's layout is a bare
            // pointer, so the cycle's size is finite and no glue descends.
            Ty::Wrapper(..) => false,
            Ty::Array(item) | Ty::FixedArray(item, _) | Ty::Range(item) | Ty::Option(item) => {
                self.ty_reaches(item, target, visited)
            }
            Ty::Map(key, value) => {
                self.ty_reaches(key, target, visited) || self.ty_reaches(value, target, visited)
            }
            Ty::Result(ok, err) => {
                self.ty_reaches(ok, target, visited) || self.ty_reaches(err, target, visited)
            }
            Ty::Tuple(items) => items
                .iter()
                .any(|item| self.ty_reaches(item, target, visited)),
            // Scalars, text, `Param` (substituted at the use edge via the
            // targ walk above), `Fn` (a `Copy` code pointer stores no
            // value), and inference/poison types close no cycle.
            _ => false,
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
            // `Array` stays the *growable* heap sequence; the fixed-size
            // `[T; N]` is structural syntax (`TypeRef::FixedArray`) and
            // never enters name resolution or this builtin table.
            "Array" => {
                if !self.check_type_arity("Array", 1, args.len(), span) {
                    return Some(Ty::Error);
                }
                return Some(Ty::Array(Box::new(
                    args.into_iter().next().unwrap_or(Ty::Error),
                )));
            }
            // `Map[K, V]` — the builtin hash map (ADR-0011). The type takes
            // any two arguments; the v0 *operation* surface is checked per
            // call (`check_map_builtin_call`).
            "Map" => {
                if !self.check_type_arity("Map", 2, args.len(), span) {
                    return Some(Ty::Error);
                }
                let mut args = args.into_iter();
                let key = args.next().unwrap_or(Ty::Error);
                let value = args.next().unwrap_or(Ty::Error);
                return Some(Ty::Map(Box::new(key), Box::new(value)));
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
            TypeRef::FixedArray(array) => {
                let element = array
                    .element()
                    .map_or(Ty::Error, |element| self.lower_type(element));
                let Some(len) = array.len() else {
                    return Ty::Error;
                };
                match self.fixed_array_len(len.text, len.span) {
                    Some(n) => Ty::FixedArray(Box::new(element), n),
                    None => Ty::Error,
                }
            }
            TypeRef::Fn(fn_ty) => {
                // `fn(mode T, …) -> R` (ADR-0008 Tier 1). Modes are part of
                // the type; an absent mode (recovery state) defaults to `in`.
                let params = fn_ty
                    .params()
                    .map(|param| FnParam {
                        mode: mode_from_text(param.mode),
                        ty: self.lower_type(param.ty),
                    })
                    .collect();
                let ret = fn_ty.ret().map_or(Ty::Unit, |ty| self.lower_type(ty));
                Ty::Fn(Box::new(FnTy { params, ret }))
            }
        }
    }

    /// Parse and validate the `INT_LITERAL` length of a `[T; N]` type or a
    /// `[x; N]` repeat literal (ADR-0004 Stage 2): decimal / `0x` / `0o` /
    /// `0b` with `_` separators, exactly the radix handling integer
    /// literals get, but with **no type suffix admitted**, and capped at
    /// [`MAX_FIXED_ARRAY_LEN`]. Reports `T0014` and returns `None` when
    /// invalid; non-negativity is structural (`-` cannot begin an
    /// `INT_LITERAL`).
    fn fixed_array_len(&mut self, text: &str, span: Span) -> Option<u64> {
        const INT_SUFFIXES: [&str; 10] = [
            "isize", "usize", "i16", "i32", "i64", "u16", "u32", "u64", "i8", "u8",
        ];
        if INT_SUFFIXES.iter().any(|suffix| text.ends_with(suffix)) {
            self.push(
                Diagnostic::error(
                    code(14),
                    "a fixed-size array length must be a plain integer literal",
                    span,
                )
                .with_primary_label("type suffix not allowed here")
                .with_actual(StructuredValue::Name(text.to_owned())),
            );
            return None;
        }
        let digits = text.replace('_', "");
        let parsed = if let Some(hex) = digits
            .strip_prefix("0x")
            .or_else(|| digits.strip_prefix("0X"))
        {
            u64::from_str_radix(hex, 16)
        } else if let Some(bin) = digits
            .strip_prefix("0b")
            .or_else(|| digits.strip_prefix("0B"))
        {
            u64::from_str_radix(bin, 2)
        } else if let Some(oct) = digits
            .strip_prefix("0o")
            .or_else(|| digits.strip_prefix("0O"))
        {
            u64::from_str_radix(oct, 8)
        } else {
            digits.parse()
        };
        let Ok(n) = parsed else {
            self.push(
                Diagnostic::error(
                    code(14),
                    format!("invalid fixed-size array length `{text}`"),
                    span,
                )
                .with_actual(StructuredValue::Name(text.to_owned())),
            );
            return None;
        };
        if n > MAX_FIXED_ARRAY_LEN {
            self.push(
                Diagnostic::error(
                    code(14),
                    format!(
                        "fixed-size array length {n} exceeds the maximum {MAX_FIXED_ARRAY_LEN}"
                    ),
                    span,
                )
                .with_expected(StructuredValue::Count(MAX_FIXED_ARRAY_LEN))
                .with_actual(StructuredValue::Count(n)),
            );
            return None;
        }
        Some(n)
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
        self.body_exprs = Vec::new();
        self.locals = HashMap::new();
        self.ret = ret;
        self.frames = Vec::new();
        self.fallback = fallback;
        self.current_fn = None;
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
        let exprs = std::mem::take(&mut self.body_exprs);
        for (span, ty) in exprs {
            let ty = self.icx.apply(&ty);
            self.expr_types.insert(span, ty);
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
        // The purity call graph records edges out of module-level functions
        // only (impl bodies have no module-callable symbol until the trait
        // system lands; consts and specs are handled through the resolver's
        // spec dependencies).
        self.current_fn = self
            .declared_at(decl.name_ref())
            .filter(|symbol| self.resolution.symbol(*symbol).kind == SymbolKind::Function);
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
                // Shorthand `{ y }` reads the local `y`; HIR expands it
                // to a path expression carrying the field-name span, so
                // record that span's type for downstream stages.
                let actual = self
                    .symbol_at(init.name_ref())
                    .and_then(|symbol| self.locals.get(&symbol).cloned())
                    .unwrap_or(Ty::Error);
                self.record(Some(name.span), &actual);
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

    /// Type an array literal (ADR-0004 Stage 2).
    ///
    /// List form `[e0, …, ek-1]`: every element unifies against one fresh
    /// element variable (a mismatch is reported at the offending element's
    /// span) and the element *count* is the length — `[]` is a
    /// `[_; 0]` whose element type must be solved from context or the
    /// annotation-needed error fires at the literal. Repeat form `[x; N]`:
    /// the operand types the element and the validated `N` is the length;
    /// the `Copy` requirement on the repeated element is enforced by the
    /// ownership checker (duplicating a non-`Copy` value is what §2
    /// forbids).
    fn array_literal(&mut self, literal: ArrayLiteralExpr<'_>, span: Span) -> Ty {
        match literal.kind() {
            Some(ArrayLiteralKind::List(elements)) => {
                let elem = self.fresh(span);
                let mut count: u64 = 0;
                for element in elements {
                    let element_span = self.at(value_span(&element));
                    let actual = self.expr(element);
                    self.expect_ty(&elem, &actual, element_span);
                    count += 1;
                }
                Ty::FixedArray(Box::new(elem), count)
            }
            Some(ArrayLiteralKind::Repeat { value, len }) => {
                let elem = self.expr(value);
                match self.fixed_array_len(len.text, len.span) {
                    Some(n) => Ty::FixedArray(Box::new(elem), n),
                    None => Ty::Error,
                }
            }
            None => Ty::Error,
        }
    }

    // ------------------------------------------------------------------
    // Expressions
    // ------------------------------------------------------------------

    fn expr(&mut self, expr: Expr<'_>) -> Ty {
        let ty = self.expr_kind(expr);
        self.record(expr.span(), &ty);
        ty
    }

    /// Record `ty` as the (raw) type of the expression at `span`, for the
    /// per-expression type table downstream stages consume.
    fn record(&mut self, span: Option<Span>, ty: &Ty) {
        if let Some(span) = span {
            self.body_exprs.push((span, ty.clone()));
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one arm per expression form; the dispatch reads best in one place"
    )]
    fn expr_kind(&mut self, expr: Expr<'_>) -> Ty {
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
            Expr::ArrayLiteral(literal) => self.array_literal(literal, span),
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
                    Ty::Array(item) | Ty::FixedArray(item, _) => {
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
                // `Map == Map` is deferred (ADR-0011): structural equality
                // over an unordered container needs a canonical comparison
                // the v0 core does not define — specs compare observations
                // (`get`/`len`/`keys`) instead. Refused here so no engine
                // invents an order-sensitive stand-in.
                let applied = self.icx.apply(lhs);
                if matches!(applied, Ty::Map(..)) {
                    self.unsupported_op(op, &applied, span);
                }
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
            Ty::Range(item) | Ty::Array(item) | Ty::FixedArray(item, _) => *item,
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
                        let ty = self.if_expr(nested, nested_span);
                        // An `else if` is its own expression node in the
                        // lowered tree; give it a table entry too.
                        self.record(nested.span(), &ty);
                        (ty, nested_span)
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
            // Block-local `const`s live in the body's local table;
            // top-level ones in the module table.
            SymbolKind::Const => self
                .locals
                .get(&symbol)
                .or_else(|| self.consts.get(&symbol))
                .cloned()
                .unwrap_or(Ty::Error),
            SymbolKind::Function => {
                // A bare `fn` name used in value position becomes a
                // first-class function value (ADR-0008 Tier 1). Only ordinary
                // top-level user functions are first-class; a builtin or a
                // generic function is rejected (`T0015`). The call edge is
                // recorded conservatively so the effect closure taints the
                // referrer (the R0007 spec-gate stays sound for fn values).
                self.record_call_edge(symbol);
                self.fn_value_type(symbol, given_args, span)
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

    /// The type of a bare `fn` name used as a first-class value (ADR-0008
    /// Tier 1). Only an ordinary top-level user function is first-class:
    ///
    /// - a **builtin** (`std::rt`/`std::str`/`std::string`/`std::array`)
    ///   lowers to a dedicated MIR op, not a callable code pointer, so there
    ///   is no function value to take (`T0015`);
    /// - a **generic** function is a family of monomorphizations, and a
    ///   function value is a single monomorphic code pointer (`T0015`);
    ///   monomorphizing function values is deferred past Tier 1.
    ///
    /// Otherwise the value's type is the function's signature-as-function-type,
    /// modes included.
    fn fn_value_type(&mut self, symbol: SymbolId, given_args: Option<Vec<Ty>>, span: Span) -> Ty {
        let name = self.resolution.symbol(symbol).name.clone();
        if self.resolution.builtin(symbol).is_some() {
            self.push(
                Diagnostic::error(
                    code(15),
                    format!("builtin function `{name}` is not a first-class value"),
                    span,
                )
                .with_primary_label("only ordinary top-level `fn`s can be used as values")
                .with_help("call the builtin directly, e.g. `std::str::len(s)`")
                .with_actual(StructuredValue::Name(name)),
            );
            return Ty::Error;
        }
        if let Some(sig) = self.fns.get(&symbol) {
            if !sig.type_params.is_empty() {
                self.push(
                    Diagnostic::error(
                        code(15),
                        format!("generic function `{name}` is not a first-class value"),
                        span,
                    )
                    .with_primary_label(
                        "a function value is a single monomorphic code pointer (ADR-0008 Tier 1)",
                    )
                    .with_help("call it directly, or wrap it in a non-generic function")
                    .with_actual(StructuredValue::Name(name)),
                );
                // Still consume any turbofish args to type them.
                if let Some(args) = &given_args {
                    let _ = args;
                }
                return Ty::Error;
            }
        }
        // A non-generic function: `given_args` is either absent or an empty
        // turbofish (arity is checked by `instantiate_fn` against zero type
        // params). Build the value type with modes.
        let (params, ret) = self.instantiate_fn(symbol, given_args, span);
        let modes = self
            .fns
            .get(&symbol)
            .map(|sig| sig.modes.clone())
            .unwrap_or_default();
        let fn_params = params
            .into_iter()
            .zip(modes.into_iter().chain(std::iter::repeat(ParamMode::In)))
            .map(|(ty, mode)| FnParam { mode, ty })
            .collect();
        Ty::Fn(Box::new(FnTy {
            params: fn_params,
            ret,
        }))
    }

    /// Type-check a call to one of the growable-`Array` builtins, generic over
    /// the element type `T` (ADR-0012). Returns `Some(result_ty)` when `builtin`
    /// is an array builtin (having checked the arguments), or `None` when it is
    /// not — the caller then falls through to the ordinary fixed-signature path.
    ///
    /// The element type `T` is **witnessed by the receiver**: for
    /// `push`/`pop`/`get`/`len` the first argument's type must be `Array[T]`, and
    /// `push`'s value / `get`'s result / `pop`'s `Option[T]` follow that `T`. For
    /// `empty()` there is no receiver, so `T` is a fresh variable solved from the
    /// use site (the `let`'s binding, a later `push`), exactly as an empty
    /// `[T; N]` literal's element already is; an element that never gets
    /// determined is a `T0001`, never a silent default to `Int`.
    fn check_array_builtin_call(
        &mut self,
        builtin: Builtin,
        args: &[Expr<'_>],
        span: Span,
    ) -> Option<Ty> {
        let int = Ty::int();
        match builtin {
            Builtin::ArrayEmpty => {
                // `empty() -> Array[T]`; T is a fresh var solved from context.
                self.check_args(&[], args, span);
                let elem = self.fresh(span);
                Some(Ty::Array(Box::new(elem)))
            }
            Builtin::ArrayPush => {
                // `push(Array[T], T) -> Unit`; T is witnessed by the receiver.
                let elem = self.fresh(span);
                let array = Ty::Array(Box::new(elem.clone()));
                self.check_args(&[array, elem.clone()], args, span);
                self.reject_unsupported_array_element(&elem, span);
                Some(Ty::Unit)
            }
            Builtin::ArrayPop => {
                // `pop(Array[T]) -> Option[T]`.
                let elem = self.fresh(span);
                let array = Ty::Array(Box::new(elem.clone()));
                self.check_args(&[array], args, span);
                self.reject_unsupported_array_element(&elem, span);
                Some(Ty::Option(Box::new(elem)))
            }
            Builtin::ArrayGet => {
                // `get(Array[T], Int) -> T`.
                let elem = self.fresh(span);
                let array = Ty::Array(Box::new(elem.clone()));
                self.check_args(&[array, int], args, span);
                self.reject_unsupported_array_element(&elem, span);
                Some(elem)
            }
            Builtin::ArraySet => {
                // `set(mut Array[T], Int, T) -> Unit` (ADR-0016): the
                // in-place element overwrite, `get`'s write mirror.
                let elem = self.fresh(span);
                let array = Ty::Array(Box::new(elem.clone()));
                self.check_args(&[array, int, elem.clone()], args, span);
                self.reject_unsupported_array_element(&elem, span);
                Some(Ty::Unit)
            }
            Builtin::ArrayLen => {
                // `len(Array[T]) -> Int`; the count, always `Int`.
                let elem = self.fresh(span);
                let array = Ty::Array(Box::new(elem));
                self.check_args(&[array], args, span);
                Some(int)
            }
            _ => None,
        }
    }

    /// Check a call to one of the key/value-parametric `std::map` builtins
    /// (ADR-0011). `K` and `V` are witnessed by the receiver argument (fresh
    /// vars solved from context for `empty()`); the v0 operation surface is
    /// `Map[Int, Int]` and `Map[Str, Int]` — any other *determined* pair is
    /// a `T0001` (an undetermined `empty()` is `T0011` "type annotation
    /// needed", reported by the ordinary unsolved-variable path). Returns
    /// `None` for non-map builtins.
    fn check_map_builtin_call(
        &mut self,
        builtin: Builtin,
        args: &[Expr<'_>],
        span: Span,
    ) -> Option<Ty> {
        let int = Ty::int();
        match builtin {
            Builtin::MapEmpty => {
                // `empty() -> Map[K, V]`; K/V are fresh vars solved from context.
                self.check_args(&[], args, span);
                let key = self.fresh(span);
                let value = self.fresh(span);
                Some(Ty::Map(Box::new(key), Box::new(value)))
            }
            Builtin::MapInsert => {
                // `insert(mut Map[K, V], K, V) -> Option[V]`.
                let key = self.fresh(span);
                let value = self.fresh(span);
                let map = Ty::Map(Box::new(key.clone()), Box::new(value.clone()));
                self.check_args(&[map, key.clone(), value.clone()], args, span);
                self.reject_unsupported_map_pair(&key, &value, span);
                Some(Ty::Option(Box::new(value)))
            }
            Builtin::MapGet => {
                // `get(in Map[K, V], K) -> Option[V]`.
                let key = self.fresh(span);
                let value = self.fresh(span);
                let map = Ty::Map(Box::new(key.clone()), Box::new(value.clone()));
                self.check_args(&[map, key.clone()], args, span);
                self.reject_unsupported_map_pair(&key, &value, span);
                Some(Ty::Option(Box::new(value)))
            }
            Builtin::MapContainsKey => {
                // `contains_key(in Map[K, V], K) -> Bool`.
                let key = self.fresh(span);
                let value = self.fresh(span);
                let map = Ty::Map(Box::new(key.clone()), Box::new(value.clone()));
                self.check_args(&[map, key.clone()], args, span);
                self.reject_unsupported_map_pair(&key, &value, span);
                Some(Ty::Bool)
            }
            Builtin::MapRemove => {
                // `remove(mut Map[K, V], K) -> Option[V]`.
                let key = self.fresh(span);
                let value = self.fresh(span);
                let map = Ty::Map(Box::new(key.clone()), Box::new(value.clone()));
                self.check_args(&[map, key.clone()], args, span);
                self.reject_unsupported_map_pair(&key, &value, span);
                Some(Ty::Option(Box::new(value)))
            }
            Builtin::MapLen => {
                // `len(in Map[K, V]) -> Int`; the count, always `Int`.
                let key = self.fresh(span);
                let value = self.fresh(span);
                let map = Ty::Map(Box::new(key), Box::new(value));
                self.check_args(&[map], args, span);
                Some(int)
            }
            Builtin::MapKeys => {
                // `keys(in Map[K, V]) -> Array[K]` (insertion order).
                let key = self.fresh(span);
                let value = self.fresh(span);
                let map = Ty::Map(Box::new(key.clone()), Box::new(value.clone()));
                self.check_args(&[map], args, span);
                self.reject_unsupported_map_pair(&key, &value, span);
                Some(Ty::Array(Box::new(key)))
            }
            _ => None,
        }
    }

    /// Refuse a `Map[K, V]` whose key/value pair is outside the v0 operation
    /// surface (ADR-0011) with a `T0001`. The supported pairs are exactly
    /// `Map[Int, Int]` and `Map[Str, Int]` — the scalar keys whose equality
    /// and hash the language owns, with `Int` values. An unsolved/`Error`
    /// component is left alone (an undetermined `empty()` is `T0011`,
    /// reported by the unsolved-variable path).
    fn reject_unsupported_map_pair(&mut self, key: &Ty, value: &Ty, span: Span) {
        let key = self.icx.apply(key);
        let value = self.icx.apply(value);
        let undetermined = |ty: &Ty| matches!(ty, Ty::Var(_) | Ty::Error | Ty::Never);
        if undetermined(&key) || undetermined(&value) {
            return;
        }
        let key_ok = matches!(key, Ty::Str) || key == Ty::int();
        let value_ok = value == Ty::int();
        if key_ok && value_ok {
            return;
        }
        let key_text = self.render(&key);
        let value_text = self.render(&value);
        let rendered = format!("Map[{key_text}, {value_text}]");
        self.push(
            Diagnostic::error(
                code(1),
                format!("`{rendered}` is not supported in v0"),
                span,
            )
            .with_primary_label(format!(
                "the v0 map operation surface is `Map[Int, Int]` and `Map[Str, Int]`, \
                     not `{rendered}`"
            ))
            .with_help(
                "user key types await the trait system's `Hash`/`Eq`, and non-`Int` \
                     values are a later additive increment (ADR-0011); use an `Int` or \
                     `Str` key with an `Int` value",
            )
            .with_actual(StructuredValue::Type(rendered)),
        );
    }

    /// Refuse an `Array[T]` whose element type `T` is outside the v0-supported
    /// set (ADR-0012 Stage A) with a `T0001`. The supported set is the scalars
    /// `Int`/`Bool`/`Str`/`String` and user structs/enums whose fields are
    /// themselves supported; nested owned containers (`Array[Array[T]]`,
    /// `Array[Map]`) and wrapped values (`Box`/`Shared`/`Weak`) stay refused.
    /// An unsolved/`Error` element is left alone (it is reported elsewhere).
    fn reject_unsupported_array_element(&mut self, elem: &Ty, span: Span) {
        let resolved = self.icx.apply(elem);
        if !self.is_supported_array_element(&resolved) {
            let rendered = self.render(&resolved);
            self.push(
                Diagnostic::error(
                    code(1),
                    format!("`Array[{rendered}]` is not supported in v0"),
                    span,
                )
                .with_primary_label(format!(
                    "the growable `Array` element type must be a scalar \
                     (`Int`/`Float`/`Bool`/`Str`/`String`) or a supported \
                     struct/enum, not `{rendered}`"
                ))
                .with_help(
                    "nested owned containers and wrapped values are outside the v0 element \
                     set (ADR-0012); use a supported element type",
                )
                .with_actual(StructuredValue::Type(rendered)),
            );
        }
    }

    /// Is `ty` a v0-supported growable-`Array` element type (ADR-0012 Stage A)?
    /// Unsolved variables and `Error` are treated as supported here (not this
    /// check's job to report them), so only a *determined, unsupported* type is
    /// rejected.
    fn is_supported_array_element(&self, ty: &Ty) -> bool {
        match ty {
            // `Float` joined the scalar element set with ADR-0016 — a `Copy`
            // scalar with a fixed layout, exactly like `Int`.
            Ty::Int(_) | Ty::Float(_) | Ty::Bool | Ty::Str | Ty::String => true,
            // A struct/enum is supported iff every field/variant-field type is.
            // The recursion terminates: a type cannot contain itself by value
            // or through a container (the ADR-0016 recursion boundary,
            // `T0016`), and we only descend through already-instantiated
            // shapes.
            Ty::Struct(symbol, targs) => self.aggregate_fields_supported(*symbol, targs, true),
            Ty::Enum(symbol, targs) => self.aggregate_fields_supported(*symbol, targs, false),
            // Unsolved / poisoned: not this check's responsibility.
            Ty::Var(_) | Ty::Error | Ty::Never => true,
            // Everything else — nested Array/FixedArray/Option/Result/Tuple/Fn/
            // Wrapper/Range/Char/Unit — is out of the v0 element set.
            _ => false,
        }
    }

    /// Are all field types of a struct (`is_struct == true`) or enum's variants
    /// supported array elements, under the given type arguments?
    fn aggregate_fields_supported(&self, symbol: SymbolId, targs: &[Ty], is_struct: bool) -> bool {
        if is_struct {
            let Some(def) = self.structs.get(&symbol) else {
                return false;
            };
            let map = substitution(&def.type_params, targs);
            def.fields
                .iter()
                .all(|(_, ty)| self.is_supported_array_element(&substitute(ty, &map)))
        } else {
            let Some(def) = self.enums.get(&symbol) else {
                return false;
            };
            let map = substitution(&def.type_params, targs);
            def.variants.iter().all(|(_, fields)| {
                fields
                    .iter()
                    .all(|(_, ty)| self.is_supported_array_element(&substitute(ty, &map)))
            })
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
                    self.record_call_edge(symbol);
                    // The growable-`Array` builtins are generic over the element
                    // type `T` (ADR-0012): their signatures are element-parametric
                    // and `T` is witnessed by the receiver argument, not fixed to
                    // `Int`. Handle them here rather than through the fixed
                    // `builtin_signature`, which cannot spell a type variable.
                    if let Some(builtin) = self.resolution.builtin(symbol) {
                        if let Some(ret) = self.check_array_builtin_call(builtin, &args, span) {
                            return ret;
                        }
                        // The `std::map` builtins are key/value-parametric the
                        // same way (ADR-0011).
                        if let Some(ret) = self.check_map_builtin_call(builtin, &args, span) {
                            return ret;
                        }
                    }
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
                // An indirect call through a function-typed value (ADR-0008
                // Tier 1). Argument types are checked against the function
                // type's parameters; the per-argument modes are enforced by
                // the ownership checker, which reads the same `Ty::Fn`.
                let param_tys: Vec<Ty> =
                    fn_ty.params.iter().map(|param| param.ty.clone()).collect();
                self.check_args(&param_tys, &args, span);
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

/// Map a written parameter-mode keyword to a [`ParamMode`]; an absent or
/// unrecognized mode defaults to `in` (the ownership default). Parsing already
/// requires a mode on every parameter, so the default is only a fail-safe.
fn mode_from_text(mode: Option<&str>) -> ParamMode {
    match mode {
        Some("mut") => ParamMode::Mut,
        Some("take") => ParamMode::Take,
        _ => ParamMode::In,
    }
}

/// Replace `Ty::Param`s per `map` (identity for unmapped params).
fn substitute(ty: &Ty, map: &HashMap<SymbolId, Ty>) -> Ty {
    match ty {
        Ty::Param(symbol) => map.get(symbol).cloned().unwrap_or_else(|| ty.clone()),
        Ty::Tuple(items) => Ty::Tuple(items.iter().map(|item| substitute(item, map)).collect()),
        Ty::Array(item) => Ty::Array(Box::new(substitute(item, map))),
        Ty::FixedArray(item, n) => Ty::FixedArray(Box::new(substitute(item, map)), *n),
        Ty::Range(item) => Ty::Range(Box::new(substitute(item, map))),
        Ty::Fn(fn_ty) => Ty::Fn(Box::new(FnTy {
            params: fn_ty
                .params
                .iter()
                .map(|param| FnParam {
                    mode: param.mode,
                    ty: substitute(&param.ty, map),
                })
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
