//! The flow-sensitive ownership analysis over function (and spec) bodies.
//!
//! One [`Body`] checks one body: it tracks the initialization state of every
//! place (`specification/ownership.md` §1), applies the move/borrow rules of
//! §3–§10, and verifies statically known drop state at every drop point
//! (§11). Reporting is one diagnostic per root cause: after a place is
//! diagnosed it is *healed* (treated as reinitialized) so one mistake never
//! cascades.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use tuo_ast::{
    ArrayLiteralKind, AssignExpr, Ast, BindingStmt, Block, CallExpr, ElseBranch, Expr, FieldPat,
    FnDecl, ForExpr, IfExpr, Item, LoopExpr, MatchExpr, Name, Pattern, SpecDecl, SpecStatement,
    Statement, UnaryExpr, WhileExpr,
};
use tuo_diagnostics::{Confidence, Diagnostic, DiagnosticCode, Edit, Namespace};
use tuo_resolve::{Resolution, SymbolId, SymbolKind};
use tuo_source::{Span, TextRange};
use tuo_types::{Ty, TypeckResult};

use crate::env::TypeEnv;
use crate::place::{DisplayPlace, Place};

fn code(number: u16) -> DiagnosticCode {
    DiagnosticCode::new(Namespace::Ownership, number)
}

/// A parameter mode as declared in a signature.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    In,
    Mut,
    Take,
}

impl Mode {
    fn parse(text: &str) -> Option<Self> {
        match text {
            "in" => Some(Self::In),
            "mut" => Some(Self::Mut),
            "take" => Some(Self::Take),
            _ => None,
        }
    }

    fn is_borrow(self) -> bool {
        matches!(self, Self::In | Self::Mut)
    }
}

/// What kind of declaration a tracked root came from; decides mutability
/// (§7) and whether the root is a borrow (§5).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Origin {
    /// A `let` binding (immutable, owned). `deferred` records whether it was
    /// declared without an initializer (definite-assignment, §7).
    Let { deferred: bool },
    /// A `var` binding (mutable, owned).
    Var,
    /// A pattern binding (match arm, `for` loop): immutable, owned.
    PatternBinding,
    /// An `in` parameter: immutable, borrowed.
    InParam,
    /// A `mut` parameter: mutable, borrowed.
    MutParam,
    /// A `take` parameter: mutable, owned.
    TakeParam,
}

impl Origin {
    fn is_mutable(self) -> bool {
        matches!(self, Self::Var | Self::MutParam | Self::TakeParam)
    }

    /// The borrow mode keyword when the root is a borrowed parameter.
    fn borrow_mode(self) -> Option<&'static str> {
        match self {
            Self::InParam => Some("in"),
            Self::MutParam => Some("mut"),
            _ => None,
        }
    }

    fn describe(self) -> &'static str {
        match self {
            Self::Let { .. } => "a `let` binding",
            Self::Var => "a `var` binding",
            Self::PatternBinding => "a pattern binding",
            Self::InParam => "an `in` parameter",
            Self::MutParam => "a `mut` parameter",
            Self::TakeParam => "a `take` parameter",
        }
    }
}

/// One tracked root binding or parameter.
struct Root {
    name: String,
    decl: Span,
    ty: Ty,
    origin: Origin,
}

/// The abnormal initialization state of a place. Absence from the state map
/// means fully initialized.
#[derive(Clone, PartialEq, Eq, Debug)]
enum PlaceState {
    /// Declared without an initializer; holds no value yet on this path.
    Uninit,
    /// Initialized on some paths to here but not all.
    MaybeInit { init: Span },
    /// Moved out on every path to here.
    Moved { at: Span },
    /// Moved on some paths to here but not all.
    MaybeMoved { at: Span },
}

type StateMap = BTreeMap<Place, PlaceState>;

/// One enclosing loop: where `break`/`continue` states accumulate.
struct LoopFrame {
    label: Option<String>,
    depth: usize,
    breaks: Vec<StateMap>,
    continues: Vec<StateMap>,
}

/// Program-snapshot-wide context shared by every body check.
pub(crate) struct Cx<'a> {
    resolution: &'a Resolution,
    types: &'a TypeckResult,
    env: TypeEnv<'a>,
    /// Name-use span → resolved symbol.
    refs: HashMap<Span, SymbolId>,
    /// Declaration name span → declared symbol.
    decls: HashMap<Span, SymbolId>,
    /// Function symbol → declared parameter modes, in order.
    fn_modes: HashMap<SymbolId, Vec<Mode>>,
    /// Primary spans of type diagnostics: bodies containing one are skipped
    /// (§16 — poisoned types would produce noise, not signal).
    type_error_spans: Vec<Span>,
}

/// Run the analysis over every body in `files`.
pub(crate) fn run(
    files: &[Ast<'_>],
    resolution: &Resolution,
    types: &TypeckResult,
) -> Vec<Diagnostic> {
    let refs = resolution
        .references()
        .iter()
        .map(|reference| (reference.span, reference.symbol))
        .collect();
    let decls = resolution
        .symbols()
        .filter_map(|(id, symbol)| symbol.declaration.map(|span| (span, id)))
        .collect();
    let mut cx = Cx {
        resolution,
        types,
        env: TypeEnv::new(types),
        refs,
        decls,
        fn_modes: HashMap::new(),
        type_error_spans: types
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.primary_span)
            .collect(),
    };
    for ast in files {
        cx.collect_modes(*ast);
    }
    let mut diagnostics = Vec::new();
    for ast in files {
        for item in ast.file().items() {
            match item {
                Item::Fn(decl) => cx.check_fn(decl, &mut diagnostics),
                Item::Impl(decl) => {
                    for function in decl.functions() {
                        cx.check_fn(function, &mut diagnostics);
                    }
                }
                Item::Spec(decl) => cx.check_spec(decl, &mut diagnostics),
                _ => {}
            }
        }
    }
    diagnostics
}

