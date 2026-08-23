//! The mandatory MIR verifier.
//!
//! MIR is the single executable semantic representation, so its
//! well-formedness is a load-bearing invariant: the interpreter and every
//! backend are entitled to *assume* verified MIR and are forbidden to
//! consume MIR that has not been verified. This module is that gate.
//!
//! [`verify`] walks a [`Program`] and returns a list of structured
//! [`Diagnostic`]s (`Mxxxx` codes) describing every way the MIR is
//! malformed. An empty list means the program is well-formed. The verifier
//! is **total**: it never panics, never indexes out of bounds, and never
//! trusts a field it has not first validated — a malformed program is
//! reported as data, not by aborting.
//!
//! # What is verified
//!
//! Per function, in one pass:
//!
//! - **Function signatures** — the parameter [`PassMode`] list and the
//!   locals agree (`params.len() <= locals.len()`), and there is at least
//!   one basic block (the entry, `bb0`).
//! - **Block targets** — every [`BlockId`] mentioned by a terminator names
//!   a block that exists.
//! - **Instruction operands** — every [`LocalId`] mentioned by a place,
//!   operand, argument, or rvalue names a local that exists, and every
//!   place projection is legal for the type it projects (a field index is
//!   in range for a struct/enum-variant, an index projection's index local
//!   is a `Usize`, and so on).
//! - **Value definitions / no undefined use** — a data-flow pass over the
//!   control-flow graph proves that no read (`Copy`/`Move`, a `Drop`, a
//!   borrow argument, a discriminant or length) can observe a local that
//!   is not initialized on *every* path reaching it, and that no local is
//!   read after it was moved out. Parameters begin initialized; ordinary
//!   locals begin uninitialized; an `Assign`/`Call` destination and a
//!   `BorrowMut` (which may write through) initialize; a `Move` and a
//!   `Drop` de-initialize.
//! - **Type consistency** — the two operands of a [`Rvalue::Binary`] share
//!   a type, a [`Rvalue::Discriminant`]/[`Rvalue::Len`] projects an
//!   enum-like / array place, a [`Terminator::Branch`]/[`Terminator::Assert`]
//!   condition is `Bool`, and a call/return destination's arity is right.
//! - **Terminators** — a [`Terminator::Switch`]'s arm values are pairwise
//!   distinct.
//! - **Ownership invariants that must survive lowering** — a borrow
//!   argument (`Arg::Borrow`/`Arg::BorrowMut`) is never paired against a
//!   by-value read of the same call, and a `Copy`-typed place is never
//!   consumed by `Move` (Copy values are read by `Copy`); a moved-from
//!   local is re-initialized before the next read (checked by the
//!   data-flow pass above).
//!
//! Deep, fully-instantiated type equality of every operand against its
//! expected slot is intentionally **not** attempted here: reconstructing a
//! generic field's concrete type would duplicate lowering. The checks above
//! are the structural and flow invariants a backend actually relies on, and
//! they are the ones a mis-lowering or a future optimization pass would
//! violate.

use std::collections::{BTreeMap, BTreeSet};

use tuo_diagnostics::{Diagnostic, DiagnosticCode, Namespace};
use tuo_types::{Ty, TypeckResult};

use crate::mir::{
    AggregateKind, Arg, BinOp, BlockId, Callee, EffectOp, Function, HeapMutOp, HeapOp, LocalId,
    Operand, Place, Program, Projection, Rvalue, Statement, StrOp, Terminator,
};

/// MIR diagnostic codes. Once shipped a number is never reused (the code
/// space is documented in `tuo-diagnostics`).
mod code {
    /// A terminator names a block that does not exist.
    pub(super) const BAD_BLOCK_TARGET: u16 = 1;
    /// A place, operand, or argument names a local that does not exist.
    pub(super) const BAD_LOCAL: u16 = 2;
    /// A place projection is illegal for the type it projects.
    pub(super) const BAD_PROJECTION: u16 = 3;
    /// A read observes a local that is not initialized on every path.
    pub(super) const USE_UNINIT: u16 = 4;
    /// A read observes a local after it was moved out.
    pub(super) const USE_MOVED: u16 = 5;
    /// Two operands that must share a type do not.
    pub(super) const TYPE_MISMATCH: u16 = 6;
    /// A function signature is malformed (params/locals/blocks disagree).
    pub(super) const BAD_SIGNATURE: u16 = 7;
    /// A terminator is malformed (e.g. duplicated switch arm values).
    pub(super) const BAD_TERMINATOR: u16 = 8;
    /// An ownership invariant that must survive lowering is violated.
    pub(super) const OWNERSHIP: u16 = 9;
    /// A `StrOp` rvalue is malformed (wrong operand count or operand
    /// types for its op) — ADR-0006 Stage A.
    pub(super) const STR_OP: u16 = 10;
    /// An `Effect` statement is malformed (wrong argument count, argument
    /// kinds or types, or destination type) — ADR-0006/ADR-0009 Stage A.
    pub(super) const EFFECT: u16 = 11;
    /// A `HeapOp` rvalue is malformed (missing/extra/mistyped subject,
    /// wrong operand count or types, or a destination that is not the
    /// op's result type) — ADR-0009 Stage A.
    pub(super) const HEAP_OP: u16 = 12;
    /// A `HeapMutate` statement is malformed (mistyped target, wrong
    /// operand count or types, or a destination that is not the op's
    /// result type) — ADR-0009 Stage A.
    pub(super) const HEAP_MUTATE: u16 = 13;
    /// A function value or indirect call is malformed: a `Const::Fn(sym)`
    /// whose `sym` is not a function or whose signature-as-function-type is
    /// ill-formed, or a `Call` whose `Indirect` callee operand is not of
    /// function type — ADR-0008 Tier 1.
    pub(super) const INDIRECT_CALL: u16 = 14;
}

/// Verify a whole lowered [`Program`], returning one [`Diagnostic`] per
/// problem found (empty when the MIR is well-formed).
///
/// `types` is the type-check result the program was lowered from; the
/// verifier reads local and field types from it to check consistency.
#[must_use]
pub fn verify(program: &Program, types: &TypeckResult) -> Vec<Diagnostic> {
    let mut sink = Vec::new();
    for function in &program.functions {
        Verifier {
            function,
            types,
            sink: &mut sink,
        }
        .run();
    }
    sink
}

/// Convenience: does the program verify clean?
#[must_use]
pub fn is_well_formed(program: &Program, types: &TypeckResult) -> bool {
    verify(program, types).is_empty()
}

/// The debug/test-configuration re-verification hook.
///
/// The verifier runs unconditionally after lowering; it must **also** run
/// after every future optimization pass so that a pass which corrupts the
/// MIR is caught immediately. Optimization passes are not implemented yet,
/// so this is the contract they will call: in debug and test builds it
/// re-verifies and panics on any problem (a pass that produces malformed
/// MIR is a compiler bug, never user-facing); in release builds it is a
/// no-op, since the after-lowering verification already gated the input.
///
/// # Panics
///
/// In debug/test builds, panics if `program` does not verify, naming the
/// offending pass and the diagnostics.
#[track_caller]
pub fn debug_assert_verified(pass: &str, program: &Program, types: &TypeckResult) {
    if cfg!(debug_assertions) {
        let problems = verify(program, types);
        assert!(
            problems.is_empty(),
            "MIR pass `{pass}` produced unverifiable MIR: {}",
            problems
                .iter()
                .map(|diagnostic| format!("{}: {}", diagnostic.code, diagnostic.message))
                .collect::<Vec<_>>()
                .join("; ")
        );
    }
}

struct Verifier<'a> {
    function: &'a Function,
    types: &'a TypeckResult,
    sink: &'a mut Vec<Diagnostic>,
}

