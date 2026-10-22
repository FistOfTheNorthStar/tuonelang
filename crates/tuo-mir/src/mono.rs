//! Monomorphization (ADR-0027 Stage B): one concrete function per
//! instantiation of a generic `fn`, so a backend never sees [`Ty::Param`].
//!
//! The interpreter runs generic MIR as it is — its values carry their own
//! types — so this pass belongs to the native path alone, run after
//! [`crate::retain_reachable`] and before [`crate::optimize`]. The MIR the
//! interpreter, the incremental session, and `tuo debug mir` see is never
//! monomorphized, which keeps the reference semantics on the program as
//! written.
//!
//! # Identity
//!
//! An instance is a `(generic symbol, type arguments)` pair. Each gets a fresh
//! [`SymbolId`] minted downward from `u32::MAX`, far above any symbol the
//! resolver assigns, and calls are rewritten to name it, so `Callee::Direct`
//! keeps meaning one body per symbol and no other layer changes. The
//! instance's name renders its arguments (`ident[I64]`) for `debug mir`.
//!
//! # Finding the type arguments
//!
//! MIR records no type arguments at a call. The pass recovers them by
//! matching the callee's declared parameter and return types — which mention
//! the type parameters — against the caller's argument and destination
//! types, which are concrete once the caller is. A type parameter that
//! appears in neither cannot be recovered and is refused by name rather than
//! guessed.
//!
//! # Termination
//!
//! A generic function may reach itself at a larger type (`f[T]` calling
//! `f[Array[T]]`), which has no finite monomorphization. Function bodies do
//! not depend on their type arguments (there is no type-directed dispatch),
//! so a chain that reaches the same function at a larger type will do so
//! forever; the pass refuses it by name the first time it happens, never
//! with a silent depth cap (ADR-0027 Q4).

use std::collections::HashMap;

use tuo_resolve::{Resolution, SymbolId};
use tuo_types::{FnParam, FnTy, Ty, TypeckResult};

use crate::mir::{
    AggregateKind, Arg, Callee, Const, Function, LocalDecl, Operand, Place, Program, Projection,
    Rvalue, Statement, Terminator,
};

/// Why a program could not be monomorphized. Each reason is a native-path
/// refusal only: the interpreter still runs the program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MonoError {
    /// What went wrong, naming the function(s) involved.
    pub message: String,
}

/// Replace every generic function in `program` by the instances its
/// non-generic functions reach, rewriting each call to name its instance.
/// A generic function nothing concrete reaches produces no instance. On an
/// error `program` is left as it was.
///
/// # Errors
///
/// A call whose type arguments cannot be recovered from its argument and
/// destination types, or an instantiation chain that grows without bound.
pub fn monomorphize(
    program: &mut Program,
    types: &TypeckResult,
    resolution: &Resolution,
) -> Result<(), MonoError> {
    if !program.functions.iter().any(is_generic) {
        return Ok(());
    }
    let (generic, mut concrete): (Vec<Function>, Vec<Function>) =
        program.functions.iter().cloned().partition(is_generic);
    let generics: HashMap<SymbolId, Generic> = generic
        .into_iter()
        .map(|body| {
            let params = type_params(&body);
            (body.symbol, Generic { params, body })
        })
        .collect();
    let mut mono = Mono {
        types,
        resolution,
        generics: &generics,
        instances: Vec::new(),
        next_raw: u32::MAX,
    };
    for function in &mut concrete {
        mono.rewrite_calls(function, None)?;
    }
    // Each instance is scanned after it is made, so instances reached only
    // through other instances are found too.
    let mut cursor = 0;
    while cursor < mono.instances.len() {
        let mut body = mono.instances[cursor].function.clone();
        mono.rewrite_calls(&mut body, Some(cursor))?;
        mono.instances[cursor].function = body;
        cursor += 1;
    }
    concrete.extend(mono.instances.into_iter().map(|instance| instance.function));
    program.functions = concrete;
    Ok(())
}

/// Does `function` mention a type parameter anywhere in its signature or
/// locals? (Every value lives in a local, so this covers the body too.)
fn is_generic(function: &Function) -> bool {
    contains_param(&function.ret)
        || function
            .locals
            .iter()
            .any(|local| contains_param(&local.ty))
}

