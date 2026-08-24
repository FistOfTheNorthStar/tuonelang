//! Lowering typed HIR into MIR.
//!
//! [`lower`] walks every function body of a **front-end-clean** program
//! snapshot (no parse, resolution, type, or ownership errors — callers
//! must not lower rejected programs) and produces one [`Function`] per
//! body, or a [`Skipped`] entry naming why the v0 lowering declines it.
//! Nothing is ever mis-lowered silently: any construct outside the
//! documented v0 subset skips its whole function with a reason.
//!
//! The lowering makes the ownership model's memory operations explicit:
//!
//! - by-value uses of non-`Copy` places become [`Operand::Move`]s;
//!   everything else is a [`Operand::Copy`] or a constant;
//! - `in`/`mut` arguments become [`Arg::Borrow`]/[`Arg::BorrowMut`] of
//!   places, alive exactly for the call;
//! - drops are elaborated at scope ends, assignments, and early exits
//!   (`return`, `break`, `continue`, `?`) from the statically known
//!   initialization states the ownership checker guarantees ("no hidden
//!   drop flags"): every emitted [`Statement::Drop`] targets a place that
//!   is definitely initialized. Temporaries are dropped at the end of
//!   their enclosing scope, which is unobservable in v0 (no user
//!   destructors, ADR-0003).
//!
//! Desugarings: `&&`/`||` become control flow; `match` becomes sequential
//! arm tests over [`Terminator::Switch`]/[`Terminator::Branch`] plus
//! explicit payload extraction and husk drops; `?` becomes a discriminant
//! switch with an early return; `for` becomes an index loop; indexing
//! gains an explicit [`Terminator::Assert`] bounds check.

use std::collections::{BTreeSet, HashMap};

use tuo_hir::{
    Arm, BindingDef, Block, Expr, ExprKind, Function as HirFunction, Hir, Item, Lit, ParamMode,
    Pat, PatKind, Res, SpecStmt, Stmt, StmtKind,
};
use tuo_resolve::{Builtin, Resolution, SymbolId, SymbolKind};
use tuo_source::Span;
use tuo_types::{FnTy, IntKind, Ty, TypeckResult};

use crate::mir::{
    AggregateKind, Arg, BasicBlock, BinOp, BlockId, Callee, CastKind, Const, EffectOp, Function,
    HeapMutOp, HeapOp, LocalDecl, LocalId, Operand, PassMode, Place, Program, Projection, Rvalue,
    Skipped, Statement, StrOp, Terminator, Trap, UnOp,
};

/// Lower every function body of a front-end-clean snapshot.
#[must_use]
pub fn lower(hir: &Hir, resolution: &Resolution, types: &TypeckResult) -> Program {
    let mut cx = Cx {
        resolution,
        types,
        modes: HashMap::new(),
        consts: HashMap::new(),
    };
    for item in &hir.items {
        match item {
            Item::Fn(function) => cx.collect_fn(function),
            Item::Impl(impl_def) => {
                for function in &impl_def.functions {
                    cx.collect_fn(function);
                }
            }
            Item::Const(def) => cx.collect_const(def),
            _ => {}
        }
    }
    let mut program = Program::default();
    for item in &hir.items {
        match item {
            Item::Fn(function) => cx.lower_fn(function, &mut program),
            Item::Impl(impl_def) => {
                for function in &impl_def.functions {
                    cx.lower_fn(function, &mut program);
                }
            }
            _ => {}
        }
    }
    program
}

/// Why a function body is not lowered; aborts that body's lowering.
type Skip = String;

/// The result of lowering one expression: `None` when the expression
/// diverged (terminated the current block), so nothing follows it.
type Lowered = Result<Option<Value>, Skip>;

/// A lowered expression result: either a memory location or a value that
/// has no location (a constant, or unit).
#[derive(Clone, Debug)]
enum Value {
    /// The expression names this place.
    Place(Place),
    /// The expression produced this operand directly.
    Operand(Operand),
}

/// Program-wide lowering context.
struct Cx<'a> {
    resolution: &'a Resolution,
    types: &'a TypeckResult,
    /// Parameter passing modes per function symbol.
    modes: HashMap<SymbolId, Vec<PassMode>>,
    /// Literal-initialized `const` items, pre-converted per declaration.
    consts: HashMap<SymbolId, Const>,
}

impl Cx<'_> {
    fn collect_fn(&mut self, function: &HirFunction) {
        let Res::Symbol(symbol) = function.symbol else {
            return;
        };
        let modes = function
            .params
            .iter()
            .map(|param| match param.mode {
                ParamMode::In => PassMode::Borrow,
                ParamMode::Mut => PassMode::BorrowMut,
                ParamMode::Take => PassMode::Value,
            })
            .collect();
        self.modes.insert(symbol, modes);
    }

    /// Register a `const` item usable from MIR: v0 supports literal (and
    /// negated-literal) initializers; anything else simply stays out of
    /// the table and skips the functions that reference it.
    fn collect_const(&mut self, def: &tuo_hir::ConstDef) {
        let Res::Symbol(symbol) = def.symbol else {
            return;
        };
        let Some(ty) = self.types.type_of(symbol) else {
            return;
        };
        let Some(value) = &def.value else {
            return;
        };
        if let Ok(constant) = literal_const(value, ty) {
            self.consts.insert(symbol, constant);
        }
    }

    /// Reject a poisoned or unsolved type at the program level (specs are
    /// checked, so this is a defensive guard, not an expected path).
    fn clean_ty_free(&self, ty: Ty) -> Result<Ty, Skip> {
        if contains_error(&ty) {
            Err("spec expression has type errors or unresolved types".to_owned())
        } else {
            Ok(ty)
        }
    }

    fn lower_fn(&self, function: &HirFunction, program: &mut Program) {
        if function.body.is_none() {
            return;
        }
        let Res::Symbol(symbol) = function.symbol else {
            // Impl functions have no resolved symbol until the trait
            // system lands; never drop a body silently.
            program.skipped.push(Skipped {
                name: "{impl}".to_owned(),
                reason: "impl functions are not lowered in v0 (pending the trait system)"
                    .to_owned(),
                span: function.span,
            });
            return;
        };
        let name = self.resolution.symbol(symbol).name.clone();
        match FnLower::new(self, symbol, function).run(function) {
            Ok(lowered) => program.functions.push(lowered),
            Err(reason) => program.skipped.push(Skipped {
                name,
                reason,
                span: function.span,
            }),
        }
    }
}

/// Lower every `spec` block of a front-end-clean snapshot into runnable
/// MIR.
///
/// The returned [`SpecProgram`] carries the ordinary program (the specs'
/// dependencies) plus, appended to it, one synthetic function per `then` /
/// `assert` clause (and, for `==` assertions, two more for the operands).
/// The interpreter runs those functions by name; every function — real or
/// synthetic — is ordinary verifiable MIR. Specs outside the v0 subset are
/// recorded in [`SpecProgram::skipped`], never silently dropped. See
/// [`crate::spec`] and ADR-0002.
#[must_use]
pub fn lower_specs(
    hir: &Hir,
    resolution: &Resolution,
    types: &TypeckResult,
) -> crate::spec::SpecProgram {
    use crate::spec::{
        AssertionKind, Comparison, SkippedSpec, SpecAssertion, SpecMir, SpecProgram,
    };

    let mut program = lower(hir, resolution, types);
    let cx = Cx {
        resolution,
        types,
        modes: {
            // The synthetic assertion functions call the specs' dependency
            // functions; their pass modes must be known just as `lower`
            // registers them.
            let mut modes = HashMap::new();
            for item in &hir.items {
                if let Item::Fn(function) = item {
                    if let Res::Symbol(symbol) = function.symbol {
                        modes.insert(
                            symbol,
                            function
                                .params
                                .iter()
                                .map(|param| match param.mode {
                                    ParamMode::In => PassMode::Borrow,
                                    ParamMode::Mut => PassMode::BorrowMut,
                                    ParamMode::Take => PassMode::Value,
                                })
                                .collect(),
                        );
                    }
                }
            }
            modes
        },
        consts: HashMap::new(),
    };
    // Synthetic functions need fresh symbols that cannot collide with any
    // real one; hand them out above the highest real symbol.
    let mut next_symbol = resolution
        .symbols()
        .map(|(id, _)| id.as_u32())
        .max()
        .map_or(0, |max| max + 1);
    let mut fresh_symbol = || {
        let id = SymbolId::from_raw(next_symbol);
        next_symbol += 1;
        id
    };

    let mut specs = Vec::new();
    let mut skipped = Vec::new();
    for item in &hir.items {
        let Item::Spec(spec) = item else { continue };
        // A spec with no own symbol never resolved (malformed); the front
        // end would have rejected the program, but guard defensively.
        let Some(symbol) = spec.spec_symbol else {
            continue;
        };
        // Split the body into setup clauses (given/when/plain statements)
        // and the assertions, preserving source order: each assertion sees
        // exactly the setup written before it.
        let mut setup: Vec<Stmt> = Vec::new();
        let mut assertions: Vec<SpecAssertion> = Vec::new();
        let mut skip: Option<String> = None;
        for statement in &spec.statements {
            match statement {
                SpecStmt::Given(bindings) => {
                    for binding in bindings {
                        setup.push(given_stmt(binding));
                    }
                }
                SpecStmt::When(stmt) | SpecStmt::Stmt(stmt) => setup.push(stmt.clone()),
                SpecStmt::Then(expr) | SpecStmt::Assert(expr) => {
                    let kind = if matches!(statement, SpecStmt::Then(_)) {
                        AssertionKind::Then
                    } else {
                        AssertionKind::Assert
                    };
                    match lower_assertion(
                        &cx,
                        symbol,
                        &setup,
                        expr,
                        &mut fresh_symbol,
                        &mut program,
                    ) {
                        Ok((condition, comparison)) => assertions.push(SpecAssertion {
                            kind,
                            span: expr.span,
                            condition,
                            comparison: comparison
                                .map(|(actual, expected)| Comparison { actual, expected }),
                        }),
                        Err(reason) => {
                            skip = Some(reason);
                            break;
                        }
                    }
                }
            }
        }
        if let Some(reason) = skip {
            skipped.push(SkippedSpec {
                symbol,
                name: spec.name.clone(),
                reason,
                span: spec.span,
            });
        } else {
            specs.push(SpecMir {
                symbol,
                name: spec.name.clone(),
                target: spec.target,
                span: spec.span,
                assertions,
            });
        }
    }
    SpecProgram {
        program,
        specs,
        skipped,
    }
}

/// Build a `let`-style statement lowering one `given` binding.
fn given_stmt(binding: &tuo_hir::GivenBinding) -> Stmt {
    Stmt {
        kind: StmtKind::Binding(BindingDef {
            mutable: false,
            pat: Pat {
                kind: PatKind::Binding(binding.symbol.clone()),
                span: binding.span,
            },
            ty: binding.ty.clone(),
            init: binding.init.clone(),
        }),
        span: binding.span,
    }
}

/// Lower one assertion clause: a synthetic function returning its `Bool`
/// value, plus — when it is `lhs == rhs` — two more returning the operands.
/// Returns the condition function's name and, for a comparison, the
/// `(actual, expected)` function names. Emits the functions into `program`.
fn lower_assertion(
    cx: &Cx<'_>,
    spec: SymbolId,
    setup: &[Stmt],
    assertion: &Expr,
    fresh_symbol: &mut impl FnMut() -> SymbolId,
    program: &mut Program,
) -> Result<(String, Option<(String, String)>), Skip> {
    // The whole assertion, as a Bool-returning function.
    let condition = emit_spec_fn(cx, spec, "cond", setup, assertion, fresh_symbol, program)?;
    // If it is an equality comparison, also expose each side so a failure
    // can report the actual and expected operands.
    let comparison = match &assertion.kind {
        ExprKind::Binary {
            op: tuo_hir::BinOp::Eq,
            lhs,
            rhs,
        } => {
            let actual = emit_spec_fn(cx, spec, "actual", setup, lhs, fresh_symbol, program)?;
            let expected = emit_spec_fn(cx, spec, "expected", setup, rhs, fresh_symbol, program)?;
            Some((actual, expected))
        }
        _ => None,
    };
    Ok((condition, comparison))
}

/// Emit one synthetic no-argument function: run `setup`, then `return tail`.
/// Its name is `spec$<spec>$<role>$<n>` — deterministic and collision-free.
fn emit_spec_fn(
    cx: &Cx<'_>,
    spec: SymbolId,
    role: &str,
    setup: &[Stmt],
    tail: &Expr,
    fresh_symbol: &mut impl FnMut() -> SymbolId,
    program: &mut Program,
) -> Result<String, Skip> {
    let symbol = fresh_symbol();
    let name = format!("spec${}${role}${}", spec.as_u32(), symbol.as_u32());
    let block = Block {
        stmts: setup.to_vec(),
        tail: Some(Box::new(tail.clone())),
        span: tail.span,
    };
    let ret = cx.clean_ty_free(tail_ty(cx, tail)?)?;
    let function =
        FnLower::new_spec(cx, symbol, name.clone(), tail.span).run_spec_body(&block, ret)?;
    program.functions.push(function);
    Ok(name)
}

/// The checked type of a spec-clause tail expression.
fn tail_ty(cx: &Cx<'_>, tail: &Expr) -> Result<Ty, Skip> {
    cx.types
        .expr_ty(tail.span)
        .cloned()
        .ok_or_else(|| "spec expression has no checked type".to_owned())
}

/// One loop being lowered: where `break`/`continue` jump, where a break
/// value goes, and how deep the loop's scope stack is (scopes deeper than
/// `depth` are dropped by an early exit).
struct LoopCtx {
    label: Option<String>,
    break_block: BlockId,
    continue_block: BlockId,
    result: Option<LocalId>,
    depth: usize,
    /// The initialization state every path entering the loop head must
    /// have (defensive; guaranteed by the ownership checker).
    head_state: State,
    /// The state at the first observed exit; later exits must agree.
    exit_state: Option<State>,
}

/// A snapshot of tracked initialization state (non-`Copy` locals only).
type State = (BTreeSet<LocalId>, BTreeSet<(LocalId, Vec<Projection>)>);

/// A resolved constructor: the variant's declaration-order index, its
/// declared fields (name and instantiated type, in order), and whether
/// the constructed type carries a discriminant.
type CtorInfo = (u32, Vec<(String, Ty)>, bool);

/// What a name in binding-pattern position resolved to.
enum BindingKind {
    /// A fresh local binding.
    Local(SymbolId),
    /// A payload-less enum variant path (`None`, a bare variant).
    Variant(SymbolId),
}

struct FnLower<'a> {
    cx: &'a Cx<'a>,
    symbol: SymbolId,
    locals: Vec<LocalDecl>,
    blocks: Vec<BlockBuilder>,
    current: usize,
    sym_locals: HashMap<SymbolId, LocalId>,
    /// Locals declared per lexical scope, in creation order.
    scopes: Vec<Vec<LocalId>>,
    loops: Vec<LoopCtx>,
    /// Tracked (non-`Copy`) locals that are not yet initialized.
    uninit: BTreeSet<LocalId>,
    /// Tracked places (root + projection path) whose value moved away.
    moved: BTreeSet<(LocalId, Vec<Projection>)>,
    /// Borrow-mode parameter locals: aliases, never dropped here.
    borrowed_params: BTreeSet<LocalId>,
    /// Block-local literal `const`s seen so far.
    local_consts: HashMap<SymbolId, Const>,
    ret: Ty,
    fn_span: Span,
    /// The name of a synthetic spec function; `None` for a real function,
    /// whose name comes from resolution.
    spec_name: Option<String>,
}