impl Cx<'_> {
    fn collect_modes(&mut self, ast: Ast<'_>) {
        let mut record = |decl: FnDecl<'_>| {
            let Some(symbol) = decl
                .name_ref()
                .and_then(|name| self.decls.get(&name.span).copied())
            else {
                return;
            };
            let modes = decl
                .params()
                .map(|param| param.mode().and_then(Mode::parse).unwrap_or(Mode::In))
                .collect();
            self.fn_modes.insert(symbol, modes);
        };
        for item in ast.file().items() {
            match item {
                Item::Fn(decl) => record(decl),
                Item::Impl(decl) => {
                    for function in decl.functions() {
                        record(function);
                    }
                }
                _ => {}
            }
        }
    }

    /// Does the span of `item_span` contain any type-error span?
    fn has_type_errors(&self, item_span: Option<Span>) -> bool {
        let Some(outer) = item_span else {
            return true;
        };
        self.type_error_spans.iter().any(|inner| {
            inner.source() == outer.source()
                && outer.range().start() <= inner.range().start()
                && inner.range().end() <= outer.range().end()
        })
    }

    fn check_fn(&self, decl: FnDecl<'_>, diagnostics: &mut Vec<Diagnostic>) {
        let Some(body) = decl.body() else {
            return;
        };
        if self.has_type_errors(decl.span()) {
            return;
        }
        let fallback = decl
            .name_ref()
            .map(|name| name.span)
            .or_else(|| decl.span());
        let Some(fallback) = fallback else {
            return;
        };
        let mut checker = Body::new(self, fallback);
        checker.scopes.push(Vec::new());
        for param in decl.params() {
            let Some(name) = param.name_ref() else {
                continue;
            };
            let Some(&symbol) = self.decls.get(&name.span) else {
                continue;
            };
            let origin = match param.mode().and_then(Mode::parse) {
                Some(Mode::Mut) => Origin::MutParam,
                Some(Mode::Take) => Origin::TakeParam,
                _ => Origin::InParam,
            };
            checker.register(symbol, name, origin, true);
        }
        checker.eval_block(body, Use::Value);
        if !checker.diverged {
            checker.scope_end();
        }
        diagnostics.append(&mut checker.diagnostics);
    }

    fn check_spec(&self, decl: SpecDecl<'_>, diagnostics: &mut Vec<Diagnostic>) {
        if self.has_type_errors(decl.span()) {
            return;
        }
        let Some(fallback) = decl.span() else {
            return;
        };
        let mut checker = Body::new(self, fallback);
        checker.scopes.push(Vec::new());
        for statement in decl.statements() {
            if checker.diverged {
                break;
            }
            match statement {
                SpecStatement::Given(clause) => {
                    for binding in clause.bindings() {
                        if let Some(init) = binding.initializer() {
                            checker.expr(init, Use::Value);
                        }
                        let Some(name) = binding.name_ref() else {
                            continue;
                        };
                        let Some(&symbol) = self.decls.get(&name.span) else {
                            continue;
                        };
                        // `given` bindings always hold a value (harness- or
                        // initializer-provided); they are immutable.
                        checker.register(symbol, name, Origin::Let { deferred: false }, true);
                    }
                }
                SpecStatement::When(clause) => {
                    if let Some(binding) = clause.binding() {
                        checker.binding_stmt(binding);
                    } else if let Some(expr) = clause.expr() {
                        checker.expr(expr, Use::Value);
                    }
                }
                SpecStatement::Then(clause) | SpecStatement::Assert(clause) => {
                    if let Some(expr) = clause.expr() {
                        checker.expr(expr, Use::Read);
                    }
                }
                SpecStatement::Let(binding) | SpecStatement::Var(binding) => {
                    checker.binding_stmt(binding);
                }
                SpecStatement::Expr(statement) => {
                    if let Some(expr) = statement.expr() {
                        checker.expr(expr, Use::Value);
                    }
                }
                SpecStatement::Error(_) => {}
            }
        }
        if !checker.diverged {
            checker.scope_end();
        }
        diagnostics.append(&mut checker.diagnostics);
    }
}

/// How an expression's result is consumed by its context.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Use {
    /// By value: moves a non-`Copy` place (§3).
    Value,
    /// Read-only: inspects without transferring ownership (conditions,
    /// operator operands, `in` arguments, non-binding scrutinees).
    Read,
}

/// The per-body flow analysis.
struct Body<'a> {
    cx: &'a Cx<'a>,
    roots: HashMap<SymbolId, Root>,
    state: StateMap,
    /// Last (re)initialization site per place, for messages and the
    /// definite-assignment rules.
    init_spans: HashMap<Place, Span>,
    /// Declared roots per lexical scope, innermost last.
    scopes: Vec<Vec<SymbolId>>,
    loops: Vec<LoopFrame>,
    /// Places already diagnosed: treated as reinitialized from then on.
    healed: BTreeSet<Place>,
    /// Places already diagnosed as moves out of borrows (O0003).
    reported_borrow_moves: HashSet<Place>,
    diverged: bool,
    diagnostics: Vec<Diagnostic>,
    fallback: Span,
}

impl<'a> Body<'a> {
    fn new(cx: &'a Cx<'a>, fallback: Span) -> Self {
        Self {
            cx,
            roots: HashMap::new(),
            state: BTreeMap::new(),
            init_spans: HashMap::new(),
            scopes: Vec::new(),
            loops: Vec::new(),
            healed: BTreeSet::new(),
            reported_borrow_moves: HashSet::new(),
            diverged: false,
            diagnostics: Vec::new(),
            fallback,
        }
    }

    // ------------------------------------------------------------------
    // Bookkeeping
    // ------------------------------------------------------------------

    fn at(&self, span: Option<Span>) -> Span {
        span.unwrap_or(self.fallback)
    }

    fn register(&mut self, symbol: SymbolId, name: Name<'_>, origin: Origin, initialized: bool) {
        let ty = self.cx.types.type_of(symbol).cloned().unwrap_or(Ty::Error);
        self.roots.insert(
            symbol,
            Root {
                name: name.text.to_owned(),
                decl: name.span,
                ty,
                origin,
            },
        );
        if let Some(scope) = self.scopes.last_mut() {
            scope.push(symbol);
        }
        let place = Place::root(symbol);
        if initialized {
            self.init_spans.insert(place, name.span);
        } else {
            self.state.insert(place, PlaceState::Uninit);
        }
    }

    fn display(&self, place: &Place) -> String {
        let root_name = self
            .roots
            .get(&place.root)
            .map_or("<unknown>", |root| root.name.as_str());
        DisplayPlace { root_name, place }.to_string()
    }

    fn place_ty(&self, place: &Place) -> Option<Ty> {
        let root = self.roots.get(&place.root)?;
        self.cx.env.place_ty(&root.ty, place)
    }

    fn is_copy_place(&self, place: &Place) -> bool {
        // An untypeable place means the front end already diagnosed the
        // body region; stay quiet rather than speculate.
        self.place_ty(place)
            .is_none_or(|ty| self.cx.env.is_copy(&ty))
    }

    fn report(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    /// Treat `place` (and everything under it) as reinitialized: one
    /// diagnostic per root cause, no cascades.
    fn heal(&mut self, place: &Place) {
        self.state.retain(|key, _| !place.overlaps(key));
        self.healed.insert(place.clone());
    }

    /// Remove entries for already-healed places from the current state
    /// (used after merges that fold in states captured before the heal).
    fn scrub_healed(&mut self) {
        let healed = self.healed.clone();
        self.state
            .retain(|key, _| !healed.iter().any(|h| h.overlaps(key)));
    }

    /// The governing abnormal state of `place`: the worst entry over
    /// `place` itself and its prefixes.
    fn governing<'m>(map: &'m StateMap, place: &Place) -> Option<(&'m Place, &'m PlaceState)> {
        let mut best: Option<(&Place, &PlaceState)> = None;
        for (key, state) in map {
            if key.is_prefix_of(place) {
                let rank = state_rank(state);
                if best.is_none_or(|(_, current)| rank > state_rank(current)) {
                    best = Some((key, state));
                }
            }
        }
        best
    }

    /// Entries strictly below `place` (its moved-out fields).
    fn moved_descendants(&self, place: &Place) -> Vec<(Place, PlaceState)> {
        self.state
            .iter()
            .filter(|(key, _)| place.is_prefix_of(key) && key.path.len() > place.path.len())
            .map(|(key, state)| (key.clone(), state.clone()))
            .collect()
    }