/// The type parameters `function` mentions, in first-appearance order over
/// the parameters, the return type, then the other locals — the order an
/// instance's type arguments are listed in.
fn type_params(function: &Function) -> Vec<SymbolId> {
    let mut out = Vec::new();
    let params = function.params.len();
    let signature = function.locals[..params]
        .iter()
        .map(|local| &local.ty)
        .chain(std::iter::once(&function.ret))
        .chain(function.locals[params..].iter().map(|local| &local.ty));
    for ty in signature {
        collect_params(ty, &mut out);
    }
    out
}

fn collect_params(ty: &Ty, out: &mut Vec<SymbolId>) {
    match ty {
        Ty::Param(symbol) => {
            if !out.contains(symbol) {
                out.push(*symbol);
            }
        }
        Ty::Tuple(items) | Ty::Struct(_, items) | Ty::Enum(_, items) => {
            for item in items {
                collect_params(item, out);
            }
        }
        Ty::Array(item)
        | Ty::FixedArray(item, _)
        | Ty::Range(item)
        | Ty::Option(item)
        | Ty::Wrapper(_, item) => collect_params(item, out),
        Ty::Result(a, b) | Ty::Map(a, b) => {
            collect_params(a, out);
            collect_params(b, out);
        }
        Ty::Fn(fn_ty) => {
            for param in &fn_ty.params {
                collect_params(&param.ty, out);
            }
            collect_params(&fn_ty.ret, out);
        }
        Ty::Unit
        | Ty::Never
        | Ty::Bool
        | Ty::Char
        | Ty::String
        | Ty::Str
        | Ty::Int(_)
        | Ty::Float(_)
        | Ty::Var(_)
        | Ty::Error => {}
    }
}

fn contains_param(ty: &Ty) -> bool {
    let mut found = Vec::new();
    collect_params(ty, &mut found);
    !found.is_empty()
}

/// A generic function: its type parameters, in instance-argument order, and
/// its body as lowered (still mentioning them).
struct Generic {
    params: Vec<SymbolId>,
    body: Function,
}

/// One generated instance. `parent` is the instance whose body asked for it
/// (`None` for a concrete function), so a growing chain can be reported.
struct Instance {
    generic: SymbolId,
    args: Vec<Ty>,
    parent: Option<usize>,
    function: Function,
}

struct Mono<'a> {
    types: &'a TypeckResult,
    resolution: &'a Resolution,
    generics: &'a HashMap<SymbolId, Generic>,
    instances: Vec<Instance>,
    next_raw: u32,
}

