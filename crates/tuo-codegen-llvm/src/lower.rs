//! MIR → LLVM IR lowering for the v0 scalar core.
//!
//! This is the LLVM counterpart of the Cranelift backend's `lower` module, and
//! it is written to match it — and therefore the reference interpreter —
//! instruction for instruction. [`lower_program`] declares every function (so
//! direct calls resolve), then defines each body by walking its basic blocks
//! and translating each statement and terminator to LLVM IR that computes
//! exactly what the interpreter computes. Anything outside the scalar subset is
//! reported as [`CodegenError::unsupported`]; the lowering never emits code
//! whose meaning it is unsure of.
//!
//! # Trapping semantics
//!
//! Integer arithmetic traps on overflow, and division/remainder trap on a zero
//! divisor and on `MIN / -1` (Constitution §24). Overflow is detected with
//! LLVM's `llvm.{s,u}{add,sub,mul}.with.overflow` intrinsics (the same checks
//! the Cranelift backend gets from its overflow-flagged instructions); each
//! check branches to a dedicated trap block that calls the runtime
//! [`TRAP_SYMBOL`](tuo_runtime::TRAP_SYMBOL) with the matching
//! [`TrapCode`](tuo_runtime::TrapCode) and then reaches `unreachable` — the
//! runtime call never returns. This mirrors the interpreter's deterministic
//! abort.
//!
//! # Locals
//!
//! Each MIR local is classified once, up front (see [`LocalKind`]):
//!
//! - a **scalar** local (bool/char/int) is an `alloca` of a single integer, read
//!   with `load` and written with `store`; `mem2reg` promotes it to a register;
//! - a **unit** local carries no value;
//! - an **aggregate** local (a Stage-1 product type, or an ADR-0004 Stage 2
//!   fixed array `[T; N]`) is an `alloca` of the aggregate's exact byte layout,
//!   force-aligned to the ABI alignment, whose fields/elements are accessed by
//!   GEP-ing a byte offset from the ABI (see [`tuo_runtime::abi`]).
//!
//! A local whose type has no v0 layout (the growable `Array[T]`, strings,
//! floats, heap-backed aggregates) still makes the whole function unsupported.
//! Aggregate lowering follows ADR-0004 and is byte-for-byte identical to the
//! Cranelift backend: scalar leaves, by-pointer/sret call ABI, every
//! size/offset from the runtime ABI. Fixed arrays are laid out inline —
//! element `i` at `i × stride(T)` — and indexed by unchecked address
//! arithmetic, because MIR asserts the bounds (`Assert { IndexOutOfBounds }`)
//! before every `Projection::Index` use.

use std::collections::HashMap;

use inkwell::AddressSpace;
use inkwell::IntPredicate;
use inkwell::basic_block::BasicBlock as LlvmBlock;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::intrinsics::Intrinsic;
use inkwell::module::{Linkage, Module};
use inkwell::types::{BasicMetadataTypeEnum, IntType};
use inkwell::values::{BasicValue, FunctionValue, IntValue, PointerValue};

use tuo_codegen::CodegenError;
use tuo_mir::{
    BinOp, CastKind, Const, Function, Operand, Place, Program, Projection, Rvalue, Statement,
    Terminator, Trap, UnOp,
};
use tuo_resolve::SymbolId;
use tuo_runtime::abi::{Layout, layout_of, struct_field_offsets, variant_field_offsets};
use tuo_runtime::{TRAP_SYMBOL, TrapCode};
use tuo_types::{IntKind, Ty, TypeckResult};

use crate::abi::{int_type, int_width_bits, is_signed, scalar_type};

/// How a MIR local is stored by the backend, decided once up front from its
/// declared type. Parallels the Cranelift backend's `LocalKind` so the two make
/// identical ABI choices.
enum LocalKind<'ctx> {
    /// A scalar (bool/char/int) `alloca` of the given integer type.
    Scalar(PointerValue<'ctx>, IntType<'ctx>),
    /// A `Unit` local (or a zero-sized aggregate): carries no value, no slot.
    Unit,
    /// A Stage-1 aggregate `alloca` (byte array, ABI-aligned); `ty` is the
    /// local's declared type, source of truth for field offsets.
    Aggregate {
        /// The pointer to the aggregate's byte storage.
        ptr: PointerValue<'ctx>,
        /// The aggregate's declared type.
        ty: Ty,
    },
}

/// The storage classification of a local, from its declared type and the ABI.
/// Identical rules to the Cranelift backend's `classify_storage`.
enum Storage {
    Scalar,
    Unit,
    Aggregate(Layout),
}

/// Classify a local's declared type into its backend storage. See [`Storage`].
///
/// # Errors
///
/// [`CodegenError::unsupported`] if the type has no v0 runtime layout.
fn classify_storage(ty: &Ty, types: &TypeckResult, context: &str) -> Result<Storage, CodegenError> {
    if scalar_type_is_some(ty) {
        return Ok(Storage::Scalar);
    }
    if matches!(ty, Ty::Unit) {
        return Ok(Storage::Unit);
    }
    match layout_of(ty, types) {
        Ok(layout) if layout.size == 0 => Ok(Storage::Unit),
        Ok(layout) => Ok(Storage::Aggregate(layout)),
        Err(error) => Err(CodegenError::unsupported(format!(
            "`{context}` uses a type the LLVM backend does not lower yet: {error}"
        ))),
    }
}

/// Whether `ty` is a scalar the backend maps to an LLVM integer, without needing
/// a `Context`. Mirrors `scalar_type`'s domain (bool/char/int).
fn scalar_type_is_some(ty: &Ty) -> bool {
    matches!(ty, Ty::Bool | Ty::Char | Ty::Int(_))
}

/// The linkage a compiled tuonelang function gets: external, so the entry (and
/// the `main` shim) are visible to the linker and inter-function calls resolve.
const FUNCTION_LINKAGE: Linkage = Linkage::External;

/// Declare then define every lowerable function of `program` into `module`.
///
/// Returns the [`FunctionValue`] each MIR function was declared as, keyed by its
/// stable symbol, so the caller can find the entry to build the `main` shim.
///
/// # Errors
///
/// [`CodegenError::unsupported`] if any function reachable in the program uses
/// a feature outside the v0 scalar subset; [`CodegenError::backend`] on an
/// internal LLVM builder failure.
pub(crate) fn lower_program<'ctx>(
    ctx: &'ctx Context,
    module: &Module<'ctx>,
    program: &Program,
    types: &TypeckResult,
) -> Result<HashMap<SymbolId, FunctionValue<'ctx>>, CodegenError> {
    // Pass 1: declare every function so direct calls can reference them before
    // their bodies are defined.
    let mut ids: HashMap<SymbolId, FunctionValue<'ctx>> = HashMap::new();
    for function in &program.functions {
        let fn_type = function_type(ctx, function, types)?;
        let name = mangle(function.symbol);
        let value = module.add_function(&name, fn_type, Some(FUNCTION_LINKAGE));
        ids.insert(function.symbol, value);
    }

    // Pass 2: define each body.
    let builder = ctx.create_builder();
    for function in &program.functions {
        let mut lowering = Lowering::new(
            ctx,
            module,
            &builder,
            &ids,
            function,
            &program.functions,
            types,
        )?;
        lowering.run()?;
    }
    Ok(ids)
}