    // ------------------------------------------------------------------
    // The core place judgements
    // ------------------------------------------------------------------

    /// Can `place` be used (read, borrowed, or taken by value) here? Reports
    /// O0001/O0002/O0009 and heals on failure.
    fn check_usable(&mut self, place: &Place, span: Span) -> bool {
        if let Some((key, state)) = Self::governing(&self.state, place) {
            let key = key.clone();
            let state = state.clone();
            let name = self.display(&key);
            let decl = self.roots.get(&key.root).map(|root| root.decl);
            let diagnostic = match state {
                PlaceState::Moved { at } => {
                    Diagnostic::error(code(1), format!("use of moved value `{name}`"), span)
                        .with_primary_label(format!("`{name}` used here after it was moved"))
                        .with_secondary_label(at, "value moved here")
                }
                PlaceState::MaybeMoved { at } => Diagnostic::error(
                    code(2),
                    format!("use of possibly moved value `{name}`"),
                    span,
                )
                .with_primary_label(format!("`{name}` may or may not hold a value here"))
                .with_secondary_label(at, "value moved here, on one path to this use")
                .with_help("move it on every path, or reinitialize it before this use"),
                PlaceState::Uninit => Diagnostic::error(
                    code(2),
                    format!("`{name}` used before it is initialized"),
                    span,
                )
                .with_primary_label(format!("`{name}` holds no value yet")),
                PlaceState::MaybeInit { init } => Diagnostic::error(
                    code(2),
                    format!("`{name}` is not initialized on every path to this use"),
                    span,
                )
                .with_secondary_label(init, "initialized here, on one path only")
                .with_help("initialize it on every path before this use"),
            };
            let diagnostic = match decl {
                Some(decl) => {
                    diagnostic.with_secondary_label(decl, format!("`{name}` declared here"))
                }
                None => diagnostic,
            };
            self.report(diagnostic);
            self.heal(&key);
            return false;
        }
        let moved = self.moved_descendants(place);
        if !moved.is_empty() {
            let name = self.display(place);
            let fields: Vec<String> = moved
                .iter()
                .map(|(key, _)| format!("`{}`", self.display(key)))
                .collect();
            let mut diagnostic = Diagnostic::error(
                code(9),
                format!("use of partially moved value `{name}`"),
                span,
            )
            .with_primary_label(format!("{} moved out of `{name}`", fields.join(" and ")))
            .with_help("reassign the moved field(s) before using the whole value");
            for (key, state) in &moved {
                if let PlaceState::Moved { at } | PlaceState::MaybeMoved { at } = state {
                    diagnostic = diagnostic.with_secondary_label(
                        *at,
                        format!("`{}` moved out here", self.display(key)),
                    );
                }
            }
            self.report(diagnostic);
            let keys: Vec<Place> = moved.into_iter().map(|(key, _)| key).collect();
            for key in keys {
                self.heal(&key);
            }
            return false;
        }
        true
    }

    /// Report O0003: a move out of a borrowed (`in`/`mut`) parameter.
    fn report_borrow_move(&mut self, place: &Place, mode: &'static str, span: Span) {
        if !self.reported_borrow_moves.insert(place.clone()) {
            return;
        }
        let name = self.display(place);
        let root = &self.roots[&place.root];
        let root_name = root.name.clone();
        let decl = root.decl;
        let what = if place.path.is_empty() {
            format!("cannot move out of `{mode} {root_name}`")
        } else {
            format!("cannot move `{name}` out of the borrowed parameter `{mode} {root_name}`")
        };
        self.report(
            Diagnostic::error(code(3), what, span)
                .with_primary_label("ownership cannot leave a borrowed parameter")
                .with_secondary_label(
                    decl,
                    format!("`{root_name}` is borrowed for the duration of this call"),
                )
                .with_help(format!(
                    "declare the parameter `take {root_name}` if the callee should own the value"
                )),
        );
    }

    /// Use `place` by value: copy if `Copy`, otherwise move it (§3).
    fn use_value(&mut self, place: &Place, span: Span) {
        if !self.check_usable(place, span) {
            return;
        }
        if self.is_copy_place(place) {
            return;
        }
        if let Some(mode) = self
            .roots
            .get(&place.root)
            .and_then(|root| root.origin.borrow_mode())
        {
            self.report_borrow_move(place, mode, span);
            return;
        }
        self.state
            .insert(place.clone(), PlaceState::Moved { at: span });
    }

    /// Read `place` without taking ownership.
    fn read_place(&mut self, place: &Place, span: Span) {
        self.check_usable(place, span);
    }

    // ------------------------------------------------------------------
    // Place resolution
    // ------------------------------------------------------------------

    /// The place `expr` denotes, if it is a place expression (§1).
    fn place_of(&self, expr: Expr<'_>) -> Option<Place> {
        match expr {
            Expr::Path(path) => {
                let names: Vec<Name<'_>> = path.segment_names().collect();
                let [name] = names.as_slice() else {
                    return None;
                };
                let symbol = *self.cx.refs.get(&name.span)?;
                let kind = self.cx.resolution.symbol(symbol).kind;
                matches!(kind, SymbolKind::Param | SymbolKind::Local).then(|| Place::root(symbol))
            }
            Expr::Field(field) => {
                let base = self.place_of(field.receiver()?)?;
                Some(base.child(field.name()?))
            }
            Expr::Group(group) => self.place_of(group.inner()?),
            _ => None,
        }
    }

    // ------------------------------------------------------------------
    // Expressions
    // ------------------------------------------------------------------