impl Mono<'_> {
    /// Point every direct call from `function` to a generic at the instance
    /// its argument types select, making the instance if it is new.
    fn rewrite_calls(
        &mut self,
        function: &mut Function,
        parent: Option<usize>,
    ) -> Result<(), MonoError> {
        let caller = function.name.clone();
        let locals = &function.locals;
        for block in &mut function.blocks {
            for statement in &mut block.statements {
                let Statement::Call {
                    dest,
                    callee: Callee::Direct(symbol),
                    args,
                } = statement
                else {
                    continue;
                };
                let Some(generic) = self.generics.get(symbol) else {
                    continue;
                };
                let type_args = self.infer(locals, generic, dest, args, &caller)?;
                *symbol = self.instance(*symbol, type_args, parent)?;
            }
        }
        Ok(())
    }

    /// The type arguments of one call to `generic`, recovered by matching its
    /// declared parameter and return types against the caller's types.
    fn infer(
        &self,
        locals: &[LocalDecl],
        generic: &Generic,
        dest: &Place,
        args: &[Arg],
        caller: &str,
    ) -> Result<Vec<Ty>, MonoError> {
        let mut bound = HashMap::new();
        for (declared, arg) in generic.body.locals.iter().zip(args) {
            let actual = match arg {
                Arg::Value(operand) => self.operand_ty(locals, operand),
                Arg::Borrow(place) | Arg::BorrowMut(place) => self.place_ty(locals, place),
            };
            if let Some(actual) = actual {
                bind(&declared.ty, &actual, &mut bound);
            }
        }
        if let Some(actual) = self.place_ty(locals, dest) {
            bind(&generic.body.ret, &actual, &mut bound);
        }
        generic
            .params
            .iter()
            .map(|param| {
                bound.get(param).cloned().ok_or_else(|| MonoError {
                    message: format!(
                        "cannot tell which `{}` the call to `{}` in `{caller}` uses: it appears \
                         in neither a parameter nor the return type, so no native instance can \
                         be chosen",
                        self.resolution.symbol(*param).name,
                        generic.body.name,
                    ),
                })
            })
            .collect()
    }

    /// The instance of `generic` at `args`, made (and queued for scanning)
    /// the first time it is asked for.
    fn instance(
        &mut self,
        generic: SymbolId,
        args: Vec<Ty>,
        parent: Option<usize>,
    ) -> Result<SymbolId, MonoError> {
        // `Ty` is not hashable, and a program has few instances: a scan.
        if let Some(existing) = self
            .instances
            .iter()
            .find(|instance| instance.generic == generic && instance.args == args)
        {
            return Ok(existing.function.symbol);
        }
        self.refuse_growth(generic, &args, parent)?;
        let template = &self.generics[&generic];
        let symbol = SymbolId::from_raw(self.next_raw);
        self.next_raw -= 1;
        let mut function = template.body.clone();
        substitute(&mut function, &template.params, &args);
        self.copy_what_became_copy(&mut function);
        function.symbol = symbol;
        function.name = self.instance_name(&template.body.name, &args);
        self.instances.push(Instance {
            generic,
            args,
            parent,
            function,
        });
        Ok(symbol)
    }

    /// Refuse an instance whose own ancestry already holds `generic` at a
    /// smaller type: the chain repeats with ever larger types and never ends.
    fn refuse_growth(
        &self,
        generic: SymbolId,
        args: &[Ty],
        parent: Option<usize>,
    ) -> Result<(), MonoError> {
        let size = args.iter().map(type_size).sum::<usize>();
        let mut ancestor = parent;
        while let Some(index) = ancestor {
            let earlier = &self.instances[index];
            if earlier.generic == generic
                && earlier.args.iter().map(type_size).sum::<usize>() < size
            {
                let name = &self.generics[&generic].body.name;
                return Err(MonoError {
                    message: format!(
                        "`{name}` reaches itself at a growing type (`{}` leads to `{}`), which has \
                         no finite native form",
                        self.instance_name(name, &earlier.args),
                        self.instance_name(name, args),
                    ),
                });
            }
            ancestor = earlier.parent;
        }
        Ok(())
    }

    fn instance_name(&self, name: &str, args: &[Ty]) -> String {
        let rendered: Vec<String> = args.iter().map(|arg| arg.render(self.resolution)).collect();
        format!("{name}[{}]", rendered.join(", "))
    }

    /// Turn each `Move` of a value whose type became `Copy` into a `Copy`.
    ///
    /// A generic body moves every `T` (a type parameter is never `Copy`), and
    /// the verifier holds a `Copy` value to being read by `Copy` (`M0009`),
    /// so `ident[Int]` must copy where `ident[T]` moved. A `Drop` of such a
    /// value stays: dropping a type with no heap is a no-op in every engine.
    fn copy_what_became_copy(&self, function: &mut Function) {
        let locals = function.locals.clone();
        let mut fix = |operand: &mut Operand| {
            if let Operand::Move(place) = operand
                && self
                    .place_ty(&locals, place)
                    .is_some_and(|ty| tuo_ownership::is_copy(self.types, &ty))
            {
                *operand = Operand::Copy(place.clone());
            }
        };
        for block in &mut function.blocks {
            for statement in &mut block.statements {
                match statement {
                    Statement::Assign { rvalue, .. } => match rvalue {
                        Rvalue::Use(operand)
                        | Rvalue::Unary { operand, .. }
                        | Rvalue::Cast { operand, .. } => fix(operand),
                        Rvalue::Binary { lhs, rhs, .. } => {
                            fix(lhs);
                            fix(rhs);
                        }
                        Rvalue::Aggregate {
                            fields: operands, ..
                        }
                        | Rvalue::StrOp { args: operands, .. }
                        | Rvalue::HeapOp { args: operands, .. } => {
                            operands.iter_mut().for_each(&mut fix);
                        }
                        Rvalue::Discriminant(_) | Rvalue::Len(_) => {}
                    },
                    Statement::Call { callee, args, .. } => {
                        if let Callee::Indirect(operand) = callee {
                            fix(operand);
                        }
                        for arg in args {
                            if let Arg::Value(operand) = arg {
                                fix(operand);
                            }
                        }
                    }
                    Statement::Effect { args, .. } => {
                        for arg in args {
                            if let Arg::Value(operand) = arg {
                                fix(operand);
                            }
                        }
                    }
                    Statement::HeapMutate { args, .. } => args.iter_mut().for_each(&mut fix),
                    Statement::Drop { .. } => {}
                }
            }
            match &mut block.terminator {
                Terminator::Return(operand)
                | Terminator::Branch { cond: operand, .. }
                | Terminator::Assert { cond: operand, .. }
                | Terminator::Switch { discr: operand, .. } => fix(operand),
                Terminator::Goto(_) | Terminator::Trap(_) => {}
            }
        }
    }

    fn operand_ty(&self, locals: &[LocalDecl], operand: &Operand) -> Option<Ty> {
        match operand {
            Operand::Copy(place) | Operand::Move(place) => self.place_ty(locals, place),
            Operand::Const(Const::Fn(symbol)) => self.types.type_of(*symbol).cloned(),
            Operand::Const(constant) => Some(crate::verify::const_ty(constant)),
        }
    }

    fn place_ty(&self, locals: &[LocalDecl], place: &Place) -> Option<Ty> {
        let mut ty = locals.get(place.local.0 as usize)?.ty.clone();
        for projection in &place.projection {
            ty = match projection {
                Projection::Field(index) => match &ty {
                    Ty::Tuple(items) => items.get(*index as usize)?.clone(),
                    Ty::Range(item) => (**item).clone(),
                    Ty::Struct(symbol, _) => self
                        .types
                        .struct_shape(*symbol)?
                        .fields
                        .get(*index as usize)?
                        .1
                        .clone(),
                    _ => return None,
                },
                Projection::VariantField { variant, field } => match &ty {
                    Ty::Option(item) => (**item).clone(),
                    Ty::Result(ok, err) => {
                        if *variant == 0 {
                            (**ok).clone()
                        } else {
                            (**err).clone()
                        }
                    }
                    Ty::Enum(symbol, _) => {
                        let shape = self.types.enum_shape(*symbol)?;
                        shape
                            .variants
                            .get(*variant as usize)?
                            .1
                            .get(*field as usize)?
                            .1
                            .clone()
                    }
                    _ => return None,
                },
                Projection::Index(_) => match &ty {
                    Ty::Array(item) | Ty::FixedArray(item, _) => (**item).clone(),
                    _ => return None,
                },
                Projection::Deref => match &ty {
                    Ty::Wrapper(_, item) => (**item).clone(),
                    _ => return None,
                },
            };
        }
        Some(ty)
    }
}