struct BlockBuilder {
    statements: Vec<Statement>,
    terminator: Option<Terminator>,
}

impl<'a> FnLower<'a> {
    fn new(cx: &'a Cx<'a>, symbol: SymbolId, function: &HirFunction) -> Self {
        Self {
            cx,
            symbol,
            locals: Vec::new(),
            blocks: vec![BlockBuilder {
                statements: Vec::new(),
                terminator: None,
            }],
            current: 0,
            sym_locals: HashMap::new(),
            scopes: vec![Vec::new()],
            loops: Vec::new(),
            uninit: BTreeSet::new(),
            moved: BTreeSet::new(),
            borrowed_params: BTreeSet::new(),
            local_consts: HashMap::new(),
            ret: Ty::Unit,
            fn_span: function.span,
            spec_name: None,
        }
    }

    /// A lowering context for a synthetic spec function: no HIR function
    /// backs it, so it carries its own name, fresh symbol, and span.
    fn new_spec(cx: &'a Cx<'a>, symbol: SymbolId, name: String, span: Span) -> Self {
        Self {
            cx,
            symbol,
            locals: Vec::new(),
            blocks: vec![BlockBuilder {
                statements: Vec::new(),
                terminator: None,
            }],
            current: 0,
            sym_locals: HashMap::new(),
            scopes: vec![Vec::new()],
            loops: Vec::new(),
            uninit: BTreeSet::new(),
            moved: BTreeSet::new(),
            borrowed_params: BTreeSet::new(),
            local_consts: HashMap::new(),
            ret: Ty::Unit,
            fn_span: span,
            spec_name: Some(name),
        }
    }

    /// Lower a synthetic spec-clause body: run `block`'s statements, then
    /// `return` its tail expression (an assertion or an operand). Reuses the
    /// ordinary block/statement/expression lowering, so setup clauses obey
    /// the same move and drop rules as function-body code.
    fn run_spec_body(mut self, block: &Block, ret: Ty) -> Result<Function, Skip> {
        self.ret = ret;
        if let Some(value) = self.block(block)? {
            let ret_ty = self.ret.clone();
            let operand = self.use_value(value, &ret_ty);
            self.emit_return(operand)?;
        }
        // Synthetic spec functions take no parameters.
        Ok(self.finish(Vec::new()))
    }

    /// Assemble the lowered function from the accumulated blocks.
    fn finish(self, params: Vec<PassMode>) -> Function {
        let name = self
            .spec_name
            .clone()
            .unwrap_or_else(|| self.cx.resolution.symbol(self.symbol).name.clone());
        let blocks = self
            .blocks
            .into_iter()
            .map(|builder| BasicBlock {
                statements: builder.statements,
                terminator: builder
                    .terminator
                    .unwrap_or(Terminator::Trap(Trap::Unreachable)),
            })
            .collect();
        Function {
            symbol: self.symbol,
            name,
            params,
            locals: self.locals,
            blocks,
            ret: self.ret,
            span: self.fn_span,
        }
    }

    fn run(mut self, function: &HirFunction) -> Result<Function, Skip> {
        let signature = match self.cx.types.type_of(self.symbol) {
            Some(Ty::Fn(fn_ty)) => (**fn_ty).clone(),
            _ => FnTy {
                params: Vec::new(),
                ret: Ty::Unit,
            },
        };
        self.ret = self.clean_ty(&signature.ret)?;
        let mut params = Vec::new();
        for param in &function.params {
            let Res::Symbol(symbol) = param.symbol else {
                return Err("parameter did not resolve".to_owned());
            };
            let ty = self.symbol_ty(symbol)?;
            let local = self.new_local(
                ty,
                Some(self.cx.resolution.symbol(symbol).name.clone()),
                param.span,
            );
            self.sym_locals.insert(symbol, local);
            let mode = match param.mode {
                ParamMode::In => PassMode::Borrow,
                ParamMode::Mut => PassMode::BorrowMut,
                ParamMode::Take => PassMode::Value,
            };
            if !matches!(mode, PassMode::Value) {
                self.borrowed_params.insert(local);
            }
            // Parameters arrive initialized.
            self.uninit.remove(&local);
            params.push(mode);
        }
        let body = function.body.as_ref().expect("checked by caller");
        if let Some(value) = self.block(body)? {
            let ret_ty = self.ret.clone();
            let operand = self.use_value(value, &ret_ty);
            self.emit_return(operand)?;
        }
        Ok(self.finish(params))
    }

    // ------------------------------------------------------------------
    // Infrastructure: blocks, locals, state
    // ------------------------------------------------------------------

    fn new_block(&mut self) -> BlockId {
        self.blocks.push(BlockBuilder {
            statements: Vec::new(),
            terminator: None,
        });
        BlockId(u32::try_from(self.blocks.len() - 1).expect("block count fits u32"))
    }

    fn switch_to(&mut self, block: BlockId) {
        self.current = block.0 as usize;
    }

    fn push(&mut self, statement: Statement) {
        self.blocks[self.current].statements.push(statement);
    }

    fn terminate(&mut self, terminator: Terminator) {
        let block = &mut self.blocks[self.current];
        debug_assert!(block.terminator.is_none(), "block terminated twice");
        block.terminator = Some(terminator);
    }

    fn new_local(&mut self, ty: Ty, name: Option<String>, span: Span) -> LocalId {
        let id = LocalId(u32::try_from(self.locals.len()).expect("local count fits u32"));
        let tracked = !self.is_copy(&ty);
        self.locals.push(LocalDecl { ty, name, span });
        self.scopes
            .last_mut()
            .expect("a scope is always open")
            .push(id);
        if tracked {
            self.uninit.insert(id);
        }
        id
    }

    fn temp(&mut self, ty: Ty, span: Span) -> LocalId {
        self.new_local(ty, None, span)
    }

    fn is_copy(&self, ty: &Ty) -> bool {
        tuo_ownership::is_copy(self.cx.types, ty)
    }

    fn tracked(&self, local: LocalId) -> bool {
        !self.is_copy(&self.locals[local.0 as usize].ty)
    }

    fn snapshot(&self) -> State {
        (self.uninit.clone(), self.moved.clone())
    }

    fn restore(&mut self, state: &State) {
        self.uninit = state.0.clone();
        self.moved = state.1.clone();
    }

    /// The set of locals declared in scopes shallower than `depth` (the
    /// locals that persist across a control-flow edge leaving those inner
    /// scopes). State comparisons at merges consider only these: a
    /// temporary in a scope the edge exits is re-created fresh on the
    /// other side, so its move-status is irrelevant to the merge.
    fn locals_below(&self, depth: usize) -> BTreeSet<LocalId> {
        self.scopes[..depth.min(self.scopes.len())]
            .iter()
            .flatten()
            .copied()
            .collect()
    }

    /// Project a state onto the locals shallower than `depth`.
    fn state_below(&self, state: &State, depth: usize) -> State {
        let live = self.locals_below(depth);
        (
            state
                .0
                .iter()
                .copied()
                .filter(|l| live.contains(l))
                .collect(),
            state
                .1
                .iter()
                .filter(|(root, _)| live.contains(root))
                .cloned()
                .collect(),
        )
    }

    /// Do two states agree on every local shallower than `depth`? Merges
    /// only require the persistent places to match; the ownership checker
    /// guarantees they do for accepted programs.
    fn states_agree_below(&self, a: &State, b: &State, depth: usize) -> bool {
        self.state_below(a, depth) == self.state_below(b, depth)
    }

    /// Do two states agree on every currently in-scope local?
    fn states_agree(&self, a: &State, b: &State) -> bool {
        self.states_agree_below(a, b, self.scopes.len())
    }

    /// Defensive: at a back edge, the state must match the loop head on
    /// every local that outlives the body — the ownership checker rejects
    /// programs where it would not. `depth` is the loop's scope depth.
    fn expect_state_below(&self, expected: &State, depth: usize) -> Result<(), Skip> {
        if self.states_agree_below(&self.snapshot(), expected, depth) {
            Ok(())
        } else {
            Err("initialization state diverged across merging paths (v0 lowering limit)".to_owned())
        }
    }

    /// A back edge reached from the body's own scope level (the body's
    /// scope has been closed): compare on every currently in-scope local.
    fn expect_state(&self, expected: &State) -> Result<(), Skip> {
        self.expect_state_below(expected, self.scopes.len())
    }

    fn mark_moved(&mut self, place: &Place) {
        if self.tracked(place.local) {
            self.moved.insert((place.local, place.projection.clone()));
        }
    }

    /// Is the exact place (or a prefix of it) moved or uninitialized?
    fn is_gone(&self, local: LocalId, path: &[Projection]) -> bool {
        if self.uninit.contains(&local) {
            return true;
        }
        self.moved
            .iter()
            .filter(|(root, _)| *root == local)
            .any(|(_, moved)| moved.len() <= path.len() && path[..moved.len()] == moved[..])
    }

    // ------------------------------------------------------------------
    // Types
    // ------------------------------------------------------------------

    /// Reject poisoned or unsolved types: those bodies had front-end
    /// errors and must not reach lowering.
    fn clean_ty(&self, ty: &Ty) -> Result<Ty, Skip> {
        if contains_error(ty) {
            return Err("body contains type errors or unresolved types".to_owned());
        }
        Ok(ty.clone())
    }

    fn symbol_ty(&self, symbol: SymbolId) -> Result<Ty, Skip> {
        match self.cx.types.type_of(symbol) {
            Some(ty) => self.clean_ty(ty),
            None => Err("binding has no checked type".to_owned()),
        }
    }

    fn expr_ty(&self, expr: &Expr) -> Result<Ty, Skip> {
        match self.cx.types.expr_ty(expr.span) {
            Some(ty) => self.clean_ty(ty),
            None => Err("expression has no checked type".to_owned()),
        }
    }

    /// The type one projection step reaches from `ty`.
    fn proj_ty(&self, ty: &Ty, projection: &Projection) -> Result<Ty, Skip> {
        let missing = || "projection does not type (compiler bug guard)".to_owned();
        match projection {
            Projection::Field(index) => match ty {
                Ty::Struct(symbol, args) => {
                    let shape = self.cx.types.struct_shape(*symbol).ok_or_else(missing)?;
                    let (_, field) = shape.fields.get(*index as usize).ok_or_else(missing)?;
                    Ok(instantiate(field, &shape.type_params, args))
                }
                Ty::Tuple(items) => items.get(*index as usize).cloned().ok_or_else(missing),
                Ty::Range(item) => Ok((**item).clone()),
                _ => Err(missing()),
            },
            Projection::VariantField { variant, field } => match ty {
                Ty::Enum(symbol, args) => {
                    let shape = self.cx.types.enum_shape(*symbol).ok_or_else(missing)?;
                    let (_, fields) = shape.variants.get(*variant as usize).ok_or_else(missing)?;
                    let (_, field) = fields.get(*field as usize).ok_or_else(missing)?;
                    Ok(instantiate(field, &shape.type_params, args))
                }
                Ty::Option(item) if *variant == 0 && *field == 0 => Ok((**item).clone()),
                Ty::Result(ok, _) if *variant == 0 && *field == 0 => Ok((**ok).clone()),
                Ty::Result(_, err) if *variant == 1 && *field == 0 => Ok((**err).clone()),
                _ => Err(missing()),
            },
            Projection::Index(_) => match ty {
                Ty::Array(item) | Ty::FixedArray(item, _) => Ok((**item).clone()),
                _ => Err(missing()),
            },
        }
    }

    fn place_ty(&self, place: &Place) -> Result<Ty, Skip> {
        let mut ty = self.locals[place.local.0 as usize].ty.clone();
        for projection in &place.projection {
            ty = self.proj_ty(&ty, projection)?;
        }
        Ok(ty)
    }

    // ------------------------------------------------------------------
    // Values, stores, drops
    // ------------------------------------------------------------------

    /// Consume `value` by value (the ownership model's move contexts): a
    /// non-`Copy` place is moved, everything else is copied or passed
    /// through.
    fn use_value(&mut self, value: Value, ty: &Ty) -> Operand {
        match value {
            Value::Place(place) => {
                if self.is_copy(ty) {
                    Operand::Copy(place)
                } else {
                    self.mark_moved(&place);
                    Operand::Move(place)
                }
            }
            Value::Operand(operand) => operand,
        }
    }

    /// Read `value` without consuming it (operator operands, conditions,
    /// scrutinee reads — always `Copy`-typed in accepted programs).
    fn read_value(value: Value) -> Operand {
        match value {
            Value::Place(place) => Operand::Copy(place),
            Value::Operand(operand) => operand,
        }
    }

    /// Store `rvalue` into `place`, dropping whatever the place held.
    fn store(&mut self, place: &Place, rvalue: Rvalue) -> Result<(), Skip> {
        let ty = self.place_ty(place)?;
        if !self.is_copy(&ty) {
            self.drop_residue(place, &ty)?;
            // Re-initialize: the place (and everything under it) is live
            // again; parent partial-move entries stay (siblings unchanged).
            let key = (place.local, place.projection.clone());
            self.moved.retain(|(root, path)| {
                *root != place.local || path.len() < key.1.len() || path[..key.1.len()] != key.1[..]
            });
            if place.projection.is_empty() {
                self.uninit.remove(&place.local);
            }
        }
        self.push(Statement::Assign {
            place: place.clone(),
            rvalue,
        });
        Ok(())
    }

    /// Materialize `value` into an owned place (creating a temporary when
    /// it does not already own one).
    fn place_of(&mut self, value: Value, ty: &Ty, span: Span) -> Result<Place, Skip> {
        match value {
            Value::Place(place) => Ok(place),
            Value::Operand(operand) => {
                let local = self.temp(ty.clone(), span);
                let place = Place::local(local);
                self.store(&place, Rvalue::Use(operand))?;
                Ok(place)
            }
        }
    }

    /// Materialize `rvalue` into a fresh temporary and return its place.
    fn assign_temp(&mut self, rvalue: Rvalue, ty: Ty, span: Span) -> Result<Place, Skip> {
        let local = self.temp(ty, span);
        let place = Place::local(local);
        self.store(&place, rvalue)?;
        Ok(place)
    }