impl Verifier<'_> {
    fn run(&mut self) {
        self.check_signature();
        // Structural checks first: later passes assume locals and block
        // targets resolve, so if those are broken we stop before the
        // data-flow pass (which would otherwise index blindly).
        let structural_before = self.sink.len();
        self.check_targets();
        self.check_places_and_operands();
        if self.sink.len() == structural_before {
            self.check_dataflow();
        }
    }

    fn error(&mut self, number: u16, message: impl Into<String>) {
        self.sink.push(Diagnostic::error(
            DiagnosticCode::new(Namespace::Mir, number),
            message,
            self.function.span,
        ));
    }

    fn fn_name(&self) -> &str {
        &self.function.name
    }

    // --- signatures ------------------------------------------------------

    fn check_signature(&mut self) {
        let name = self.fn_name().to_owned();
        if self.function.params.len() > self.function.locals.len() {
            self.error(
                code::BAD_SIGNATURE,
                format!(
                    "fn `{name}`: {} parameter modes but only {} locals",
                    self.function.params.len(),
                    self.function.locals.len()
                ),
            );
        }
        if self.function.blocks.is_empty() {
            self.error(
                code::BAD_SIGNATURE,
                format!("fn `{name}`: a function body has no basic blocks"),
            );
        }
    }

    // --- block targets ---------------------------------------------------

    fn block_count(&self) -> u32 {
        u32::try_from(self.function.blocks.len()).unwrap_or(u32::MAX)
    }

    fn target_ok(&self, block: BlockId) -> bool {
        block.0 < self.block_count()
    }

    fn check_targets(&mut self) {
        let name = self.fn_name().to_owned();
        let block_count = self.block_count();
        for index in 0..self.function.blocks.len() {
            let terminator = self.function.blocks[index].terminator.clone();
            let targets: Vec<BlockId> = match &terminator {
                Terminator::Return(_) | Terminator::Trap(_) => Vec::new(),
                Terminator::Goto(target) => vec![*target],
                Terminator::Branch {
                    then_block,
                    else_block,
                    ..
                } => vec![*then_block, *else_block],
                Terminator::Switch {
                    arms, otherwise, ..
                } => arms
                    .iter()
                    .map(|(_, target)| *target)
                    .chain(std::iter::once(*otherwise))
                    .collect(),
                Terminator::Assert { target, .. } => vec![*target],
            };
            for target in targets {
                if target.0 >= block_count {
                    self.error(
                        code::BAD_BLOCK_TARGET,
                        format!(
                            "fn `{name}`: bb{index}'s terminator jumps to bb{} which does not exist",
                            target.0
                        ),
                    );
                }
            }
            // A switch's arm values must be pairwise distinct.
            if let Terminator::Switch { arms, .. } = &terminator {
                let mut seen = BTreeSet::new();
                for (value, _) in arms {
                    if !seen.insert(*value) {
                        self.error(
                            code::BAD_TERMINATOR,
                            format!(
                                "fn `{name}`: bb{index}'s switch has a duplicated arm value {value}"
                            ),
                        );
                    }
                }
            }
        }
    }

    // --- operands, places, projections -----------------------------------

    fn local_count(&self) -> u32 {
        u32::try_from(self.function.locals.len()).unwrap_or(u32::MAX)
    }

    fn local_ty(&self, local: LocalId) -> Option<&Ty> {
        self.function
            .locals
            .get(local.0 as usize)
            .map(|decl| &decl.ty)
    }

    /// The type a place denotes, walking its projections, or `None` if any
    /// step is illegal (the caller reports the projection error).
    fn place_ty(&self, place: &Place) -> Option<Ty> {
        let mut ty = self.local_ty(place.local)?.clone();
        for projection in &place.projection {
            ty = self.project_ty(&ty, projection)?;
        }
        Some(ty)
    }

    /// Advance a projection over `ty`, returning the projected type or
    /// `None` if the step is not legal for that type.
    fn project_ty(&self, ty: &Ty, projection: &Projection) -> Option<Ty> {
        match projection {
            Projection::Field(index) => self.field_ty(ty, *index),
            Projection::VariantField { variant, field } => {
                self.variant_field_ty(ty, *variant, *field)
            }
            Projection::Index(_) => match ty {
                Ty::Array(item) | Ty::FixedArray(item, _) => Some((**item).clone()),
                _ => None,
            },
        }
    }

    /// The declared type of struct/tuple/range field `index`. Generic
    /// struct fields are returned in their *declaration* form (`Ty::Param`)
    /// — the verifier checks projection legality, not full instantiation.
    fn field_ty(&self, ty: &Ty, index: u32) -> Option<Ty> {
        match ty {
            Ty::Tuple(items) => items.get(index as usize).cloned(),
            Ty::Range(item) => (index < 2).then(|| (**item).clone()),
            Ty::Struct(symbol, _) => {
                let shape = self.types.struct_shape(*symbol)?;
                shape.fields.get(index as usize).map(|(_, ty)| ty.clone())
            }
            _ => None,
        }
    }

    /// The declared type of the `field`-th payload of `variant`.
    fn variant_field_ty(&self, ty: &Ty, variant: u32, field: u32) -> Option<Ty> {
        match ty {
            Ty::Option(item) => (variant == 0 && field == 0).then(|| (**item).clone()),
            Ty::Result(ok, err) => match (variant, field) {
                (0, 0) => Some((**ok).clone()),
                (1, 0) => Some((**err).clone()),
                _ => None,
            },
            Ty::Enum(symbol, _) => {
                let shape = self.types.enum_shape(*symbol)?;
                let (_, fields) = shape.variants.get(variant as usize)?;
                fields.get(field as usize).map(|(_, ty)| ty.clone())
            }
            _ => None,
        }
    }

    fn check_places_and_operands(&mut self) {
        for index in 0..self.function.blocks.len() {
            let block = &self.function.blocks[index];
            for stmt_index in 0..block.statements.len() {
                let statement = self.function.blocks[index].statements[stmt_index].clone();
                self.check_statement(index, &statement);
            }
            let terminator = self.function.blocks[index].terminator.clone();
            self.check_terminator_operands(index, &terminator);
        }
    }

    fn check_statement(&mut self, block: usize, statement: &Statement) {
        match statement {
            Statement::Assign { place, rvalue } => {
                self.check_place(block, place);
                self.check_rvalue(block, rvalue);
                self.check_assign_types(block, place, rvalue);
            }
            Statement::Call { dest, callee, args } => {
                self.check_place(block, dest);
                if let Callee::Indirect(operand) = callee {
                    self.check_operand(block, operand);
                }
                for arg in args {
                    self.check_arg(block, arg);
                }
                self.check_call_callee(block, callee);
            }
            Statement::Effect { op, args, dest } => {
                self.check_place(block, dest);
                for arg in args {
                    self.check_arg(block, arg);
                }
                self.check_effect_types(block, *op, args, dest);
            }
            Statement::HeapMutate {
                op,
                target,
                args,
                dest,
            } => {
                self.check_place(block, target);
                self.check_place(block, dest);
                for arg in args {
                    self.check_operand(block, arg);
                }
                self.check_heap_mutate_types(block, *op, target, args, dest);
            }
            Statement::Drop { place } => self.check_place(block, place),
        }
    }

    /// Type-check an `Effect` statement: exactly the op's argument count,
    /// each argument of its fixed kind and type, and an `I64` destination
    /// (`specification/mir.md` §4.2). `WriteString`'s `String` must be an
    /// `Arg::Borrow` place; every other effect argument must be an
    /// `Arg::Value` operand.
    fn check_effect_types(&mut self, block: usize, op: EffectOp, args: &[Arg], dest: &Place) {
        let name = self.fn_name().to_owned();
        if args.len() != op.arg_count() {
            self.error(
                code::EFFECT,
                format!(
                    "fn `{name}`: bb{block} performs effect `{}` with {} argument(s) (expected {})",
                    op.name(),
                    args.len(),
                    op.arg_count()
                ),
            );
            return;
        }
        // Per-argument expected (kind, type): `true` means a by-value
        // operand, `false` a read-only borrow place.
        let expected: Vec<(bool, Ty)> = match op {
            EffectOp::Write => vec![(true, Ty::int()), (true, Ty::Str)],
            EffectOp::ReadByte | EffectOp::Exit => vec![(true, Ty::int())],
            EffectOp::WriteString => vec![(true, Ty::int()), (false, Ty::String)],
            // ADR-0007: the two by-value operands (the `fn(take I64) -> I64`
            // task function, the worker count) then the borrowed tasks array,
            // matching the lowering's operands-then-borrow convention.
            EffectOp::ParMap => vec![
                (
                    true,
                    Ty::Fn(Box::new(tuo_types::FnTy {
                        params: vec![tuo_types::FnParam {
                            mode: tuo_types::ParamMode::Take,
                            ty: Ty::int(),
                        }],
                        ret: Ty::int(),
                    })),
                ),
                (true, Ty::int()),
                (false, Ty::Array(Box::new(Ty::int()))),
            ],
            // ADR-0013: the OS-boundary effects — all by-value operands.
            EffectOp::NowNanos | EffectOp::ArgCount => Vec::new(),
            EffectOp::ArgByte => vec![(true, Ty::int()), (true, Ty::int())],
            EffectOp::Open => vec![(true, Ty::Str), (true, Ty::int())],
            EffectOp::Close => vec![(true, Ty::int())],
            EffectOp::RemoveFile => vec![(true, Ty::Str)],
        };
        for (position, (arg, (want_value, want_ty))) in args.iter().zip(&expected).enumerate() {
            let ty = match (arg, want_value) {
                (Arg::Value(operand), true) => self.operand_ty(operand),
                (Arg::Borrow(place), false) => self.place_ty(place),
                _ => {
                    self.error(
                        code::EFFECT,
                        format!(
                            "fn `{name}`: bb{block} passes a wrong-kind argument {position} to \
                             effect `{}` (expected {})",
                            op.name(),
                            if *want_value {
                                "a by-value operand"
                            } else {
                                "a read-only borrow place"
                            }
                        ),
                    );
                    continue;
                }
            };
            if let Some(ty) = ty
                && !types_agree(&ty, want_ty)
            {
                self.error(
                    code::EFFECT,
                    format!(
                        "fn `{name}`: bb{block} passes a mistyped argument {position} to effect                          `{}`",
                        op.name()
                    ),
                );
            }
        }
        // Every effect's destination is `I64` except `par_map`, whose
        // results materialize as an `Array[I64]` (ADR-0007).
        let want_dest = match op {
            EffectOp::ParMap => Ty::Array(Box::new(Ty::int())),
            _ => Ty::int(),
        };
        if let Some(ty) = self.place_ty(dest)
            && !types_agree(&ty, &want_dest)
        {
            self.error(
                code::EFFECT,
                format!(
                    "fn `{name}`: bb{block} stores effect `{}` into a wrong-typed destination",
                    op.name()
                ),
            );
        }
    }

    /// Type-check a `HeapMutate` statement: the target place is the op's
    /// mutated heap type, exactly the op's operand count with its fixed
    /// types, and a destination of the op's result type
    /// (`specification/mir.md` §4.3).
    fn check_heap_mutate_types(
        &mut self,
        block: usize,
        op: HeapMutOp,
        target: &Place,
        args: &[Operand],
        dest: &Place,
    ) {
        let name = self.fn_name().to_owned();
        // The growable-`Array` element type is generic (ADR-0012): read it from
        // the target place's actual `Ty::Array(elem)` rather than assuming `Int`,
        // so `push`/`pop` validate the operand/result against the real element.
        // If the target's type is unavailable (a poisoned place), fall back to a
        // fresh `Array[?]` shape that only checks the container-ness.
        let elem = match op {
            HeapMutOp::Push | HeapMutOp::Pop => self
                .place_ty(target)
                .and_then(|ty| array_element(&ty))
                .unwrap_or_else(Ty::int),
            _ => Ty::int(),
        };
        let elem_array = Ty::Array(Box::new(elem.clone()));
        // The map's key/value pair is likewise generic (ADR-0011): read it
        // from the target place's actual `Ty::Map(k, v)`.
        let (map_key, map_value) = match op {
            HeapMutOp::MapInsert | HeapMutOp::MapRemove => self
                .place_ty(target)
                .and_then(|ty| map_pair(&ty))
                .unwrap_or_else(|| (Ty::int(), Ty::int())),
            _ => (Ty::int(), Ty::int()),
        };
        let kv_map = Ty::Map(Box::new(map_key.clone()), Box::new(map_value.clone()));
        let (target_ty, operand_tys, result): (Ty, Vec<Ty>, Ty) = match op {
            HeapMutOp::PushByte => (Ty::String, vec![Ty::int()], Ty::Unit),
            HeapMutOp::Append => (Ty::String, vec![Ty::Str], Ty::Unit),
            HeapMutOp::Push => (elem_array.clone(), vec![elem.clone()], Ty::Unit),
            HeapMutOp::Pop => (
                elem_array.clone(),
                Vec::new(),
                Ty::Option(Box::new(elem.clone())),
            ),
            HeapMutOp::MapInsert => (
                kv_map.clone(),
                vec![map_key.clone(), map_value.clone()],
                Ty::Option(Box::new(map_value.clone())),
            ),
            HeapMutOp::MapRemove => (
                kv_map.clone(),
                vec![map_key.clone()],
                Ty::Option(Box::new(map_value.clone())),
            ),
        };
        if let Some(ty) = self.place_ty(target)
            && !types_agree(&ty, &target_ty)
        {
            self.error(
                code::HEAP_MUTATE,
                format!(
                    "fn `{name}`: bb{block} applies heap mutation `{}` to a target that is not \
                     `{}`",
                    op.name(),
                    if matches!(target_ty, Ty::String) {
                        "String".to_owned()
                    } else {
                        format!("{target_ty:?}")
                    }
                ),
            );
        }
        if args.len() != op.arg_count() {
            self.error(
                code::HEAP_MUTATE,
                format!(
                    "fn `{name}`: bb{block} applies heap mutation `{}` to {} operand(s) \
                     (expected {})",
                    op.name(),
                    args.len(),
                    op.arg_count()
                ),
            );
            return;
        }
        for (position, (arg, want)) in args.iter().zip(&operand_tys).enumerate() {
            if let Some(ty) = self.operand_ty(arg)
                && !types_agree(&ty, want)
            {
                self.error(
                    code::HEAP_MUTATE,
                    format!(
                        "fn `{name}`: bb{block} passes a mistyped operand {position} to heap \
                         mutation `{}`",
                        op.name()
                    ),
                );
            }
        }
        if let Some(ty) = self.place_ty(dest)
            && !types_agree(&ty, &result)
        {
            self.error(
                code::HEAP_MUTATE,
                format!(
                    "fn `{name}`: bb{block} stores heap mutation `{}` into a destination that is \
                     not its result type",
                    op.name()
                ),
            );
        }
    }

    fn check_place(&mut self, block: usize, place: &Place) {
        let name = self.fn_name().to_owned();
        if place.local.0 >= self.local_count() {
            self.error(
                code::BAD_LOCAL,
                format!(
                    "fn `{name}`: bb{block} names local _{} which does not exist",
                    place.local.0
                ),
            );
            return;
        }
        // Validate every projection step and every index local.
        let mut ty = self.local_ty(place.local).cloned();
        for projection in &place.projection {
            if let Projection::Index(index) = projection
                && index.0 >= self.local_count()
            {
                self.error(
                    code::BAD_LOCAL,
                    format!(
                        "fn `{name}`: bb{block} indexes with local _{} which does not exist",
                        index.0
                    ),
                );
                return;
            }
            match ty.as_ref().and_then(|ty| self.project_ty(ty, projection)) {
                Some(next) => ty = Some(next),
                None => {
                    self.error(
                        code::BAD_PROJECTION,
                        format!(
                            "fn `{name}`: bb{block} applies an illegal projection to _{}",
                            place.local.0
                        ),
                    );
                    return;
                }
            }
        }
    }

    fn check_operand(&mut self, block: usize, operand: &Operand) {
        match operand {
            Operand::Copy(place) => self.check_place(block, place),
            Operand::Move(place) => {
                self.check_place(block, place);
                // Ownership invariant: a `Copy` value is read by `Copy`,
                // never consumed by `Move` (moving it would leave the source
                // spuriously de-initialized). Non-`Copy` values must move.
                let name = self.fn_name().to_owned();
                if let Some(ty) = self.place_ty(place)
                    && tuo_ownership::is_copy(self.types, &ty)
                {
                    self.error(
                        code::OWNERSHIP,
                        format!(
                            "fn `{name}`: bb{block} moves _{} which is `Copy` (should be copied)",
                            place.local.0
                        ),
                    );
                }
            }
            Operand::Const(_) => {}
        }
    }

    fn check_arg(&mut self, block: usize, arg: &Arg) {
        match arg {
            Arg::Value(operand) => self.check_operand(block, operand),
            Arg::Borrow(place) | Arg::BorrowMut(place) => self.check_place(block, place),
        }
    }

    fn check_rvalue(&mut self, block: usize, rvalue: &Rvalue) {
        match rvalue {
            Rvalue::Use(operand) | Rvalue::Unary { operand, .. } | Rvalue::Cast { operand, .. } => {
                self.check_operand(block, operand)
            }
            Rvalue::Binary { lhs, rhs, .. } => {
                self.check_operand(block, lhs);
                self.check_operand(block, rhs);
            }
            Rvalue::Aggregate { fields, .. } => {
                for field in fields {
                    self.check_operand(block, field);
                }
            }
            Rvalue::Discriminant(place) => self.check_discriminant(block, place),
            Rvalue::Len(place) => self.check_len(block, place),
            Rvalue::StrOp { args, .. } => {
                for arg in args {
                    self.check_operand(block, arg);
                }
            }
            Rvalue::HeapOp { subject, args, .. } => {
                if let Some(place) = subject {
                    self.check_place(block, place);
                }
                for arg in args {
                    self.check_operand(block, arg);
                }
            }
        }
    }

    fn check_discriminant(&mut self, block: usize, place: &Place) {
        self.check_place(block, place);
        let name = self.fn_name().to_owned();
        if let Some(ty) = self.place_ty(place)
            && !matches!(ty, Ty::Option(_) | Ty::Result(_, _) | Ty::Enum(_, _))
        {
            self.error(
                code::TYPE_MISMATCH,
                format!(
                    "fn `{name}`: bb{block} takes the discriminant of a non-enum place (_{})",
                    place.local.0
                ),
            );
        }
    }

    /// `Len` applies **only** to the growable `Array[T]`. A fixed
    /// `[T; N]`'s length is lowered as a constant, and this check enforces
    /// that invariant so a backend can rely on it (ADR-0004 Stage 2).
    fn check_len(&mut self, block: usize, place: &Place) {
        self.check_place(block, place);
        let name = self.fn_name().to_owned();
        if let Some(ty) = self.place_ty(place)
            && !matches!(ty, Ty::Array(_))
        {
            self.error(
                code::TYPE_MISMATCH,
                format!(
                    "fn `{name}`: bb{block} takes the length of a non-`Array` place (_{}); \
                     `Len` applies only to the growable `Array[T]` — a `[T; N]`'s length is \
                     lowered as a constant",
                    place.local.0
                ),
            );
        }
    }

    fn check_assign_types(&mut self, block: usize, place: &Place, rvalue: &Rvalue) {
        // A binary op's two operands must share a type; comparisons still
        // require the operands to agree with each other.
        if let Rvalue::Binary { op, lhs, rhs } = rvalue {
            if let (Some(l), Some(r)) = (self.operand_ty(lhs), self.operand_ty(rhs))
                && !types_agree(&l, &r)
            {
                let name = self.fn_name().to_owned();
                self.error(
                    code::TYPE_MISMATCH,
                    format!(
                        "fn `{name}`: bb{block} applies {} to operands of differing types",
                        bin_name(*op)
                    ),
                );
            }
        }
        // An aggregate's field count must match the constructed shape.
        if let Rvalue::Aggregate {
            kind: AggregateKind::Range,
            fields,
        } = rvalue
            && fields.len() != 2
        {
            let name = self.fn_name().to_owned();
            self.error(
                code::TYPE_MISMATCH,
                format!(
                    "fn `{name}`: bb{block} builds a range from {} operands (expected 2)",
                    fields.len()
                ),
            );
        }
        // A fixed-size array aggregate (ADR-0004 Stage 2): operand count
        // equals the declared length, every operand is the element type,
        // and the destination is exactly `[element; len]`.
        if let Rvalue::Aggregate {
            kind: AggregateKind::Array { element, len },
            fields,
        } = rvalue
        {
            let name = self.fn_name().to_owned();
            if fields.len() as u64 != *len {
                self.error(
                    code::TYPE_MISMATCH,
                    format!(
                        "fn `{name}`: bb{block} builds a fixed array of length {len} from {} \
                         operands",
                        fields.len()
                    ),
                );
            }
            for field in fields {
                if let Some(ty) = self.operand_ty(field)
                    && !types_agree(&ty, element)
                {
                    self.error(
                        code::TYPE_MISMATCH,
                        format!(
                            "fn `{name}`: bb{block} builds a fixed array from an operand that \
                             is not the element type"
                        ),
                    );
                }
            }
            if let Some(dest) = self.place_ty(place) {
                let destination_ok = matches!(
                    &dest,
                    Ty::FixedArray(item, n) if types_agree(item, element) && *n == *len
                ) || matches!(&dest, Ty::Never | Ty::Error);
                if !destination_ok {
                    self.error(
                        code::TYPE_MISMATCH,
                        format!(
                            "fn `{name}`: bb{block} assigns a fixed-array aggregate to a place \
                             that is not `[element; len]`"
                        ),
                    );
                }
            }
        }
        // A string operation (ADR-0006 Stage A): exactly the op's operand
        // count, each operand of its fixed type, and a destination of the
        // op's result type (`specification/mir.md` §5.6).
        if let Rvalue::StrOp { op, args } = rvalue {
            let name = self.fn_name().to_owned();
            if args.len() != op.arg_count() {
                self.error(
                    code::STR_OP,
                    format!(
                        "fn `{name}`: bb{block} applies string op `{}` to {} operand(s)                          (expected {})",
                        op.name(),
                        args.len(),
                        op.arg_count()
                    ),
                );
                return;
            }
            let expected: Vec<Ty> = match op {
                StrOp::Len => vec![Ty::Str],
                StrOp::ByteAt => vec![Ty::Str, Ty::int()],
                StrOp::Slice => vec![Ty::Str, Ty::int(), Ty::int()],
            };
            for (position, (arg, want)) in args.iter().zip(&expected).enumerate() {
                if let Some(ty) = self.operand_ty(arg)
                    && !types_agree(&ty, want)
                {
                    self.error(
                        code::STR_OP,
                        format!(
                            "fn `{name}`: bb{block} passes a mistyped operand {position} to                              string op `{}`",
                            op.name()
                        ),
                    );
                }
            }
            let result = match op {
                StrOp::Len | StrOp::ByteAt => Ty::int(),
                StrOp::Slice => Ty::Str,
            };
            if let Some(dest) = self.place_ty(place)
                && !types_agree(&dest, &result)
            {
                self.error(
                    code::STR_OP,
                    format!(
                        "fn `{name}`: bb{block} assigns string op `{}` to a destination that is                          not `{}`",
                        op.name(),
                        if matches!(result, Ty::Str) { "Str" } else { "I64" }
                    ),
                );
            }
        }
        // A heap operation (ADR-0009 Stage A): the op's subject place is
        // present exactly when the op reads an existing heap value (and has
        // its fixed type), exactly the op's operand count with its fixed
        // types, and a destination of the op's result type
        // (`specification/mir.md` §5.7).
        if let Rvalue::HeapOp { op, subject, args } = rvalue {
            self.check_heap_op_types(block, *op, subject.as_ref(), args, place);
        }
        let _ = place;
    }

    /// Type-check a `HeapOp` rvalue (`M0012`, §5.7).
    fn check_heap_op_types(
        &mut self,
        block: usize,
        op: HeapOp,
        subject: Option<&Place>,
        args: &[Operand],
        dest: &Place,
    ) {
        let name = self.fn_name().to_owned();
        // The growable-`Array` element type is generic (ADR-0012). Derive it from
        // the place that carries the array: the `subject` for `len`/`get`, the
        // `dest` for `empty`. A poisoned/absent type falls back to `Int`, which
        // only relaxes the check (never wrongly rejects a valid element type).
        let subject_elem = || {
            subject
                .and_then(|place| self.place_ty(place))
                .and_then(|ty| array_element(&ty))
                .unwrap_or_else(Ty::int)
        };
        // The map's key/value pair is likewise generic (ADR-0011): read it
        // from the subject place for the reading ops, the dest for `empty`.
        let subject_kv = || {
            subject
                .and_then(|place| self.place_ty(place))
                .and_then(|ty| map_pair(&ty))
                .unwrap_or_else(|| (Ty::int(), Ty::int()))
        };
        let subject_ty: Option<Ty> = match op {
            HeapOp::StringLen
            | HeapOp::StringByteAt
            | HeapOp::StringSlice
            | HeapOp::StringAsStr => Some(Ty::String),
            HeapOp::ArrayLen | HeapOp::ArrayGet => Some(Ty::Array(Box::new(subject_elem()))),
            HeapOp::MapGet | HeapOp::MapContainsKey | HeapOp::MapLen | HeapOp::MapKeys => {
                let (key, value) = subject_kv();
                Some(Ty::Map(Box::new(key), Box::new(value)))
            }
            HeapOp::StringEmpty
            | HeapOp::StringFromStr
            | HeapOp::StringConcat
            | HeapOp::ArrayEmpty
            | HeapOp::MapEmpty => None,
        };
        let operand_tys: Vec<Ty> = match op {
            HeapOp::StringEmpty
            | HeapOp::StringLen
            | HeapOp::StringAsStr
            | HeapOp::ArrayEmpty
            | HeapOp::ArrayLen
            | HeapOp::MapEmpty
            | HeapOp::MapLen
            | HeapOp::MapKeys => Vec::new(),
            HeapOp::StringFromStr => vec![Ty::Str],
            HeapOp::StringConcat => vec![Ty::Str, Ty::Str],
            HeapOp::StringByteAt | HeapOp::ArrayGet => vec![Ty::int()],
            HeapOp::StringSlice => vec![Ty::int(), Ty::int()],
            HeapOp::MapGet | HeapOp::MapContainsKey => vec![subject_kv().0],
        };
        let result: Ty = match op {
            HeapOp::StringEmpty
            | HeapOp::StringFromStr
            | HeapOp::StringConcat
            | HeapOp::StringSlice => Ty::String,
            HeapOp::StringAsStr => Ty::Str,
            HeapOp::StringLen | HeapOp::StringByteAt | HeapOp::ArrayLen | HeapOp::MapLen => {
                Ty::int()
            }
            // `get` returns the element type, read from the subject array.
            HeapOp::ArrayGet => subject_elem(),
            // `empty` produces `Array[T]`; the element is read from the dest.
            HeapOp::ArrayEmpty => Ty::Array(Box::new(
                self.place_ty(dest)
                    .and_then(|ty| array_element(&ty))
                    .unwrap_or_else(Ty::int),
            )),
            // `empty` produces `Map[K, V]`; the pair is read from the dest.
            HeapOp::MapEmpty => {
                let (key, value) = self
                    .place_ty(dest)
                    .and_then(|ty| map_pair(&ty))
                    .unwrap_or_else(|| (Ty::int(), Ty::int()));
                Ty::Map(Box::new(key), Box::new(value))
            }
            HeapOp::MapGet => Ty::Option(Box::new(subject_kv().1)),
            HeapOp::MapContainsKey => Ty::Bool,
            HeapOp::MapKeys => Ty::Array(Box::new(subject_kv().0)),
        };
        match (subject, &subject_ty) {
            (None, None) => {}
            (Some(place), Some(want)) => {
                if let Some(ty) = self.place_ty(place)
                    && !types_agree(&ty, want)
                {
                    self.error(
                        code::HEAP_OP,
                        format!(
                            "fn `{name}`: bb{block} applies heap op `{}` to a mistyped subject",
                            op.name()
                        ),
                    );
                }
            }
            (Some(_), None) => {
                self.error(
                    code::HEAP_OP,
                    format!(
                        "fn `{name}`: bb{block} passes a subject place to heap op `{}`, which \
                         takes none",
                        op.name()
                    ),
                );
            }
            (None, Some(_)) => {
                self.error(
                    code::HEAP_OP,
                    format!(
                        "fn `{name}`: bb{block} applies heap op `{}` without its subject place",
                        op.name()
                    ),
                );
            }
        }
        if args.len() != op.arg_count() {
            self.error(
                code::HEAP_OP,
                format!(
                    "fn `{name}`: bb{block} applies heap op `{}` to {} operand(s) (expected {})",
                    op.name(),
                    args.len(),
                    op.arg_count()
                ),
            );
            return;
        }
        for (position, (arg, want)) in args.iter().zip(&operand_tys).enumerate() {
            if let Some(ty) = self.operand_ty(arg)
                && !types_agree(&ty, want)
            {
                self.error(
                    code::HEAP_OP,
                    format!(
                        "fn `{name}`: bb{block} passes a mistyped operand {position} to heap op \
                         `{}`",
                        op.name()
                    ),
                );
            }
        }
        if let Some(ty) = self.place_ty(dest)
            && !types_agree(&ty, &result)
        {
            self.error(
                code::HEAP_OP,
                format!(
                    "fn `{name}`: bb{block} assigns heap op `{}` to a destination that is not \
                     its result type",
                    op.name()
                ),
            );
        }
    }

    /// Verify a call's callee (ADR-0008 Tier 1, `M0014`). A `Direct` symbol
    /// must have a function type; an `Indirect` callee operand must be of
    /// function type. A `Const::Fn(sym)` naming a non-function is caught here
    /// (its `operand_ty` is not `Ty::Fn`). Missing type information (a poison
    /// program) is not flagged — `Ty::Error` and `Ty::Never` unify with
    /// anything, exactly as elsewhere in the verifier.
    fn check_call_callee(&mut self, _block: usize, callee: &Callee) {
        let name = self.fn_name().to_owned();
        match callee {
            Callee::Direct(symbol) => {
                if let Some(ty) = self.types.type_of(*symbol)
                    && !matches!(ty, Ty::Fn(_) | Ty::Error | Ty::Never)
                {
                    self.error(
                        code::INDIRECT_CALL,
                        format!("fn `{name}`: direct call target is not a function"),
                    );
                }
            }
            Callee::Indirect(operand) => {
                if let Some(ty) = self.operand_ty(operand)
                    && !matches!(ty, Ty::Fn(_) | Ty::Error | Ty::Never)
                {
                    self.error(
                        code::INDIRECT_CALL,
                        format!(
                            "fn `{name}`: indirect call through a value that is not a function"
                        ),
                    );
                }
            }
        }
    }

    fn operand_ty(&self, operand: &Operand) -> Option<Ty> {
        match operand {
            Operand::Copy(place) | Operand::Move(place) => self.place_ty(place),
            // A function value's type is the referenced function's
            // signature-as-function-type (ADR-0008 Tier 1), which lives in
            // the type table, not the constant itself.
            Operand::Const(crate::mir::Const::Fn(symbol)) => self.types.type_of(*symbol).cloned(),
            Operand::Const(constant) => Some(const_ty(constant)),
        }
    }

    fn check_terminator_operands(&mut self, block: usize, terminator: &Terminator) {
        let name = self.fn_name().to_owned();
        match terminator {
            Terminator::Return(operand) => self.check_operand(block, operand),
            Terminator::Goto(_) | Terminator::Trap(_) => {}
            Terminator::Branch { cond, .. } | Terminator::Assert { cond, .. } => {
                self.check_operand(block, cond);
                if let Some(ty) = self.operand_ty(cond)
                    && !matches!(ty, Ty::Bool)
                {
                    self.error(
                        code::TYPE_MISMATCH,
                        format!("fn `{name}`: bb{block} branches on a non-`Bool` condition"),
                    );
                }
            }
            Terminator::Switch { discr, .. } => self.check_operand(block, discr),
        }
    }

    // --- data flow: definite initialization & move tracking --------------

    /// Prove no read observes an uninitialized or moved-out local. Runs a
    /// fixed-point over the CFG computing, for each block's entry, the set
    /// of locals that are *definitely initialized* on every path reaching
    /// it; then a linear scan reports the first offending read per block.
    fn check_dataflow(&mut self) {
        let entry_init = self.solve_init();
        let ever = self.ever_defined();
        for (index, block) in self.function.blocks.iter().enumerate() {
            let Some(mut init) = entry_init.get(&index).cloned() else {
                // Unreachable block: nothing flows in, so skip — dead code
                // is not an undefined-use bug.
                continue;
            };
            for statement in &block.statements {
                self.audit_reads(index, statement, &init, &ever);
                apply_statement(statement, &mut init);
            }
            self.audit_terminator_reads(index, &block.terminator, &init, &ever);
        }
    }

    /// Locals that are a destination somewhere in the body (a param, or the
    /// root of an `Assign`/`Call`/`BorrowMut`). A read of a local that is
    /// *not* currently initialized is a use-after-move when the local is in
    /// this set, and a use-before-init otherwise.
    fn ever_defined(&self) -> BTreeSet<u32> {
        let param_count = u32::try_from(self.function.params.len()).unwrap_or(u32::MAX);
        let mut ever: BTreeSet<u32> = (0..param_count).collect();
        for block in &self.function.blocks {
            for statement in &block.statements {
                match statement {
                    Statement::Assign { place, .. } if place.projection.is_empty() => {
                        ever.insert(place.local.0);
                    }
                    Statement::Call { dest, args, .. } => {
                        if dest.projection.is_empty() {
                            ever.insert(dest.local.0);
                        }
                        for arg in args {
                            if let Arg::BorrowMut(place) = arg
                                && place.projection.is_empty()
                            {
                                ever.insert(place.local.0);
                            }
                        }
                    }
                    Statement::Effect { dest, args, .. } => {
                        if dest.projection.is_empty() {
                            ever.insert(dest.local.0);
                        }
                        for arg in args {
                            if let Arg::BorrowMut(place) = arg
                                && place.projection.is_empty()
                            {
                                ever.insert(place.local.0);
                            }
                        }
                    }
                    // A `HeapMutate` writes its destination and writes
                    // *through* its target (a `mut` borrow).
                    Statement::HeapMutate { target, dest, .. } => {
                        if dest.projection.is_empty() {
                            ever.insert(dest.local.0);
                        }
                        if target.projection.is_empty() {
                            ever.insert(target.local.0);
                        }
                    }
                    Statement::Assign { .. } | Statement::Drop { .. } => {}
                }
            }
        }
        ever
    }

    /// Fixed-point: the definitely-initialized set on entry to each block.
    /// Block 0 starts with the parameters initialized; every other block's
    /// entry set is the intersection over its predecessors' exit sets.
    ///
    /// This is a **must**-analysis, so every reachable non-entry block is
    /// seeded at **top** (all locals) and the iteration only ever shrinks a
    /// set toward the greatest fixed point. Seeding matters for termination:
    /// an earlier version skipped not-yet-visited predecessors (treating them
    /// as top) while defaulting a block with no visited predecessors to
    /// bottom, and that mixed, non-monotone seeding oscillated forever on a
    /// loop body containing a bounds `Assert` (a back edge from a
    /// higher-numbered block into the loop's continuation). With a uniform
    /// top seed every update is monotone decreasing over a finite lattice,
    /// so the loop terminates.
    fn solve_init(&self) -> BTreeMap<usize, BTreeSet<u32>> {
        let param_count = u32::try_from(self.function.params.len()).unwrap_or(u32::MAX);
        let params: BTreeSet<u32> = (0..param_count).collect();
        let all_locals: BTreeSet<u32> = (0..self.local_count()).collect();

        // Reachable blocks, so unreachable ones never seed the intersection.
        let reachable = self.reachable_blocks();

        let mut entry: BTreeMap<usize, BTreeSet<u32>> = BTreeMap::new();
        for &index in &reachable {
            let seed = if index == 0 {
                params.clone()
            } else {
                all_locals.clone()
            };
            entry.insert(index, seed);
        }

        let mut changed = true;
        while changed {
            changed = false;
            for &index in &reachable {
                // Entry = intersection of predecessor exits (params for bb0).
                let mut incoming: Option<BTreeSet<u32>> = if index == 0 {
                    Some(params.clone())
                } else {
                    None
                };
                for pred in self.predecessors(index) {
                    let Some(pred_entry) = entry.get(&pred) else {
                        // Unreachable predecessor: nothing flows in from it.
                        continue;
                    };
                    let exit = self.exit_set(pred, pred_entry);
                    incoming = Some(match incoming {
                        Some(acc) => acc.intersection(&exit).copied().collect(),
                        None => exit,
                    });
                }
                // A reachable non-entry block always has a reachable
                // predecessor; the fallback only guards impossible shapes.
                let new = incoming.unwrap_or_default();
                if entry.get(&index) != Some(&new) {
                    entry.insert(index, new);
                    changed = true;
                }
            }
        }
        entry
    }

    /// The initialized set at the *end* of a block, given its entry set.
    fn exit_set(&self, index: usize, entry: &BTreeSet<u32>) -> BTreeSet<u32> {
        let mut init = entry.clone();
        if let Some(block) = self.function.blocks.get(index) {
            for statement in &block.statements {
                apply_statement(statement, &mut init);
            }
        }
        init
    }

    /// Predecessor blocks of `index`, via terminator targets.
    fn predecessors(&self, index: usize) -> Vec<usize> {
        let mut preds = Vec::new();
        for (candidate, block) in self.function.blocks.iter().enumerate() {
            if self.terminator_targets(&block.terminator).contains(&index) {
                preds.push(candidate);
            }
        }
        preds
    }

    fn terminator_targets(&self, terminator: &Terminator) -> Vec<usize> {
        let ids: Vec<BlockId> = match terminator {
            Terminator::Return(_) | Terminator::Trap(_) => Vec::new(),
            Terminator::Goto(target) => vec![*target],
            Terminator::Branch {
                then_block,
                else_block,
                ..
            } => vec![*then_block, *else_block],
            Terminator::Switch {
                arms, otherwise, ..
            } => arms
                .iter()
                .map(|(_, target)| *target)
                .chain(std::iter::once(*otherwise))
                .collect(),
            Terminator::Assert { target, .. } => vec![*target],
        };
        ids.into_iter()
            .filter(|id| self.target_ok(*id))
            .map(|id| id.0 as usize)
            .collect()
    }

    fn reachable_blocks(&self) -> BTreeSet<usize> {
        let mut seen = BTreeSet::new();
        let mut stack = vec![0usize];
        while let Some(index) = stack.pop() {
            if !seen.insert(index) {
                continue;
            }
            if let Some(block) = self.function.blocks.get(index) {
                for target in self.terminator_targets(&block.terminator) {
                    stack.push(target);
                }
            }
        }
        seen
    }

    /// Report reads in `statement` that observe an uninitialized local, and
    /// the ownership pairing invariant on calls.
    fn audit_reads(
        &mut self,
        block: usize,
        statement: &Statement,
        init: &BTreeSet<u32>,
        ever: &BTreeSet<u32>,
    ) {
        let mut reads: Vec<(LocalId, ReadKind)> = Vec::new();
        match statement {
            Statement::Assign { rvalue, .. } => collect_rvalue_reads(rvalue, &mut reads),
            Statement::Call { callee, args, .. } => {
                // The indirect callee operand is read to obtain the code
                // pointer (ADR-0008 Tier 1). A function value is `Copy`, so
                // this is a plain read.
                if let Callee::Indirect(operand) = callee {
                    collect_operand_read(operand, &mut reads);
                }
                for arg in args {
                    collect_arg_reads(arg, &mut reads);
                }
                self.check_call_ownership(block, args);
            }
            Statement::Effect { args, .. } => {
                for arg in args {
                    collect_arg_reads(arg, &mut reads);
                }
            }
            Statement::HeapMutate { target, args, .. } => {
                // The target is read (and re-written) through a `mut`
                // borrow: it must be initialized here.
                reads.push((target.local, ReadKind::Borrow));
                for arg in args {
                    collect_operand_read(arg, &mut reads);
                }
            }
            Statement::Drop { place } => reads.push((place.local, ReadKind::Consume)),
        }
        self.report_reads(block, &reads, init, ever);
    }

    /// Ownership invariant that must survive lowering: within one call, a
    /// place lent by borrow must not simultaneously be moved out by value.
    fn check_call_ownership(&mut self, block: usize, args: &[Arg]) {
        let mut borrowed: BTreeSet<u32> = BTreeSet::new();
        let mut moved: BTreeSet<u32> = BTreeSet::new();
        for arg in args {
            match arg {
                Arg::Borrow(place) | Arg::BorrowMut(place) => {
                    borrowed.insert(place.local.0);
                }
                Arg::Value(Operand::Move(place)) => {
                    moved.insert(place.local.0);
                }
                Arg::Value(_) => {}
            }
        }
        let name = self.fn_name().to_owned();
        for local in borrowed.intersection(&moved) {
            self.error(
                code::OWNERSHIP,
                format!("fn `{name}`: bb{block} both borrows and moves _{local} in one call"),
            );
        }
    }

    fn audit_terminator_reads(
        &mut self,
        block: usize,
        terminator: &Terminator,
        init: &BTreeSet<u32>,
        ever: &BTreeSet<u32>,
    ) {
        let mut reads: Vec<(LocalId, ReadKind)> = Vec::new();
        match terminator {
            Terminator::Return(operand)
            | Terminator::Branch { cond: operand, .. }
            | Terminator::Assert { cond: operand, .. }
            | Terminator::Switch { discr: operand, .. } => {
                collect_operand_read(operand, &mut reads)
            }
            Terminator::Goto(_) | Terminator::Trap(_) => {}
        }
        self.report_reads(block, &reads, init, ever);
    }

    fn report_reads(
        &mut self,
        block: usize,
        reads: &[(LocalId, ReadKind)],
        init: &BTreeSet<u32>,
        ever: &BTreeSet<u32>,
    ) {
        let name = self.fn_name().to_owned();
        for (local, _kind) in reads {
            if local.0 >= self.local_count() {
                // Already reported by the structural pass; skip here.
                continue;
            }
            if init.contains(&local.0) {
                continue;
            }
            // Not initialized on every path: a use-after-move if the local
            // was ever defined, otherwise a use-before-init.
            if ever.contains(&local.0) {
                self.error(
                    code::USE_MOVED,
                    format!(
                        "fn `{name}`: bb{block} reads _{} after it was moved out",
                        local.0
                    ),
                );
            } else {
                self.error(
                    code::USE_UNINIT,
                    format!(
                        "fn `{name}`: bb{block} reads _{} which is not initialized on every path",
                        local.0
                    ),
                );
            }
        }
    }
}