    fn expr(&mut self, expr: Expr<'_>, usage: Use) {
        if self.diverged {
            return;
        }
        if let Some(place) = self.place_of(expr) {
            let span = self.at(expr.span());
            match usage {
                Use::Value => self.use_value(&place, span),
                Use::Read => self.read_place(&place, span),
            }
            return;
        }
        match expr {
            Expr::Literal(_) | Expr::Path(_) | Expr::Continue(_) => {}
            Expr::StructLiteral(literal) => {
                for init in literal.inits() {
                    if let Some(value) = init.value() {
                        self.expr(value, Use::Value);
                    } else if let Some(name) = init.name_ref() {
                        // Shorthand `Point { x }` uses the binding `x` by
                        // value.
                        if let Some(&symbol) = self.cx.refs.get(&name.span) {
                            let kind = self.cx.resolution.symbol(symbol).kind;
                            if matches!(kind, SymbolKind::Param | SymbolKind::Local) {
                                self.use_value(&Place::root(symbol), name.span);
                            }
                        }
                    }
                }
            }
            Expr::ArrayLiteral(literal) => match literal.kind() {
                Some(ArrayLiteralKind::List(elements)) => {
                    // Each element is its own value, moved into the array.
                    for element in elements {
                        self.expr(element, Use::Value);
                    }
                }
                Some(ArrayLiteralKind::Repeat { value, .. }) => {
                    self.expr(value, Use::Value);
                    // §2: `[x; N]` duplicates its operand `N` times, and a
                    // non-`Copy` value cannot be duplicated — uniformly,
                    // with no `N == 0`/`1` special cases (O0010, ADR-0004
                    // Stage 2). This is also what makes the MIR lowering's
                    // `N × Operand::Copy(temp)` expansion verifier-legal.
                    let element_ty = value.span().and_then(|span| self.cx.types.expr_ty(span));
                    if let Some(ty) = element_ty
                        && !self.cx.env.is_copy(ty)
                    {
                        let span = self.at(literal.span());
                        self.report(
                            Diagnostic::error(
                                code(10),
                                "the repeated element of `[x; N]` must be a `Copy` type",
                                span,
                            )
                            .with_primary_label("a non-`Copy` value cannot be duplicated")
                            .with_help(
                                "list the elements explicitly, or use a `Copy` element type",
                            ),
                        );
                    }
                }
                None => {}
            },
            Expr::Unary(unary) => self.unary(unary, usage),
            Expr::Binary(binary) => {
                if let Some(lhs) = binary.lhs() {
                    self.expr(lhs, Use::Read);
                }
                if let Some(rhs) = binary.rhs() {
                    self.expr(rhs, Use::Read);
                }
            }
            Expr::Range(range) => {
                if let Some(lhs) = range.lhs() {
                    self.expr(lhs, Use::Read);
                }
                if let Some(rhs) = range.rhs() {
                    self.expr(rhs, Use::Read);
                }
            }
            Expr::Assign(assign) => self.assign(assign),
            Expr::Call(call) => self.call(call),
            Expr::MethodCall(call) => {
                // Method signatures arrive with the trait system; until then
                // receiver and arguments are treated as reads (the type
                // checker poisons method calls the same way).
                if let Some(receiver) = call.receiver() {
                    self.expr(receiver, Use::Read);
                }
                for arg in call.args() {
                    self.expr(arg, Use::Read);
                }
            }
            Expr::Field(field) => {
                if let Some(receiver) = field.receiver() {
                    self.expr(receiver, Use::Read);
                }
            }
            Expr::Index(index) => self.index(index, usage),
            Expr::Try(try_expr) => {
                if let Some(inner) = try_expr.inner() {
                    self.expr(inner, Use::Value);
                }
                // `?` is a conditional early exit: every scope's drops may
                // run here, so drop state must be statically known (§11).
                self.drop_sanity_from(0);
            }
            Expr::Cast(cast) => {
                if let Some(inner) = cast.inner() {
                    self.expr(inner, Use::Read);
                }
            }
            Expr::Group(group) => {
                if let Some(inner) = group.inner() {
                    self.expr(inner, usage);
                }
            }
            Expr::If(if_expr) => self.if_expr(if_expr, usage),
            Expr::Match(match_expr) => self.match_expr(match_expr, usage),
            Expr::While(while_expr) => self.while_expr(while_expr),
            Expr::Loop(loop_expr) => self.loop_expr(loop_expr),
            Expr::For(for_expr) => self.for_expr(for_expr),
            Expr::Unsafe(unsafe_expr) => {
                // §14: ownership rules apply identically inside `unsafe`.
                if let Some(body) = unsafe_expr.body() {
                    self.eval_block(body, usage);
                }
            }
            Expr::Return(ret) => {
                if let Some(value) = ret.value() {
                    self.expr(value, Use::Value);
                }
                self.drop_sanity_from(0);
                self.diverged = true;
            }
            Expr::Break(brk) => {
                if let Some(value) = brk.value() {
                    self.expr(value, Use::Value);
                }
                if let Some(index) = self.frame_index(brk.label()) {
                    let depth = self.loops[index].depth;
                    self.drop_sanity_from(depth);
                    let snapshot = self.state.clone();
                    self.loops[index].breaks.push(snapshot);
                }
                self.diverged = true;
            }
            Expr::Block(block) => self.eval_block(block, usage),
        }
        // `continue` needs its frame bookkeeping even though it has no
        // subexpressions.
        if let Expr::Continue(cont) = expr {
            if let Some(index) = self.frame_index(cont.label()) {
                let depth = self.loops[index].depth;
                self.drop_sanity_from(depth);
                let snapshot = self.state.clone();
                self.loops[index].continues.push(snapshot);
            }
            self.diverged = true;
        }
    }

    fn frame_index(&self, label: Option<&str>) -> Option<usize> {
        match label {
            Some(label) => self
                .loops
                .iter()
                .rposition(|frame| frame.label.as_deref() == Some(label)),
            None => self.loops.len().checked_sub(1),
        }
    }

    fn unary(&mut self, unary: UnaryExpr<'_>, _usage: Use) {
        let span = self.at(unary.span());
        if unary.op() == Some("move") {
            let Some(operand) = unary.operand() else {
                return;
            };
            match self.place_of(operand) {
                Some(place) => {
                    let ty = self.place_ty(&place);
                    let is_copy = ty.as_ref().is_some_and(|ty| self.cx.env.is_copy(ty));
                    if is_copy {
                        let rendered = ty
                            .map(|ty| ty.render(self.cx.resolution))
                            .unwrap_or_default();
                        let mut diagnostic = Diagnostic::error(
                            code(7),
                            format!(
                                "invalid explicit `move` of a `Copy` value of type `{rendered}`"
                            ),
                            span,
                        )
                        .with_primary_label("`Copy` values are duplicated, never moved")
                        .with_help("remove the `move` marker");
                        if let Some(edit) = self.remove_move_edit(unary) {
                            diagnostic = diagnostic.with_suggestion(
                                "remove the meaningless `move`",
                                vec![edit],
                                Confidence::MachineApplicable,
                            );
                        }
                        self.report(diagnostic);
                        self.read_place(&place, span);
                    } else {
                        let operand_span = self.at(operand.span());
                        self.use_value(&place, operand_span);
                    }
                }
                None => {
                    self.report(
                        Diagnostic::error(
                            code(7),
                            "invalid explicit `move` of a non-place expression",
                            span,
                        )
                        .with_primary_label(
                            "`move` applies to bindings, parameters, and their fields",
                        )
                        .with_help("remove the `move` marker; the value moves anyway"),
                    );
                    self.expr(operand, Use::Value);
                }
            }
            return;
        }
        if let Some(operand) = unary.operand() {
            self.expr(operand, Use::Read);
        }
    }

    /// The deletion that removes a `move ` marker, when both spans exist.
    fn remove_move_edit(&self, unary: UnaryExpr<'_>) -> Option<Edit> {
        let whole = unary.span()?;
        let operand = unary.operand()?.span()?;
        if operand.range().start() <= whole.range().start() {
            return None;
        }
        let range = TextRange::new(whole.range().start(), operand.range().start()).ok()?;
        Some(Edit {
            span: Span::new(whole.source(), range),
            replacement: String::new(),
        })
    }