/// The LLVM function type of a MIR function (v0 ABI, Stage-1 aggregates).
///
/// Applies ADR-0004 Stage 1's two aggregate rules, identically to the Cranelift
/// backend's `function_signature`:
/// - an **aggregate return** prepends an sret `ptr` at argument index 0 and the
///   native return type becomes `void`;
/// - an **aggregate parameter** is a `ptr` to a caller-owned copy.
///
/// Every size/align derives from [`tuo_runtime::abi`], never from LLVM's native
/// small-struct classification.
fn function_type<'ctx>(
    ctx: &'ctx Context,
    function: &Function,
    types: &TypeckResult,
) -> Result<inkwell::types::FunctionType<'ctx>, CodegenError> {
    let ptr_ty = ctx.ptr_type(AddressSpace::default());
    let mut params: Vec<BasicMetadataTypeEnum<'ctx>> =
        Vec::with_capacity(function.params.len() + 1);

    // sret hidden out-pointer is ALWAYS argument index 0 (prepended).
    let ret_storage = classify_storage(&function.ret, types, &function.name)?;
    if matches!(ret_storage, Storage::Aggregate(_)) {
        params.push(ptr_ty.into());
    }

    // Every parameter must be `Value` mode. Borrow-mode parameters are not
    // lowered yet (they alias caller memory).
    for (index, mode) in function.params.iter().enumerate() {
        if *mode != tuo_mir::PassMode::Value {
            return Err(CodegenError::unsupported(format!(
                "`{}` takes a borrow-mode parameter, which the LLVM backend does not lower yet",
                function.name
            )));
        }
        let ty = &function.locals[index].ty;
        match classify_storage(ty, types, &function.name)? {
            Storage::Scalar => params.push(require_scalar(ctx, ty, &function.name)?.into()),
            Storage::Unit => {}
            Storage::Aggregate(_) => params.push(ptr_ty.into()),
        }
    }

    // Return: scalar by value; aggregate is void (written through sret); unit is
    // void.
    Ok(match ret_storage {
        Storage::Scalar => {
            require_scalar(ctx, &function.ret, &function.name)?.fn_type(&params, false)
        }
        Storage::Unit | Storage::Aggregate(_) => ctx.void_type().fn_type(&params, false),
    })
}

/// The scalar LLVM integer type of `ty`, or an unsupported error naming
/// `context`.
fn require_scalar<'ctx>(
    ctx: &'ctx Context,
    ty: &Ty,
    context: &str,
) -> Result<IntType<'ctx>, CodegenError> {
    scalar_type(ctx, ty).ok_or_else(|| {
        CodegenError::unsupported(format!(
            "`{context}` uses a non-scalar type the LLVM backend does not lower yet"
        ))
    })
}

/// The per-function lowering state.
struct Lowering<'a, 'ctx> {
    ctx: &'ctx Context,
    module: &'a Module<'ctx>,
    builder: &'a Builder<'ctx>,
    ids: &'a HashMap<SymbolId, FunctionValue<'ctx>>,
    function: &'a Function,
    /// Every function in the program, so a direct call can read its callee's
    /// return type and name for the aggregate call ABI.
    functions: &'a [Function],
    types: &'a TypeckResult,
    /// The opaque pointer type (for slot addresses and aggregate args).
    ptr_ty: inkwell::types::PointerType<'ctx>,
    value: FunctionValue<'ctx>,
    /// The LLVM block for each MIR block index.
    blocks: Vec<LlvmBlock<'ctx>>,
    /// How each MIR local is stored (index = local id): scalar alloca, unit, or
    /// aggregate byte alloca. Filled during `run`.
    kinds: Vec<LocalKind<'ctx>>,
}

impl<'a, 'ctx> Lowering<'a, 'ctx> {
    fn new(
        ctx: &'ctx Context,
        module: &'a Module<'ctx>,
        builder: &'a Builder<'ctx>,
        ids: &'a HashMap<SymbolId, FunctionValue<'ctx>>,
        function: &'a Function,
        functions: &'a [Function],
        types: &'a TypeckResult,
    ) -> Result<Self, CodegenError> {
        // Classify every local up front so an unsupported (Stage-2) type fails
        // before any IR is built. Concrete allocas are created in `run`.
        for local in &function.locals {
            classify_storage(&local.ty, types, &function.name)?;
        }
        let value = ids[&function.symbol];
        Ok(Self {
            ctx,
            module,
            builder,
            ids,
            function,
            functions,
            types,
            ptr_ty: ctx.ptr_type(AddressSpace::default()),
            value,
            blocks: Vec::new(),
            kinds: Vec::new(),
        })
    }

    /// Lower the whole body.
    fn run(&mut self) -> Result<(), CodegenError> {
        // Create an LLVM block per MIR block.
        self.blocks = (0..self.function.blocks.len())
            .map(|index| {
                self.ctx
                    .append_basic_block(self.value, &format!("bb{index}"))
            })
            .collect();

        // Entry block: allocate a slot per local, then seed parameters. All
        // allocas live at the top of block 0 so `mem2reg` can promote them.
        let entry = self.blocks[0];
        self.builder.position_at_end(entry);

        // Allocate storage for every local: a scalar `alloca` of its integer
        // type; a unit local carries no value; an aggregate `alloca` of its exact
        // byte layout, force-aligned to the ABI alignment.
        self.kinds = Vec::with_capacity(self.function.locals.len());
        for (index, local) in self.function.locals.iter().enumerate() {
            let kind = match classify_storage(&local.ty, self.types, &self.function.name)? {
                Storage::Scalar => {
                    let ty = require_scalar(self.ctx, &local.ty, &self.function.name)?;
                    let ptr = self
                        .builder
                        .build_alloca(ty, &format!("local{index}"))
                        .map_err(builder_err("allocating a scalar slot"))?;
                    LocalKind::Scalar(ptr, ty)
                }
                Storage::Unit => LocalKind::Unit,
                Storage::Aggregate(layout) => {
                    let ptr = self.alloca_aggregate(layout, index)?;
                    LocalKind::Aggregate {
                        ptr,
                        ty: local.ty.clone(),
                    }
                }
            };
            self.kinds.push(kind);
        }

        // If this function returns an aggregate, native parameter 0 is the sret
        // out-pointer; the MIR parameters start at native index 1.
        let returns_aggregate = matches!(
            classify_storage(&self.function.ret, self.types, &self.function.name)?,
            Storage::Aggregate(_)
        );
        let mut native = u32::from(returns_aggregate);

        // Seed each MIR parameter local from its native parameter. A scalar is
        // stored into its slot; an aggregate parameter's incoming pointer is
        // memcpy'd into the callee's own slot (owned-copy semantics); a unit
        // parameter occupies no native slot.
        for index in 0..self.function.params.len() {
            match &self.kinds[index] {
                LocalKind::Scalar(ptr, _) => {
                    let ptr = *ptr;
                    let param = self
                        .value
                        .get_nth_param(native)
                        .ok_or_else(|| CodegenError::backend("missing LLVM parameter"))?
                        .into_int_value();
                    self.builder
                        .build_store(ptr, param)
                        .map_err(builder_err("storing a parameter"))?;
                    native += 1;
                }
                LocalKind::Unit => {}
                LocalKind::Aggregate { ptr, ty } => {
                    let dest = *ptr;
                    let ty = ty.clone();
                    let src = self
                        .value
                        .get_nth_param(native)
                        .ok_or_else(|| CodegenError::backend("missing LLVM parameter"))?
                        .into_pointer_value();
                    native += 1;
                    let layout = self.layout(&ty)?;
                    self.emit_memcpy(dest, src, layout)?;
                }
            }
        }

        // Lower each block's statements and terminator. Block 0's body follows
        // the allocas/parameter stores in the same LLVM block.
        for (index, block) in self.function.blocks.iter().enumerate() {
            self.builder.position_at_end(self.blocks[index]);
            for statement in &block.statements {
                self.lower_statement(statement)?;
            }
            self.lower_terminator(&block.terminator)?;
        }
        Ok(())
    }

    // ----- statements -----

    fn lower_statement(&mut self, statement: &Statement) -> Result<(), CodegenError> {
        match statement {
            Statement::Assign { place, rvalue } => self.lower_assign(place, rvalue),
            Statement::Call { dest, callee, args } => self.lower_call(dest, *callee, args),
            Statement::Drop { .. } => {
                // v0 supported values own no host resource; a Stage-1 aggregate
                // owns no heap (scalar fields only), so its drop is a no-op too,
                // exactly like a scalar's. Nothing to emit.
                Ok(())
            }
        }
    }