    /// Emit drops for whatever remains initialized under `place`: the
    /// whole value when untouched, the unmoved fields after a partial
    /// move, nothing when the value is gone. Also marks what it drops as
    /// moved, so no path drops twice.
    fn drop_residue(&mut self, place: &Place, ty: &Ty) -> Result<(), Skip> {
        if self.is_copy(ty) || self.is_gone(place.local, &place.projection) {
            return Ok(());
        }
        let sub_moved: Vec<Vec<Projection>> = self
            .moved
            .iter()
            .filter(|(root, path)| {
                *root == place.local
                    && path.len() > place.projection.len()
                    && path[..place.projection.len()] == place.projection[..]
            })
            .map(|(_, path)| path[place.projection.len()..].to_vec())
            .collect();
        if sub_moved.is_empty() {
            self.push(Statement::Drop {
                place: place.clone(),
            });
            self.mark_moved(&place.clone());
            return Ok(());
        }
        // A partial move: recurse into the components the moves name.
        // Moves out of enum payloads pin the active variant; moves out of
        // structs walk fields. Every moved path starts with the same head
        // shape here, because diverging shapes cannot merge (ownership).
        let heads: BTreeSet<&Projection> = sub_moved.iter().map(|path| &path[0]).collect();
        let components: Vec<Projection> = match heads.iter().next() {
            Some(Projection::Field(_)) => match ty {
                Ty::Struct(symbol, _) => {
                    let shape = self
                        .cx
                        .types
                        .struct_shape(*symbol)
                        .ok_or_else(|| "missing struct shape".to_owned())?;
                    (0..shape.fields.len())
                        .map(|index| {
                            Projection::Field(u32::try_from(index).expect("field index fits"))
                        })
                        .collect()
                }
                Ty::Tuple(items) => (0..items.len())
                    .map(|index| Projection::Field(u32::try_from(index).expect("index fits")))
                    .collect(),
                Ty::Range(_) => vec![Projection::Field(0), Projection::Field(1)],
                _ => return Err("partial move of a non-aggregate".to_owned()),
            },
            Some(Projection::VariantField { variant, .. }) => {
                let variant = *variant;
                let field_count = match ty {
                    Ty::Enum(symbol, _) => {
                        let shape = self
                            .cx
                            .types
                            .enum_shape(*symbol)
                            .ok_or_else(|| "missing enum shape".to_owned())?;
                        shape
                            .variants
                            .get(variant as usize)
                            .map_or(0, |(_, fields)| fields.len())
                    }
                    Ty::Option(_) | Ty::Result(..) => 1,
                    _ => return Err("partial move of a non-enum".to_owned()),
                };
                (0..field_count)
                    .map(|field| Projection::VariantField {
                        variant,
                        field: u32::try_from(field).expect("field index fits"),
                    })
                    .collect()
            }
            _ => return Err("unexpected moved-path shape".to_owned()),
        };
        for component in components {
            let mut sub_place = place.clone();
            sub_place.projection.push(component.clone());
            let sub_ty = self.proj_ty(ty, &component)?;
            self.drop_residue(&sub_place, &sub_ty)?;
        }
        Ok(())
    }

    /// Emit drops for every local of every scope deeper than `depth`
    /// (used by `break`/`continue`/`return`/`?`), without popping scopes.
    fn drops_from(&mut self, depth: usize) -> Result<(), Skip> {
        let locals: Vec<LocalId> = self.scopes[depth..]
            .iter()
            .flat_map(|scope| scope.iter().rev().copied())
            .collect();
        // Reverse declaration order across the whole exited region.
        let mut ordered: Vec<LocalId> = locals;
        ordered.sort_by(|a, b| b.cmp(a));
        let saved = self.snapshot();
        for local in ordered {
            if self.borrowed_params.contains(&local) {
                continue;
            }
            let ty = self.locals[local.0 as usize].ty.clone();
            self.drop_residue(&Place::local(local), &ty)?;
        }
        // The drops themselves belong to the exiting path only.
        self.restore(&saved);
        Ok(())
    }

    fn push_scope(&mut self) {
        self.scopes.push(Vec::new());
    }

    /// Close the innermost scope on a live path: drop what it still owns
    /// (reverse declaration order) and forget its locals.
    fn end_scope(&mut self) -> Result<(), Skip> {
        let scope = self.scopes.pop().expect("a scope is open");
        for local in scope.iter().rev() {
            if self.borrowed_params.contains(local) {
                continue;
            }
            let ty = self.locals[local.0 as usize].ty.clone();
            self.drop_residue(&Place::local(*local), &ty)?;
        }
        for local in scope {
            self.uninit.remove(&local);
            self.moved.retain(|(root, _)| *root != local);
        }
        Ok(())
    }

    /// Discard the innermost scope on a diverged path: no drops (the path
    /// already left), just forget the locals.
    fn discard_scope(&mut self) {
        let scope = self.scopes.pop().expect("a scope is open");
        for local in scope {
            self.uninit.remove(&local);
            self.moved.retain(|(root, _)| *root != local);
        }
    }
}