/// How a read consumes the local it names.
#[derive(Clone, Copy)]
enum ReadKind {
    /// A `Copy` read leaves the local initialized.
    Copy,
    /// A `Move` / `Drop` ends the local's initialization.
    Consume,
    /// A borrow reads without consuming.
    Borrow,
}

/// Apply a statement's effect to the definitely-initialized set: reads by
/// move and drops de-initialize; assignment, call destinations, and
/// exclusive borrows initialize.
fn apply_statement(statement: &Statement, init: &mut BTreeSet<u32>) {
    // De-initialize moved-out roots first (a move reads the old value),
    // then initialize destinations.
    let mut consumed: Vec<u32> = Vec::new();
    let mut defined: Vec<u32> = Vec::new();
    match statement {
        Statement::Assign { place, rvalue } => {
            collect_consumed_rvalue(rvalue, &mut consumed);
            if place.projection.is_empty() {
                defined.push(place.local.0);
            }
        }
        Statement::Call { dest, callee, args } => {
            // The indirect callee operand is consumed if moved (a function
            // value is `Copy`, so lowering copies it — but honor `Move` for
            // completeness).
            if let Callee::Indirect(operand) = callee {
                collect_consumed_operand(operand, &mut consumed);
            }
            for arg in args {
                match arg {
                    Arg::Value(operand) => collect_consumed_operand(operand, &mut consumed),
                    Arg::BorrowMut(place) if place.projection.is_empty() => {
                        defined.push(place.local.0);
                    }
                    Arg::Borrow(_) | Arg::BorrowMut(_) => {}
                }
            }
            if dest.projection.is_empty() {
                defined.push(dest.local.0);
            }
        }
        Statement::Effect { args, dest, .. } => {
            for arg in args {
                match arg {
                    Arg::Value(operand) => collect_consumed_operand(operand, &mut consumed),
                    Arg::BorrowMut(place) if place.projection.is_empty() => {
                        defined.push(place.local.0);
                    }
                    Arg::Borrow(_) | Arg::BorrowMut(_) => {}
                }
            }
            if dest.projection.is_empty() {
                defined.push(dest.local.0);
            }
        }
        Statement::HeapMutate {
            target, args, dest, ..
        } => {
            for arg in args {
                collect_consumed_operand(arg, &mut consumed);
            }
            // The target is a `mut` borrow: read, mutated in place, and
            // still initialized after — exactly a `BorrowMut` argument.
            if target.projection.is_empty() {
                defined.push(target.local.0);
            }
            if dest.projection.is_empty() {
                defined.push(dest.local.0);
            }
        }
        Statement::Drop { place } => {
            if place.projection.is_empty() {
                consumed.push(place.local.0);
            }
        }
    }
    for local in consumed {
        init.remove(&local);
    }
    for local in defined {
        init.insert(local);
    }
}