    /// Lower `place = rvalue`, splitting aggregate from scalar exactly as the
    /// Cranelift backend does.
    fn lower_assign(&mut self, place: &Place, rvalue: &Rvalue) -> Result<(), CodegenError> {
        match rvalue {
            Rvalue::Aggregate { kind, fields } => self.lower_aggregate(place, kind, fields),
            // A unit-valued copy/move (a unit local, or a zero-sized aggregate
            // such as `[T; 0]`) carries no bytes: nothing to emit.
            Rvalue::Use(operand) if self.operand_is_unit(operand) => Ok(()),
            Rvalue::Use(operand) if self.operand_is_aggregate(operand) => {
                let (dest_addr, dest_ty) = self.place_address(place)?;
                let layout = self.layout(&dest_ty)?;
                let src_addr = self.operand_aggregate_address(operand)?;
                self.emit_memcpy(dest_addr, src_addr, layout)
            }
            _ => {
                let value = self.lower_rvalue(rvalue)?;
                self.write_place(place, value)
            }
        }
    }

    fn lower_call(
        &mut self,
        dest: &Place,
        callee: SymbolId,
        args: &[tuo_mir::Arg],
    ) -> Result<(), CodegenError> {
        let Some(&callee_fn) = self.ids.get(&callee) else {
            return Err(CodegenError::unsupported(
                "call to a function outside the lowered program (v0 has no external calls)",
            ));
        };
        let callee_mir = self.function_named(callee)?;
        let dest_ty = self.place_type(dest);
        let ret_is_aggregate = matches!(
            classify_storage(&callee_mir.ret, self.types, &callee_mir.name)?,
            Storage::Aggregate(_)
        );

        let mut arg_values: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
            Vec::with_capacity(args.len() + 1);

        // sret out-pointer, if the callee returns an aggregate.
        let sret = if ret_is_aggregate {
            let layout = self.layout(&callee_mir.ret)?;
            let (ptr, addr) = self.sret_destination(dest, &dest_ty, layout)?;
            arg_values.push(addr.into());
            Some((ptr, layout))
        } else {
            None
        };

        for arg in args {
            match arg {
                tuo_mir::Arg::Value(operand) => {
                    if self.operand_is_unit(operand) {
                        // A unit argument carries no native value.
                    } else if self.operand_is_aggregate(operand) {
                        let addr = self.materialize_aggregate_arg(operand)?;
                        arg_values.push(addr.into());
                    } else {
                        arg_values.push(self.lower_operand(operand)?.into());
                    }
                }
                tuo_mir::Arg::Borrow(_) | tuo_mir::Arg::BorrowMut(_) => {
                    return Err(CodegenError::unsupported(
                        "borrow-mode call arguments are not lowered by the LLVM backend yet",
                    ));
                }
            }
        }

        let call = self
            .builder
            .build_call(callee_fn, &arg_values, "call")
            .map_err(builder_err("emitting a call"))?;

        if let Some((ptr, layout)) = sret {
            // The callee wrote the aggregate result into the sret slot. If dest
            // is projected, copy from the temporary into the projected address.
            if !dest.projection.is_empty() {
                let (dest_addr, _leaf) = self.place_address(dest)?;
                self.emit_memcpy(dest_addr, ptr, layout)?;
            }
        } else if let inkwell::values::ValueKind::Basic(value) = call.try_as_basic_value() {
            // A unit-returning callee yields no result; a scalar callee yields
            // one, stored into the (scalar) destination.
            self.write_place(dest, value.into_int_value())?;
        }
        Ok(())
    }

    /// The sret destination for a call returning an aggregate: `(ptr, address)`.
    /// A bare aggregate local reuses its own alloca; a projected destination gets
    /// a fresh temporary alloca copied into the projected address after the call.
    fn sret_destination(
        &mut self,
        dest: &Place,
        _dest_ty: &Ty,
        layout: Layout,
    ) -> Result<(PointerValue<'ctx>, PointerValue<'ctx>), CodegenError> {
        if dest.projection.is_empty() {
            if let LocalKind::Aggregate { ptr, .. } = &self.kinds[dest.local.0 as usize] {
                let ptr = *ptr;
                return Ok((ptr, ptr));
            }
        }
        let ptr = self.alloca_aggregate(layout, usize::MAX)?;
        Ok((ptr, ptr))
    }