    fn index(&mut self, index: tuo_ast::IndexExpr<'_>, usage: Use) {
        let base_place = index.base().and_then(|base| self.place_of(base));
        if let Some(place) = &base_place {
            let span = self.at(index.base().and_then(|base| base.span()));
            self.read_place(place, span);
        } else if let Some(base) = index.base() {
            self.expr(base, Use::Read);
        }
        if let Some(sub) = index.index() {
            self.expr(sub, Use::Read);
        }
        // §1: indexing reads an element; only `Copy` elements can be used by
        // value, and nothing moves out through an index.
        if usage == Use::Value {
            if let Some(Ty::Array(element) | Ty::FixedArray(element, _)) =
                base_place.as_ref().and_then(|place| self.place_ty(place))
            {
                if !self.cx.env.is_copy(&element) {
                    let span = self.at(index.span());
                    self.report(
                        Diagnostic::error(
                            code(7),
                            "cannot move an element out of an index expression",
                            span,
                        )
                        .with_primary_label(
                            "index expressions are not places in v0; only `Copy` elements \
                             can be read out by value",
                        ),
                    );
                }
            }
        }
    }

    fn assign(&mut self, assign: AssignExpr<'_>) {
        if let Some(rhs) = assign.rhs() {
            self.expr(rhs, Use::Value);
        }
        let Some(lhs) = assign.lhs() else {
            return;
        };
        let span = self.at(lhs.span());
        let Some(place) = self.place_of(lhs) else {
            self.expr(lhs, Use::Read);
            self.report(
                Diagnostic::error(code(4), "assignment target is not a mutable place", span)
                    .with_primary_label(
                        "only bindings, mutable parameters, and their fields can be assigned \
                     (index expressions are not places in v0)",
                    ),
            );
            return;
        };
        // Writing through a moved (or possibly moved) prefix is a use of
        // that prefix.
        let mut prefix = Place::root(place.root);
        for field in &place.path {
            if self.state.contains_key(&prefix) && !self.check_usable(&prefix.clone(), span) {
                break;
            }
            prefix = prefix.child(field);
        }
        let Some(root) = self.roots.get(&place.root) else {
            return;
        };
        let origin = root.origin;
        let root_name = root.name.clone();
        let decl = root.decl;
        if !origin.is_mutable() {
            let deferred = matches!(origin, Origin::Let { deferred: true });
            if deferred && place.path.is_empty() {
                match self.state.get(&place) {
                    Some(PlaceState::Uninit) => {
                        // First assignment on this path: initialization, not
                        // mutation (§7).
                        self.state.remove(&place);
                        self.init_spans.insert(place, span);
                        return;
                    }
                    _ => {
                        let mut diagnostic = Diagnostic::error(
                            code(4),
                            format!("`{root_name}` is already initialized"),
                            span,
                        )
                        .with_primary_label(
                            "a `let` binding may be initialized at most once per path",
                        );
                        if let Some(init) = self.init_spans.get(&place) {
                            diagnostic =
                                diagnostic.with_secondary_label(*init, "first initialized here");
                        }
                        self.report(
                            diagnostic
                                .with_help(format!("declare it `var {root_name}` to reassign")),
                        );
                        return;
                    }
                }
            }
            let what = if place.path.is_empty() {
                format!(
                    "cannot assign to `{root_name}`: it is {}",
                    origin.describe()
                )
            } else {
                format!(
                    "cannot assign to `{}`: `{root_name}` is {}",
                    self.display(&place),
                    origin.describe()
                )
            };
            let mut diagnostic = Diagnostic::error(code(4), what, span)
                .with_secondary_label(decl, format!("`{root_name}` declared immutable here"));
            if matches!(origin, Origin::Let { .. } | Origin::PatternBinding) {
                diagnostic = diagnostic
                    .with_help(format!("declare it `var {root_name}` to make it mutable"));
            }
            self.report(diagnostic);
            return;
        }
        // Mutable place: the old value is dropped here, so its state must be
        // statically known (§11).
        if let Some(state) = self.state.get(&place).cloned() {
            match state {
                PlaceState::MaybeMoved { at } => {
                    self.report_conditional_move(&place, at);
                }
                PlaceState::MaybeInit { init } => {
                    if !self.is_copy_place(&place) {
                        self.report_conditional_move(&place, init);
                    }
                }
                PlaceState::Moved { .. } | PlaceState::Uninit => {}
            }
        }
        // The store (re)initializes the place and everything under it.
        self.state.retain(|key, _| !place.is_prefix_of(key));
        self.init_spans.insert(place, span);
    }

    /// Report O0008: a conditionally moved (or conditionally initialized)
    /// non-`Copy` place reached a drop point.
    fn report_conditional_move(&mut self, place: &Place, at: Span) {
        let name = self.display(place);
        let root_decl = self.roots.get(&place.root).map(|root| root.decl);
        let mut diagnostic = Diagnostic::error(
            code(8),
            format!(
                "`{name}` is moved conditionally, but its drop point needs a statically \
                     known state"
            ),
            at,
        )
        .with_primary_label(format!(
            "`{name}` loses its value here, but keeps it on the other path"
        ))
        .with_help(
            "move it on every path, reinitialize it before the paths join, or drop it \
             explicitly on the moving path — tuonelang never inserts hidden drop flags",
        );
        if let Some(decl) = root_decl {
            diagnostic = diagnostic.with_secondary_label(decl, format!("`{name}` declared here"));
        }
        self.report(diagnostic);
        self.heal(place);
    }