/// Collect the roots a whole-local move consumes in an rvalue. Only a move
/// of the *entire* local (empty projection) de-initializes it; a field move
/// leaves the local partially live and v0 lowering drops the husk, so
/// treating the root as still-defined keeps the flow analysis sound (a
/// later whole-local read the ownership checker already forbade will still
/// be caught, since that read moves the whole local).
fn collect_consumed_rvalue(rvalue: &Rvalue, out: &mut Vec<u32>) {
    let mut reads = Vec::new();
    collect_rvalue_move_roots(rvalue, &mut reads);
    out.extend(reads);
}

fn collect_rvalue_move_roots(rvalue: &Rvalue, out: &mut Vec<u32>) {
    let push = |operand: &Operand, out: &mut Vec<u32>| {
        if let Operand::Move(place) = operand
            && place.projection.is_empty()
        {
            out.push(place.local.0);
        }
    };
    match rvalue {
        Rvalue::Use(operand) | Rvalue::Unary { operand, .. } | Rvalue::Cast { operand, .. } => {
            push(operand, out);
        }
        Rvalue::Binary { lhs, rhs, .. } => {
            push(lhs, out);
            push(rhs, out);
        }
        Rvalue::Aggregate { fields, .. } => {
            for field in fields {
                push(field, out);
            }
        }
        Rvalue::StrOp { args, .. } => {
            for arg in args {
                push(arg, out);
            }
        }
        // A `HeapOp`'s subject is a borrow-read (never consumed); only its
        // operand moves consume.
        Rvalue::HeapOp { args, .. } => {
            for arg in args {
                push(arg, out);
            }
        }
        Rvalue::Discriminant(_) | Rvalue::Len(_) => {}
    }
}