/// Record what each type parameter in `declared` stands for in `actual`.
/// The first binding wins; a later conflicting one cannot arise from checked
/// code, and the verifier would catch the resulting body if it did.
fn bind(declared: &Ty, actual: &Ty, bound: &mut HashMap<SymbolId, Ty>) {
    match (declared, actual) {
        (_, Ty::Never | Ty::Error | Ty::Var(_)) => {}
        (Ty::Param(param), actual) => {
            bound.entry(*param).or_insert_with(|| actual.clone());
        }
        (Ty::Tuple(a), Ty::Tuple(b))
        | (Ty::Struct(_, a), Ty::Struct(_, b))
        | (Ty::Enum(_, a), Ty::Enum(_, b)) => {
            for (declared, actual) in a.iter().zip(b) {
                bind(declared, actual, bound);
            }
        }
        (Ty::Array(a), Ty::Array(b))
        | (Ty::FixedArray(a, _), Ty::FixedArray(b, _))
        | (Ty::Range(a), Ty::Range(b))
        | (Ty::Option(a), Ty::Option(b))
        | (Ty::Wrapper(_, a), Ty::Wrapper(_, b)) => bind(a, b, bound),
        (Ty::Result(a_ok, a_err), Ty::Result(b_ok, b_err))
        | (Ty::Map(a_ok, a_err), Ty::Map(b_ok, b_err)) => {
            bind(a_ok, b_ok, bound);
            bind(a_err, b_err, bound);
        }
        (Ty::Fn(a), Ty::Fn(b)) => {
            for (declared, actual) in a.params.iter().zip(&b.params) {
                bind(&declared.ty, &actual.ty, bound);
            }
            bind(&a.ret, &b.ret, bound);
        }
        _ => {}
    }
}