impl FnLower<'_> {
    // ------------------------------------------------------------------
    // Blocks and statements
    // ------------------------------------------------------------------

    fn block(&mut self, block: &Block) -> Lowered {
        self.push_scope();
        for stmt in &block.stmts {
            if !self.stmt(stmt)? {
                self.discard_scope();
                return Ok(None);
            }
        }
        let result = match &block.tail {
            Some(tail) => {
                let ty = self.expr_ty(tail)?;
                let Some(value) = self.expr(tail)? else {
                    self.discard_scope();
                    return Ok(None);
                };
                // Consume the tail *before* the scope's drops, so a moved
                // tail is not dropped; freeze a `Copy` read of a local
                // this scope is about to drop.
                let operand = self.use_value(value, &ty);
                self.escape_operand(operand, &ty, tail.span)?
            }
            None => Operand::Const(Const::Unit),
        };
        self.end_scope()?;
        Ok(Some(Value::Operand(result)))
    }

    /// If `operand` copies out of a place the innermost scope owns,
    /// materialize it into a parent-scope temporary — the source is about
    /// to be dropped.
    fn escape_operand(&mut self, operand: Operand, ty: &Ty, span: Span) -> Result<Operand, Skip> {
        let Operand::Copy(place) = &operand else {
            return Ok(operand);
        };
        let root = place.local;
        let in_scope = self
            .scopes
            .last()
            .is_some_and(|scope| scope.contains(&root));
        if !in_scope || !self.tracked(root) || self.is_gone(root, &[]) {
            return Ok(operand);
        }
        let local = LocalId(u32::try_from(self.locals.len()).expect("local count fits u32"));
        self.locals.push(LocalDecl {
            ty: ty.clone(),
            name: None,
            span,
        });
        let depth = self.scopes.len() - 2;
        self.scopes[depth].push(local);
        let place = Place::local(local);
        self.push(Statement::Assign {
            place: place.clone(),
            rvalue: Rvalue::Use(operand),
        });
        Ok(Operand::Copy(place))
    }

    /// Lower one statement; `false` means the statement diverged.
    fn stmt(&mut self, stmt: &Stmt) -> Result<bool, Skip> {
        match &stmt.kind {
            StmtKind::Binding(def) => self.binding_stmt(def, stmt.span),
            StmtKind::Const(def) => {
                if let (Res::Symbol(symbol), Some(value)) = (&def.symbol, &def.value) {
                    if let Some(ty) = self.cx.types.type_of(*symbol) {
                        if let Ok(constant) = literal_const(value, ty) {
                            self.local_consts.insert(*symbol, constant);
                        }
                    }
                }
                Ok(true)
            }
            StmtKind::Expr(expr) => {
                let ty = self.expr_ty(expr)?;
                let Some(value) = self.expr(expr)? else {
                    return Ok(false);
                };
                // An expression statement is a by-value use; a non-`Copy`
                // result is owned by a temporary and dropped with the
                // scope. A discarded place stays put and is dropped where
                // it lives.
                if !self.is_copy(&ty) {
                    if let Value::Operand(operand) = value {
                        let local = self.temp(ty.clone(), expr.span);
                        let place = Place::local(local);
                        self.store(&place, Rvalue::Use(operand))?;
                    }
                }
                Ok(true)
            }
            StmtKind::Err => Err("body contains parse errors".to_owned()),
        }
    }

    fn binding_stmt(&mut self, def: &BindingDef, span: Span) -> Result<bool, Skip> {
        let symbol = match &def.pat.kind {
            PatKind::Binding(res) => match self.binding_kind(res)? {
                BindingKind::Local(symbol) => Some(symbol),
                BindingKind::Variant(_) => {
                    return Err("destructuring `let` patterns are not lowered in v0".to_owned());
                }
            },
            PatKind::Wildcard => None,
            _ => return Err("destructuring `let` patterns are not lowered in v0".to_owned()),
        };
        let Some(init) = &def.init else {
            // Deferred initialization: the local exists, uninitialized.
            if let Some(symbol) = symbol {
                self.local_for(symbol, span)?;
            }
            return Ok(true);
        };
        let ty = self.expr_ty(init)?;
        let Some(value) = self.expr(init)? else {
            return Ok(false);
        };
        let operand = self.use_value(value, &ty);
        match symbol {
            Some(symbol) => {
                let local = self.local_for(symbol, span)?;
                self.store(&Place::local(local), Rvalue::Use(operand))?;
            }
            None => {
                // `let _ = …;` consumes the value; park it in a
                // temporary so the scope drops it.
                if !self.is_copy(&ty) {
                    let local = self.temp(ty.clone(), span);
                    self.store(&Place::local(local), Rvalue::Use(operand))?;
                }
            }
        }
        Ok(true)
    }

    fn local_for(&mut self, symbol: SymbolId, span: Span) -> Result<LocalId, Skip> {
        if let Some(local) = self.sym_locals.get(&symbol) {
            return Ok(*local);
        }
        let ty = self.symbol_ty(symbol)?;
        let info = self.cx.resolution.symbol(symbol);
        let span = info.declaration.unwrap_or(span);
        let name = info.name.clone();
        let local = self.new_local(ty, Some(name), span);
        self.sym_locals.insert(symbol, local);
        Ok(local)
    }

    // ------------------------------------------------------------------
    // Expressions
    // ------------------------------------------------------------------

    fn expr(&mut self, expr: &Expr) -> Lowered {
        match &expr.kind {
            ExprKind::Lit(lit) => {
                let ty = self.expr_ty(expr)?;
                Ok(Some(Value::Operand(Operand::Const(lit_const(
                    lit, &ty, false,
                )?))))
            }
            ExprKind::Path { res, .. } => self.path_expr(expr, res),
            ExprKind::StructLit { ctor, fields, .. } => self.struct_lit(expr, ctor, fields),
            ExprKind::Array(elements) => self.array_lit(expr, elements),
            ExprKind::ArrayRepeat { value, .. } => self.array_repeat(expr, value),
            ExprKind::Unary { op, operand } => self.unary(expr, *op, operand),
            ExprKind::Binary { op, lhs, rhs } => self.binary(expr, *op, lhs, rhs),
            ExprKind::Range { lo, hi } => {
                let ty = self.expr_ty(expr)?;
                let Some(lo_value) = self.expr(lo)? else {
                    return Ok(None);
                };
                let lo_op = self.freeze_before(Self::read_value(lo_value), lo.span, &[hi])?;
                let Some(hi_value) = self.expr(hi)? else {
                    return Ok(None);
                };
                let hi_op = Self::read_value(hi_value);
                let place = self.assign_temp(
                    Rvalue::Aggregate {
                        kind: AggregateKind::Range,
                        fields: vec![lo_op, hi_op],
                    },
                    ty,
                    expr.span,
                )?;
                Ok(Some(Value::Place(place)))
            }
            ExprKind::Assign { target, value } => {
                let place = self.lower_place(target)?;
                let ty = self.expr_ty(value)?;
                let Some(rhs) = self.expr(value)? else {
                    return Ok(None);
                };
                let operand = self.use_value(rhs, &ty);
                self.store(&place, Rvalue::Use(operand))?;
                Ok(Some(Value::Operand(Operand::Const(Const::Unit))))
            }
            ExprKind::Call { callee, args } => self.call(expr, callee, args),
            ExprKind::MethodCall { .. } => {
                Err("method calls are not lowered in v0 (pending the trait system)".to_owned())
            }
            ExprKind::Field { receiver, name } => {
                let receiver_ty = self.expr_ty(receiver)?;
                let Some(value) = self.expr(receiver)? else {
                    return Ok(None);
                };
                let base = self.place_of(value, &receiver_ty, receiver.span)?;
                let (index, _) = self.field_index(&receiver_ty, name)?;
                let mut place = base;
                place.projection.push(Projection::Field(index));
                Ok(Some(Value::Place(place)))
            }
            ExprKind::Index { base, index } => self.index_expr(base, index),
            ExprKind::Try(inner) => self.try_expr(expr, inner),
            ExprKind::Cast { value, ty: _ } => self.cast(expr, value),
            ExprKind::If { cond, then, els } => self.if_expr(expr, cond, then, els.as_deref()),
            ExprKind::Match { scrutinee, arms } => self.match_expr(expr, scrutinee, arms),
            ExprKind::While { cond, body } => self.while_expr(cond, body),
            ExprKind::Loop { label, body } => self.loop_expr(expr, label.clone(), body),
            ExprKind::For { pat, iter, body } => self.for_expr(pat, iter, body),
            ExprKind::Unsafe(block) | ExprKind::Block(block) => self.block(block),
            ExprKind::Return(value) => {
                let operand = match value {
                    Some(value) => {
                        let ty = self.expr_ty(value)?;
                        let Some(lowered) = self.expr(value)? else {
                            return Ok(None);
                        };
                        self.use_value(lowered, &ty)
                    }
                    None => Operand::Const(Const::Unit),
                };
                self.emit_return(operand)?;
                Ok(None)
            }
            ExprKind::Break { label, value } => self.break_expr(label.as_deref(), value.as_deref()),
            ExprKind::Continue { label } => self.continue_expr(label.as_deref()),
            ExprKind::Err => Err("body contains parse errors".to_owned()),
        }
    }

    /// Freeze a `Copy`-place operand into a temporary so later sibling
    /// evaluation (in a multi-operand construct) cannot change what it
    /// reads. Only needed when some later sibling can actually write
    /// (contains an assignment or a call).
    fn freeze(&mut self, operand: Operand, span: Span) -> Result<Operand, Skip> {
        let Operand::Copy(place) = &operand else {
            return Ok(operand);
        };
        let ty = self.place_ty(place)?;
        let frozen = self.assign_temp(Rvalue::Use(operand), ty, span)?;
        Ok(Operand::Copy(frozen))
    }

    /// Freeze `operand` only when a later sibling expression can write.
    fn freeze_before(
        &mut self,
        operand: Operand,
        span: Span,
        later: &[&Expr],
    ) -> Result<Operand, Skip> {
        if later.iter().any(|expr| can_write(expr)) {
            self.freeze(operand, span)
        } else {
            Ok(operand)
        }
    }

    fn path_expr(&mut self, expr: &Expr, res: &Res) -> Lowered {
        let Res::Symbol(symbol) = res else {
            return Err("body contains unresolved names".to_owned());
        };
        match self.cx.resolution.symbol(*symbol).kind {
            SymbolKind::Param | SymbolKind::Local => {
                let local = self.local_for(*symbol, expr.span)?;
                Ok(Some(Value::Place(Place::local(local))))
            }
            SymbolKind::Const => {
                let constant = self
                    .local_consts
                    .get(symbol)
                    .or_else(|| self.cx.consts.get(symbol))
                    .cloned()
                    .ok_or_else(|| {
                        "`const` references are lowered only for literal initializers in v0"
                            .to_owned()
                    })?;
                Ok(Some(Value::Operand(Operand::Const(constant))))
            }
            SymbolKind::Variant => {
                // A payload-less variant value (`None`, a bare variant).
                let ty = self.expr_ty(expr)?;
                let (variant, fields, _) = self.ctor_variant(*symbol, &ty)?;
                if !fields.is_empty() {
                    return Err("payload-carrying variant used as a value".to_owned());
                }
                let place = self.assign_temp(
                    Rvalue::Aggregate {
                        kind: AggregateKind::Adt {
                            ty: ty.clone(),
                            variant,
                        },
                        fields: Vec::new(),
                    },
                    ty,
                    expr.span,
                )?;
                Ok(Some(Value::Place(place)))
            }
            SymbolKind::Function => {
                // A bare `fn` name used as a value: a compile-time-known code
                // pointer (ADR-0008 Tier 1). The type checker has already
                // rejected builtins and generics (`T0015`), so any function
                // symbol reaching here is a first-class value.
                Ok(Some(Value::Operand(Operand::Const(Const::Fn(*symbol)))))
            }
            _ => Err("unsupported name in value position".to_owned()),
        }
    }

    fn struct_lit(&mut self, expr: &Expr, ctor: &Res, inits: &[tuo_hir::FieldInit]) -> Lowered {
        let Res::Symbol(symbol) = ctor else {
            return Err("body contains unresolved names".to_owned());
        };
        let ty = self.expr_ty(expr)?;
        let (variant, declared, _) = self.ctor_variant(*symbol, &ty)?;
        // Evaluate in source order; the aggregate wants declaration
        // order. Freezing keeps reordering sound.
        let mut by_name: Vec<(String, Operand)> = Vec::new();
        for (position, init) in inits.iter().enumerate() {
            let field_ty = self.expr_ty(&init.value)?;
            let Some(value) = self.expr(&init.value)? else {
                return Ok(None);
            };
            let operand = self.use_value(value, &field_ty);
            let later: Vec<&Expr> = inits[position + 1..]
                .iter()
                .map(|init| &init.value)
                .collect();
            let operand = self.freeze_before(operand, init.value.span, &later)?;
            by_name.push((init.name.clone(), operand));
        }
        let mut fields = Vec::new();
        for (name, _) in &declared {
            let position = by_name
                .iter()
                .position(|(field, _)| field == name)
                .ok_or_else(|| "struct literal is missing a field".to_owned())?;
            fields.push(by_name[position].1.clone());
        }
        let place = self.assign_temp(
            Rvalue::Aggregate {
                kind: AggregateKind::Adt {
                    ty: ty.clone(),
                    variant,
                },
                fields,
            },
            ty,
            expr.span,
        )?;
        Ok(Some(Value::Place(place)))
    }

    /// Lower a list-form array literal `[a, b, c]` (ADR-0004 Stage 2):
    /// evaluate the elements left to right, then one `Aggregate` with
    /// [`AggregateKind::Array`]. Non-`Copy` element operands are `Move`,
    /// mirroring `Adt` field lowering.
    fn array_lit(&mut self, expr: &Expr, elements: &[Expr]) -> Lowered {
        let ty = self.expr_ty(expr)?;
        let Ty::FixedArray(element_ty, len) = &ty else {
            return Err("array literal has a non-array type (compiler bug guard)".to_owned());
        };
        let (element_ty, len) = ((**element_ty).clone(), *len);
        let mut fields = Vec::new();
        for (position, element) in elements.iter().enumerate() {
            let Some(value) = self.expr(element)? else {
                return Ok(None);
            };
            let operand = self.use_value(value, &element_ty);
            let later: Vec<&Expr> = elements[position + 1..].iter().collect();
            let operand = self.freeze_before(operand, element.span, &later)?;
            fields.push(operand);
        }
        let place = self.assign_temp(
            Rvalue::Aggregate {
                kind: AggregateKind::Array {
                    element: element_ty,
                    len,
                },
                fields,
            },
            ty,
            expr.span,
        )?;
        Ok(Some(Value::Place(place)))
    }

    /// Lower a repeat-form array literal `[x; N]` (ADR-0004 Stage 2):
    /// evaluate the operand **once**, then one `Aggregate` whose fields are
    /// `N` copies of `Operand::Copy` of it — legal because the front end
    /// required a `Copy` element (O0010). `N == 0` still evaluates the
    /// operand once (its value is then dead; the drop is a no-op since it
    /// is `Copy`). A deliberate O(N)-statements expansion — no
    /// `Rvalue::Repeat`, no loop — bounded by `MAX_FIXED_ARRAY_LEN`.
    fn array_repeat(&mut self, expr: &Expr, value: &Expr) -> Lowered {
        let ty = self.expr_ty(expr)?;
        let Ty::FixedArray(element_ty, len) = &ty else {
            return Err("array literal has a non-array type (compiler bug guard)".to_owned());
        };
        let (element_ty, len) = ((**element_ty).clone(), *len);
        if !self.is_copy(&element_ty) {
            // The front end rejects this (O0010); refuse rather than emit
            // an illegal N-fold duplication if it ever slips through.
            return Err("a repeat literal of a non-`Copy` element is not lowered".to_owned());
        }
        let Some(lowered) = self.expr(value)? else {
            return Ok(None);
        };
        let source = self.place_of(lowered, &element_ty, value.span)?;
        let fields = vec![
            Operand::Copy(source);
            usize::try_from(len).map_err(|_| {
                "fixed-array length exceeds the addressable range".to_owned()
            })?
        ];
        let place = self.assign_temp(
            Rvalue::Aggregate {
                kind: AggregateKind::Array {
                    element: element_ty,
                    len,
                },
                fields,
            },
            ty,
            expr.span,
        )?;
        Ok(Some(Value::Place(place)))
    }

    fn unary(&mut self, expr: &Expr, op: tuo_hir::UnOp, operand: &Expr) -> Lowered {
        let ty = self.expr_ty(expr)?;
        match op {
            tuo_hir::UnOp::Move => {
                // `move` is an explicit ownership transfer of a place.
                let place = self.lower_place(operand)?;
                self.mark_moved(&place);
                Ok(Some(Value::Operand(Operand::Move(place))))
            }
            tuo_hir::UnOp::Neg => {
                if let ExprKind::Lit(lit) = &operand.kind {
                    // Fold so `-9223372036854775808` is representable.
                    let constant = lit_const(lit, &ty, true)?;
                    return Ok(Some(Value::Operand(Operand::Const(constant))));
                }
                let Some(value) = self.expr(operand)? else {
                    return Ok(None);
                };
                let inner = Self::read_value(value);
                let place = self.assign_temp(
                    Rvalue::Unary {
                        op: UnOp::Neg,
                        operand: inner,
                    },
                    ty,
                    expr.span,
                )?;
                Ok(Some(Value::Place(place)))
            }
            tuo_hir::UnOp::Not => {
                let Some(value) = self.expr(operand)? else {
                    return Ok(None);
                };
                let inner = Self::read_value(value);
                let place = self.assign_temp(
                    Rvalue::Unary {
                        op: UnOp::Not,
                        operand: inner,
                    },
                    ty,
                    expr.span,
                )?;
                Ok(Some(Value::Place(place)))
            }
        }
    }

    fn binary(&mut self, expr: &Expr, op: tuo_hir::BinOp, lhs: &Expr, rhs: &Expr) -> Lowered {
        use tuo_hir::BinOp as H;
        if matches!(op, H::And | H::Or) {
            return self.short_circuit(expr, op, lhs, rhs);
        }
        let ty = self.expr_ty(expr)?;
        let mir_op = match op {
            H::Add => BinOp::Add,
            H::Sub => BinOp::Sub,
            H::Mul => BinOp::Mul,
            H::Div => BinOp::Div,
            H::Rem => BinOp::Rem,
            H::Eq => BinOp::Eq,
            H::Ne => BinOp::Ne,
            H::Lt => BinOp::Lt,
            H::Le => BinOp::Le,
            H::Gt => BinOp::Gt,
            H::Ge => BinOp::Ge,
            H::And | H::Or => unreachable!("handled above"),
        };
        let Some(lhs_value) = self.expr(lhs)? else {
            return Ok(None);
        };
        let lhs_op = self.freeze_before(Self::read_value(lhs_value), lhs.span, &[rhs])?;
        let Some(rhs_value) = self.expr(rhs)? else {
            return Ok(None);
        };
        let rhs_op = Self::read_value(rhs_value);
        let place = self.assign_temp(
            Rvalue::Binary {
                op: mir_op,
                lhs: lhs_op,
                rhs: rhs_op,
            },
            ty,
            expr.span,
        )?;
        Ok(Some(Value::Place(place)))
    }

    /// `&&` / `||` become control flow: the right operand only evaluates
    /// when the left one does not decide the result.
    fn short_circuit(
        &mut self,
        expr: &Expr,
        op: tuo_hir::BinOp,
        lhs: &Expr,
        rhs: &Expr,
    ) -> Lowered {
        let result = self.temp(Ty::Bool, expr.span);
        let result_place = Place::local(result);
        let Some(lhs_value) = self.expr(lhs)? else {
            return Ok(None);
        };
        let lhs_op = Self::read_value(lhs_value);
        let rhs_block = self.new_block();
        let short_block = self.new_block();
        let join = self.new_block();
        let is_and = matches!(op, tuo_hir::BinOp::And);
        if is_and {
            self.terminate(Terminator::Branch {
                cond: lhs_op,
                then_block: rhs_block,
                else_block: short_block,
            });
        } else {
            self.terminate(Terminator::Branch {
                cond: lhs_op,
                then_block: short_block,
                else_block: rhs_block,
            });
        }
        self.switch_to(short_block);
        self.store(
            &result_place,
            Rvalue::Use(Operand::Const(Const::Bool(!is_and))),
        )?;
        self.terminate(Terminator::Goto(join));
        self.switch_to(rhs_block);
        if let Some(rhs_value) = self.expr(rhs)? {
            let rhs_op = Self::read_value(rhs_value);
            self.store(&result_place, Rvalue::Use(rhs_op))?;
            self.terminate(Terminator::Goto(join));
        }
        self.switch_to(join);
        Ok(Some(Value::Place(result_place)))
    }

    fn call(&mut self, expr: &Expr, callee: &Expr, args: &[Expr]) -> Lowered {
        // A direct call: the callee is a bare path resolving to a user
        // function symbol. Anything else is an indirect call through a
        // function value (ADR-0008 Tier 1).
        if let ExprKind::Path {
            res: Res::Symbol(symbol),
            ..
        } = &callee.kind
        {
            if self.cx.resolution.symbol(*symbol).kind == SymbolKind::Function {
                // A builtin function (ADR-0006) has no body to call: the call
                // lowers to its dedicated MIR form instead.
                if let Some(builtin) = self.cx.resolution.builtin(*symbol) {
                    return self.builtin_call(expr, builtin, args);
                }
                let modes = self
                    .cx
                    .modes
                    .get(symbol)
                    .cloned()
                    .ok_or_else(|| "callee has no lowered signature".to_owned())?;
                let Some(lowered_args) = self.lower_call_args(args, &modes)? else {
                    return Ok(None);
                };
                return self.emit_call(expr, Callee::Direct(*symbol), lowered_args);
            }
        }
        // Indirect call: the callee expression's checked type is a `Ty::Fn`
        // whose modes drive the argument passing (ADR-0008 Tier 1).
        let modes = self.indirect_call_modes(callee)?;
        let Some(callee_value) = self.expr(callee)? else {
            return Ok(None);
        };
        let callee_ty = self.expr_ty(callee)?;
        let callee_operand = self.use_value(callee_value, &callee_ty);
        let Some(lowered_args) = self.lower_call_args(args, &modes)? else {
            return Ok(None);
        };
        self.emit_call(expr, Callee::Indirect(callee_operand), lowered_args)
    }

    /// The per-argument [`PassMode`]s for an indirect call, read from the
    /// callee's checked function type (ADR-0008 Tier 1).
    fn indirect_call_modes(&self, callee: &Expr) -> Result<Vec<PassMode>, String> {
        let ty = self.expr_ty(callee)?;
        let Ty::Fn(fn_ty) = ty else {
            return Err("indirect call through a non-function value".to_owned());
        };
        Ok(fn_ty
            .params
            .iter()
            .map(|param| match param.mode {
                tuo_types::ParamMode::Take => PassMode::Value,
                tuo_types::ParamMode::In => PassMode::Borrow,
                tuo_types::ParamMode::Mut => PassMode::BorrowMut,
            })
            .collect())
    }

    /// Lower a call's arguments under the given per-argument modes, shared by
    /// direct and indirect calls. Returns `None` when an argument diverges.
    fn lower_call_args(
        &mut self,
        args: &[Expr],
        modes: &[PassMode],
    ) -> Result<Option<Vec<Arg>>, String> {
        let mut lowered_args = Vec::new();
        for (position, (arg, mode)) in args.iter().zip(modes.iter()).enumerate() {
            let arg_ty = self.expr_ty(arg)?;
            match mode {
                PassMode::Value => {
                    let Some(value) = self.expr(arg)? else {
                        return Ok(None);
                    };
                    let operand = self.use_value(value, &arg_ty);
                    let later: Vec<&Expr> = args[position + 1..].iter().collect();
                    let operand = self.freeze_before(operand, arg.span, &later)?;
                    lowered_args.push(Arg::Value(operand));
                }
                PassMode::Borrow => {
                    let Some(value) = self.expr(arg)? else {
                        return Ok(None);
                    };
                    let place = self.place_of(value, &arg_ty, arg.span)?;
                    lowered_args.push(Arg::Borrow(place));
                }
                PassMode::BorrowMut => {
                    let place = self.lower_place(arg)?;
                    lowered_args.push(Arg::BorrowMut(place));
                }
            }
        }
        Ok(Some(lowered_args))
    }

    /// Emit a [`Statement::Call`] with the given callee and lowered args,
    /// storing the return value in a fresh temporary.
    fn emit_call(&mut self, expr: &Expr, callee: Callee, args: Vec<Arg>) -> Lowered {
        let ret_ty = self.expr_ty(expr)?;
        let dest = self.temp(ret_ty, expr.span);
        let dest_place = Place::local(dest);
        self.push(Statement::Call {
            dest: dest_place.clone(),
            callee,
            args,
        });
        self.uninit.remove(&dest);
        Ok(Some(Value::Place(dest_place)))
    }

    /// Lower a call to one of the builtin functions (ADR-0006/ADR-0009
    /// Stage A) into its dedicated MIR form: `std::str` builtins become a
    /// pure [`Rvalue::StrOp`] assignment, `std::rt` builtins become a
    /// [`Statement::Effect`], the non-mutating `std::string`/`std::array`
    /// builtins become a pure [`Rvalue::HeapOp`] assignment, and the
    /// in-place mutators become a [`Statement::HeapMutate`].
    ///
    /// Each argument lowers by its declared [`BuiltinParamMode`], mirroring
    /// [`Self::call`]'s per-mode discipline: a `take` argument (and a
    /// `Copy`-typed `in` argument — `Int`, `Str`) is an ordinary operand
    /// with the evaluation-order freeze; a non-`Copy` `in` argument (an
    /// owned `String`/`Array` lent read-only) is a borrow-read **place**;
    /// and a `mut` argument is a mutable place, exactly like a
    /// [`PassMode::BorrowMut`] call argument.
    fn builtin_call(&mut self, expr: &Expr, builtin: Builtin, args: &[Expr]) -> Lowered {
        enum LoweredArg {
            Operand(Operand),
            Borrow(Place),
            Mut(Place),
        }
        let modes = builtin.param_modes();
        let mut lowered: Vec<LoweredArg> = Vec::with_capacity(args.len());
        for (position, arg) in args.iter().enumerate() {
            let mode = modes
                .get(position)
                .copied()
                .unwrap_or(tuo_resolve::BuiltinParamMode::Take);
            let arg_ty = self.expr_ty(arg)?;
            match mode {
                tuo_resolve::BuiltinParamMode::In if !self.is_copy(&arg_ty) => {
                    let Some(value) = self.expr(arg)? else {
                        return Ok(None);
                    };
                    let place = self.place_of(value, &arg_ty, arg.span)?;
                    lowered.push(LoweredArg::Borrow(place));
                }
                tuo_resolve::BuiltinParamMode::Take | tuo_resolve::BuiltinParamMode::In => {
                    let Some(value) = self.expr(arg)? else {
                        return Ok(None);
                    };
                    let operand = self.use_value(value, &arg_ty);
                    let later: Vec<&Expr> = args[position + 1..].iter().collect();
                    let operand = self.freeze_before(operand, arg.span, &later)?;
                    lowered.push(LoweredArg::Operand(operand));
                }
                tuo_resolve::BuiltinParamMode::Mut => {
                    let place = self.lower_place(arg)?;
                    lowered.push(LoweredArg::Mut(place));
                }
            }
        }
        // Split the lowered arguments into the shapes the MIR forms carry.
        let operands: Vec<Operand> = lowered
            .iter()
            .filter_map(|arg| match arg {
                LoweredArg::Operand(operand) => Some(operand.clone()),
                LoweredArg::Borrow(_) | LoweredArg::Mut(_) => None,
            })
            .collect();
        let first_borrow = lowered.iter().find_map(|arg| match arg {
            LoweredArg::Borrow(place) => Some(place.clone()),
            _ => None,
        });
        let first_mut = lowered.iter().find_map(|arg| match arg {
            LoweredArg::Mut(place) => Some(place.clone()),
            _ => None,
        });
        let ret_ty = self.expr_ty(expr)?;
        let dest = self.temp(ret_ty, expr.span);
        let dest_place = Place::local(dest);
        let statement = match builtin {
            Builtin::StrLen | Builtin::StrByteAt | Builtin::StrSlice => {
                let op = match builtin {
                    Builtin::StrLen => StrOp::Len,
                    Builtin::StrByteAt => StrOp::ByteAt,
                    _ => StrOp::Slice,
                };
                Statement::Assign {
                    place: dest_place.clone(),
                    rvalue: Rvalue::StrOp { op, args: operands },
                }
            }
            Builtin::RtWrite
            | Builtin::RtReadByte
            | Builtin::RtExit
            | Builtin::RtWriteString
            | Builtin::RtParMap
            | Builtin::RtNowNanos
            | Builtin::RtArgCount
            | Builtin::RtArgByte
            | Builtin::RtOpen
            | Builtin::RtClose
            | Builtin::RtRemoveFile
            | Builtin::RtListen
            | Builtin::RtBoundPort
            | Builtin::RtAccept
            | Builtin::RtConnect
            | Builtin::RtChanNew
            | Builtin::RtChanSend
            | Builtin::RtChanRecv
            | Builtin::RtChanClose
            | Builtin::RtMutexNew
            | Builtin::RtMutexLock
            | Builtin::RtMutexUnlock => {
                let op = match builtin {
                    Builtin::RtWrite => EffectOp::Write,
                    Builtin::RtReadByte => EffectOp::ReadByte,
                    Builtin::RtExit => EffectOp::Exit,
                    Builtin::RtParMap => EffectOp::ParMap,
                    Builtin::RtNowNanos => EffectOp::NowNanos,
                    Builtin::RtArgCount => EffectOp::ArgCount,
                    Builtin::RtArgByte => EffectOp::ArgByte,
                    Builtin::RtOpen => EffectOp::Open,
                    Builtin::RtClose => EffectOp::Close,
                    Builtin::RtRemoveFile => EffectOp::RemoveFile,
                    Builtin::RtListen => EffectOp::Listen,
                    Builtin::RtBoundPort => EffectOp::BoundPort,
                    Builtin::RtAccept => EffectOp::Accept,
                    Builtin::RtConnect => EffectOp::Connect,
                    Builtin::RtChanNew => EffectOp::ChanNew,
                    Builtin::RtChanSend => EffectOp::ChanSend,
                    Builtin::RtChanRecv => EffectOp::ChanRecv,
                    Builtin::RtChanClose => EffectOp::ChanClose,
                    Builtin::RtMutexNew => EffectOp::MutexNew,
                    Builtin::RtMutexLock => EffectOp::MutexLock,
                    Builtin::RtMutexUnlock => EffectOp::MutexUnlock,
                    _ => EffectOp::WriteString,
                };
                // Effect arguments are call-style: operands pass by value,
                // and `write_string`'s `String` (and `par_map`'s tasks
                // array) is a read-only borrow, appended after the operands.
                let mut effect_args: Vec<Arg> = operands.into_iter().map(Arg::Value).collect();
                if let Some(place) = first_borrow {
                    effect_args.push(Arg::Borrow(place));
                }
                Statement::Effect {
                    op,
                    args: effect_args,
                    dest: dest_place.clone(),
                }
            }
            Builtin::StringEmpty
            | Builtin::StringFromStr
            | Builtin::StringConcat
            | Builtin::StringLen
            | Builtin::StringByteAt
            | Builtin::StringSlice
            | Builtin::StringAsStr
            | Builtin::ArrayEmpty
            | Builtin::ArrayLen
            | Builtin::ArrayGet
            | Builtin::MapEmpty
            | Builtin::MapGet
            | Builtin::MapContainsKey
            | Builtin::MapLen
            | Builtin::MapKeys => {
                let op = match builtin {
                    Builtin::StringEmpty => HeapOp::StringEmpty,
                    Builtin::StringFromStr => HeapOp::StringFromStr,
                    Builtin::StringConcat => HeapOp::StringConcat,
                    Builtin::StringLen => HeapOp::StringLen,
                    Builtin::StringByteAt => HeapOp::StringByteAt,
                    Builtin::StringSlice => HeapOp::StringSlice,
                    Builtin::StringAsStr => HeapOp::StringAsStr,
                    Builtin::ArrayEmpty => HeapOp::ArrayEmpty,
                    Builtin::ArrayLen => HeapOp::ArrayLen,
                    Builtin::MapEmpty => HeapOp::MapEmpty,
                    Builtin::MapGet => HeapOp::MapGet,
                    Builtin::MapContainsKey => HeapOp::MapContainsKey,
                    Builtin::MapLen => HeapOp::MapLen,
                    Builtin::MapKeys => HeapOp::MapKeys,
                    _ => HeapOp::ArrayGet,
                };
                Statement::Assign {
                    place: dest_place.clone(),
                    rvalue: Rvalue::HeapOp {
                        op,
                        subject: first_borrow,
                        args: operands,
                    },
                }
            }
            Builtin::StringPushByte
            | Builtin::StringAppend
            | Builtin::ArrayPush
            | Builtin::ArrayPop
            | Builtin::ArraySet
            | Builtin::MapInsert
            | Builtin::MapRemove => {
                let op = match builtin {
                    Builtin::StringPushByte => HeapMutOp::PushByte,
                    Builtin::StringAppend => HeapMutOp::Append,
                    Builtin::ArrayPush => HeapMutOp::Push,
                    Builtin::ArraySet => HeapMutOp::Set,
                    Builtin::MapInsert => HeapMutOp::MapInsert,
                    Builtin::MapRemove => HeapMutOp::MapRemove,
                    _ => HeapMutOp::Pop,
                };
                let target = first_mut
                    .ok_or_else(|| "a heap mutator call has no `mut` place argument".to_owned())?;
                Statement::HeapMutate {
                    op,
                    target,
                    args: operands,
                    dest: dest_place.clone(),
                }
            }
        };
        self.push(statement);
        self.uninit.remove(&dest);
        Ok(Some(Value::Place(dest_place)))
    }

    fn index_expr(&mut self, base: &Expr, index: &Expr) -> Lowered {
        let base_ty = self.expr_ty(base)?;
        let Some(base_value) = self.expr(base)? else {
            return Ok(None);
        };
        let base_place = self.place_of(base_value, &base_ty, base.span)?;
        let Some(index_value) = self.expr(index)? else {
            return Ok(None);
        };
        let index_op = Self::read_value(index_value);
        let index_place =
            self.assign_temp(Rvalue::Use(index_op), Ty::Int(IntKind::Usize), index.span)?;
        // A `[T; N]`'s length never exists at runtime: it is a constant,
        // and `Rvalue::Len` stays a growable-`Array`-only operation
        // (ADR-0004 Stage 2). Everything downstream of the `len` temp —
        // the `Lt` compare, the `Assert`, the `Index` projection — is
        // identical for both array types.
        let len_rvalue = match &base_ty {
            Ty::FixedArray(_, n) => {
                Rvalue::Use(Operand::Const(Const::Int(i128::from(*n), IntKind::Usize)))
            }
            _ => Rvalue::Len(base_place.clone()),
        };
        let len = self.assign_temp(len_rvalue, Ty::Int(IntKind::Usize), index.span)?;
        let cond = self.assign_temp(
            Rvalue::Binary {
                op: BinOp::Lt,
                lhs: Operand::Copy(index_place.clone()),
                rhs: Operand::Copy(len),
            },
            Ty::Bool,
            index.span,
        )?;
        let target = self.new_block();
        self.terminate(Terminator::Assert {
            cond: Operand::Copy(cond),
            trap: Trap::IndexOutOfBounds,
            target,
        });
        self.switch_to(target);
        let mut place = base_place;
        place.projection.push(Projection::Index(index_place.local));
        Ok(Some(Value::Place(place)))
    }

    fn cast(&mut self, expr: &Expr, value: &Expr) -> Lowered {
        let from = self.expr_ty(value)?;
        let to = self.expr_ty(expr)?;
        let Some(lowered) = self.expr(value)? else {
            return Ok(None);
        };
        if from == to {
            return Ok(Some(lowered));
        }
        let operand = Self::read_value(lowered);
        let kind = match (&from, &to) {
            (Ty::Int(_), Ty::Int(_)) => CastKind::IntToInt,
            (Ty::Int(_), Ty::Float(_)) => CastKind::IntToFloat,
            (Ty::Float(_), Ty::Int(_)) => CastKind::FloatToInt,
            (Ty::Float(_), Ty::Float(_)) => CastKind::FloatToFloat,
            _ => return Err("unsupported cast".to_owned()),
        };
        let place = self.assign_temp(
            Rvalue::Cast {
                kind,
                operand,
                to: to.clone(),
            },
            to,
            expr.span,
        )?;
        Ok(Some(Value::Place(place)))
    }

    /// Terminate the current block with a full-function exit: drops for
    /// every scope, then `return`. The operand was formed first, so a
    /// moved-out local is already marked and will not be dropped; a
    /// `Copy` read *out of* something the exit is about to drop is frozen
    /// into a temporary first.
    fn emit_return(&mut self, operand: Operand) -> Result<(), Skip> {
        let operand = match operand {
            Operand::Copy(place)
                if self.tracked(place.local)
                    && !self.is_gone(place.local, &[])
                    && !self.borrowed_params.contains(&place.local) =>
            {
                let ty = self.place_ty(&place)?;
                let span = self.fn_span;
                let frozen = self.assign_temp(Rvalue::Use(Operand::Copy(place)), ty, span)?;
                Operand::Copy(frozen)
            }
            other => other,
        };
        self.drops_from(0)?;
        self.terminate(Terminator::Return(operand));
        Ok(())
    }
}