fn collect_consumed_operand(operand: &Operand, out: &mut Vec<u32>) {
    if let Operand::Move(place) = operand
        && place.projection.is_empty()
    {
        out.push(place.local.0);
    }
}

fn collect_operand_read(operand: &Operand, out: &mut Vec<(LocalId, ReadKind)>) {
    match operand {
        Operand::Copy(place) => out.push((place.local, ReadKind::Copy)),
        Operand::Move(place) => out.push((place.local, ReadKind::Consume)),
        Operand::Const(_) => {}
    }
}

fn collect_arg_reads(arg: &Arg, out: &mut Vec<(LocalId, ReadKind)>) {
    match arg {
        Arg::Value(operand) => collect_operand_read(operand, out),
        Arg::Borrow(place) | Arg::BorrowMut(place) => out.push((place.local, ReadKind::Borrow)),
    }
}

fn collect_rvalue_reads(rvalue: &Rvalue, out: &mut Vec<(LocalId, ReadKind)>) {
    match rvalue {
        Rvalue::Use(operand) | Rvalue::Unary { operand, .. } | Rvalue::Cast { operand, .. } => {
            collect_operand_read(operand, out);
        }
        Rvalue::Binary { lhs, rhs, .. } => {
            collect_operand_read(lhs, out);
            collect_operand_read(rhs, out);
        }
        Rvalue::Aggregate { fields, .. } => {
            for field in fields {
                collect_operand_read(field, out);
            }
        }
        Rvalue::StrOp { args, .. } => {
            for arg in args {
                collect_operand_read(arg, out);
            }
        }
        Rvalue::HeapOp { subject, args, .. } => {
            if let Some(place) = subject {
                out.push((place.local, ReadKind::Borrow));
            }
            for arg in args {
                collect_operand_read(arg, out);
            }
        }
        Rvalue::Discriminant(place) | Rvalue::Len(place) => {
            out.push((place.local, ReadKind::Borrow));
        }
    }
}