    fn call(&mut self, call: CallExpr<'_>) {
        let callee_symbol = call.callee().and_then(|callee| match callee {
            Expr::Path(path) => {
                let name = path.segment_names().last()?;
                self.cx.refs.get(&name.span).copied()
            }
            _ => None,
        });
        if let Some(callee) = call.callee() {
            if let Some(place) = self.place_of(callee) {
                let span = self.at(callee.span());
                self.read_place(&place, span);
            }
        }
        let modes = callee_symbol.and_then(|symbol| self.cx.fn_modes.get(&symbol));
        let Some(modes) = modes.cloned() else {
            // Unknown callee signature (function-typed value, unresolved
            // name): arguments are treated as `in` borrows. Deliberate and
            // documented: v0 function *types* do not carry modes yet.
            for arg in call.args() {
                if let Some(place) = self.place_of(arg) {
                    let span = self.at(arg.span());
                    self.read_place(&place, span);
                } else {
                    self.expr(arg, Use::Read);
                }
            }
            return;
        };
        struct ArgRec {
            place: Place,
            mode: Mode,
            span: Span,
        }
        let mut recs: Vec<ArgRec> = Vec::new();
        for (index, arg) in call.args().enumerate() {
            let mode = modes.get(index).copied().unwrap_or(Mode::In);
            let span = self.at(arg.span());
            match self.place_of(arg) {
                Some(place) => recs.push(ArgRec { place, mode, span }),
                None => self.expr(
                    arg,
                    if mode == Mode::Take {
                        Use::Value
                    } else {
                        Use::Read
                    },
                ),
            }
        }
        // Borrow arguments must be usable and — for `mut` — mutable (§4, §7).
        for rec in &recs {
            if rec.mode.is_borrow() {
                let place = rec.place.clone();
                self.check_usable(&place, rec.span);
            }
        }
        let mut_violations: Vec<(Place, Span)> = recs
            .iter()
            .filter(|rec| rec.mode == Mode::Mut)
            .filter(|rec| {
                self.roots
                    .get(&rec.place.root)
                    .is_some_and(|root| !root.origin.is_mutable())
            })
            .map(|rec| (rec.place.clone(), rec.span))
            .collect();
        for (place, span) in mut_violations {
            let root = &self.roots[&place.root];
            let root_name = root.name.clone();
            let describe = root.origin.describe();
            let decl = root.decl;
            let what = if place.path.is_empty() {
                format!("cannot pass `{root_name}` as `mut`: it is {describe}")
            } else {
                format!(
                    "cannot pass `{}` as `mut`: `{root_name}` is {describe}",
                    self.display(&place)
                )
            };
            self.report(
                Diagnostic::error(code(4), what, span)
                    .with_primary_label("a `mut` argument needs a mutable place")
                    .with_secondary_label(decl, format!("`{root_name}` declared immutable here"))
                    .with_help("`mut` borrows require a `var` binding or a `mut`/`take` parameter"),
            );
        }
        // Conflicts inside one argument list (§6). One report per call.
        let mut suppressed_takes: Vec<usize> = Vec::new();
        'conflicts: for i in 0..recs.len() {
            for j in (i + 1)..recs.len() {
                let (a, b) = (&recs[i], &recs[j]);
                if !a.place.overlaps(&b.place) {
                    continue;
                }
                if a.mode.is_borrow() && b.mode.is_borrow() {
                    if a.mode == Mode::Mut || b.mode == Mode::Mut {
                        let name_a = self.display(&a.place);
                        let name_b = self.display(&b.place);
                        let (span_a, span_b) = (a.span, b.span);
                        self.report(
                            Diagnostic::error(
                                code(5),
                                format!(
                                    "conflicting borrows of `{name_a}` and `{name_b}` in one call"
                                ),
                                span_b,
                            )
                            .with_primary_label(
                                "overlapping places cannot be borrowed with a `mut` borrow \
                                 in the same argument list",
                            )
                            .with_secondary_label(span_a, "first borrow of the overlapping place")
                            .with_help(
                                "a value has either any number of `in` borrows or exactly one \
                                 `mut` borrow; pass disjoint fields, or split the call in two",
                            ),
                        );
                        break 'conflicts;
                    }
                } else if a.mode.is_borrow() != b.mode.is_borrow()
                    && (a.mode == Mode::Take || b.mode == Mode::Take)
                {
                    let (take, borrow) = if a.mode == Mode::Take { (i, j) } else { (j, i) };
                    let take_name = self.display(&recs[take].place);
                    let borrow_name = self.display(&recs[borrow].place);
                    let (take_span, borrow_span) = (recs[take].span, recs[borrow].span);
                    self.report(
                        Diagnostic::error(
                            code(6),
                            format!(
                                "`{take_name}` is moved into a `take` argument while \
                                 `{borrow_name}` is borrowed in the same call"
                            ),
                            take_span,
                        )
                        .with_primary_label("moved here by the `take` argument")
                        .with_secondary_label(borrow_span, "borrowed here, in the same call")
                        .with_help("evaluate the borrowing argument in a separate call first"),
                    );
                    suppressed_takes.push(take);
                    break 'conflicts;
                }
            }
        }
        // Apply the `take` moves in argument order (left to right, §6):
        // overlapping double-takes surface as ordinary use-after-move.
        for (index, rec) in recs.iter().enumerate() {
            if rec.mode == Mode::Take && !suppressed_takes.contains(&index) {
                let place = rec.place.clone();
                self.use_value(&place, rec.span);
            }
        }
    }

    // ------------------------------------------------------------------
    // Control flow
    // ------------------------------------------------------------------

    fn if_expr(&mut self, if_expr: IfExpr<'_>, usage: Use) {
        if let Some(cond) = if_expr.condition() {
            self.expr(cond, Use::Read);
        }
        if self.diverged {
            return;
        }
        let base = self.state.clone();
        if let Some(then) = if_expr.then_block() {
            self.eval_block(then, usage);
        }
        let then_state = std::mem::replace(&mut self.state, base.clone());
        let then_diverged = std::mem::replace(&mut self.diverged, false);
        match if_expr.else_branch() {
            Some(ElseBranch::If(nested)) => self.if_expr(nested, usage),
            Some(ElseBranch::Block(block)) => self.eval_block(block, usage),
            None => {}
        }
        match (then_diverged, self.diverged) {
            (false, false) => {
                let else_state = std::mem::take(&mut self.state);
                self.state = self.merge(&then_state, &else_state);
                self.scrub_healed();
            }
            (false, true) => {
                self.state = then_state;
                self.diverged = false;
                self.scrub_healed();
            }
            (true, false) => self.scrub_healed(),
            (true, true) => self.diverged = true,
        }
    }

    fn match_expr(&mut self, match_expr: MatchExpr<'_>, usage: Use) {
        // §10: a match moves its scrutinee iff some arm binds a non-`Copy`
        // part of it; the decision is static.
        let mut noncopy_binding: Option<Name<'_>> = None;
        for arm in match_expr.arms() {
            if let Some(pattern) = arm.pattern() {
                for name in collect_bindings(pattern) {
                    // Only a real binding (recorded as a declaration) can move
                    // out of the scrutinee. A bare name that resolved to a unit
                    // variant (`None`) is a discriminant test, not a binding, so
                    // it binds nothing and never forces a move.
                    let Some(&symbol) = self.cx.decls.get(&name.span) else {
                        continue;
                    };
                    let is_copy = self
                        .cx
                        .types
                        .type_of(symbol)
                        .is_some_and(|ty| self.cx.env.is_copy(ty));
                    if !is_copy && noncopy_binding.is_none() {
                        noncopy_binding = Some(name);
                    }
                }
            }
        }
        if let Some(scrutinee) = match_expr.scrutinee() {
            let span = self.at(scrutinee.span());
            match (self.place_of(scrutinee), noncopy_binding) {
                (Some(place), Some(binding)) => {
                    if let Some(mode) = self
                        .roots
                        .get(&place.root)
                        .and_then(|root| root.origin.borrow_mode())
                    {
                        if self.check_usable(&place, span) {
                            self.report_borrow_move(&place, mode, binding.span);
                        }
                    } else {
                        self.use_value(&place, span);
                    }
                }
                (Some(place), None) => self.read_place(&place, span),
                (None, Some(_)) => self.expr(scrutinee, Use::Value),
                (None, None) => self.expr(scrutinee, Use::Read),
            }
        }
        if self.diverged {
            return;
        }
        let base = self.state.clone();
        let mut joined: Option<StateMap> = None;
        for arm in match_expr.arms() {
            self.state = base.clone();
            self.diverged = false;
            self.scopes.push(Vec::new());
            if let Some(pattern) = arm.pattern() {
                for name in collect_bindings(pattern) {
                    if let Some(&symbol) = self.cx.decls.get(&name.span) {
                        self.register(symbol, name, Origin::PatternBinding, true);
                    }
                }
            }
            if let Some(guard) = arm.guard() {
                self.expr(guard, Use::Read);
            }
            if let Some(value) = arm.value() {
                self.expr(value, usage);
            }
            if !self.diverged {
                self.scope_end();
                joined = Some(match joined {
                    Some(acc) => self.merge(&acc, &self.state.clone()),
                    None => self.state.clone(),
                });
            } else {
                self.discard_scope();
            }
        }
        match joined {
            Some(state) => {
                self.state = state;
                self.diverged = false;
                self.scrub_healed();
            }
            None => {
                self.state = base;
                self.diverged = true;
            }
        }
    }

    fn while_expr(&mut self, while_expr: WhileExpr<'_>) {
        if let Some(cond) = while_expr.condition() {
            self.expr(cond, Use::Read);
        }
        if self.diverged {
            return;
        }
        let head = self.state.clone();
        self.loops.push(LoopFrame {
            label: None,
            depth: self.scopes.len(),
            breaks: Vec::new(),
            continues: Vec::new(),
        });
        if let Some(body) = while_expr.body() {
            self.eval_block(body, Use::Value);
        }
        let frame = self.loops.pop().expect("frame pushed above");
        let mut back_states = frame.continues;
        if !self.diverged {
            back_states.push(self.state.clone());
        }
        self.back_edge_check(&head, &back_states);
        // The loop exits from its head (condition false) or through `break`.
        let mut exit = head;
        for brk in &frame.breaks {
            exit = self.merge(&exit, brk);
        }
        self.state = exit;
        self.diverged = false;
        self.scrub_healed();
    }

    fn loop_expr(&mut self, loop_expr: LoopExpr<'_>) {
        let head = self.state.clone();
        self.loops.push(LoopFrame {
            label: loop_expr.label().map(str::to_owned),
            depth: self.scopes.len(),
            breaks: Vec::new(),
            continues: Vec::new(),
        });
        if let Some(body) = loop_expr.body() {
            self.eval_block(body, Use::Value);
        }
        let frame = self.loops.pop().expect("frame pushed above");
        let mut back_states = frame.continues;
        if !self.diverged {
            back_states.push(self.state.clone());
        }
        self.back_edge_check(&head, &back_states);
        // A `loop` only exits through `break`.
        let mut exit: Option<StateMap> = None;
        for brk in &frame.breaks {
            exit = Some(match exit {
                Some(acc) => self.merge(&acc, brk),
                None => brk.clone(),
            });
        }
        match exit {
            Some(state) => {
                self.state = state;
                self.diverged = false;
                self.scrub_healed();
            }
            None => {
                self.state = head;
                self.diverged = true;
            }
        }
    }

    fn for_expr(&mut self, for_expr: ForExpr<'_>) {
        // §3/§10: `for` moves its iterable once, before the first iteration.
        if let Some(iterable) = for_expr.iterable() {
            self.expr(iterable, Use::Value);
        }
        if self.diverged {
            return;
        }
        let head = self.state.clone();
        self.loops.push(LoopFrame {
            label: None,
            depth: self.scopes.len(),
            breaks: Vec::new(),
            continues: Vec::new(),
        });
        self.scopes.push(Vec::new());
        if let Some(pattern) = for_expr.pattern() {
            for name in collect_bindings(pattern) {
                if let Some(&symbol) = self.cx.decls.get(&name.span) {
                    self.register(symbol, name, Origin::PatternBinding, true);
                }
            }
        }
        if let Some(body) = for_expr.body() {
            self.eval_block(body, Use::Value);
        }
        if !self.diverged {
            self.scope_end();
        } else {
            self.discard_scope();
        }
        let frame = self.loops.pop().expect("frame pushed above");
        let mut back_states = frame.continues;
        if !self.diverged {
            back_states.push(self.state.clone());
        }
        self.back_edge_check(&head, &back_states);
        let mut exit = head;
        for brk in &frame.breaks {
            exit = self.merge(&exit, brk);
        }
        self.state = exit;
        self.diverged = false;
        self.scrub_healed();
    }

    /// §10 loops: a place that was usable at the loop head but is (possibly)
    /// moved when the back edge rejoins it would be possibly-moved at the
    /// next iteration's use — reported at the responsible move. A deferred
    /// `let` initialized inside the body would be initialized twice.
    fn back_edge_check(&mut self, head: &StateMap, back_states: &[StateMap]) {
        let Some(back) = back_states
            .iter()
            .cloned()
            .reduce(|acc, next| self.merge(&acc, &next))
        else {
            return;
        };
        let mut keys: BTreeSet<Place> = head.keys().cloned().collect();
        keys.extend(back.keys().cloned());
        for key in keys {
            let head_state = Self::governing(head, &key).map(|(_, state)| state.clone());
            let back_state = Self::governing(&back, &key).map(|(_, state)| state.clone());
            match (head_state, back_state) {
                (None, Some(PlaceState::Moved { at } | PlaceState::MaybeMoved { at })) => {
                    let name = self.display(&key);
                    let decl = self.roots.get(&key.root).map(|root| root.decl);
                    let mut diagnostic = Diagnostic::error(
                        code(2),
                        format!(
                            "`{name}` may already be moved when the next iteration reaches \
                             this point"
                        ),
                        at,
                    )
                    .with_primary_label(format!(
                        "`{name}` is moved here without being reinitialized before the \
                         loop's back edge"
                    ))
                    .with_help(
                        "reinitialize it before the end of the loop body, or move it only \
                         on paths that exit the loop",
                    );
                    if let Some(decl) = decl {
                        diagnostic = diagnostic
                            .with_secondary_label(decl, format!("`{name}` declared here"));
                    }
                    self.report(diagnostic);
                    self.heal(&key);
                }
                (
                    Some(PlaceState::Uninit),
                    None
                    | Some(
                        PlaceState::MaybeInit { .. }
                        | PlaceState::Moved { .. }
                        | PlaceState::MaybeMoved { .. },
                    ),
                ) => {
                    if matches!(
                        self.roots.get(&key.root).map(|root| root.origin),
                        Some(Origin::Let { deferred: true })
                    ) {
                        let name = self.display(&key);
                        let span = self
                            .init_spans
                            .get(&key)
                            .copied()
                            .or_else(|| self.roots.get(&key.root).map(|root| root.decl))
                            .unwrap_or(self.fallback);
                        self.report(
                            Diagnostic::error(
                                code(4),
                                format!(
                                    "`{name}` may be initialized more than once across loop \
                                     iterations"
                                ),
                                span,
                            )
                            .with_primary_label(
                                "a `let` binding may be initialized at most once per path",
                            )
                            .with_help(format!("declare it `var {name}` to reassign")),
                        );
                        self.heal(&key);
                    }
                }
                _ => {}
            }
        }
    }

    // ------------------------------------------------------------------
    // Blocks, statements, scopes
    // ------------------------------------------------------------------

    fn eval_block(&mut self, block: Block<'_>, usage: Use) {
        self.scopes.push(Vec::new());
        let statements: Vec<Statement<'_>> = block.statements().collect();
        // A final semicolon-less expression statement is the block's value
        // (block-form expressions parse as statements, never as the
        // syntactic tail).
        let tail_statement = match (block.tail(), statements.last()) {
            (None, Some(Statement::Expr(statement))) if !statement.has_semicolon() => {
                statement.expr()
            }
            _ => None,
        };
        let statement_count = statements.len() - usize::from(tail_statement.is_some());
        for statement in &statements[..statement_count] {
            if self.diverged {
                break;
            }
            match *statement {
                Statement::Let(binding) | Statement::Var(binding) => self.binding_stmt(binding),
                Statement::Expr(statement) => {
                    if let Some(expr) = statement.expr() {
                        self.expr(expr, Use::Value);
                    }
                }
                Statement::Const(_) | Statement::Empty(_) | Statement::Error(_) => {}
            }
        }
        if let Some(tail) = block.tail().or(tail_statement) {
            if !self.diverged {
                self.expr(tail, usage);
            }
        }
        if self.diverged {
            self.discard_scope();
        } else {
            self.scope_end();
        }
    }

    fn binding_stmt(&mut self, binding: BindingStmt<'_>) {
        let initializer = binding.initializer();
        if let Some(init) = initializer {
            self.expr(init, Use::Value);
        }
        let Some(pattern) = binding.pattern() else {
            return;
        };
        let origin = if binding.is_var() {
            Origin::Var
        } else {
            Origin::Let {
                deferred: initializer.is_none(),
            }
        };
        for name in collect_bindings(pattern) {
            if let Some(&symbol) = self.cx.decls.get(&name.span) {
                self.register(symbol, name, origin, initializer.is_some());
            }
        }
    }

    /// Leave the innermost scope normally: its roots reach their drop point
    /// (§11 — reverse declaration order), so their state must be statically
    /// known; then they stop being tracked.
    fn scope_end(&mut self) {
        let Some(scope) = self.scopes.pop() else {
            return;
        };
        for &symbol in scope.iter().rev() {
            self.drop_sanity_root(symbol);
            self.state.retain(|key, _| key.root != symbol);
            self.init_spans.retain(|key, _| key.root != symbol);
        }
    }

    /// Leave the innermost scope on a diverged path: no drop-point checks
    /// (the exit's own check already ran), just stop tracking.
    fn discard_scope(&mut self) {
        let Some(scope) = self.scopes.pop() else {
            return;
        };
        for symbol in scope {
            self.state.retain(|key, _| key.root != symbol);
            self.init_spans.retain(|key, _| key.root != symbol);
        }
    }

    /// Drop-point check for every root in scopes `depth..` (used by early
    /// exits: `return`, `?`, `break`, `continue`), without untracking.
    fn drop_sanity_from(&mut self, depth: usize) {
        let symbols: Vec<SymbolId> = self.scopes[depth.min(self.scopes.len())..]
            .iter()
            .flatten()
            .copied()
            .collect();
        for symbol in symbols.into_iter().rev() {
            self.drop_sanity_root(symbol);
        }
    }

    /// §11 "no hidden drop flags": every place rooted at `symbol` must have
    /// statically known drop state.
    fn drop_sanity_root(&mut self, symbol: SymbolId) {
        let entries: Vec<(Place, PlaceState)> = self
            .state
            .iter()
            .filter(|(key, _)| key.root == symbol)
            .map(|(key, state)| (key.clone(), state.clone()))
            .collect();
        for (place, state) in entries {
            match state {
                PlaceState::MaybeMoved { at } => self.report_conditional_move(&place, at),
                PlaceState::MaybeInit { init } => {
                    if !self.is_copy_place(&place) {
                        self.report_conditional_move(&place, init);
                    }
                }
                PlaceState::Moved { .. } | PlaceState::Uninit => {}
            }
        }
    }

    // ------------------------------------------------------------------
    // Joins
    // ------------------------------------------------------------------

    /// Merge the states of two arriving paths (§10): a place is initialized
    /// after a join iff it is initialized on every path.
    fn merge(&self, a: &StateMap, b: &StateMap) -> StateMap {
        let mut keys: BTreeSet<Place> = a.keys().cloned().collect();
        keys.extend(b.keys().cloned());
        let mut out = StateMap::new();
        for key in keys {
            let sa = Self::governing(a, &key).map(|(_, state)| state.clone());
            let sb = Self::governing(b, &key).map(|(_, state)| state.clone());
            if let Some(state) = self.merge_pair(&key, sa, sb) {
                out.insert(key, state);
            }
        }
        out
    }

    fn merge_pair(
        &self,
        key: &Place,
        a: Option<PlaceState>,
        b: Option<PlaceState>,
    ) -> Option<PlaceState> {
        use PlaceState::{MaybeInit, MaybeMoved, Moved, Uninit};
        let init_span = || {
            self.init_spans
                .get(key)
                .copied()
                .or_else(|| self.roots.get(&key.root).map(|root| root.decl))
                .unwrap_or(self.fallback)
        };
        match (a, b) {
            (None, None) => None,
            (None, Some(Uninit)) | (Some(Uninit), None) => Some(MaybeInit { init: init_span() }),
            (None, Some(Moved { at } | MaybeMoved { at }))
            | (Some(Moved { at } | MaybeMoved { at }), None) => Some(MaybeMoved { at }),
            (None, Some(MaybeInit { init })) | (Some(MaybeInit { init }), None) => {
                Some(MaybeInit { init })
            }
            (Some(Moved { at }), Some(Moved { .. } | Uninit))
            | (Some(Uninit), Some(Moved { at })) => Some(Moved { at }),
            (Some(Moved { at }), Some(MaybeMoved { .. } | MaybeInit { .. }))
            | (Some(MaybeMoved { .. } | MaybeInit { .. }), Some(Moved { at })) => {
                Some(MaybeMoved { at })
            }
            (Some(Uninit), Some(Uninit)) => Some(Uninit),
            (Some(Uninit), Some(MaybeInit { init })) | (Some(MaybeInit { init }), Some(Uninit)) => {
                Some(MaybeInit { init })
            }
            (Some(MaybeMoved { at }), Some(MaybeMoved { .. } | MaybeInit { .. } | Uninit))
            | (Some(MaybeInit { .. } | Uninit), Some(MaybeMoved { at })) => Some(MaybeMoved { at }),
            (Some(MaybeInit { init }), Some(MaybeInit { .. })) => Some(MaybeInit { init }),
        }
    }
}