impl FnLower<'_> {
    // ------------------------------------------------------------------
    // Control flow
    // ------------------------------------------------------------------

    /// A result slot for a value-producing branching expression (`None`
    /// for `()`-typed and diverging ones, which need no storage).
    fn branch_result(&mut self, ty: &Ty, span: Span) -> Option<LocalId> {
        if matches!(ty, Ty::Unit | Ty::Never) {
            None
        } else {
            Some(self.temp(ty.clone(), span))
        }
    }

    /// Close one branch: store its value into the result slot, then jump
    /// to the (lazily created) join block. Returns the branch-end state
    /// when the branch stayed live.
    fn close_branch(
        &mut self,
        value: Option<Value>,
        ty: &Ty,
        result: Option<LocalId>,
        join: &mut Option<BlockId>,
    ) -> Result<Option<State>, Skip> {
        let Some(value) = value else {
            return Ok(None);
        };
        let operand = self.use_value(value, ty);
        if let Some(result) = result {
            self.store(&Place::local(result), Rvalue::Use(operand))?;
        }
        let target = *join.get_or_insert_with(|| {
            self.blocks.push(BlockBuilder {
                statements: Vec::new(),
                terminator: None,
            });
            BlockId(u32::try_from(self.blocks.len() - 1).expect("block count fits u32"))
        });
        self.terminate(Terminator::Goto(target));
        Ok(Some(self.snapshot()))
    }

    /// Merge to the join block, defensively requiring all live branch
    /// states to agree; diverges when no branch was live.
    fn join_branches(
        &mut self,
        states: Vec<State>,
        join: Option<BlockId>,
        result: Option<LocalId>,
    ) -> Result<Option<Value>, Skip> {
        let Some(join) = join else {
            return Ok(None);
        };
        let first = states.first().cloned().expect("join implies a live branch");
        for state in &states[1..] {
            if !self.states_agree(state, &first) {
                return Err(
                    "initialization state diverged across merging paths (v0 lowering limit)"
                        .to_owned(),
                );
            }
        }
        self.restore(&first);
        self.switch_to(join);
        Ok(Some(match result {
            Some(result) => Value::Place(Place::local(result)),
            None => Value::Operand(Operand::Const(Const::Unit)),
        }))
    }

    fn if_expr(&mut self, expr: &Expr, cond: &Expr, then: &Block, els: Option<&Expr>) -> Lowered {
        let ty = self.expr_ty(expr)?;
        let result = self.branch_result(&ty, expr.span);
        let Some(cond_value) = self.expr(cond)? else {
            return Ok(None);
        };
        let cond_op = Self::read_value(cond_value);
        let then_block = self.new_block();
        let else_block = self.new_block();
        self.terminate(Terminator::Branch {
            cond: cond_op,
            then_block,
            else_block,
        });
        let before = self.snapshot();
        let mut join = None;
        let mut states = Vec::new();
        self.switch_to(then_block);
        let then_value = self.block(then)?;
        states.extend(self.close_branch(then_value, &ty, result, &mut join)?);
        self.restore(&before);
        self.switch_to(else_block);
        let else_value = match els {
            // The else arm is a bare expression (an `else if` chain is a
            // nested `if` expression, not a block), so it gets its own scope:
            // otherwise a temporary the arm creates — e.g. the nested `if`'s
            // result slot — lands in the enclosing scope and the merge states
            // disagree (initialized on one path, absent on the other).
            Some(els) => self.scoped_value(els, &ty)?,
            None => Some(Value::Operand(Operand::Const(Const::Unit))),
        };
        states.extend(self.close_branch(else_value, &ty, result, &mut join)?);
        self.join_branches(states, join, result)
    }

    /// Lower a branch-arm expression inside its own scope, mirroring
    /// [`Self::block`]'s tail handling: the value escapes as an operand
    /// before the scope's drops run, so temporaries the arm creates never
    /// leak into the branch-merge state.
    fn scoped_value(&mut self, expr: &Expr, ty: &Ty) -> Lowered {
        self.push_scope();
        let Some(value) = self.expr(expr)? else {
            self.discard_scope();
            return Ok(None);
        };
        let operand = self.use_value(value, ty);
        let operand = self.escape_operand(operand, ty, expr.span)?;
        self.end_scope()?;
        Ok(Some(Value::Operand(operand)))
    }

    fn while_expr(&mut self, cond: &Expr, body: &Block) -> Lowered {
        let head = self.new_block();
        self.terminate(Terminator::Goto(head));
        self.switch_to(head);
        let head_state = self.snapshot();
        // Condition temporaries live per iteration: scope them so the
        // back edge re-enters the head with an unchanged state.
        self.push_scope();
        let Some(cond_value) = self.expr(cond)? else {
            self.discard_scope();
            return Ok(None);
        };
        let cond_op = Self::read_value(cond_value);
        self.end_scope()?;
        self.expect_state(&head_state)?;
        let body_block = self.new_block();
        let exit_block = self.new_block();
        self.terminate(Terminator::Branch {
            cond: cond_op,
            then_block: body_block,
            else_block: exit_block,
        });
        self.loops.push(LoopCtx {
            label: None,
            break_block: exit_block,
            continue_block: head,
            result: None,
            depth: self.scopes.len(),
            head_state: head_state.clone(),
            exit_state: Some(head_state.clone()),
        });
        self.switch_to(body_block);
        if let Some(_unit) = self.block(body)? {
            self.expect_state(&head_state)?;
            self.terminate(Terminator::Goto(head));
        }
        self.loops.pop();
        self.restore(&head_state);
        self.switch_to(exit_block);
        Ok(Some(Value::Operand(Operand::Const(Const::Unit))))
    }

    fn loop_expr(&mut self, expr: &Expr, label: Option<String>, body: &Block) -> Lowered {
        let ty = self.expr_ty(expr)?;
        let result = self.branch_result(&ty, expr.span);
        let head = self.new_block();
        let exit_block = self.new_block();
        self.terminate(Terminator::Goto(head));
        self.switch_to(head);
        let head_state = self.snapshot();
        self.loops.push(LoopCtx {
            label,
            break_block: exit_block,
            continue_block: head,
            result,
            depth: self.scopes.len(),
            head_state: head_state.clone(),
            exit_state: None,
        });
        if let Some(_unit) = self.block(body)? {
            self.expect_state(&head_state)?;
            self.terminate(Terminator::Goto(head));
        }
        let ctx = self.loops.pop().expect("loop context pushed above");
        let Some(exit_state) = ctx.exit_state else {
            // No `break`: the loop diverges and the exit is unreachable.
            return Ok(None);
        };
        self.restore(&exit_state);
        self.switch_to(exit_block);
        Ok(Some(match result {
            Some(result) => Value::Place(Place::local(result)),
            None => Value::Operand(Operand::Const(Const::Unit)),
        }))
    }

    fn for_expr(&mut self, pat: &Pat, iter: &Expr, body: &Block) -> Lowered {
        let binding = match &pat.kind {
            PatKind::Binding(res) => match self.binding_kind(res)? {
                BindingKind::Local(symbol) => Some(symbol),
                BindingKind::Variant(_) => {
                    return Err("destructuring `for` patterns are not lowered in v0".to_owned());
                }
            },
            PatKind::Wildcard => None,
            _ => return Err("destructuring `for` patterns are not lowered in v0".to_owned()),
        };
        let iter_ty = self.expr_ty(iter)?;
        self.push_scope();
        let outcome = match &iter_ty {
            Ty::Range(item) => self.for_range(binding, iter, item, body),
            Ty::Array(item) | Ty::FixedArray(item, _) => self.for_array(binding, iter, item, body),
            _ => Err("`for` iterates ranges and arrays in v0".to_owned()),
        };
        match outcome {
            Ok(Some(())) => {
                self.end_scope()?;
                Ok(Some(Value::Operand(Operand::Const(Const::Unit))))
            }
            Ok(None) => {
                self.discard_scope();
                Ok(None)
            }
            Err(skip) => Err(skip),
        }
    }

    fn for_range(
        &mut self,
        binding: Option<SymbolId>,
        iter: &Expr,
        item: &Ty,
        body: &Block,
    ) -> Result<Option<()>, Skip> {
        // Split a literal `a .. b` to skip the aggregate round-trip;
        // anything else lands in a range temporary first.
        let (lo_op, hi_op) = if let ExprKind::Range { lo, hi } = &iter.kind {
            let Some(lo_value) = self.expr(lo)? else {
                return Ok(None);
            };
            let lo_op = self.freeze_before(Self::read_value(lo_value), lo.span, &[hi])?;
            let Some(hi_value) = self.expr(hi)? else {
                return Ok(None);
            };
            (lo_op, Self::read_value(hi_value))
        } else {
            let Some(value) = self.expr(iter)? else {
                return Ok(None);
            };
            let range_ty = Ty::Range(Box::new(item.clone()));
            let place = self.place_of(value, &range_ty, iter.span)?;
            let mut lo_place = place.clone();
            lo_place.projection.push(Projection::Field(0));
            let mut hi_place = place;
            hi_place.projection.push(Projection::Field(1));
            (Operand::Copy(lo_place), Operand::Copy(hi_place))
        };
        let cursor = match binding {
            Some(symbol) => self.local_for(symbol, iter.span)?,
            None => self.temp(item.clone(), iter.span),
        };
        let cursor_place = Place::local(cursor);
        self.store(&cursor_place, Rvalue::Use(lo_op))?;
        let end = self.assign_temp(Rvalue::Use(hi_op), item.clone(), iter.span)?;
        let head = self.new_block();
        self.terminate(Terminator::Goto(head));
        self.switch_to(head);
        let head_state = self.snapshot();
        let cond = self.assign_temp(
            Rvalue::Binary {
                op: BinOp::Lt,
                lhs: Operand::Copy(cursor_place.clone()),
                rhs: Operand::Copy(end),
            },
            Ty::Bool,
            iter.span,
        )?;
        let body_block = self.new_block();
        let step_block = self.new_block();
        let exit_block = self.new_block();
        self.terminate(Terminator::Branch {
            cond: Operand::Copy(cond),
            then_block: body_block,
            else_block: exit_block,
        });
        self.loops.push(LoopCtx {
            label: None,
            break_block: exit_block,
            continue_block: step_block,
            result: None,
            depth: self.scopes.len(),
            head_state: head_state.clone(),
            exit_state: Some(head_state.clone()),
        });
        self.switch_to(body_block);
        let live = self.block(body)?.is_some();
        if live {
            self.expect_state(&head_state)?;
            self.terminate(Terminator::Goto(step_block));
        }
        self.loops.pop();
        self.restore(&head_state);
        self.switch_to(step_block);
        let one = Operand::Const(Const::Int(
            1,
            match item {
                Ty::Int(kind) => *kind,
                _ => IntKind::I64,
            },
        ));
        self.store(
            &cursor_place,
            Rvalue::Binary {
                op: BinOp::Add,
                lhs: Operand::Copy(cursor_place.clone()),
                rhs: one,
            },
        )?;
        self.terminate(Terminator::Goto(head));
        self.restore(&head_state);
        self.switch_to(exit_block);
        Ok(Some(()))
    }

    fn for_array(
        &mut self,
        binding: Option<SymbolId>,
        iter: &Expr,
        item: &Ty,
        body: &Block,
    ) -> Result<Option<()>, Skip> {
        if !self.is_copy(item) {
            return Err(
                "iterating an array of non-`Copy` elements is not lowered in v0".to_owned(),
            );
        }
        // The iterable is either the growable `Array[T]` or the fixed
        // `[T; N]` (ADR-0004 Stage 2); the counted loop is identical, only
        // the `len` source differs below.
        let array_ty = self.expr_ty(iter)?;
        let Some(value) = self.expr(iter)? else {
            return Ok(None);
        };
        // The iterable is a by-value use: the loop owns the array.
        let operand = self.use_value(value, &array_ty);
        let array = match operand {
            Operand::Move(_) | Operand::Const(_) => {
                let local = self.temp(array_ty.clone(), iter.span);
                let place = Place::local(local);
                self.store(&place, Rvalue::Use(operand))?;
                place
            }
            Operand::Copy(place) => place,
        };
        let index = self.assign_temp(
            Rvalue::Use(Operand::Const(Const::Int(0, IntKind::Usize))),
            Ty::Int(IntKind::Usize),
            iter.span,
        )?;
        // As in `index_expr`: a `[T; N]`'s length is a constant, never a
        // `Rvalue::Len`.
        let len_rvalue = match &array_ty {
            Ty::FixedArray(_, n) => {
                Rvalue::Use(Operand::Const(Const::Int(i128::from(*n), IntKind::Usize)))
            }
            _ => Rvalue::Len(array.clone()),
        };
        let len = self.assign_temp(len_rvalue, Ty::Int(IntKind::Usize), iter.span)?;
        let element = match binding {
            Some(symbol) => self.local_for(symbol, iter.span)?,
            None => self.temp(item.clone(), iter.span),
        };
        let head = self.new_block();
        self.terminate(Terminator::Goto(head));
        self.switch_to(head);
        let head_state = self.snapshot();
        let cond = self.assign_temp(
            Rvalue::Binary {
                op: BinOp::Lt,
                lhs: Operand::Copy(index.clone()),
                rhs: Operand::Copy(len),
            },
            Ty::Bool,
            iter.span,
        )?;
        let body_block = self.new_block();
        let step_block = self.new_block();
        let exit_block = self.new_block();
        self.terminate(Terminator::Branch {
            cond: Operand::Copy(cond),
            then_block: body_block,
            else_block: exit_block,
        });
        self.loops.push(LoopCtx {
            label: None,
            break_block: exit_block,
            continue_block: step_block,
            result: None,
            depth: self.scopes.len(),
            head_state: head_state.clone(),
            exit_state: Some(head_state.clone()),
        });
        self.switch_to(body_block);
        let mut element_source = array.clone();
        element_source
            .projection
            .push(Projection::Index(index.local));
        self.store(
            &Place::local(element),
            Rvalue::Use(Operand::Copy(element_source)),
        )?;
        let live = self.block(body)?.is_some();
        if live {
            self.expect_state(&head_state)?;
            self.terminate(Terminator::Goto(step_block));
        }
        self.loops.pop();
        self.restore(&head_state);
        self.switch_to(step_block);
        self.store(
            &index,
            Rvalue::Binary {
                op: BinOp::Add,
                lhs: Operand::Copy(index.clone()),
                rhs: Operand::Const(Const::Int(1, IntKind::Usize)),
            },
        )?;
        self.terminate(Terminator::Goto(head));
        self.restore(&head_state);
        self.switch_to(exit_block);
        Ok(Some(()))
    }

    fn break_expr(&mut self, label: Option<&str>, value: Option<&Expr>) -> Lowered {
        let index = self.loop_index(label)?;
        let (break_block, depth, result) = {
            let ctx = &self.loops[index];
            (ctx.break_block, ctx.depth, ctx.result)
        };
        if let Some(value) = value {
            let ty = self.expr_ty(value)?;
            let Some(lowered) = self.expr(value)? else {
                return Ok(None);
            };
            let operand = self.use_value(lowered, &ty);
            if let Some(result) = result {
                self.store(&Place::local(result), Rvalue::Use(operand))?;
            }
        }
        self.drops_from(depth)?;
        let state = self.snapshot();
        match self.loops[index].exit_state.clone() {
            None => self.loops[index].exit_state = Some(state),
            Some(expected) => {
                // Breaks must agree on the locals that outlive the loop.
                if !self.states_agree_below(&expected, &state, depth) {
                    return Err(
                        "initialization state diverged across merging paths (v0 lowering limit)"
                            .to_owned(),
                    );
                }
            }
        }
        self.terminate(Terminator::Goto(break_block));
        Ok(None)
    }

    fn continue_expr(&mut self, label: Option<&str>) -> Lowered {
        let index = self.loop_index(label)?;
        let (continue_block, depth, head_state) = {
            let ctx = &self.loops[index];
            (ctx.continue_block, ctx.depth, ctx.head_state.clone())
        };
        self.drops_from(depth)?;
        // Compare against the head only on locals that outlive the body;
        // per-iteration temporaries in the exited scopes are irrelevant.
        self.expect_state_below(&head_state, depth)?;
        self.terminate(Terminator::Goto(continue_block));
        Ok(None)
    }

    fn loop_index(&self, label: Option<&str>) -> Result<usize, Skip> {
        let found = match label {
            None => self.loops.len().checked_sub(1),
            Some(label) => self
                .loops
                .iter()
                .rposition(|ctx| ctx.label.as_deref() == Some(label)),
        };
        found.ok_or_else(|| "`break`/`continue` outside its loop".to_owned())
    }

    /// `inner?`: switch on the discriminant; the failure variant returns
    /// early (rebuilt in the function's return type), the success variant
    /// yields its payload.
    fn try_expr(&mut self, expr: &Expr, inner: &Expr) -> Lowered {
        let inner_ty = self.expr_ty(inner)?;
        let payload_ty = self.expr_ty(expr)?;
        let Some(value) = self.expr(inner)? else {
            return Ok(None);
        };
        let operand = self.use_value(value, &inner_ty);
        let subject = match operand {
            Operand::Copy(place) => place,
            operand => {
                let local = self.temp(inner_ty.clone(), inner.span);
                let place = Place::local(local);
                self.store(&place, Rvalue::Use(operand))?;
                place
            }
        };
        let discr = self.assign_temp(
            Rvalue::Discriminant(subject.clone()),
            Ty::Int(IntKind::Usize),
            inner.span,
        )?;
        let ok_block = self.new_block();
        let err_block = self.new_block();
        self.terminate(Terminator::Switch {
            discr: Operand::Copy(discr),
            arms: vec![(0, ok_block)],
            otherwise: err_block,
        });
        let before = self.snapshot();
        // Failure path: rebuild the failure value in the return type and
        // return it after the usual exit drops.
        self.switch_to(err_block);
        let ret_ty = self.ret.clone();
        let failure = match (&inner_ty, &ret_ty) {
            (Ty::Option(_), Ty::Option(_)) => self.assign_temp(
                Rvalue::Aggregate {
                    kind: AggregateKind::Adt {
                        ty: ret_ty.clone(),
                        variant: 1,
                    },
                    fields: Vec::new(),
                },
                ret_ty.clone(),
                expr.span,
            )?,
            (Ty::Result(..), Ty::Result(..)) => {
                let mut error_place = subject.clone();
                error_place.projection.push(Projection::VariantField {
                    variant: 1,
                    field: 0,
                });
                let error_ty = self.place_ty(&error_place)?;
                let error_op = if self.is_copy(&error_ty) {
                    Operand::Copy(error_place)
                } else {
                    self.mark_moved(&error_place);
                    Operand::Move(error_place)
                };
                self.assign_temp(
                    Rvalue::Aggregate {
                        kind: AggregateKind::Adt {
                            ty: ret_ty.clone(),
                            variant: 1,
                        },
                        fields: vec![error_op],
                    },
                    ret_ty.clone(),
                    expr.span,
                )?
            }
            _ => return Err("`?` on a non-`Option`/`Result` value".to_owned()),
        };
        let failure_ty = self.place_ty(&failure)?;
        let return_op = self.use_value(Value::Place(failure), &failure_ty);
        self.emit_return(return_op)?;
        // Success path: extract the payload into its own slot.
        self.restore(&before);
        self.switch_to(ok_block);
        let mut payload_place = subject;
        payload_place.projection.push(Projection::VariantField {
            variant: 0,
            field: 0,
        });
        let payload_op = if self.is_copy(&payload_ty) {
            Operand::Copy(payload_place)
        } else {
            self.mark_moved(&payload_place);
            Operand::Move(payload_place)
        };
        let extracted = self.assign_temp(Rvalue::Use(payload_op), payload_ty, expr.span)?;
        Ok(Some(Value::Place(extracted)))
    }
}