/// Whether two operand types may share a binary operator. `Never` and
/// `Error` unify with anything (the front end already reported any real
/// mismatch; the verifier only catches gross lowering bugs).
fn types_agree(a: &Ty, b: &Ty) -> bool {
    if matches!(a, Ty::Never | Ty::Error) || matches!(b, Ty::Never | Ty::Error) {
        return true;
    }
    a == b
}

/// The element type of a growable `Array[T]` (ADR-0012), or `None` for any other
/// type. Used to validate the generic `push`/`pop`/`get` element against the
/// array place's actual element type rather than a hardcoded `Int`.
fn array_element(ty: &Ty) -> Option<Ty> {
    match ty {
        Ty::Array(item) => Some((**item).clone()),
        _ => None,
    }
}

/// The key/value pair of a `Map[K, V]` (ADR-0011), or `None` for any other
/// type. Used to validate the generic map ops against the map place's actual
/// pair rather than a hardcoded `Int`.
fn map_pair(ty: &Ty) -> Option<(Ty, Ty)> {
    match ty {
        Ty::Map(key, value) => Some(((**key).clone(), (**value).clone())),
        _ => None,
    }
}

fn const_ty(constant: &crate::mir::Const) -> Ty {
    use crate::mir::Const;
    match constant {
        Const::Unit => Ty::Unit,
        Const::Bool(_) => Ty::Bool,
        Const::Int(_, kind) => Ty::Int(*kind),
        Const::Float(_, kind) => Ty::Float(*kind),
        Const::Char(_) => Ty::Char,
        Const::Str(_) => Ty::Str,
        // A function value's type needs the type table; `operand_ty` handles
        // it directly. Reaching here (a raw `const_ty` on a `Const::Fn`)
        // means no type context, so it is poison.
        Const::Fn(_) => Ty::Error,
    }
}

fn bin_name(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "add",
        BinOp::Sub => "sub",
        BinOp::Mul => "mul",
        BinOp::Div => "div",
        BinOp::Rem => "rem",
        BinOp::Eq => "eq",
        BinOp::Ne => "ne",
        BinOp::Lt => "lt",
        BinOp::Le => "le",
        BinOp::Gt => "gt",
        BinOp::Ge => "ge",
    }
}

#[cfg(test)]
mod tests {
    use tuo_diagnostics::{Namespace, Severity};
    use tuo_resolve::SymbolId;
    use tuo_source::{SourceId, Span, TextRange};
    use tuo_types::{IntKind, Ty, TypeckResult};

    use super::{debug_assert_verified, is_well_formed, verify};
    use crate::mir::{
        Arg, BasicBlock, BinOp, BlockId, Callee, Const, EffectOp, Function, LocalDecl, LocalId,
        Operand, PassMode, Place, Program, Projection, Rvalue, Statement, StrOp, Terminator,
    };

    fn span() -> Span {
        Span::new(
            SourceId::from_raw(0),
            TextRange::new(0u32, 1u32).expect("forward range"),
        )
    }

    fn local(ty: Ty) -> LocalDecl {
        LocalDecl {
            ty,
            name: None,
            span: span(),
        }
    }

    /// A minimal one-block function skeleton the tests then corrupt.
    fn func(
        params: Vec<PassMode>,
        locals: Vec<LocalDecl>,
        blocks: Vec<BasicBlock>,
        ret: Ty,
    ) -> Program {
        Program {
            functions: vec![Function {
                symbol: SymbolId::from_raw(0),
                name: "f".to_owned(),
                params,
                locals,
                blocks,
                ret,
                span: span(),
            }],
            skipped: Vec::new(),
        }
    }

    fn int(value: i128) -> Operand {
        Operand::Const(Const::Int(value, IntKind::I64))
    }

    fn codes(program: &Program) -> Vec<u16> {
        let diagnostics = verify(program, &TypeckResult::default());
        for diag in &diagnostics {
            assert_eq!(diag.severity, Severity::Error);
            assert_eq!(diag.code.namespace(), Namespace::Mir);
        }
        diagnostics.iter().map(|d| d.code.number()).collect()
    }

    #[test]
    fn a_well_formed_function_verifies_clean() {
        // fn f() -> I64 { bb0: _1 = const 1; return copy _1 }
        // (`_1` is `Copy`, so it is read by `copy`, not `move`.)
        let program = func(
            vec![],
            vec![local(Ty::int()), local(Ty::int())],
            vec![BasicBlock {
                statements: vec![Statement::Assign {
                    place: Place::local(LocalId(1)),
                    rvalue: Rvalue::Use(int(1)),
                }],
                terminator: Terminator::Return(Operand::Copy(Place::local(LocalId(1)))),
            }],
            Ty::int(),
        );
        assert!(is_well_formed(&program, &TypeckResult::default()));
        assert!(codes(&program).is_empty());
    }

    #[test]
    fn moving_a_copy_value_is_reported() {
        // return move _1 where _1 is `Copy` — should be a copy.
        let program = func(
            vec![],
            vec![local(Ty::int()), local(Ty::int())],
            vec![BasicBlock {
                statements: vec![Statement::Assign {
                    place: Place::local(LocalId(1)),
                    rvalue: Rvalue::Use(int(1)),
                }],
                terminator: Terminator::Return(Operand::Move(Place::local(LocalId(1)))),
            }],
            Ty::int(),
        );
        assert_eq!(codes(&program), vec![super::code::OWNERSHIP]);
    }

    #[test]
    fn dangling_block_target_is_reported() {
        // bb0 jumps to bb9, which does not exist.
        let program = func(
            vec![],
            vec![local(Ty::int())],
            vec![BasicBlock {
                statements: vec![],
                terminator: Terminator::Goto(BlockId(9)),
            }],
            Ty::Unit,
        );
        assert_eq!(codes(&program), vec![super::code::BAD_BLOCK_TARGET]);
    }

    #[test]
    fn out_of_range_local_is_reported() {
        // return move _7, but there is no _7.
        let program = func(
            vec![],
            vec![local(Ty::int())],
            vec![BasicBlock {
                statements: vec![],
                terminator: Terminator::Return(Operand::Move(Place::local(LocalId(7)))),
            }],
            Ty::int(),
        );
        assert_eq!(codes(&program), vec![super::code::BAD_LOCAL]);
    }

    #[test]
    fn illegal_projection_is_reported() {
        // _0 is an I64; taking field .0 of it is not a legal projection.
        let place = Place {
            local: LocalId(0),
            projection: vec![Projection::Field(0)],
        };
        let program = func(
            vec![],
            vec![local(Ty::int())],
            vec![BasicBlock {
                statements: vec![Statement::Drop { place }],
                terminator: Terminator::Return(int(0)),
            }],
            Ty::int(),
        );
        assert_eq!(codes(&program), vec![super::code::BAD_PROJECTION]);
    }

    #[test]
    fn use_of_uninitialized_local_is_reported() {
        // _1 is never assigned, yet it is returned.
        let program = func(
            vec![],
            vec![local(Ty::int()), local(Ty::int())],
            vec![BasicBlock {
                statements: vec![],
                terminator: Terminator::Return(Operand::Copy(Place::local(LocalId(1)))),
            }],
            Ty::int(),
        );
        assert_eq!(codes(&program), vec![super::code::USE_UNINIT]);
    }

    #[test]
    fn use_after_move_is_reported() {
        // A non-`Copy` local (`String`): move it out, then read it again.
        // _1 begins uninitialized; a param seeds an initialized source.
        let program = func(
            vec![PassMode::Value],
            vec![local(Ty::String), local(Ty::String), local(Ty::String)],
            vec![BasicBlock {
                statements: vec![
                    // _1 = move _0 (initialize _1 from the parameter)
                    Statement::Assign {
                        place: Place::local(LocalId(1)),
                        rvalue: Rvalue::Use(Operand::Move(Place::local(LocalId(0)))),
                    },
                    // _2 = move _1 (consume _1)
                    Statement::Assign {
                        place: Place::local(LocalId(2)),
                        rvalue: Rvalue::Use(Operand::Move(Place::local(LocalId(1)))),
                    },
                ],
                // read _1 again — used after move.
                terminator: Terminator::Return(Operand::Move(Place::local(LocalId(1)))),
            }],
            Ty::String,
        );
        assert_eq!(codes(&program), vec![super::code::USE_MOVED]);
    }

    #[test]
    fn binary_type_mismatch_is_reported() {
        // add(const 1_i64, const true) — operands of differing types.
        let program = func(
            vec![],
            vec![local(Ty::int()), local(Ty::int())],
            vec![BasicBlock {
                statements: vec![Statement::Assign {
                    place: Place::local(LocalId(1)),
                    rvalue: Rvalue::Binary {
                        op: BinOp::Add,
                        lhs: int(1),
                        rhs: Operand::Const(Const::Bool(true)),
                    },
                }],
                terminator: Terminator::Return(Operand::Copy(Place::local(LocalId(1)))),
            }],
            Ty::int(),
        );
        assert_eq!(codes(&program), vec![super::code::TYPE_MISMATCH]);
    }

    #[test]
    fn branch_on_non_bool_is_reported() {
        // branch on an integer condition.
        let program = func(
            vec![],
            vec![local(Ty::int())],
            vec![
                BasicBlock {
                    statements: vec![],
                    terminator: Terminator::Branch {
                        cond: int(1),
                        then_block: BlockId(1),
                        else_block: BlockId(1),
                    },
                },
                BasicBlock {
                    statements: vec![],
                    terminator: Terminator::Return(int(0)),
                },
            ],
            Ty::int(),
        );
        assert_eq!(codes(&program), vec![super::code::TYPE_MISMATCH]);
    }

    #[test]
    fn discriminant_of_non_enum_is_reported() {
        let program = func(
            vec![],
            vec![local(Ty::int()), local(Ty::Int(IntKind::Usize))],
            vec![BasicBlock {
                statements: vec![Statement::Assign {
                    place: Place::local(LocalId(1)),
                    rvalue: Rvalue::Discriminant(Place::local(LocalId(0))),
                }],
                terminator: Terminator::Return(int(0)),
            }],
            Ty::int(),
        );
        // _0 (a param) is initialized, so the only complaint is the type.
        assert_eq!(
            codes(&program),
            vec![super::code::TYPE_MISMATCH],
            "discriminant of a non-enum place",
        );
    }

    #[test]
    fn duplicated_switch_arm_value_is_reported() {
        let program = func(
            vec![PassMode::Value],
            vec![local(Ty::int())],
            vec![
                BasicBlock {
                    statements: vec![],
                    terminator: Terminator::Switch {
                        discr: Operand::Copy(Place::local(LocalId(0))),
                        arms: vec![(0, BlockId(1)), (0, BlockId(1))],
                        otherwise: BlockId(1),
                    },
                },
                BasicBlock {
                    statements: vec![],
                    terminator: Terminator::Return(int(0)),
                },
            ],
            Ty::int(),
        );
        assert_eq!(codes(&program), vec![super::code::BAD_TERMINATOR]);
    }