/// The number of type constructors in `ty`, the measure a growing
/// instantiation chain increases.
fn type_size(ty: &Ty) -> usize {
    1 + match ty {
        Ty::Tuple(items) | Ty::Struct(_, items) | Ty::Enum(_, items) => {
            items.iter().map(type_size).sum()
        }
        Ty::Array(item)
        | Ty::FixedArray(item, _)
        | Ty::Range(item)
        | Ty::Option(item)
        | Ty::Wrapper(_, item) => type_size(item),
        Ty::Result(a, b) | Ty::Map(a, b) => type_size(a) + type_size(b),
        Ty::Fn(fn_ty) => {
            fn_ty
                .params
                .iter()
                .map(|param| type_size(&param.ty))
                .sum::<usize>()
                + type_size(&fn_ty.ret)
        }
        _ => 0,
    }
}

/// Substitute `args` for `params` in every type `function` carries: its
/// return type, its locals, and the types its rvalues name.
fn substitute(function: &mut Function, params: &[SymbolId], args: &[Ty]) {
    function.ret = instantiate(&function.ret, params, args);
    for local in &mut function.locals {
        local.ty = instantiate(&local.ty, params, args);
    }
    for block in &mut function.blocks {
        for statement in &mut block.statements {
            let Statement::Assign { rvalue, .. } = statement else {
                continue;
            };
            match rvalue {
                Rvalue::Cast { to, .. } => *to = instantiate(to, params, args),
                Rvalue::Aggregate { kind, .. } => match kind {
                    AggregateKind::Adt { ty, .. } => *ty = instantiate(ty, params, args),
                    AggregateKind::Array { element, .. } => {
                        *element = instantiate(element, params, args);
                    }
                    AggregateKind::Range => {}
                },
                _ => {}
            }
        }
    }
}

/// `ty` with each [`Ty::Param`] in `params` replaced by its argument.
fn instantiate(ty: &Ty, params: &[SymbolId], args: &[Ty]) -> Ty {
    let go = |item: &Ty| instantiate(item, params, args);
    match ty {
        Ty::Param(symbol) => params
            .iter()
            .position(|param| param == symbol)
            .and_then(|index| args.get(index))
            .cloned()
            .unwrap_or_else(|| ty.clone()),
        Ty::Tuple(items) => Ty::Tuple(items.iter().map(go).collect()),
        Ty::Array(item) => Ty::Array(Box::new(go(item))),
        Ty::FixedArray(item, n) => Ty::FixedArray(Box::new(go(item)), *n),
        Ty::Range(item) => Ty::Range(Box::new(go(item))),
        Ty::Option(item) => Ty::Option(Box::new(go(item))),
        Ty::Result(ok, err) => Ty::Result(Box::new(go(ok)), Box::new(go(err))),
        Ty::Map(key, value) => Ty::Map(Box::new(go(key)), Box::new(go(value))),
        Ty::Wrapper(kind, item) => Ty::Wrapper(*kind, Box::new(go(item))),
        Ty::Struct(symbol, items) => Ty::Struct(*symbol, items.iter().map(go).collect()),
        Ty::Enum(symbol, items) => Ty::Enum(*symbol, items.iter().map(go).collect()),
        Ty::Fn(fn_ty) => Ty::Fn(Box::new(FnTy {
            params: fn_ty
                .params
                .iter()
                .map(|param| FnParam {
                    mode: param.mode,
                    ty: go(&param.ty),
                })
                .collect(),
            ret: go(&fn_ty.ret),
        })),
        Ty::Unit
        | Ty::Never
        | Ty::Bool
        | Ty::Char
        | Ty::String
        | Ty::Str
        | Ty::Int(_)
        | Ty::Float(_)
        | Ty::Var(_)
        | Ty::Error => ty.clone(),
    }
}