impl FnLower<'_> {
    // ------------------------------------------------------------------
    // `match`, places, constructors, patterns
    // ------------------------------------------------------------------

    fn match_expr(&mut self, expr: &Expr, scrutinee: &Expr, arms: &[Arm]) -> Lowered {
        let ty = self.expr_ty(expr)?;
        let scrut_ty = self.expr_ty(scrutinee)?;
        let mut moves = false;
        for arm in arms {
            if arm.guard.is_some() && self.pattern_moves(&arm.pat, &scrut_ty)? {
                return Err(
                    "match guards on arms that bind non-`Copy` values are not lowered in v0"
                        .to_owned(),
                );
            }
            moves = moves || self.pattern_moves(&arm.pat, &scrut_ty)?;
        }
        let Some(value) = self.expr(scrutinee)? else {
            return Ok(None);
        };
        // The scrutinee is consumed exactly when some arm binds a
        // non-`Copy` value (the ownership model's rule); otherwise it is
        // read in place.
        let owned = moves && !self.is_copy(&scrut_ty);
        let subject = if owned {
            let operand = self.use_value(value, &scrut_ty);
            let local = self.temp(scrut_ty.clone(), scrutinee.span);
            let place = Place::local(local);
            self.store(&place, Rvalue::Use(operand))?;
            place
        } else {
            self.place_of(value, &scrut_ty, scrutinee.span)?
        };
        let result = self.branch_result(&ty, expr.span);
        let base_state = self.snapshot();
        let mut join = None;
        let mut states = Vec::new();
        for arm in arms {
            self.restore(&base_state);
            let fail = self.new_block();
            let body_block = self.new_block();
            self.pat_test(&subject, &scrut_ty, &arm.pat, body_block, fail)?;
            self.switch_to(body_block);
            self.push_scope();
            let arm_depth = self.scopes.len() - 1;
            let mut arm_live = true;
            if let Some(guard) = &arm.guard {
                // Guard arms bind only `Copy` values (checked above), so
                // extracting before the guard leaves the subject intact
                // for the next arm when the guard fails.
                self.extract(&subject, &arm.pat, &scrut_ty, owned)?;
                match self.expr(guard)? {
                    Some(guard_value) => {
                        let guard_op = Self::read_value(guard_value);
                        let commit = self.new_block();
                        let guard_fail = self.new_block();
                        self.terminate(Terminator::Branch {
                            cond: guard_op,
                            then_block: commit,
                            else_block: guard_fail,
                        });
                        self.switch_to(guard_fail);
                        self.drops_from(arm_depth)?;
                        self.terminate(Terminator::Goto(fail));
                        self.switch_to(commit);
                    }
                    None => arm_live = false,
                }
            } else if arm_live {
                self.extract(&subject, &arm.pat, &scrut_ty, owned)?;
            }
            if arm_live {
                if owned {
                    self.consume_residue(&subject, &arm.pat, &scrut_ty)?;
                    // Canonicalize: the subject is now fully consumed,
                    // however the extraction partitioned it.
                    self.moved.retain(|(root, _)| *root != subject.local);
                    self.moved.insert((subject.local, Vec::new()));
                }
                match self.expr(&arm.value)? {
                    Some(arm_value) => {
                        let operand = self.use_value(arm_value, &ty);
                        if let Some(result) = result {
                            self.store(&Place::local(result), Rvalue::Use(operand))?;
                        }
                        self.end_scope()?;
                        let target = *join.get_or_insert_with(|| {
                            self.blocks.push(BlockBuilder {
                                statements: Vec::new(),
                                terminator: None,
                            });
                            BlockId(u32::try_from(self.blocks.len() - 1).expect("block count fits"))
                        });
                        self.terminate(Terminator::Goto(target));
                        states.push(self.snapshot());
                    }
                    None => self.discard_scope(),
                }
            } else {
                self.discard_scope();
            }
            self.switch_to(fail);
        }
        // The front end proved the arms exhaustive; the last failure
        // edge is unreachable.
        self.terminate(Terminator::Trap(Trap::Unreachable));
        self.join_branches(states, join, result)
    }

    fn lower_place(&mut self, expr: &Expr) -> Result<Place, Skip> {
        match &expr.kind {
            ExprKind::Path {
                res: Res::Symbol(symbol),
                ..
            } if matches!(
                self.cx.resolution.symbol(*symbol).kind,
                SymbolKind::Param | SymbolKind::Local
            ) =>
            {
                let local = self.local_for(*symbol, expr.span)?;
                Ok(Place::local(local))
            }
            ExprKind::Field { receiver, name } => {
                let receiver_ty = self.expr_ty(receiver)?;
                let mut place = self.lower_place(receiver)?;
                let (index, _) = self.field_index(&receiver_ty, name)?;
                place.projection.push(Projection::Field(index));
                Ok(place)
            }
            _ => Err("assignment target or `mut` argument is not a place".to_owned()),
        }
    }

    fn field_index(&self, ty: &Ty, name: &str) -> Result<(u32, Ty), Skip> {
        match ty {
            Ty::Struct(symbol, args) => {
                let shape = self
                    .cx
                    .types
                    .struct_shape(*symbol)
                    .ok_or_else(|| "missing struct shape".to_owned())?;
                let position = shape
                    .fields
                    .iter()
                    .position(|(field, _)| field == name)
                    .ok_or_else(|| "unknown field".to_owned())?;
                let (_, field_ty) = &shape.fields[position];
                Ok((
                    u32::try_from(position).expect("field index fits"),
                    instantiate(field_ty, &shape.type_params, args),
                ))
            }
            Ty::Tuple(items) => {
                let index: usize = name.parse().map_err(|_| "bad tuple index".to_owned())?;
                let item = items
                    .get(index)
                    .ok_or_else(|| "bad tuple index".to_owned())?;
                Ok((u32::try_from(index).expect("index fits"), item.clone()))
            }
            _ => Err("field access on a non-aggregate".to_owned()),
        }
    }

    /// Resolve a constructor symbol against the constructed type: the
    /// variant's declaration-order index, its declared fields (name and
    /// instantiated type, in order), and whether a discriminant exists.
    fn ctor_variant(&self, symbol: SymbolId, ty: &Ty) -> Result<CtorInfo, Skip> {
        let prelude = |name: &str| self.cx.resolution.prelude_symbol(name);
        match ty {
            Ty::Struct(owner, args) => {
                let shape = self
                    .cx
                    .types
                    .struct_shape(*owner)
                    .ok_or_else(|| "missing struct shape".to_owned())?;
                let fields = shape
                    .fields
                    .iter()
                    .map(|(name, field)| {
                        (name.clone(), instantiate(field, &shape.type_params, args))
                    })
                    .collect();
                Ok((0, fields, false))
            }
            Ty::Enum(owner, args) => {
                let shape = self
                    .cx
                    .types
                    .enum_shape(*owner)
                    .ok_or_else(|| "missing enum shape".to_owned())?;
                let position = shape
                    .variants
                    .iter()
                    .position(|(variant, _)| *variant == symbol)
                    .ok_or_else(|| "constructor is not a variant of its type".to_owned())?;
                let fields = shape.variants[position]
                    .1
                    .iter()
                    .map(|(name, field)| {
                        (name.clone(), instantiate(field, &shape.type_params, args))
                    })
                    .collect();
                Ok((
                    u32::try_from(position).expect("variant index fits"),
                    fields,
                    true,
                ))
            }
            Ty::Option(item) => {
                if prelude("Some") == Some(symbol) {
                    Ok((0, vec![("value".to_owned(), (**item).clone())], true))
                } else if prelude("None") == Some(symbol) {
                    Ok((1, Vec::new(), true))
                } else {
                    Err("constructor is not an `Option` variant".to_owned())
                }
            }
            Ty::Result(ok, err) => {
                if prelude("Ok") == Some(symbol) {
                    Ok((0, vec![("value".to_owned(), (**ok).clone())], true))
                } else if prelude("Err") == Some(symbol) {
                    Ok((1, vec![("error".to_owned(), (**err).clone())], true))
                } else {
                    Err("constructor is not a `Result` variant".to_owned())
                }
            }
            _ => Err("constructor on an unsupported type".to_owned()),
        }
    }

    /// What a `PatKind::Binding` actually is: syntax cannot distinguish a
    /// fresh binding name from a payload-less variant path (`None`), so
    /// resolution decides.
    fn binding_kind(&self, res: &Res) -> Result<BindingKind, Skip> {
        match res {
            Res::Symbol(symbol) => match self.cx.resolution.symbol(*symbol).kind {
                SymbolKind::Local | SymbolKind::Param => Ok(BindingKind::Local(*symbol)),
                SymbolKind::Variant => Ok(BindingKind::Variant(*symbol)),
                _ => Err("unsupported name pattern".to_owned()),
            },
            _ => Err("body contains unresolved names".to_owned()),
        }
    }

    /// Does the pattern match without testing anything?
    fn pat_is_irrefutable(&self, pat: &Pat) -> bool {
        match &pat.kind {
            PatKind::Wildcard => true,
            PatKind::Binding(res) => matches!(self.binding_kind(res), Ok(BindingKind::Local(_))),
            _ => false,
        }
    }

    /// Does the pattern introduce any binding?
    fn pattern_binds(&self, pat: &Pat) -> bool {
        match &pat.kind {
            PatKind::Binding(res) => {
                matches!(self.binding_kind(res), Ok(BindingKind::Local(_)))
            }
            PatKind::Ctor { fields, .. } => {
                fields.iter().any(|field| self.pattern_binds(&field.pat))
            }
            PatKind::Or(alternatives) => alternatives
                .iter()
                .any(|alternative| self.pattern_binds(alternative)),
            _ => false,
        }
    }

    /// Does matching the pattern move a non-`Copy` value out of the
    /// scrutinee?
    fn pattern_moves(&self, pat: &Pat, ty: &Ty) -> Result<bool, Skip> {
        match &pat.kind {
            PatKind::Binding(res) => match self.binding_kind(res)? {
                BindingKind::Local(_) => Ok(!self.is_copy(ty)),
                BindingKind::Variant(_) => Ok(false),
            },
            PatKind::Ctor {
                ctor: Res::Symbol(symbol),
                fields,
                ..
            } => {
                let (_, declared, _) = self.ctor_variant(*symbol, ty)?;
                for field in fields {
                    let (_, field_ty) = declared
                        .iter()
                        .find(|(name, _)| *name == field.name)
                        .ok_or_else(|| "unknown pattern field".to_owned())?;
                    if self.pattern_moves(&field.pat, field_ty)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            PatKind::Or(alternatives) => {
                for alternative in alternatives {
                    if self.pattern_moves(alternative, ty)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            _ => Ok(false),
        }
    }

    /// Emit the runtime test for `pat` against `place`: control reaches
    /// `success` when it matches, `fail` otherwise. Always terminates the
    /// current block.
    fn pat_test(
        &mut self,
        place: &Place,
        ty: &Ty,
        pat: &Pat,
        success: BlockId,
        fail: BlockId,
    ) -> Result<(), Skip> {
        match &pat.kind {
            PatKind::Wildcard => {
                self.terminate(Terminator::Goto(success));
                Ok(())
            }
            PatKind::Binding(res) => match self.binding_kind(res)? {
                BindingKind::Local(_) => {
                    self.terminate(Terminator::Goto(success));
                    Ok(())
                }
                BindingKind::Variant(symbol) => {
                    // A payload-less variant path (`None`): a pure
                    // discriminant test.
                    let (variant, _, _) = self.ctor_variant(symbol, ty)?;
                    let discr = self.assign_temp(
                        Rvalue::Discriminant(place.clone()),
                        Ty::Int(IntKind::Usize),
                        pat.span,
                    )?;
                    self.terminate(Terminator::Switch {
                        discr: Operand::Copy(discr),
                        arms: vec![(i128::from(variant), success)],
                        otherwise: fail,
                    });
                    Ok(())
                }
            },
            PatKind::Lit(lit) => {
                let constant = lit_const(lit, ty, false)?;
                match &constant {
                    Const::Bool(expected) => {
                        let (then_block, else_block) = if *expected {
                            (success, fail)
                        } else {
                            (fail, success)
                        };
                        self.terminate(Terminator::Branch {
                            cond: Operand::Copy(place.clone()),
                            then_block,
                            else_block,
                        });
                    }
                    Const::Int(value, _) => {
                        self.terminate(Terminator::Switch {
                            discr: Operand::Copy(place.clone()),
                            arms: vec![(*value, success)],
                            otherwise: fail,
                        });
                    }
                    Const::Char(value) => {
                        self.terminate(Terminator::Switch {
                            discr: Operand::Copy(place.clone()),
                            arms: vec![(i128::from(u32::from(*value)), success)],
                            otherwise: fail,
                        });
                    }
                    _ => {
                        let eq = self.assign_temp(
                            Rvalue::Binary {
                                op: BinOp::Eq,
                                lhs: Operand::Copy(place.clone()),
                                rhs: Operand::Const(constant.clone()),
                            },
                            Ty::Bool,
                            pat.span,
                        )?;
                        self.terminate(Terminator::Branch {
                            cond: Operand::Copy(eq),
                            then_block: success,
                            else_block: fail,
                        });
                    }
                }
                Ok(())
            }
            PatKind::Ctor {
                ctor: Res::Symbol(symbol),
                fields,
                ..
            } => {
                let (variant, declared, has_discriminant) = self.ctor_variant(*symbol, ty)?;
                if has_discriminant {
                    let discr = self.assign_temp(
                        Rvalue::Discriminant(place.clone()),
                        Ty::Int(IntKind::Usize),
                        pat.span,
                    )?;
                    let next = self.new_block();
                    self.terminate(Terminator::Switch {
                        discr: Operand::Copy(discr),
                        arms: vec![(i128::from(variant), next)],
                        otherwise: fail,
                    });
                    self.switch_to(next);
                }
                // Chain the sub-tests that actually test something.
                let mut tests = Vec::new();
                for field in fields {
                    if self.pat_is_irrefutable(&field.pat) {
                        continue;
                    }
                    let position = declared
                        .iter()
                        .position(|(name, _)| *name == field.name)
                        .ok_or_else(|| "unknown pattern field".to_owned())?;
                    let (_, field_ty) = &declared[position];
                    let mut sub_place = place.clone();
                    sub_place.projection.push(self.field_projection(
                        has_discriminant,
                        variant,
                        position,
                    ));
                    tests.push((sub_place, field_ty.clone(), &field.pat));
                }
                if tests.is_empty() {
                    self.terminate(Terminator::Goto(success));
                    return Ok(());
                }
                let last = tests.len() - 1;
                for (index, (sub_place, sub_ty, sub_pat)) in tests.into_iter().enumerate() {
                    if index == last {
                        self.pat_test(&sub_place, &sub_ty, sub_pat, success, fail)?;
                    } else {
                        let next = self.new_block();
                        self.pat_test(&sub_place, &sub_ty, sub_pat, next, fail)?;
                        self.switch_to(next);
                    }
                }
                Ok(())
            }
            PatKind::Ctor { .. } => Err("body contains unresolved names".to_owned()),
            PatKind::Or(alternatives) => {
                if alternatives
                    .iter()
                    .any(|alternative| self.pattern_binds(alternative))
                {
                    return Err("or-patterns that bind are not lowered in v0".to_owned());
                }
                let last = alternatives.len() - 1;
                for (index, alternative) in alternatives.iter().enumerate() {
                    if index == last {
                        self.pat_test(place, ty, alternative, success, fail)?;
                    } else {
                        let next_alt = self.new_block();
                        self.pat_test(place, ty, alternative, success, next_alt)?;
                        self.switch_to(next_alt);
                    }
                }
                Ok(())
            }
            PatKind::Err => Err("body contains parse errors".to_owned()),
        }
    }

    const fn field_projection(
        &self,
        has_discriminant: bool,
        variant: u32,
        position: usize,
    ) -> Projection {
        let field = position as u32;
        if has_discriminant {
            Projection::VariantField { variant, field }
        } else {
            Projection::Field(field)
        }
    }

    /// Bind the pattern's names from a matched `place` (copies for
    /// `Copy` positions, moves out of an owned scrutinee otherwise).
    fn extract(&mut self, place: &Place, pat: &Pat, ty: &Ty, owned: bool) -> Result<(), Skip> {
        match &pat.kind {
            PatKind::Binding(res) => {
                let BindingKind::Local(symbol) = self.binding_kind(res)? else {
                    // A payload-less variant path binds nothing.
                    return Ok(());
                };
                let local = self.local_for(symbol, pat.span)?;
                let operand = if self.is_copy(ty) {
                    Operand::Copy(place.clone())
                } else {
                    if !owned {
                        return Err("binding moves out of an unowned scrutinee".to_owned());
                    }
                    self.mark_moved(place);
                    Operand::Move(place.clone())
                };
                self.store(&Place::local(local), Rvalue::Use(operand))
            }
            PatKind::Ctor {
                ctor: Res::Symbol(symbol),
                fields,
                ..
            } => {
                let (variant, declared, has_discriminant) = self.ctor_variant(*symbol, ty)?;
                for field in fields {
                    let position = declared
                        .iter()
                        .position(|(name, _)| *name == field.name)
                        .ok_or_else(|| "unknown pattern field".to_owned())?;
                    let (_, field_ty) = declared[position].clone();
                    let mut sub_place = place.clone();
                    sub_place.projection.push(self.field_projection(
                        has_discriminant,
                        variant,
                        position,
                    ));
                    self.extract(&sub_place, &field.pat, &field_ty, owned)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Drop whatever an owned, matched scrutinee still holds after the
    /// pattern's bindings moved their parts out (the "husk").
    fn consume_residue(&mut self, place: &Place, pat: &Pat, ty: &Ty) -> Result<(), Skip> {
        if !self.pattern_binds(pat) {
            return self.drop_residue(place, ty);
        }
        match &pat.kind {
            PatKind::Binding(_) => Ok(()),
            PatKind::Ctor {
                ctor: Res::Symbol(symbol),
                fields,
                ..
            } => {
                let (variant, declared, has_discriminant) = self.ctor_variant(*symbol, ty)?;
                for (position, (name, field_ty)) in declared.iter().enumerate() {
                    let mut sub_place = place.clone();
                    sub_place.projection.push(self.field_projection(
                        has_discriminant,
                        variant,
                        position,
                    ));
                    match fields.iter().find(|field| field.name == *name) {
                        Some(field) => self.consume_residue(&sub_place, &field.pat, field_ty)?,
                        None => self.drop_residue(&sub_place, field_ty)?,
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

/// Can evaluating `expr` write to memory (directly or through a call)?
/// Used to decide when an earlier `Copy` read must be frozen.
fn can_write(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Assign { .. } | ExprKind::Call { .. } | ExprKind::MethodCall { .. } => true,
        ExprKind::Lit(_) | ExprKind::Path { .. } | ExprKind::Continue { .. } | ExprKind::Err => {
            false
        }
        ExprKind::StructLit { fields, .. } => fields.iter().any(|field| can_write(&field.value)),
        ExprKind::Array(elements) => elements.iter().any(can_write),
        ExprKind::ArrayRepeat { value, .. } => can_write(value),
        ExprKind::Unary { operand, .. } => can_write(operand),
        ExprKind::Binary { lhs, rhs, .. } => can_write(lhs) || can_write(rhs),
        ExprKind::Range { lo, hi } => can_write(lo) || can_write(hi),
        ExprKind::Field { receiver, .. } => can_write(receiver),
        ExprKind::Index { base, index } => can_write(base) || can_write(index),
        ExprKind::Try(inner) | ExprKind::Cast { value: inner, .. } => can_write(inner),
        ExprKind::If { cond, then, els } => {
            can_write(cond) || block_can_write(then) || els.as_deref().is_some_and(can_write)
        }
        ExprKind::Match { scrutinee, arms } => {
            can_write(scrutinee)
                || arms
                    .iter()
                    .any(|arm| arm.guard.as_ref().is_some_and(can_write) || can_write(&arm.value))
        }
        ExprKind::While { cond, body } => can_write(cond) || block_can_write(body),
        ExprKind::Loop { body, .. } => block_can_write(body),
        ExprKind::For { iter, body, .. } => can_write(iter) || block_can_write(body),
        ExprKind::Unsafe(block) | ExprKind::Block(block) => block_can_write(block),
        ExprKind::Return(value) => value.as_deref().is_some_and(can_write),
        ExprKind::Break { value, .. } => value.as_deref().is_some_and(can_write),
    }
}

fn block_can_write(block: &Block) -> bool {
    let in_stmts = block.stmts.iter().any(|stmt| match &stmt.kind {
        StmtKind::Binding(def) => def.init.as_ref().is_some_and(can_write),
        StmtKind::Const(def) => def.value.as_ref().is_some_and(can_write),
        StmtKind::Expr(expr) => can_write(expr),
        StmtKind::Err => false,
    });
    in_stmts || block.tail.as_deref().is_some_and(can_write)
}

/// Does `ty` contain the poison type or an unsolved inference variable?
fn contains_error(ty: &Ty) -> bool {
    match ty {
        Ty::Error | Ty::Var(_) => true,
        Ty::Unit
        | Ty::Never
        | Ty::Bool
        | Ty::Char
        | Ty::String
        | Ty::Str
        | Ty::Int(_)
        | Ty::Float(_)
        | Ty::Param(_) => false,
        Ty::Tuple(items) => items.iter().any(contains_error),
        Ty::Array(item)
        | Ty::FixedArray(item, _)
        | Ty::Range(item)
        | Ty::Option(item)
        | Ty::Wrapper(_, item) => contains_error(item),
        Ty::Result(ok, err) | Ty::Map(ok, err) => contains_error(ok) || contains_error(err),
        Ty::Struct(_, args) | Ty::Enum(_, args) => args.iter().any(contains_error),
        Ty::Fn(fn_ty) => {
            fn_ty.params.iter().any(|param| contains_error(&param.ty)) || contains_error(&fn_ty.ret)
        }
    }
}

/// Substitute a declaration-context type: each [`Ty::Param`] listed in
/// `params` is replaced by the corresponding type argument.
fn instantiate(ty: &Ty, params: &[SymbolId], args: &[Ty]) -> Ty {
    match ty {
        Ty::Param(symbol) => params
            .iter()
            .position(|param| param == symbol)
            .and_then(|index| args.get(index))
            .cloned()
            .unwrap_or_else(|| ty.clone()),
        Ty::Tuple(items) => Ty::Tuple(
            items
                .iter()
                .map(|item| instantiate(item, params, args))
                .collect(),
        ),
        Ty::Array(item) => Ty::Array(Box::new(instantiate(item, params, args))),
        Ty::FixedArray(item, n) => Ty::FixedArray(Box::new(instantiate(item, params, args)), *n),
        Ty::Range(item) => Ty::Range(Box::new(instantiate(item, params, args))),
        Ty::Option(item) => Ty::Option(Box::new(instantiate(item, params, args))),
        Ty::Result(ok, err) => Ty::Result(
            Box::new(instantiate(ok, params, args)),
            Box::new(instantiate(err, params, args)),
        ),
        Ty::Wrapper(kind, item) => Ty::Wrapper(*kind, Box::new(instantiate(item, params, args))),
        Ty::Struct(symbol, inner) => Ty::Struct(
            *symbol,
            inner
                .iter()
                .map(|item| instantiate(item, params, args))
                .collect(),
        ),
        Ty::Enum(symbol, inner) => Ty::Enum(
            *symbol,
            inner
                .iter()
                .map(|item| instantiate(item, params, args))
                .collect(),
        ),
        Ty::Fn(fn_ty) => Ty::Fn(Box::new(FnTy {
            params: fn_ty
                .params
                .iter()
                .map(|param| tuo_types::FnParam {
                    mode: param.mode,
                    ty: instantiate(&param.ty, params, args),
                })
                .collect(),
            ret: instantiate(&fn_ty.ret, params, args),
        })),
        _ => ty.clone(),
    }
}

/// Convert a literal (or negated-literal) expression into a constant of
/// type `ty`.
fn literal_const(expr: &Expr, ty: &Ty) -> Result<Const, Skip> {
    match &expr.kind {
        ExprKind::Lit(lit) => lit_const(lit, ty, false),
        ExprKind::Unary {
            op: tuo_hir::UnOp::Neg,
            operand,
        } => match &operand.kind {
            ExprKind::Lit(lit) => lit_const(lit, ty, true),
            _ => Err("not a literal".to_owned()),
        },
        _ => Err("not a literal".to_owned()),
    }
}

/// The value range of an integer kind (`Isize`/`Usize` are 64-bit in v0).
fn int_range(kind: IntKind) -> (i128, i128) {
    match kind {
        IntKind::I8 => (i128::from(i8::MIN), i128::from(i8::MAX)),
        IntKind::I16 => (i128::from(i16::MIN), i128::from(i16::MAX)),
        IntKind::I32 => (i128::from(i32::MIN), i128::from(i32::MAX)),
        IntKind::I64 | IntKind::Isize => (i128::from(i64::MIN), i128::from(i64::MAX)),
        IntKind::U8 => (0, i128::from(u8::MAX)),
        IntKind::U16 => (0, i128::from(u16::MAX)),
        IntKind::U32 => (0, i128::from(u32::MAX)),
        IntKind::U64 | IntKind::Usize => (0, i128::from(u64::MAX)),
    }
}

fn lit_const(lit: &Lit, ty: &Ty, negated: bool) -> Result<Const, Skip> {
    match (lit, ty) {
        (Lit::Unit, _) => Ok(Const::Unit),
        (Lit::Bool(value), _) => Ok(Const::Bool(*value)),
        (Lit::Int(digits), Ty::Int(kind)) => {
            let magnitude = parse_int(digits)?;
            let value = if negated { -magnitude } else { magnitude };
            let (min, max) = int_range(*kind);
            if value < min || value > max {
                return Err(format!(
                    "integer literal out of range for `{}`",
                    kind.name()
                ));
            }
            Ok(Const::Int(value, *kind))
        }
        (Lit::Float(digits), Ty::Float(kind)) => {
            let value: f64 = digits
                .parse()
                .map_err(|_| "malformed float literal".to_owned())?;
            Ok(Const::Float(if negated { -value } else { value }, *kind))
        }
        (Lit::Char(text), _) => Ok(Const::Char(decode_char(text)?)),
        (Lit::Str(text), _) => Ok(Const::Str(decode_str(text)?)),
        _ => Err("literal does not match its type".to_owned()),
    }
}

fn parse_int(digits: &str) -> Result<i128, Skip> {
    let parsed = if let Some(hex) = digits
        .strip_prefix("0x")
        .or_else(|| digits.strip_prefix("0X"))
    {
        i128::from_str_radix(hex, 16)
    } else if let Some(bin) = digits
        .strip_prefix("0b")
        .or_else(|| digits.strip_prefix("0B"))
    {
        i128::from_str_radix(bin, 2)
    } else if let Some(oct) = digits
        .strip_prefix("0o")
        .or_else(|| digits.strip_prefix("0O"))
    {
        i128::from_str_radix(oct, 8)
    } else {
        digits.parse()
    };
    parsed.map_err(|_| "unparsable integer literal".to_owned())
}

/// Decode the contents of a character literal (between the quotes).
fn decode_char(text: &str) -> Result<char, Skip> {
    let decoded = decode_str(text)?;
    let mut chars = decoded.chars();
    match (chars.next(), chars.next()) {
        (Some(value), None) => Ok(value),
        _ => Err("malformed character literal".to_owned()),
    }
}

/// Decode the escape sequences of a string/char literal body.
fn decode_str(text: &str) -> Result<String, Skip> {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(next) = chars.next() {
        if next != '\\' {
            out.push(next);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('0') => out.push('\0'),
            Some('\\') => out.push('\\'),
            Some('\'') => out.push('\''),
            Some('"') => out.push('"'),
            Some('u') => {
                let hex: String = chars
                    .by_ref()
                    .skip_while(|c| *c == '{')
                    .take_while(|c| *c != '}')
                    .collect();
                let value = u32::from_str_radix(&hex, 16)
                    .map_err(|_| "malformed unicode escape".to_owned())?;
                out.push(char::from_u32(value).ok_or("invalid unicode escape")?);
            }
            _ => return Err("unsupported escape sequence".to_owned()),
        }
    }
    Ok(out)
}