    #[test]
    fn malformed_signature_is_reported() {
        // Two parameter modes but zero locals: params cannot be locals.
        let program = func(
            vec![PassMode::Value, PassMode::Value],
            vec![],
            vec![BasicBlock {
                statements: vec![],
                terminator: Terminator::Return(Operand::Const(Const::Unit)),
            }],
            Ty::Unit,
        );
        let found = codes(&program);
        assert!(
            found.contains(&super::code::BAD_SIGNATURE),
            "signature error expected, got {found:?}",
        );
    }

    #[test]
    fn borrowing_and_moving_the_same_local_in_one_call_is_reported() {
        // call g(borrow _0, move _0) — the same (non-`Copy`) place is lent
        // and consumed in one call.
        let program = func(
            vec![PassMode::Value],
            vec![local(Ty::String), local(Ty::Unit)],
            vec![BasicBlock {
                statements: vec![Statement::Call {
                    dest: Place::local(LocalId(1)),
                    callee: Callee::Direct(SymbolId::from_raw(1)),
                    args: vec![
                        Arg::Borrow(Place::local(LocalId(0))),
                        Arg::Value(Operand::Move(Place::local(LocalId(0)))),
                    ],
                }],
                terminator: Terminator::Return(Operand::Const(Const::Unit)),
            }],
            Ty::Unit,
        );
        assert_eq!(codes(&program), vec![super::code::OWNERSHIP]);
    }

    #[test]
    fn an_indirect_call_through_a_non_function_value_is_reported() {
        // call (copy _0)() where _0 is an `Int` — the indirect callee operand
        // is not of function type (ADR-0008 Tier 1, M0014).
        let program = func(
            vec![PassMode::Value],
            vec![local(Ty::int()), local(Ty::Unit)],
            vec![BasicBlock {
                statements: vec![Statement::Call {
                    dest: Place::local(LocalId(1)),
                    callee: Callee::Indirect(Operand::Copy(Place::local(LocalId(0)))),
                    args: Vec::new(),
                }],
                terminator: Terminator::Return(Operand::Const(Const::Unit)),
            }],
            Ty::Unit,
        );
        assert_eq!(codes(&program), vec![super::code::INDIRECT_CALL]);
    }

    #[test]
    fn an_index_local_out_of_range_is_reported() {
        // _0[_9] where _9 does not exist.
        let place = Place {
            local: LocalId(0),
            projection: vec![Projection::Index(LocalId(9))],
        };
        let program = func(
            vec![PassMode::Value],
            vec![local(Ty::Array(Box::new(Ty::int()))), local(Ty::int())],
            vec![BasicBlock {
                statements: vec![Statement::Assign {
                    place: Place::local(LocalId(1)),
                    rvalue: Rvalue::Use(Operand::Copy(place)),
                }],
                terminator: Terminator::Return(Operand::Copy(Place::local(LocalId(1)))),
            }],
            Ty::int(),
        );
        assert_eq!(codes(&program), vec![super::code::BAD_LOCAL]);
    }

    #[test]
    fn an_unreachable_block_is_not_a_false_undefined_use() {
        // bb1 reads an uninitialized _1 but is never reached from bb0.
        let program = func(
            vec![],
            vec![local(Ty::int()), local(Ty::int())],
            vec![
                BasicBlock {
                    statements: vec![],
                    terminator: Terminator::Return(int(0)),
                },
                BasicBlock {
                    statements: vec![],
                    terminator: Terminator::Return(Operand::Copy(Place::local(LocalId(1)))),
                },
            ],
            Ty::int(),
        );
        assert!(
            codes(&program).is_empty(),
            "dead code must not raise an undefined-use error",
        );
    }

    #[test]
    fn a_loop_body_with_a_bounds_assert_terminates_the_init_solver() {
        // Regression: the exact CFG shape `tuo-mir` lowering emits for a `for`
        // loop whose body indexes an array — a back edge (bb5 → bb3) from a
        // block numbered *higher* than its successor, introduced by the bounds
        // `Assert`'s continuation block. The old `solve_init` seeding (skip
        // unvisited predecessors, default a pred-less block to bottom) was
        // non-monotone on this shape and oscillated forever; the top-seeded
        // must-analysis converges. The test's assertion is that `verify`
        // *returns* (and finds nothing wrong).
        //
        //   bb0: init; goto bb1            (loop head is bb1)
        //   bb1: cond; branch bb2 / bb4    (bb4 = exit)
        //   bb2: bounds check; assert -> bb5
        //   bb3: step; goto bb1            (back edge target, numbered low)
        //   bb4: return
        //   bb5: body; goto bb3            (back edge source, numbered high)
        let usize_local = || local(Ty::Int(IntKind::Usize));
        let usize_const = |value: i128| Operand::Const(Const::Int(value, IntKind::Usize));
        let assign = |index: u32, rvalue: Rvalue| Statement::Assign {
            place: Place::local(LocalId(index)),
            rvalue,
        };
        let copy = |index: u32| Operand::Copy(Place::local(LocalId(index)));
        let program = func(
            vec![],
            vec![
                local(Ty::int()), // _0: accumulator
                usize_local(),    // _1: index
                usize_local(),    // _2: len (a fixed array's constant length)
                local(Ty::Bool),  // _3: loop condition
                local(Ty::Bool),  // _4: bounds condition
                local(Ty::int()), // _5: body temp
            ],
            vec![
                BasicBlock {
                    statements: vec![
                        assign(0, Rvalue::Use(int(0))),
                        assign(1, Rvalue::Use(usize_const(0))),
                        assign(2, Rvalue::Use(usize_const(3))),
                    ],
                    terminator: Terminator::Goto(BlockId(1)),
                },
                BasicBlock {
                    statements: vec![assign(
                        3,
                        Rvalue::Binary {
                            op: BinOp::Lt,
                            lhs: copy(1),
                            rhs: copy(2),
                        },
                    )],
                    terminator: Terminator::Branch {
                        cond: copy(3),
                        then_block: BlockId(2),
                        else_block: BlockId(4),
                    },
                },
                BasicBlock {
                    statements: vec![assign(
                        4,
                        Rvalue::Binary {
                            op: BinOp::Lt,
                            lhs: copy(1),
                            rhs: copy(2),
                        },
                    )],
                    terminator: Terminator::Assert {
                        cond: copy(4),
                        trap: crate::mir::Trap::IndexOutOfBounds,
                        target: BlockId(5),
                    },
                },
                BasicBlock {
                    statements: vec![assign(
                        1,
                        Rvalue::Binary {
                            op: BinOp::Add,
                            lhs: copy(1),
                            rhs: usize_const(1),
                        },
                    )],
                    terminator: Terminator::Goto(BlockId(1)),
                },
                BasicBlock {
                    statements: vec![],
                    terminator: Terminator::Return(copy(0)),
                },
                BasicBlock {
                    statements: vec![
                        assign(
                            5,
                            Rvalue::Binary {
                                op: BinOp::Add,
                                lhs: copy(0),
                                rhs: int(1),
                            },
                        ),
                        assign(0, Rvalue::Use(copy(5))),
                    ],
                    terminator: Terminator::Goto(BlockId(3)),
                },
            ],
            Ty::int(),
        );
        assert!(
            codes(&program).is_empty(),
            "the loop-with-assert shape must verify clean (and terminate)",
        );
    }

    #[test]
    fn debug_assert_verified_accepts_well_formed_mir() {
        let program = func(
            vec![],
            vec![local(Ty::int())],
            vec![BasicBlock {
                statements: vec![],
                terminator: Terminator::Return(int(0)),
            }],
            Ty::int(),
        );
        // Must not panic.
        debug_assert_verified("noop", &program, &TypeckResult::default());
    }

    // ----- ADR-0006 Stage A: StrOp rvalues and Effect statements -----

    fn str_const(text: &str) -> Operand {
        Operand::Const(Const::Str(text.to_owned()))
    }

    /// `_0 = <rvalue>; return copy _0` over one local of type `ret`.
    fn one_assign(rvalue: Rvalue, ret: Ty) -> Program {
        func(
            vec![],
            vec![local(ret.clone())],
            vec![BasicBlock {
                statements: vec![Statement::Assign {
                    place: Place::local(LocalId(0)),
                    rvalue,
                }],
                terminator: Terminator::Return(Operand::Copy(Place::local(LocalId(0)))),
            }],
            ret,
        )
    }

    #[test]
    fn well_formed_str_ops_verify_clean() {
        for (op, args, ret) in [
            (StrOp::Len, vec![str_const("abc")], Ty::int()),
            (StrOp::ByteAt, vec![str_const("abc"), int(0)], Ty::int()),
            (
                StrOp::Slice,
                vec![str_const("abc"), int(0), int(1)],
                Ty::Str,
            ),
        ] {
            let program = one_assign(Rvalue::StrOp { op, args }, ret);
            assert!(
                is_well_formed(&program, &TypeckResult::default()),
                "{op:?} should verify"
            );
        }
    }

    #[test]
    fn a_str_op_with_the_wrong_operand_count_is_reported() {
        let program = one_assign(
            Rvalue::StrOp {
                op: StrOp::ByteAt,
                args: vec![str_const("abc")],
            },
            Ty::int(),
        );
        assert_eq!(codes(&program), vec![super::code::STR_OP]);
    }

    #[test]
    fn a_str_op_with_a_mistyped_operand_is_reported() {
        // byte_at(Int, Int): the string operand is not a `Str`.
        let program = one_assign(
            Rvalue::StrOp {
                op: StrOp::ByteAt,
                args: vec![int(1), int(0)],
            },
            Ty::int(),
        );
        assert_eq!(codes(&program), vec![super::code::STR_OP]);
    }

    #[test]
    fn a_str_op_with_the_wrong_destination_type_is_reported() {
        // slice yields a `Str`; storing it into an `I64` local is malformed.
        let program = one_assign(
            Rvalue::StrOp {
                op: StrOp::Slice,
                args: vec![str_const("abc"), int(0), int(1)],
            },
            Ty::int(),
        );
        assert_eq!(codes(&program), vec![super::code::STR_OP]);
    }

    #[test]
    fn a_well_formed_effect_verifies_and_initializes_its_destination() {
        // _0 = effect read_byte(const 0); return copy _0 — the destination
        // counts as initialized after the statement (dataflow).
        let program = func(
            vec![],
            vec![local(Ty::int())],
            vec![BasicBlock {
                statements: vec![Statement::Effect {
                    op: EffectOp::ReadByte,
                    args: vec![Arg::Value(int(0))],
                    dest: Place::local(LocalId(0)),
                }],
                terminator: Terminator::Return(Operand::Copy(Place::local(LocalId(0)))),
            }],
            Ty::int(),
        );
        assert!(is_well_formed(&program, &TypeckResult::default()));
    }

    #[test]
    fn an_effect_with_the_wrong_operand_count_is_reported() {
        let program = func(
            vec![],
            vec![local(Ty::int())],
            vec![BasicBlock {
                statements: vec![Statement::Effect {
                    op: EffectOp::Write,
                    args: vec![Arg::Value(int(1))],
                    dest: Place::local(LocalId(0)),
                }],
                terminator: Terminator::Return(Operand::Copy(Place::local(LocalId(0)))),
            }],
            Ty::int(),
        );
        assert_eq!(codes(&program), vec![super::code::EFFECT]);
    }