    /// Materialize an aggregate call argument into a caller-owned temporary alloca
    /// and return its pointer, per the by-pointer call ABI.
    fn materialize_aggregate_arg(
        &mut self,
        operand: &Operand,
    ) -> Result<PointerValue<'ctx>, CodegenError> {
        let ty = self
            .operand_ty(operand)
            .ok_or_else(|| CodegenError::backend("aggregate argument has no static type"))?;
        let layout = self.layout(&ty)?;
        let src = self.operand_aggregate_address(operand)?;
        let dest = self.alloca_aggregate(layout, usize::MAX)?;
        self.emit_memcpy(dest, src, layout)?;
        Ok(dest)
    }

    /// The MIR function with symbol `callee`, for its return type and name.
    fn function_named(&self, callee: SymbolId) -> Result<&'a Function, CodegenError> {
        self.functions
            .iter()
            .find(|f| f.symbol == callee)
            .ok_or_else(|| {
                CodegenError::unsupported(
                    "call to a function outside the lowered program (v0 has no external calls)",
                )
            })
    }

    // ----- terminators -----

    fn lower_terminator(&mut self, terminator: &Terminator) -> Result<(), CodegenError> {
        match terminator {
            Terminator::Return(operand) => {
                match classify_storage(&self.function.ret, self.types, &self.function.name)? {
                    Storage::Unit => {
                        self.builder
                            .build_return(None)
                            .map_err(builder_err("emitting a unit return"))?;
                    }
                    Storage::Scalar => {
                        let value = self.lower_operand(operand)?;
                        self.builder
                            .build_return(Some(&value as &dyn BasicValue))
                            .map_err(builder_err("emitting a return"))?;
                    }
                    Storage::Aggregate(layout) => {
                        // Copy the operand's aggregate into the sret out-pointer
                        // (native parameter 0), then return void.
                        let src = self.operand_aggregate_address(operand)?;
                        let sret = self
                            .value
                            .get_nth_param(0)
                            .ok_or_else(|| CodegenError::backend("missing sret parameter"))?
                            .into_pointer_value();
                        self.emit_memcpy(sret, src, layout)?;
                        self.builder
                            .build_return(None)
                            .map_err(builder_err("emitting an aggregate return"))?;
                    }
                }
                Ok(())
            }
            Terminator::Goto(target) => {
                let block = self.blocks[target.0 as usize];
                self.builder
                    .build_unconditional_branch(block)
                    .map_err(builder_err("emitting a goto"))?;
                Ok(())
            }
            Terminator::Branch {
                cond,
                then_block,
                else_block,
            } => {
                let value = self.lower_operand(cond)?;
                // The condition is a `Bool` (i8, 0/1); LLVM branches want an i1.
                let cond_i1 = self.truthy(value)?;
                let then_b = self.blocks[then_block.0 as usize];
                let else_b = self.blocks[else_block.0 as usize];
                self.builder
                    .build_conditional_branch(cond_i1, then_b, else_b)
                    .map_err(builder_err("emitting a branch"))?;
                Ok(())
            }
            Terminator::Switch {
                discr,
                arms,
                otherwise,
            } => {
                let value = self.lower_operand(discr)?;
                let otherwise_block = self.blocks[otherwise.0 as usize];
                let cases: Vec<(IntValue<'ctx>, LlvmBlock<'ctx>)> = arms
                    .iter()
                    .map(|(case_value, target)| {
                        // Each case constant has the discriminant's own type; MIR
                        // guarantees arm values fit that type.
                        let konst = value.get_type().const_int(*case_value as u64, false);
                        (konst, self.blocks[target.0 as usize])
                    })
                    .collect();
                self.builder
                    .build_switch(value, otherwise_block, &cases)
                    .map_err(builder_err("emitting a switch"))?;
                Ok(())
            }
            Terminator::Assert { cond, trap, target } => {
                let value = self.lower_operand(cond)?;
                let cond_i1 = self.truthy(value)?;
                let ok_block = self.blocks[target.0 as usize];
                let trap_block = self.ctx.append_basic_block(self.value, "assert_fail");
                self.builder
                    .build_conditional_branch(cond_i1, ok_block, trap_block)
                    .map_err(builder_err("emitting an assert"))?;
                self.builder.position_at_end(trap_block);
                self.emit_trap_call(trap_code_of(*trap))?;
                Ok(())
            }
            Terminator::Trap(trap) => self.emit_trap_call(trap_code_of(*trap)),
        }
    }

    // ----- rvalues & operands -----

    fn lower_rvalue(&mut self, rvalue: &Rvalue) -> Result<IntValue<'ctx>, CodegenError> {
        match rvalue {
            Rvalue::Use(operand) => self.lower_operand(operand),
            Rvalue::Unary { op, operand } => {
                let value = self.lower_operand(operand)?;
                self.lower_unary(*op, operand, value)
            }
            Rvalue::Binary { op, lhs, rhs } => {
                let l = self.lower_operand(lhs)?;
                let r = self.lower_operand(rhs)?;
                self.lower_binary(*op, lhs, l, r)
            }
            Rvalue::Cast { kind, operand, to } => {
                let value = self.lower_operand(operand)?;
                self.lower_cast(*kind, operand, value, to)
            }
            Rvalue::Aggregate { .. } => Err(CodegenError::backend(
                "aggregate construction reached the scalar rvalue path",
            )),
            Rvalue::Discriminant(place) => self.lower_discriminant(place),
            // `Len` applies only to the growable `Array[T]` (a `[T; N]`'s
            // length is lowered as a constant and the MIR verifier rejects
            // `Len` of a fixed-array place), so refusing it entirely stays
            // sound for fixed arrays.
            Rvalue::Len(_) => Err(CodegenError::unsupported(
                "the growable `Array[T]` (and its `Len`) is not lowered by the LLVM backend yet",
            )),
        }
    }

    fn lower_operand(&mut self, operand: &Operand) -> Result<IntValue<'ctx>, CodegenError> {
        match operand {
            Operand::Copy(place) | Operand::Move(place) => self.read_place(place),
            Operand::Const(constant) => self.lower_const(constant),
        }
    }

    fn lower_const(&mut self, constant: &Const) -> Result<IntValue<'ctx>, CodegenError> {
        match constant {
            Const::Bool(b) => Ok(self.ctx.i8_type().const_int(u64::from(*b), false)),
            Const::Char(c) => Ok(self
                .ctx
                .i32_type()
                .const_int(u64::from(u32::from(*c)), false)),
            Const::Int(value, kind) => {
                let ty = int_type(self.ctx, *kind);
                // Reinterpret the mathematical value's low bits at the target
                // width; MIR guarantees it is in range for the kind. `const_int`
                // takes the low 64 bits and (with sign_extend=false) zero-fills
                // above the type width, so the stored bit pattern is exactly the
                // value's two's-complement representation at that width.
                Ok(ty.const_int(*value as u64, false))
            }
            Const::Unit => Err(CodegenError::unsupported(
                "a unit constant has no scalar representation to place in a value context",
            )),
            Const::Float(..) => Err(CodegenError::unsupported(
                "float constants are not lowered by the LLVM backend yet",
            )),
            Const::Str(_) => Err(CodegenError::unsupported(
                "string constants are not lowered by the LLVM backend yet",
            )),
        }
    }

    // ----- arithmetic -----

    fn lower_unary(
        &mut self,
        op: UnOp,
        operand: &Operand,
        value: IntValue<'ctx>,
    ) -> Result<IntValue<'ctx>, CodegenError> {
        match op {
            UnOp::Not => {
                // Boolean negation: xor with 1 (values are 0/1).
                let one = value.get_type().const_int(1, false);
                self.builder
                    .build_xor(value, one, "not")
                    .map_err(builder_err("emitting boolean not"))
            }
            UnOp::Neg => {
                let kind = self.operand_int_kind(operand)?;
                // Integer negation traps on MIN (two's complement, §24): -MIN is
                // not representable. Detect it as `value == MIN`.
                let ty = int_type(self.ctx, kind);
                let min = min_const(ty, kind);
                let is_min = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, value, min, "is_min")
                    .map_err(builder_err("comparing against MIN"))?;
                self.guard(is_min, TrapCode::IntegerOverflow)?;
                self.builder
                    .build_int_neg(value, "neg")
                    .map_err(builder_err("emitting negation"))
            }
        }
    }

    fn lower_binary(
        &mut self,
        op: BinOp,
        lhs: &Operand,
        l: IntValue<'ctx>,
        r: IntValue<'ctx>,
    ) -> Result<IntValue<'ctx>, CodegenError> {
        // Comparisons on chars/bools/ints all reduce to integer compares; the
        // arithmetic operators are integer-only in the supported subset.
        if let Some(pred) = comparison_pred(op, self.operand_signed(lhs)) {
            let cmp = self
                .builder
                .build_int_compare(pred, l, r, "cmp")
                .map_err(builder_err("emitting a comparison"))?;
            // `icmp` yields an i1; widen to the backend's `Bool` (i8, 0/1) so it
            // shares the scalar representation the rest of the lowering expects.
            return self
                .builder
                .build_int_z_extend(cmp, self.ctx.i8_type(), "cmp_bool")
                .map_err(builder_err("widening a comparison result"));
        }

        let kind = self.operand_int_kind(lhs)?;
        match op {
            BinOp::Add => self.checked_arith(kind, l, r, ArithOp::Add),
            BinOp::Sub => self.checked_arith(kind, l, r, ArithOp::Sub),
            BinOp::Mul => self.checked_arith(kind, l, r, ArithOp::Mul),
            BinOp::Div => self.checked_divrem(kind, l, r, true),
            BinOp::Rem => self.checked_divrem(kind, l, r, false),
            // Comparisons handled above; reaching here is impossible.
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => Err(
                CodegenError::backend("comparison fell through arithmetic path"),
            ),
        }
    }

    /// Lower a trapping add/sub/mul: use LLVM's overflow-checking intrinsic,
    /// trap if the overflow flag is set, and yield the wrapped result. This is
    /// the same behavior the Cranelift backend gets from its overflow-flagged
    /// instructions, so both agree with the interpreter.
    fn checked_arith(
        &mut self,
        kind: IntKind,
        l: IntValue<'ctx>,
        r: IntValue<'ctx>,
        op: ArithOp,
    ) -> Result<IntValue<'ctx>, CodegenError> {
        let signed = is_signed(kind);
        let ty = int_type(self.ctx, kind);
        let intrinsic_name = match (op, signed) {
            (ArithOp::Add, true) => "llvm.sadd.with.overflow",
            (ArithOp::Add, false) => "llvm.uadd.with.overflow",
            (ArithOp::Sub, true) => "llvm.ssub.with.overflow",
            (ArithOp::Sub, false) => "llvm.usub.with.overflow",
            (ArithOp::Mul, true) => "llvm.smul.with.overflow",
            (ArithOp::Mul, false) => "llvm.umul.with.overflow",
        };
        let intrinsic = Intrinsic::find(intrinsic_name).ok_or_else(|| {
            CodegenError::backend(format!("missing LLVM intrinsic {intrinsic_name}"))
        })?;
        let decl = intrinsic
            .get_declaration(self.module, &[ty.into()])
            .ok_or_else(|| {
                CodegenError::backend(format!("declaring LLVM intrinsic {intrinsic_name}"))
            })?;
        let call = self
            .builder
            .build_call(decl, &[l.into(), r.into()], "arith")
            .map_err(builder_err("calling an overflow intrinsic"))?;
        let agg = call.try_as_basic_value().unwrap_basic().into_struct_value();
        let result = self
            .builder
            .build_extract_value(agg, 0, "arith_val")
            .map_err(builder_err("extracting the arithmetic result"))?
            .into_int_value();
        let overflow = self
            .builder
            .build_extract_value(agg, 1, "arith_ovf")
            .map_err(builder_err("extracting the overflow flag"))?
            .into_int_value();
        self.guard(overflow, TrapCode::IntegerOverflow)?;
        Ok(result)
    }

    /// Lower a trapping div (`is_div`) or rem: trap on a zero divisor, and on
    /// the signed `MIN / -1` overflow case (matching the interpreter).
    fn checked_divrem(
        &mut self,
        kind: IntKind,
        l: IntValue<'ctx>,
        r: IntValue<'ctx>,
        is_div: bool,
    ) -> Result<IntValue<'ctx>, CodegenError> {
        let ty = int_type(self.ctx, kind);
        let signed = is_signed(kind);

        // Divisor == 0 → division-by-zero trap.
        let zero = ty.const_zero();
        let is_zero = self
            .builder
            .build_int_compare(IntPredicate::EQ, r, zero, "is_zero")
            .map_err(builder_err("comparing the divisor to zero"))?;
        self.guard(is_zero, TrapCode::DivisionByZero)?;

        if signed {
            // MIN / -1 (and MIN % -1) overflow → integer-overflow trap.
            let min = min_const(ty, kind);
            let neg_one = ty.const_all_ones();
            let l_is_min = self
                .builder
                .build_int_compare(IntPredicate::EQ, l, min, "l_is_min")
                .map_err(builder_err("comparing the dividend to MIN"))?;
            let r_is_neg_one = self
                .builder
                .build_int_compare(IntPredicate::EQ, r, neg_one, "r_is_neg1")
                .map_err(builder_err("comparing the divisor to -1"))?;
            let both = self
                .builder
                .build_and(l_is_min, r_is_neg_one, "min_div_neg1")
                .map_err(builder_err("combining the overflow conditions"))?;
            self.guard(both, TrapCode::IntegerOverflow)?;
            if is_div {
                self.builder
                    .build_int_signed_div(l, r, "sdiv")
                    .map_err(builder_err("emitting signed division"))
            } else {
                self.builder
                    .build_int_signed_rem(l, r, "srem")
                    .map_err(builder_err("emitting signed remainder"))
            }
        } else if is_div {
            self.builder
                .build_int_unsigned_div(l, r, "udiv")
                .map_err(builder_err("emitting unsigned division"))
        } else {
            self.builder
                .build_int_unsigned_rem(l, r, "urem")
                .map_err(builder_err("emitting unsigned remainder"))
        }
    }

    fn lower_cast(
        &mut self,
        kind: CastKind,
        operand: &Operand,
        value: IntValue<'ctx>,
        to: &Ty,
    ) -> Result<IntValue<'ctx>, CodegenError> {
        match kind {
            CastKind::IntToInt => {
                let Ty::Int(target) = to else {
                    return Err(CodegenError::backend("int-to-int cast to a non-integer"));
                };
                let from = self.operand_int_kind(operand)?;
                self.resize_int(value, from, *target)
            }
            CastKind::IntToFloat | CastKind::FloatToInt | CastKind::FloatToFloat => Err(
                CodegenError::unsupported("float casts are not lowered by the LLVM backend yet"),
            ),
        }
    }

    /// Resize an integer from `from` to `target` width with two's-complement
    /// wrapping (truncate when narrowing; sign/zero-extend from the source's
    /// signedness when widening) — matching the interpreter's `wrap_int` and the
    /// Cranelift backend.
    fn resize_int(
        &mut self,
        value: IntValue<'ctx>,
        from: IntKind,
        target: IntKind,
    ) -> Result<IntValue<'ctx>, CodegenError> {
        let from_bits = int_width_bits(from);
        let to_bits = int_width_bits(target);
        let to_ty = int_type(self.ctx, target);
        if to_bits == from_bits {
            Ok(value)
        } else if to_bits < from_bits {
            self.builder
                .build_int_truncate(value, to_ty, "trunc")
                .map_err(builder_err("truncating an integer cast"))
        } else if is_signed(from) {
            self.builder
                .build_int_s_extend(value, to_ty, "sext")
                .map_err(builder_err("sign-extending an integer cast"))
        } else {
            self.builder
                .build_int_z_extend(value, to_ty, "zext")
                .map_err(builder_err("zero-extending an integer cast"))
        }
    }

    // ----- places -----

    /// Read the scalar value of `place`. An empty projection on a scalar local
    /// loads its alloca; a non-empty projection resolves to a leaf byte address
    /// (a scalar leaf in Stage 1) and loads it.
    fn read_place(&mut self, place: &Place) -> Result<IntValue<'ctx>, CodegenError> {
        if place.projection.is_empty() {
            let index = place.local.0 as usize;
            return match &self.kinds[index] {
                LocalKind::Scalar(ptr, ty) => Ok(self
                    .builder
                    .build_load(*ty, *ptr, "load")
                    .map_err(builder_err("loading a local"))?
                    .into_int_value()),
                LocalKind::Unit => Err(CodegenError::backend(
                    "reading a scalar value from a unit local",
                )),
                LocalKind::Aggregate { .. } => Err(CodegenError::backend(
                    "reading a whole aggregate as a scalar (should go through the aggregate path)",
                )),
            };
        }
        let (addr, leaf_ty) = self.place_address(place)?;
        let int_ty = require_scalar(self.ctx, &leaf_ty, &self.function.name)?;
        Ok(self
            .builder
            .build_load(int_ty, addr, "fload")
            .map_err(builder_err("loading a projected field"))?
            .into_int_value())
    }

    /// Write the scalar `value` to `place`. An empty projection on a scalar local
    /// stores its alloca; a non-empty projection resolves to a leaf byte address
    /// (a scalar leaf in Stage 1) and stores there. A unit destination carries no
    /// value.
    fn write_place(&mut self, place: &Place, value: IntValue<'ctx>) -> Result<(), CodegenError> {
        if place.projection.is_empty() {
            let index = place.local.0 as usize;
            return match &self.kinds[index] {
                LocalKind::Scalar(ptr, _) => {
                    self.builder
                        .build_store(*ptr, value)
                        .map_err(builder_err("storing a local"))?;
                    Ok(())
                }
                LocalKind::Unit => Ok(()),
                LocalKind::Aggregate { .. } => Err(CodegenError::backend(
                    "writing a scalar into a whole aggregate local (should go through the \
                     aggregate path)",
                )),
            };
        }
        let (addr, _leaf_ty) = self.place_address(place)?;
        self.builder
            .build_store(addr, value)
            .map_err(builder_err("storing a projected field"))?;
        Ok(())
    }

    /// The shared address walk: resolve `place` to a `(byte address, leaf type)`
    /// pair by GEP-ing each projection's ABI offset from the root aggregate's
    /// pointer. Byte-identical in structure to the Cranelift backend's
    /// `place_address`. A `Field`/`VariantField` step advances by a constant
    /// ABI offset; an `Index` step advances by the runtime index value times
    /// the element stride — **unchecked**, because MIR asserts the bounds
    /// (`Assert { IndexOutOfBounds }`) before every `Index` use, exactly like
    /// the interpreter's post-check access.
    fn place_address(&mut self, place: &Place) -> Result<(PointerValue<'ctx>, Ty), CodegenError> {
        let local = place.local.0 as usize;
        let (mut addr, mut cur_ty) = match &self.kinds[local] {
            LocalKind::Aggregate { ptr, ty } => (*ptr, ty.clone()),
            // A zero-sized aggregate root (e.g. `[T; 0]`) classifies as unit
            // storage yet may still be projected in MIR — only in code the
            // preceding bounds `Assert` makes unreachable at runtime (any index
            // into a zero-length array traps first). Give it a null base; no
            // load/store computed from it can ever execute.
            LocalKind::Unit if !place.projection.is_empty() => (
                self.ptr_ty.const_null(),
                self.function.locals[local].ty.clone(),
            ),
            _ => {
                return Err(CodegenError::backend("projection on a non-aggregate local"));
            }
        };

        for step in &place.projection {
            match step {
                Projection::Field(index) => {
                    let offsets = struct_field_offsets(&cur_ty, self.types)
                        .map_err(|error| self.layout_error(error))?;
                    let i = *index as usize;
                    let offset = *offsets.get(i).ok_or_else(|| {
                        CodegenError::backend("struct/tuple field index out of range")
                    })?;
                    cur_ty = self.field_type(&cur_ty, i)?;
                    addr = self.byte_gep(addr, offset)?;
                }
                Projection::VariantField { variant, field } => {
                    let offsets = variant_field_offsets(&cur_ty, *variant as usize, self.types)
                        .map_err(|error| self.layout_error(error))?;
                    let f = *field as usize;
                    let offset = *offsets
                        .get(f)
                        .ok_or_else(|| CodegenError::backend("variant field index out of range"))?;
                    cur_ty = self.variant_field_type(&cur_ty, *variant as usize, f)?;
                    addr = self.byte_gep(addr, offset)?;
                }
                Projection::Index(index_local) => {
                    // Only the fixed `[T; N]` is indexable natively; element
                    // `i` lives at `i × stride(T)` (the ABI's inline layout).
                    let element = match &cur_ty {
                        Ty::FixedArray(element, _) => (**element).clone(),
                        _ => {
                            return Err(CodegenError::unsupported(
                                "indexing the growable `Array[T]` is not lowered by the LLVM \
                                 backend (only the fixed `[T; N]` is)",
                            ));
                        }
                    };
                    let stride = self.layout(&element)?.stride();
                    // The index is a `Usize` scalar local, pre-asserted in
                    // bounds by the MIR before this use.
                    let index_value = match &self.kinds[index_local.0 as usize] {
                        LocalKind::Scalar(ptr, int_ty) => self
                            .builder
                            .build_load(*int_ty, *ptr, "idx")
                            .map_err(builder_err("loading an array index"))?
                            .into_int_value(),
                        _ => {
                            return Err(CodegenError::backend("array index local is not a scalar"));
                        }
                    };
                    addr = self.dynamic_byte_gep(addr, index_value, stride)?;
                    cur_ty = element;
                }
            }
        }
        Ok((addr, cur_ty))
    }

    /// Advance a byte pointer by `offset` bytes. Computed as
    /// `inttoptr(ptrtoint(base) + offset)` — pointer arithmetic without an
    /// `unsafe` GEP (`unsafe_code` is forbidden workspace-wide). The offset is a
    /// byte displacement from the ABI, so no LLVM struct type is needed; the
    /// result addresses the same bytes an i8-GEP would.
    fn byte_gep(
        &mut self,
        base: PointerValue<'ctx>,
        offset: u64,
    ) -> Result<PointerValue<'ctx>, CodegenError> {
        if offset == 0 {
            return Ok(base);
        }
        // Pointer-width integer (POINTER_SIZE = 8 on the supported hosts).
        let usize_ty = self.ctx.i64_type();
        let base_int = self
            .builder
            .build_ptr_to_int(base, usize_ty, "base_int")
            .map_err(builder_err("converting a base pointer to an integer"))?;
        let off = usize_ty.const_int(offset, false);
        let sum = self
            .builder
            .build_int_add(base_int, off, "field_int")
            .map_err(builder_err("adding a field offset"))?;
        self.builder
            .build_int_to_ptr(sum, self.ptr_ty, "field")
            .map_err(builder_err("converting a field integer back to a pointer"))
    }

    /// Advance a byte pointer by a **runtime** element index times a constant
    /// `stride`. The dynamic counterpart of [`Self::byte_gep`], used only by
    /// `Projection::Index`: `inttoptr(ptrtoint(base) + index × stride)`.
    fn dynamic_byte_gep(
        &mut self,
        base: PointerValue<'ctx>,
        index: IntValue<'ctx>,
        stride: u64,
    ) -> Result<PointerValue<'ctx>, CodegenError> {
        let usize_ty = self.ctx.i64_type();
        let base_int = self
            .builder
            .build_ptr_to_int(base, usize_ty, "base_int")
            .map_err(builder_err("converting a base pointer to an integer"))?;
        let stride = usize_ty.const_int(stride, false);
        let scaled = self
            .builder
            .build_int_mul(index, stride, "elem_off")
            .map_err(builder_err("scaling an array index by the element stride"))?;
        let sum = self
            .builder
            .build_int_add(base_int, scaled, "elem_int")
            .map_err(builder_err("adding an element offset"))?;
        self.builder
            .build_int_to_ptr(sum, self.ptr_ty, "elem")
            .map_err(builder_err(
                "converting an element integer back to a pointer",
            ))
    }

    /// Materialize a `Rvalue::Aggregate` in place into `dest`'s byte storage.
    /// Byte-identical to the Cranelift backend's `lower_aggregate`. A
    /// fixed-array aggregate takes its own path (element `i` at `i × stride`).
    fn lower_aggregate(
        &mut self,
        dest: &Place,
        kind: &tuo_mir::AggregateKind,
        fields: &[Operand],
    ) -> Result<(), CodegenError> {
        let (ty, variant) = match kind {
            tuo_mir::AggregateKind::Adt { ty, variant } => (ty.clone(), *variant),
            tuo_mir::AggregateKind::Range => {
                return Err(CodegenError::unsupported(
                    "range construction is not lowered by the LLVM backend (Stage 2)",
                ));
            }
            tuo_mir::AggregateKind::Array { element, len } => {
                return self.lower_array_aggregate(dest, element, *len, fields);
            }
        };

        let base = self.aggregate_dest_address(dest)?;

        // Enum-like: store the u32 tag at offset 0. Struct/tuple: no tag.
        let enum_like = matches!(ty, Ty::Enum(..) | Ty::Option(_) | Ty::Result(..));
        if enum_like {
            let tag = self.ctx.i32_type().const_int(u64::from(variant), false);
            self.builder
                .build_store(base, tag)
                .map_err(builder_err("storing an enum discriminant"))?;
        }

        let offsets = if enum_like {
            variant_field_offsets(&ty, variant as usize, self.types)
                .map_err(|error| self.layout_error(error))?
        } else {
            struct_field_offsets(&ty, self.types).map_err(|error| self.layout_error(error))?
        };
        if fields.len() != offsets.len() {
            return Err(CodegenError::backend(
                "aggregate field count does not match the ABI offset count",
            ));
        }

        for (i, operand) in fields.iter().enumerate() {
            if self.operand_is_unit(operand) {
                continue;
            }
            let field_addr = self.byte_gep(base, offsets[i])?;
            if self.operand_is_aggregate(operand) {
                let field_ty = self
                    .operand_ty(operand)
                    .ok_or_else(|| CodegenError::backend("aggregate field has no static type"))?;
                let layout = self.layout(&field_ty)?;
                let src = self.operand_aggregate_address(operand)?;
                self.emit_memcpy(field_addr, src, layout)?;
            } else {
                let value = self.lower_operand(operand)?;
                self.builder
                    .build_store(field_addr, value)
                    .map_err(builder_err("storing an aggregate field"))?;
            }
        }
        Ok(())
    }

    /// Materialize an `AggregateKind::Array { element, len }` — a fixed-size
    /// array value `[element; len]` — into `dest`'s byte storage: the operands
    /// are the `len` elements in index order, and element `i` is stored at byte
    /// offset `i × stride(element)` per the runtime ABI's inline layout
    /// (`abi::layout_of`; no header, no allocation). A scalar element is a
    /// scalar store; a nested-aggregate element is a whole-aggregate memcpy —
    /// mirroring `lower_aggregate`'s field dispatch. Byte-identical to the
    /// Cranelift backend's `lower_array_aggregate`.
    fn lower_array_aggregate(
        &mut self,
        dest: &Place,
        element: &Ty,
        len: u64,
        fields: &[Operand],
    ) -> Result<(), CodegenError> {
        if fields.len() as u64 != len {
            return Err(CodegenError::backend(
                "fixed-array operand count does not match its length",
            ));
        }
        let elem_layout = self.layout(element)?;
        let stride = elem_layout.stride();
        // A zero-sized array (`len == 0`, or a zero-sized element type) has no
        // bytes to write and its destination classifies as unit storage — the
        // construction is a no-op (element operands are `Copy`/`Move`/`Const`
        // places, which carry no side effect to preserve).
        if len == 0 || stride == 0 {
            return Ok(());
        }
        let base = self.aggregate_dest_address(dest)?;
        for (i, operand) in fields.iter().enumerate() {
            let offset = (i as u64)
                .checked_mul(stride)
                .ok_or_else(|| CodegenError::backend("array element offset overflows"))?;
            let element_addr = self.byte_gep(base, offset)?;
            if self.operand_is_aggregate(operand) {
                let src = self.operand_aggregate_address(operand)?;
                self.emit_memcpy(element_addr, src, elem_layout)?;
            } else {
                let value = self.lower_operand(operand)?;
                self.builder
                    .build_store(element_addr, value)
                    .map_err(builder_err("storing an array element"))?;
            }
        }
        Ok(())
    }

    /// The base address to materialize an aggregate into, for an `Assign` whose
    /// destination is an aggregate.
    fn aggregate_dest_address(&mut self, dest: &Place) -> Result<PointerValue<'ctx>, CodegenError> {
        if dest.projection.is_empty() {
            if let LocalKind::Aggregate { ptr, .. } = &self.kinds[dest.local.0 as usize] {
                return Ok(*ptr);
            }
            return Err(CodegenError::backend(
                "aggregate rvalue assigned to a non-aggregate local",
            ));
        }
        let (addr, _leaf) = self.place_address(dest)?;
        Ok(addr)
    }

    /// Lower `Rvalue::Discriminant`: load the u32 tag at offset 0 and zero-extend
    /// to a `Usize` (i64), matching the interpreter's `Value::Int(discr, Usize)`.
    fn lower_discriminant(&mut self, place: &Place) -> Result<IntValue<'ctx>, CodegenError> {
        let base = if place.projection.is_empty() {
            match &self.kinds[place.local.0 as usize] {
                LocalKind::Aggregate { ptr, .. } => *ptr,
                _ => {
                    return Err(CodegenError::backend(
                        "discriminant of a non-aggregate local",
                    ));
                }
            }
        } else {
            let (addr, _leaf) = self.place_address(place)?;
            addr
        };
        let tag = self
            .builder
            .build_load(self.ctx.i32_type(), base, "discr")
            .map_err(builder_err("loading a discriminant"))?
            .into_int_value();
        self.builder
            .build_int_z_extend(tag, self.ctx.i64_type(), "discr_usize")
            .map_err(builder_err("zero-extending a discriminant"))
    }

    // ----- traps -----

    /// If `condition` (an i1 or i8 boolean) is true, trap with `code`: branch to
    /// a fresh trap block that calls the runtime and reaches `unreachable`;
    /// otherwise continue in a fresh block. Leaves the builder positioned at the
    /// continuation.
    fn guard(&mut self, condition: IntValue<'ctx>, code: TrapCode) -> Result<(), CodegenError> {
        let cond_i1 = self.truthy(condition)?;
        let trap_block = self.ctx.append_basic_block(self.value, "trap");
        let continue_block = self.ctx.append_basic_block(self.value, "cont");
        self.builder
            .build_conditional_branch(cond_i1, trap_block, continue_block)
            .map_err(builder_err("branching to a trap check"))?;
        self.builder.position_at_end(trap_block);
        self.emit_trap_call(code)?;
        self.builder.position_at_end(continue_block);
        Ok(())
    }

    /// Emit a call to the runtime trap symbol with `code`, then `unreachable`
    /// (the call never returns). Terminates the current block.
    fn emit_trap_call(&mut self, code: TrapCode) -> Result<(), CodegenError> {
        let trap_fn = self.trap_function();
        let code_value = self.ctx.i32_type().const_int(code.as_i32() as u64, false);
        self.builder
            .build_call(trap_fn, &[code_value.into()], "")
            .map_err(builder_err("calling the runtime trap"))?;
        self.builder
            .build_unreachable()
            .map_err(builder_err("emitting unreachable after a trap"))?;
        Ok(())
    }

    /// Declare (once per module, on demand) and return the runtime trap symbol,
    /// a `void (i32)` external function.
    fn trap_function(&self) -> FunctionValue<'ctx> {
        if let Some(existing) = self.module.get_function(TRAP_SYMBOL) {
            return existing;
        }
        let fn_type = self
            .ctx
            .void_type()
            .fn_type(&[self.ctx.i32_type().into()], false);
        self.module
            .add_function(TRAP_SYMBOL, fn_type, Some(Linkage::External))
    }

    /// Coerce a scalar boolean value (an i1 or the backend's i8 `Bool`) to an
    /// i1 for use as a branch/select condition.
    fn truthy(&self, value: IntValue<'ctx>) -> Result<IntValue<'ctx>, CodegenError> {
        if value.get_type().get_bit_width() == 1 {
            return Ok(value);
        }
        let zero = value.get_type().const_zero();
        self.builder
            .build_int_compare(IntPredicate::NE, value, zero, "truthy")
            .map_err(builder_err("coercing a boolean to i1"))
    }

    // ----- operand type helpers -----

    /// The integer kind of a scalar operand, if it is an integer. Comparisons
    /// and arithmetic use it for signedness and bounds.
    fn operand_int_kind(&self, operand: &Operand) -> Result<IntKind, CodegenError> {
        match self.operand_ty(operand) {
            Some(Ty::Int(kind)) => Ok(kind),
            _ => Err(CodegenError::backend(
                "expected an integer operand in the arithmetic path",
            )),
        }
    }

    /// Whether an operand's integer type is signed (chars/bools compare
    /// unsigned).
    fn operand_signed(&self, operand: &Operand) -> bool {
        matches!(self.operand_ty(operand), Some(Ty::Int(kind)) if is_signed(kind))
    }

    /// The static type of an operand, from the local's declared type (following
    /// projections to the leaf field type) or the constant's kind. Non-primitive
    /// constants return `None`.
    fn operand_ty(&self, operand: &Operand) -> Option<Ty> {
        match operand {
            Operand::Copy(place) | Operand::Move(place) => Some(self.place_type(place)),
            Operand::Const(Const::Int(_, kind)) => Some(Ty::Int(*kind)),
            Operand::Const(Const::Bool(_)) => Some(Ty::Bool),
            Operand::Const(Const::Char(_)) => Some(Ty::Char),
            _ => None,
        }
    }

    // ----- aggregate helpers -----

    /// The declared type of `place`, following its projections to the leaf.
    fn place_type(&self, place: &Place) -> Ty {
        let mut cur = self.function.locals[place.local.0 as usize].ty.clone();
        for step in &place.projection {
            cur = match step {
                Projection::Field(i) => self.field_type(&cur, *i as usize).unwrap_or(Ty::Error),
                Projection::VariantField { variant, field } => self
                    .variant_field_type(&cur, *variant as usize, *field as usize)
                    .unwrap_or(Ty::Error),
                // Indexing yields the element type (growable or fixed; only
                // the fixed `[T; N]` is lowered, but the type walk is total).
                Projection::Index(_) => match cur {
                    Ty::Array(element) | Ty::FixedArray(element, _) => (*element).clone(),
                    _ => Ty::Error,
                },
            };
        }
        cur
    }

    /// The i-th field type of a struct/tuple `cur_ty` (declaration order).
    fn field_type(&self, cur_ty: &Ty, i: usize) -> Result<Ty, CodegenError> {
        match cur_ty {
            Ty::Tuple(fields) => fields
                .get(i)
                .cloned()
                .ok_or_else(|| CodegenError::backend("tuple field index out of range")),
            Ty::Struct(symbol, _) => {
                let shape = self
                    .types
                    .struct_shape(*symbol)
                    .ok_or_else(|| CodegenError::backend("field of an unknown struct"))?;
                shape
                    .fields
                    .get(i)
                    .map(|(_, ty)| ty.clone())
                    .ok_or_else(|| CodegenError::backend("struct field index out of range"))
            }
            _ => Err(CodegenError::backend(
                "field projection on a non-struct, non-tuple type",
            )),
        }
    }

    /// The payload field type of variant `variant`, field `field` of an
    /// enum-like `cur_ty`.
    fn variant_field_type(
        &self,
        cur_ty: &Ty,
        variant: usize,
        field: usize,
    ) -> Result<Ty, CodegenError> {
        match cur_ty {
            Ty::Enum(symbol, _) => {
                let shape = self
                    .types
                    .enum_shape(*symbol)
                    .ok_or_else(|| CodegenError::backend("variant field of an unknown enum"))?;
                let (_, payload) = shape
                    .variants
                    .get(variant)
                    .ok_or_else(|| CodegenError::backend("enum variant index out of range"))?;
                payload
                    .get(field)
                    .map(|(_, ty)| ty.clone())
                    .ok_or_else(|| CodegenError::backend("variant field index out of range"))
            }
            // `Some` = variant 0 (payload at field 0), `None` = variant 1
            // (empty), matching the interpreter/MIR and the runtime ABI.
            Ty::Option(inner) => match (variant, field) {
                (0, 0) => Ok((**inner).clone()),
                _ => Err(CodegenError::backend("bad Option variant field")),
            },
            Ty::Result(ok, err) => match (variant, field) {
                (0, 0) => Ok((**ok).clone()),
                (1, 0) => Ok((**err).clone()),
                _ => Err(CodegenError::backend("bad Result variant field")),
            },
            _ => Err(CodegenError::backend(
                "variant-field projection on a non-enum type",
            )),
        }
    }

    /// Whether an operand refers to an aggregate value: a place whose leaf type
    /// (the declared type of a bare local, or the projected leaf — e.g. a
    /// struct-typed array element) classifies as aggregate storage.
    fn operand_is_aggregate(&self, operand: &Operand) -> bool {
        match operand {
            Operand::Copy(place) | Operand::Move(place) => {
                matches!(
                    classify_storage(&self.place_type(place), self.types, &self.function.name),
                    Ok(Storage::Aggregate(_))
                )
            }
            _ => false,
        }
    }

    /// Whether an operand is unit-valued (a unit constant or a bare-local place
    /// of unit/zero-sized type).
    fn operand_is_unit(&self, operand: &Operand) -> bool {
        match operand {
            Operand::Const(Const::Unit) => true,
            Operand::Copy(place) | Operand::Move(place) if place.projection.is_empty() => {
                matches!(
                    classify_storage(
                        &self.function.locals[place.local.0 as usize].ty,
                        self.types,
                        &self.function.name,
                    ),
                    Ok(Storage::Unit)
                )
            }
            _ => false,
        }
    }

    /// The base pointer of the aggregate an operand names: a `Copy`/`Move` of a
    /// bare aggregate local, or of a projected place whose leaf is an aggregate
    /// (e.g. a struct-typed array element), resolved through the shared address
    /// walk. A move leaves the source husk untouched: a v0 aggregate owns no
    /// heap, so no zeroing is needed (the ownership checker forbids re-reading
    /// a moved value).
    fn operand_aggregate_address(
        &mut self,
        operand: &Operand,
    ) -> Result<PointerValue<'ctx>, CodegenError> {
        match operand {
            Operand::Copy(place) | Operand::Move(place) if place.projection.is_empty() => {
                match &self.kinds[place.local.0 as usize] {
                    LocalKind::Aggregate { ptr, .. } => Ok(*ptr),
                    _ => Err(CodegenError::backend(
                        "aggregate address requested for a non-aggregate local",
                    )),
                }
            }
            Operand::Copy(place) | Operand::Move(place) => {
                let (addr, _leaf) = self.place_address(place)?;
                Ok(addr)
            }
            _ => Err(CodegenError::backend(
                "aggregate address requested for a non-place operand",
            )),
        }
    }

    /// The runtime ABI layout of `ty`, or a backend error (the type was already
    /// classified as an aggregate, so this should not fail).
    fn layout(&self, ty: &Ty) -> Result<Layout, CodegenError> {
        layout_of(ty, self.types).map_err(|error| self.layout_error(error))
    }

    /// Turn an ABI layout error into a backend error.
    fn layout_error(&self, error: tuo_runtime::abi::LayoutError) -> CodegenError {
        CodegenError::backend(format!("ABI layout failed during lowering: {error}"))
    }

    /// Allocate an aggregate slot of `layout`: an `[size x i8]` alloca whose
    /// alignment is forced to the ABI alignment (LLVM's default alloca align may
    /// be smaller, which would make aligned field loads UB under O2). `index` is
    /// only for a readable name.
    fn alloca_aggregate(
        &self,
        layout: Layout,
        index: usize,
    ) -> Result<PointerValue<'ctx>, CodegenError> {
        let i8 = self.ctx.i8_type();
        // The element count must be a pointer-width constant: an i8-typed count
        // would silently truncate any aggregate larger than 255 bytes.
        let size = self.ctx.i64_type().const_int(layout.size, false);
        let name = if index == usize::MAX {
            "agg_tmp".to_owned()
        } else {
            format!("agg{index}")
        };
        let ptr = self
            .builder
            .build_array_alloca(i8, size, &name)
            .map_err(builder_err("allocating an aggregate slot"))?;
        // Force the alloca's alignment to the ABI alignment.
        let align = u32::try_from(layout.align)
            .map_err(|_| CodegenError::backend("aggregate alignment exceeds u32"))?;
        ptr.as_instruction()
            .ok_or_else(|| CodegenError::backend("aggregate alloca has no instruction"))?
            .set_alignment(align)
            .map_err(|error| {
                CodegenError::backend(format!("setting aggregate alloca alignment: {error}"))
            })?;
        Ok(ptr)
    }

    /// Copy `layout.size` bytes from `src` to `dest` (both aggregate pointers)
    /// using the aggregate's ABI alignment. A zero-sized copy is a no-op.
    fn emit_memcpy(
        &self,
        dest: PointerValue<'ctx>,
        src: PointerValue<'ctx>,
        layout: Layout,
    ) -> Result<(), CodegenError> {
        if layout.size == 0 {
            return Ok(());
        }
        let align = u32::try_from(layout.align)
            .map_err(|_| CodegenError::backend("aggregate alignment exceeds u32"))?;
        let size = self.ctx.i64_type().const_int(layout.size, false);
        self.builder
            .build_memcpy(dest, align, src, align, size)
            .map_err(builder_err("copying an aggregate"))?;
        Ok(())
    }
}