fn state_rank(state: &PlaceState) -> u8 {
    match state {
        PlaceState::MaybeInit { .. } | PlaceState::MaybeMoved { .. } => 1,
        PlaceState::Uninit | PlaceState::Moved { .. } => 2,
    }
}

/// Every name a pattern binds, in source order.
fn collect_bindings<'a>(pattern: Pattern<'a>) -> Vec<Name<'a>> {
    fn walk<'a>(pattern: Pattern<'a>, out: &mut Vec<Name<'a>>) {
        match pattern {
            Pattern::Binding(binding) => {
                if let Some(name) = binding.name_ref() {
                    out.push(name);
                }
            }
            Pattern::Path(path) => {
                for field in path.fields() {
                    walk_field(field, out);
                }
            }
            Pattern::Or(or) => {
                for alternative in or.alternatives() {
                    walk(alternative, out);
                }
            }
            Pattern::Group(group) => {
                if let Some(inner) = group.inner() {
                    walk(inner, out);
                }
            }
            Pattern::Literal(_) | Pattern::Wildcard(_) => {}
        }
    }
    fn walk_field<'a>(field: FieldPat<'a>, out: &mut Vec<Name<'a>>) {
        match field.pattern() {
            Some(sub) => walk(sub, out),
            None => {
                if let Some(name) = field.name_ref() {
                    out.push(name);
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(pattern, &mut out);
    out
}