    #[test]
    fn an_effect_with_a_mistyped_operand_is_reported() {
        // write(Str, Str): the fd operand is not an `I64`.
        let program = func(
            vec![],
            vec![local(Ty::int())],
            vec![BasicBlock {
                statements: vec![Statement::Effect {
                    op: EffectOp::Write,
                    args: vec![Arg::Value(str_const("nope")), Arg::Value(str_const("x"))],
                    dest: Place::local(LocalId(0)),
                }],
                terminator: Terminator::Return(Operand::Copy(Place::local(LocalId(0)))),
            }],
            Ty::int(),
        );
        assert_eq!(codes(&program), vec![super::code::EFFECT]);
    }

    #[test]
    fn an_effect_with_a_non_int_destination_is_reported() {
        let program = func(
            vec![],
            vec![local(Ty::Bool), local(Ty::int())],
            vec![BasicBlock {
                statements: vec![
                    Statement::Effect {
                        op: EffectOp::Exit,
                        args: vec![Arg::Value(int(0))],
                        dest: Place::local(LocalId(0)),
                    },
                    Statement::Assign {
                        place: Place::local(LocalId(1)),
                        rvalue: Rvalue::Use(int(0)),
                    },
                ],
                terminator: Terminator::Return(Operand::Copy(Place::local(LocalId(1)))),
            }],
            Ty::int(),
        );
        assert_eq!(codes(&program), vec![super::code::EFFECT]);
    }

    #[test]
    fn an_effect_reading_an_uninitialized_operand_is_reported() {
        // _1 is never assigned but is passed to the effect.
        let program = func(
            vec![],
            vec![local(Ty::int()), local(Ty::int())],
            vec![BasicBlock {
                statements: vec![Statement::Effect {
                    op: EffectOp::ReadByte,
                    args: vec![Arg::Value(Operand::Copy(Place::local(LocalId(1))))],
                    dest: Place::local(LocalId(0)),
                }],
                terminator: Terminator::Return(Operand::Copy(Place::local(LocalId(0)))),
            }],
            Ty::int(),
        );
        assert_eq!(codes(&program), vec![super::code::USE_UNINIT]);
    }

    // ----- ADR-0009 Stage A: HeapOp rvalues, HeapMutate, WriteString -----

    use crate::mir::{HeapMutOp, HeapOp};

    fn int_array() -> Ty {
        Ty::Array(Box::new(Ty::int()))
    }

    #[test]
    fn well_formed_heap_ops_verify_clean() {
        // Constructors and Str-only ops: no subject.
        for (op, args, ret) in [
            (HeapOp::StringEmpty, vec![], Ty::String),
            (HeapOp::StringFromStr, vec![str_const("ab")], Ty::String),
            (
                HeapOp::StringConcat,
                vec![str_const("a"), str_const("b")],
                Ty::String,
            ),
            (HeapOp::ArrayEmpty, vec![], int_array()),
        ] {
            let program = one_assign(
                Rvalue::HeapOp {
                    op,
                    subject: None,
                    args,
                },
                ret,
            );
            assert!(
                is_well_formed(&program, &TypeckResult::default()),
                "{op:?} should verify"
            );
        }
        // Subject-reading ops: `_0: String/Array` is the (parameter) subject.
        for (op, subject_ty, args, ret) in [
            (HeapOp::StringLen, Ty::String, vec![], Ty::int()),
            (HeapOp::StringByteAt, Ty::String, vec![int(0)], Ty::int()),
            (
                HeapOp::StringSlice,
                Ty::String,
                vec![int(0), int(1)],
                Ty::String,
            ),
            // ADR-0010: `as_str` reads a String subject and produces a Str.
            (HeapOp::StringAsStr, Ty::String, vec![], Ty::Str),
            (HeapOp::ArrayLen, int_array(), vec![], Ty::int()),
            (HeapOp::ArrayGet, int_array(), vec![int(0)], Ty::int()),
        ] {
            let program = func(
                vec![PassMode::Borrow],
                vec![local(subject_ty), local(ret.clone())],
                vec![BasicBlock {
                    statements: vec![Statement::Assign {
                        place: Place::local(LocalId(1)),
                        rvalue: Rvalue::HeapOp {
                            op,
                            subject: Some(Place::local(LocalId(0))),
                            args,
                        },
                    }],
                    terminator: Terminator::Return(int(0)),
                }],
                Ty::int(),
            );
            assert!(
                is_well_formed(&program, &TypeckResult::default()),
                "{op:?} should verify"
            );
        }
    }

    #[test]
    fn a_heap_op_missing_its_subject_is_reported() {
        let program = one_assign(
            Rvalue::HeapOp {
                op: HeapOp::StringLen,
                subject: None,
                args: vec![],
            },
            Ty::int(),
        );
        assert_eq!(codes(&program), vec![super::code::HEAP_OP]);
    }

    #[test]
    fn a_heap_op_with_a_mistyped_subject_is_reported() {
        // string_len of an Array subject.
        let program = func(
            vec![PassMode::Borrow],
            vec![local(int_array()), local(Ty::int())],
            vec![BasicBlock {
                statements: vec![Statement::Assign {
                    place: Place::local(LocalId(1)),
                    rvalue: Rvalue::HeapOp {
                        op: HeapOp::StringLen,
                        subject: Some(Place::local(LocalId(0))),
                        args: vec![],
                    },
                }],
                terminator: Terminator::Return(int(0)),
            }],
            Ty::int(),
        );
        assert_eq!(codes(&program), vec![super::code::HEAP_OP]);
    }

    #[test]
    fn a_heap_op_with_the_wrong_operand_count_is_reported() {
        let program = one_assign(
            Rvalue::HeapOp {
                op: HeapOp::StringConcat,
                subject: None,
                args: vec![str_const("a")],
            },
            Ty::String,
        );
        assert_eq!(codes(&program), vec![super::code::HEAP_OP]);
    }

    #[test]
    fn a_heap_op_with_the_wrong_destination_type_is_reported() {
        // string_concat yields a `String`; an `I64` destination is malformed.
        let program = one_assign(
            Rvalue::HeapOp {
                op: HeapOp::StringConcat,
                subject: None,
                args: vec![str_const("a"), str_const("b")],
            },
            Ty::int(),
        );
        assert_eq!(codes(&program), vec![super::code::HEAP_OP]);
    }

    /// `_0: mut <target_ty>` param; `_1 = heap_mut <op>(mut _0, args…)`.
    fn one_mutate(op: HeapMutOp, target_ty: Ty, args: Vec<Operand>, dest_ty: Ty) -> Program {
        func(
            vec![PassMode::BorrowMut],
            vec![local(target_ty), local(dest_ty)],
            vec![BasicBlock {
                statements: vec![Statement::HeapMutate {
                    op,
                    target: Place::local(LocalId(0)),
                    args,
                    dest: Place::local(LocalId(1)),
                }],
                terminator: Terminator::Return(int(0)),
            }],
            Ty::int(),
        )
    }

    #[test]
    fn well_formed_heap_mutations_verify_clean() {
        for (op, target_ty, args, dest_ty) in [
            (HeapMutOp::PushByte, Ty::String, vec![int(65)], Ty::Unit),
            (
                HeapMutOp::Append,
                Ty::String,
                vec![str_const("x")],
                Ty::Unit,
            ),
            (HeapMutOp::Push, int_array(), vec![int(1)], Ty::Unit),
            (
                HeapMutOp::Pop,
                int_array(),
                vec![],
                Ty::Option(Box::new(Ty::int())),
            ),
        ] {
            let program = one_mutate(op, target_ty, args, dest_ty);
            assert!(
                is_well_formed(&program, &TypeckResult::default()),
                "{op:?} should verify"
            );
        }
    }

    #[test]
    fn a_heap_mutation_with_a_mistyped_target_is_reported() {
        // push_byte into an Array target.
        let program = one_mutate(HeapMutOp::PushByte, int_array(), vec![int(65)], Ty::Unit);
        assert_eq!(codes(&program), vec![super::code::HEAP_MUTATE]);
    }

    #[test]
    fn a_heap_mutation_with_the_wrong_operand_count_is_reported() {
        let program = one_mutate(HeapMutOp::Push, int_array(), vec![], Ty::Unit);
        assert_eq!(codes(&program), vec![super::code::HEAP_MUTATE]);
    }

    #[test]
    fn a_heap_mutation_with_the_wrong_destination_type_is_reported() {
        // pop yields Option[I64]; a Unit destination is malformed.
        let program = one_mutate(HeapMutOp::Pop, int_array(), vec![], Ty::Unit);
        assert_eq!(codes(&program), vec![super::code::HEAP_MUTATE]);
    }

    #[test]
    fn a_heap_mutation_keeps_its_target_initialized() {
        // The target of a mutation is readable afterwards (a `mut` borrow):
        // heap_mut push(mut _0, 1); _1 = heap_array_len(borrow _0).
        let program = func(
            vec![PassMode::BorrowMut],
            vec![local(int_array()), local(Ty::Unit), local(Ty::int())],
            vec![BasicBlock {
                statements: vec![
                    Statement::HeapMutate {
                        op: HeapMutOp::Push,
                        target: Place::local(LocalId(0)),
                        args: vec![int(1)],
                        dest: Place::local(LocalId(1)),
                    },
                    Statement::Assign {
                        place: Place::local(LocalId(2)),
                        rvalue: Rvalue::HeapOp {
                            op: HeapOp::ArrayLen,
                            subject: Some(Place::local(LocalId(0))),
                            args: vec![],
                        },
                    },
                ],
                terminator: Terminator::Return(Operand::Copy(Place::local(LocalId(2)))),
            }],
            Ty::int(),
        );
        assert!(is_well_formed(&program, &TypeckResult::default()));
    }

    #[test]
    fn a_well_formed_write_string_effect_verifies() {
        // _0: in String param; _1 = effect write_string(const 1, borrow _0).
        let program = func(
            vec![PassMode::Borrow],
            vec![local(Ty::String), local(Ty::int())],
            vec![BasicBlock {
                statements: vec![Statement::Effect {
                    op: EffectOp::WriteString,
                    args: vec![Arg::Value(int(1)), Arg::Borrow(Place::local(LocalId(0)))],
                    dest: Place::local(LocalId(1)),
                }],
                terminator: Terminator::Return(Operand::Copy(Place::local(LocalId(1)))),
            }],
            Ty::int(),
        );
        assert!(is_well_formed(&program, &TypeckResult::default()));
    }

    #[test]
    fn a_write_string_with_a_by_value_string_is_reported() {
        // The `String` argument must be a borrow place, not a value operand.
        let program = func(
            vec![PassMode::Value],
            vec![local(Ty::String), local(Ty::int())],
            vec![BasicBlock {
                statements: vec![Statement::Effect {
                    op: EffectOp::WriteString,
                    args: vec![
                        Arg::Value(int(1)),
                        Arg::Value(Operand::Move(Place::local(LocalId(0)))),
                    ],
                    dest: Place::local(LocalId(1)),
                }],
                terminator: Terminator::Return(Operand::Copy(Place::local(LocalId(1)))),
            }],
            Ty::int(),
        );
        assert_eq!(codes(&program), vec![super::code::EFFECT]);
    }

    #[test]
    #[should_panic(expected = "produced unverifiable MIR")]
    fn debug_assert_verified_rejects_malformed_mir() {
        // A dangling goto: a pass that produced this is a compiler bug.
        let program = func(
            vec![],
            vec![local(Ty::int())],
            vec![BasicBlock {
                statements: vec![],
                terminator: Terminator::Goto(BlockId(9)),
            }],
            Ty::Unit,
        );
        debug_assert_verified("broken-pass", &program, &TypeckResult::default());
    }
}