/// A trapping arithmetic operation.
#[derive(Clone, Copy)]
enum ArithOp {
    Add,
    Sub,
    Mul,
}

/// The minimum value of an integer kind as an LLVM constant of type `ty`.
///
/// For a signed kind this is `1 << (bits-1)` (the two's-complement minimum); for
/// an unsigned kind it is `0`. Only the negation/div overflow checks consult it,
/// exactly where the Cranelift backend consults its `int_bounds` minimum.
fn min_const<'ctx>(ty: IntType<'ctx>, kind: IntKind) -> IntValue<'ctx> {
    if is_signed(kind) {
        let bits = ty.get_bit_width();
        // MIN = 1 << (bits - 1), the sign bit set and all others clear.
        ty.const_int(1u64 << (bits - 1), false)
    } else {
        ty.const_zero()
    }
}

/// The LLVM condition predicate for a comparison operator, or `None` if `op` is
/// not a comparison. `signed` selects signed vs unsigned ordering.
fn comparison_pred(op: BinOp, signed: bool) -> Option<IntPredicate> {
    Some(match op {
        BinOp::Eq => IntPredicate::EQ,
        BinOp::Ne => IntPredicate::NE,
        BinOp::Lt if signed => IntPredicate::SLT,
        BinOp::Lt => IntPredicate::ULT,
        BinOp::Le if signed => IntPredicate::SLE,
        BinOp::Le => IntPredicate::ULE,
        BinOp::Gt if signed => IntPredicate::SGT,
        BinOp::Gt => IntPredicate::UGT,
        BinOp::Ge if signed => IntPredicate::SGE,
        BinOp::Ge => IntPredicate::UGE,
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => return None,
    })
}

/// The runtime trap code for a MIR terminator trap.
fn trap_code_of(trap: Trap) -> TrapCode {
    match trap {
        Trap::IndexOutOfBounds => TrapCode::IndexOutOfBounds,
        Trap::Unreachable => TrapCode::Unreachable,
    }
}

/// The exported symbol name of a MIR function, from its stable symbol id.
///
/// Identical to the Cranelift backend's mangling (`tuo_fn_<n>`), so a program
/// built by either backend uses the same internal symbol names — and the same
/// `main` shim entry point.
fn mangle(symbol: SymbolId) -> String {
    format!("tuo_fn_{}", symbol.as_u32())
}

/// Map an inkwell [`BuilderError`](inkwell::builder::BuilderError) to a backend
/// [`CodegenError`], naming the operation that failed. A builder error here is a
/// backend fault (misplaced builder, malformed IR request), never a user error.
fn builder_err(what: &'static str) -> impl Fn(inkwell::builder::BuilderError) -> CodegenError {
    move |error| CodegenError::backend(format!("LLVM builder failed while {what}: {error}"))
}
