//! MIR → Cranelift IR lowering for the v0 scalar core.
//!
//! [`lower_program`] declares every function (so direct calls resolve), then
//! defines each body by walking its basic blocks and translating each
//! statement and terminator to Cranelift IR that computes exactly what the
//! reference interpreter computes. Anything outside the scalar subset is
//! reported as [`CodegenError::unsupported`]; the lowering never emits code
//! whose meaning it is unsure of.
//!
//! # Trapping semantics
//!
//! Integer arithmetic traps on overflow, and division/remainder trap on a zero
//! divisor and on `MIN / -1` (Constitution §24). Each check is lowered as a
//! branch to a dedicated trap block that calls the runtime
//! [`TRAP_SYMBOL`](tuo_runtime::TRAP_SYMBOL) with the matching
//! [`TrapCode`](tuo_runtime::TrapCode) and then reaches an unreachable
//! terminator — the runtime call never returns. This mirrors the interpreter's
//! deterministic abort.
//!
//! # Locals
//!
//! Each MIR local is classified once, up front (see [`LocalKind`]):
//!
//! - a **scalar** local (bool/char/int/float) becomes a Cranelift [`Variable`],
//!   read and written by index, held in a register;
//! - a scalar local whose address a **borrow-mode call argument** takes is
//!   demoted to an explicit [`StackSlot`] (reads load, writes store), so the
//!   callee's pointer aliases real caller memory;
//! - a **unit** local carries no value;
//! - an **aggregate** local (a Stage-1 product type — struct/tuple/enum whose
//!   transitive fields are all scalars — or an ADR-0004 Stage 2 fixed array
//!   `[T; N]`) gets one explicit [`StackSlot`], and its fields/elements are
//!   accessed by computing a byte address into the slot from the runtime ABI's
//!   offsets (see [`tuo_runtime::abi`]);
//! - a **borrow-mode (`in`/`mut`) parameter** is a pointer to caller-owned
//!   memory; reads and writes go through the pointer directly.
//!
//! A local whose type is heap-backed (`Str`, `String`, the growable
//! `Array[T]`, `Box`/`Shared`/`Weak`) still makes the whole function
//! unsupported. Aggregate lowering follows ADR-0004: scalar leaves,
//! by-pointer/sret call ABI. Fixed arrays are laid out inline — element `i` at
//! `i × stride(T)` — and indexed by unchecked address arithmetic, because MIR
//! asserts the bounds (`Assert { IndexOutOfBounds }`) before every
//! `Projection::Index` use.
//!
//! # Borrow-mode calling convention
//!
//! Pinned here and implemented **identically** in the LLVM backend (see
//! `specification/abi.md`, "Passing modes"):
//!
//! - the **caller** passes the ADDRESS of the argument place as a pointer
//!   argument — for a scalar root local that local is demoted to slot storage
//!   so it has an address; an aggregate already lives in a slot;
//! - the **callee** receives that pointer and reads/writes through it
//!   directly: **no copy-in and no copy-back**. The interpreter's
//!   copy-in/copy-back is observably identical because the borrow checker
//!   forbids aliasing (any number of `in` XOR one `mut`) and the borrow lives
//!   only for the call;
//! - forwarding a borrowed parameter as another `in`/`mut` argument passes the
//!   pointer value itself;
//! - a unit-typed borrow occupies no ABI slot, like every unit value;
//! - `take` parameters and returns are unchanged.

use std::collections::{HashMap, HashSet};

use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::stackslot::{StackSlotData, StackSlotKind};
use cranelift_codegen::ir::{
    AbiParam, InstBuilder, MemFlags, Signature, StackSlot, Type, Value as ClifValue, types,
};
use cranelift_codegen::isa::{CallConv, TargetFrontendConfig};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Switch, Variable};
use cranelift_module::{FuncId, Linkage, Module};
use cranelift_object::ObjectModule;

use tuo_codegen::CodegenError;
use tuo_mir::{
    BinOp, CastKind, Const, Function, Operand, PassMode, Place, Program, Projection, Rvalue,
    Statement, Terminator, Trap, UnOp,
};
use tuo_resolve::SymbolId;
use tuo_runtime::abi::{Layout, layout_of, struct_field_offsets, variant_field_offsets};
use tuo_runtime::{TRAP_SYMBOL, TrapCode};
use tuo_types::{FloatKind, IntKind, Ty, TypeckResult};

use crate::abi::{float_type, int_type, int_width_bits, is_signed, scalar_type};
use crate::{CodegenCtx, FUNCTION_LINKAGE};

/// How a MIR local is stored by the backend, decided once up front from its
/// declared type. This is the third outcome added to the original scalar/unit
/// pair: an aggregate local backed by an explicit stack slot.
enum LocalKind {
    /// A scalar (bool/char/int/float) held in an SSA [`Variable`] of the given
    /// type.
    Scalar(Variable, Type),
    /// A scalar local forced into an explicit stack slot because a borrow-mode
    /// call argument takes its address. Reads load and writes store through the
    /// slot, so a callee's pointer aliases this local's real memory.
    ScalarSlot(StackSlot, Type),
    /// A `Unit` local (or a zero-sized aggregate): carries no value, no slot.
    Unit,
    /// A Stage-1 aggregate held in an explicit stack slot; `ty` is the local's
    /// declared type, from which field offsets are computed via the ABI.
    Aggregate {
        /// The stack slot holding the aggregate's bytes.
        slot: StackSlot,
        /// The aggregate's declared type (source of truth for offsets).
        ty: Ty,
    },
    /// A borrow-mode (`in`/`mut`) parameter: a pointer to caller-owned memory,
    /// held in a pointer-typed [`Variable`]. Scalar reads load through the
    /// pointer, `mut` scalar writes store through it, projections use it as the
    /// base address, and forwarding it as another borrow argument passes the
    /// pointer value itself. There is **no** copy-in and **no** copy-back.
    Borrowed {
        /// The pointer-typed variable holding the incoming address.
        var: Variable,
        /// The parameter's declared type (source of truth for offsets).
        ty: Ty,
    },
}

/// The storage classification of a local, from its declared type and the ABI.
///
/// Mirrors the LLVM backend's `classify_storage` exactly so the two backends make
/// identical ABI choices. Returns:
/// - `Ok(None)` — a scalar (the caller resolves its register type) or unit;
/// - `Ok(Some(layout))` — a Stage-1 aggregate with a non-zero layout;
/// - `Err(..)` — the local's type has no v0 layout (Stage 2 / unsupported).
///
/// A type that `scalar_type` maps is a scalar; `Ty::Unit` is unit; anything else
/// whose [`layout_of`] succeeds and is non-zero is an aggregate; a zero-sized
/// aggregate is treated as unit (matching the interpreter's `Value::Unit`).
enum Storage {
    Scalar,
    Unit,
    Aggregate(Layout),
}

/// Classify a local's declared type into its backend storage. See [`Storage`].
///
/// # Errors
///
/// [`CodegenError::unsupported`] if the type is heap-backed (`Str`, `String`,
/// the growable `Array[T]`, `Box`/`Shared`/`Weak` — refused **here**, before
/// any layout query, so the refusal names the concrete type) or has no v0
/// runtime layout.
fn classify_storage(ty: &Ty, types: &TypeckResult, context: &str) -> Result<Storage, CodegenError> {
    // Heap-backed types have an ABI *layout* (their headers), but the backend
    // has no allocator or string runtime to give them meaning yet. Refuse them
    // at classification time with a message naming the type and the road back,
    // so they can never wander into an internal invariant error downstream.
    if let Some(refusal) = heap_type_refusal(ty, context) {
        return Err(CodegenError::unsupported(refusal));
    }
    if scalar_type(ty).is_some() {
        return Ok(Storage::Scalar);
    }
    if matches!(ty, Ty::Unit) {
        return Ok(Storage::Unit);
    }
    // Not scalar, not unit: it must be an aggregate with a real layout — a
    // scalar-leaf product type (Stage 1) or a fixed array `[T; N]` (Stage 2).
    match layout_of(ty, types) {
        Ok(layout) if layout.size == 0 => Ok(Storage::Unit),
        Ok(layout) => Ok(Storage::Aggregate(layout)),
        Err(error) => Err(CodegenError::unsupported(format!(
            "`{context}` uses a type the Cranelift backend does not lower yet: {error}"
        ))),
    }
}

/// The clean refusal message for a heap-backed type the backend does not lower
/// yet, or `None` if `ty` is not one. Mirrors the LLVM backend's
/// `heap_type_refusal` word for word (bar the backend name), so the two
/// backends refuse the same boundary with the same explanation.
fn heap_type_refusal(ty: &Ty, context: &str) -> Option<String> {
    let (what, road_back) = match ty {
        Ty::Str => ("a `Str` value", "runtime strings await ADR-0006"),
        Ty::String => ("a `String` value", "runtime strings await ADR-0006"),
        Ty::Array(_) => (
            "the growable `Array[T]`",
            "it awaits the allocator ADR (the fixed `[T; N]` is lowered)",
        ),
        Ty::Wrapper(kind, _) => {
            return Some(format!(
                "`{context}` uses a `{}[T]` heap wrapper, which the Cranelift backend does not \
                 lower yet (heap wrappers await the allocator ADR); the interpreter \
                 (`tuo spec`/`tuo verify`) remains the reference",
                kind.name()
            ));
        }
        _ => return None,
    };
    Some(format!(
        "`{context}` uses {what}, which the Cranelift backend does not lower yet ({road_back}); \
         the interpreter (`tuo spec`/`tuo verify`) remains the reference"
    ))
}

/// Declare then define every lowerable function of `program` into `module`.
///
/// # Errors
///
/// [`CodegenError::unsupported`] if any function reachable in the program uses
/// a feature outside the v0 scalar subset; [`CodegenError::backend`] on a
/// Cranelift failure.
pub(crate) fn lower_program(
    module: &mut ObjectModule,
    program: &Program,
    types: &TypeckResult,
) -> Result<HashMap<SymbolId, FuncId>, CodegenError> {
    // Pass 1: declare every function so direct calls can reference them before
    // their bodies are defined.
    let mut ids: HashMap<SymbolId, FuncId> = HashMap::new();
    for function in &program.functions {
        let signature = function_signature(module, function, types)?;
        let symbol_name = mangle(function.symbol);
        let id = module
            .declare_function(&symbol_name, FUNCTION_LINKAGE, &signature)
            .map_err(|error| {
                CodegenError::backend(format!("declaring `{}`: {error}", function.name))
            })?;
        ids.insert(function.symbol, id);
    }

    // Pass 2: define each body.
    let mut ctx = CodegenCtx::new(module);
    let mut builder_ctx = FunctionBuilderContext::new();
    for function in &program.functions {
        define_function(
            module,
            &mut ctx,
            &mut builder_ctx,
            &ids,
            function,
            &program.functions,
            types,
        )?;
    }
    Ok(ids)
}

/// The Cranelift signature of a MIR function (v0 ABI, Stage-1 aggregates).
///
/// Applies the two aggregate calling-convention rules of ADR-0004 Stage 1,
/// identically to the LLVM backend's `function_type`:
/// - an **aggregate return** is an sret hidden out-pointer, *prepended* as
///   argument index 0, and the native return type becomes void;
/// - an **aggregate parameter** is passed as a pointer to a caller-owned copy.
///
/// Every size/align derives from [`tuo_runtime::abi`], never from Cranelift's
/// native small-struct classification.
fn function_signature(
    module: &ObjectModule,
    function: &Function,
    types: &TypeckResult,
) -> Result<Signature, CodegenError> {
    let call_conv = module.isa().default_call_conv();
    let pointer_type = module.isa().pointer_type();
    let mut signature = Signature::new(call_conv);

    // Classify the return: an aggregate return prepends an sret pointer and the
    // function returns void; a scalar return is by value; unit is no value.
    let ret_storage = classify_storage(&function.ret, types, &function.name)?;
    let returns_aggregate = matches!(ret_storage, Storage::Aggregate(_));
    if returns_aggregate {
        // sret hidden out-pointer is ALWAYS argument index 0 (prepended).
        signature.params.push(AbiParam::new(pointer_type));
    }

    // Parameters, in declaration order after the (optional) sret pointer.
    // Borrow-mode calling convention (identical in the LLVM backend's
    // `function_type`, per `specification/abi.md` "Passing modes"): an
    // `in`/`mut` parameter arrives as a **pointer to the caller's place** —
    // scalar or aggregate alike — read (and, for `mut`, written) through
    // directly, with no copy-in and no copy-back. A `take` parameter is
    // unchanged: scalar by value, aggregate by pointer to a caller-owned copy,
    // unit occupying no ABI slot (in any mode).
    for (index, mode) in function.params.iter().enumerate() {
        let ty = &function.locals[index].ty;
        let storage = classify_storage(ty, types, &function.name)?;
        match (mode, storage) {
            (_, Storage::Unit) => {}
            (PassMode::Value, Storage::Scalar) => signature
                .params
                .push(AbiParam::new(require_scalar(ty, &function.name)?)),
            (PassMode::Value, Storage::Aggregate(_))
            | (PassMode::Borrow | PassMode::BorrowMut, _) => {
                signature.params.push(AbiParam::new(pointer_type));
            }
        }
    }

    // Return value: scalar by value; aggregate is void (written through sret);
    // unit is no value.
    match ret_storage {
        Storage::Scalar => signature.returns.push(AbiParam::new(require_scalar(
            &function.ret,
            &function.name,
        )?)),
        Storage::Unit | Storage::Aggregate(_) => {}
    }
    Ok(signature)
}

/// The scalar Cranelift type of `ty`, or an unsupported error naming `context`.
fn require_scalar(ty: &Ty, context: &str) -> Result<Type, CodegenError> {
    scalar_type(ty).ok_or_else(|| {
        CodegenError::unsupported(format!(
            "`{context}` uses a non-scalar type the Cranelift backend does not lower yet"
        ))
    })
}

/// Define one function body.
fn define_function(
    module: &mut ObjectModule,
    ctx: &mut CodegenCtx,
    builder_ctx: &mut FunctionBuilderContext,
    ids: &HashMap<SymbolId, FuncId>,
    function: &Function,
    functions: &[Function],
    types: &TypeckResult,
) -> Result<(), CodegenError> {
    let signature = function_signature(module, function, types)?;
    let self_id = ids[&function.symbol];

    ctx.context_mut().func.signature = signature;
    let mut lowering = Lowering::new(
        module,
        ctx.context_mut(),
        builder_ctx,
        ids,
        function,
        functions,
        types,
    )?;
    lowering.run()?;
    lowering.finish();

    module
        .define_function(self_id, ctx.context_mut())
        .map_err(|error| CodegenError::backend(format!("defining `{}`: {error}", function.name)))?;
    ctx.clear(module);
    Ok(())
}

/// The per-function lowering state.
struct Lowering<'a> {
    builder: FunctionBuilder<'a>,
    module: &'a mut ObjectModule,
    ids: &'a HashMap<SymbolId, FuncId>,
    function: &'a Function,
    /// Every function in the program, so a direct call can read its callee's
    /// return type and name for the aggregate call ABI.
    functions: &'a [Function],
    types: &'a TypeckResult,
    /// The pointer type of the target (for slot addresses and aggregate args).
    pointer_type: Type,
    /// The frontend config, needed by `emit_small_memory_copy`.
    frontend_config: TargetFrontendConfig,
    /// The Cranelift block for each MIR block index.
    blocks: Vec<cranelift_codegen::ir::Block>,
    /// How each MIR local is stored (index = local id): scalar variable, unit,
    /// or aggregate stack slot. Filled during `run` once the builder exists.
    kinds: Vec<LocalKind>,
}

impl<'a> Lowering<'a> {
    fn new(
        module: &'a mut ObjectModule,
        context: &'a mut cranelift_codegen::Context,
        builder_ctx: &'a mut FunctionBuilderContext,
        ids: &'a HashMap<SymbolId, FuncId>,
        function: &'a Function,
        functions: &'a [Function],
        types: &'a TypeckResult,
    ) -> Result<Self, CodegenError> {
        // Classify every local up front so an unsupported (Stage-2) type fails
        // before any IR is built. The concrete slots/variables are created in
        // `run`, once the builder exists.
        for local in &function.locals {
            classify_storage(&local.ty, types, &function.name)?;
        }

        let pointer_type = module.isa().pointer_type();
        let frontend_config = module.isa().frontend_config();
        let builder = FunctionBuilder::new(&mut context.func, builder_ctx);
        Ok(Self {
            builder,
            module,
            ids,
            function,
            functions,
            types,
            pointer_type,
            frontend_config,
            blocks: Vec::new(),
            kinds: Vec::new(),
        })
    }

    /// Lower the whole body.
    fn run(&mut self) -> Result<(), CodegenError> {
        // Create a Cranelift block per MIR block.
        self.blocks = (0..self.function.blocks.len())
            .map(|_| self.builder.create_block())
            .collect();

        // Pre-scan for locals whose address a borrow-mode call argument takes:
        // those must be memory-backed even when scalar, so the callee's pointer
        // aliases real caller memory. Aggregates already live in slots.
        let borrowed_roots: HashSet<u32> = self
            .function
            .blocks
            .iter()
            .flat_map(|block| &block.statements)
            .filter_map(|statement| match statement {
                Statement::Call { args, .. } => Some(args),
                _ => None,
            })
            .flatten()
            .filter_map(|arg| match arg {
                tuo_mir::Arg::Borrow(place) | tuo_mir::Arg::BorrowMut(place) => Some(place.local.0),
                tuo_mir::Arg::Value(_) => None,
            })
            .collect();

        // Classify and allocate storage for every local: a scalar gets an SSA
        // Variable (or a stack slot, if borrowed); a unit local carries no
        // value; an aggregate local gets one explicit stack slot addressed by
        // its ABI layout; a borrow-mode parameter gets a pointer-typed Variable
        // holding the caller's address (seeded below).
        self.kinds = Vec::with_capacity(self.function.locals.len());
        for (index, local) in self.function.locals.iter().enumerate() {
            let storage = classify_storage(&local.ty, self.types, &self.function.name)?;
            let borrow_param = self
                .function
                .params
                .get(index)
                .is_some_and(|mode| *mode != PassMode::Value);
            let kind = match storage {
                // A unit-typed local carries no value in every mode (a borrow
                // of a unit place likewise occupies no ABI slot).
                Storage::Unit => LocalKind::Unit,
                _ if borrow_param => LocalKind::Borrowed {
                    var: self.builder.declare_var(self.pointer_type),
                    ty: local.ty.clone(),
                },
                Storage::Scalar => {
                    // `require_scalar` cannot fail here (classify said scalar).
                    let ty = require_scalar(&local.ty, &self.function.name)?;
                    if borrowed_roots.contains(&(index as u32)) {
                        let layout = self.layout(&local.ty)?;
                        LocalKind::ScalarSlot(self.new_temp_slot(layout)?, ty)
                    } else {
                        LocalKind::Scalar(self.builder.declare_var(ty), ty)
                    }
                }
                Storage::Aggregate(layout) => {
                    let slot = self.builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        u32::try_from(layout.size).map_err(|_| {
                            CodegenError::backend("aggregate stack slot larger than 4 GiB")
                        })?,
                        log2_align(layout.align),
                    ));
                    LocalKind::Aggregate {
                        slot,
                        ty: local.ty.clone(),
                    }
                }
            };
            self.kinds.push(kind);
        }

        // Entry block: append the parameters and seed the parameter locals.
        let entry = self.blocks[0];
        self.builder.append_block_params_for_function_params(entry);
        self.builder.switch_to_block(entry);
        let param_values: Vec<ClifValue> = self.builder.block_params(entry).to_vec();

        // If this function returns an aggregate, native parameter 0 is the sret
        // out-pointer (not a MIR local); the MIR parameters start at native
        // index 1. Otherwise they start at 0.
        let returns_aggregate = matches!(
            classify_storage(&self.function.ret, self.types, &self.function.name)?,
            Storage::Aggregate(_)
        );
        let mut native = usize::from(returns_aggregate);

        // Seed each MIR parameter local from its native parameter. A scalar is
        // defined directly (or stored into its slot, if borrowed onward); an
        // aggregate `take` parameter's incoming pointer is copied into the
        // callee's own slot (owned-copy semantics); a borrow-mode parameter's
        // incoming pointer is kept as-is (no copy-in — it aliases the caller's
        // place); a unit parameter occupies no native slot.
        for index in 0..self.function.params.len() {
            match &self.kinds[index] {
                LocalKind::Scalar(var, _) => {
                    let var = *var;
                    let value = param_values[native];
                    self.builder.def_var(var, value);
                    native += 1;
                }
                LocalKind::ScalarSlot(slot, _) => {
                    let slot = *slot;
                    let value = param_values[native];
                    self.builder.ins().stack_store(value, slot, 0);
                    native += 1;
                }
                LocalKind::Unit => {}
                LocalKind::Aggregate { slot, ty } => {
                    let slot = *slot;
                    let ty = ty.clone();
                    let src = param_values[native];
                    native += 1;
                    let layout = self.layout(&ty)?;
                    let dest = self.builder.ins().stack_addr(self.pointer_type, slot, 0);
                    self.emit_memcpy(dest, src, layout);
                }
                LocalKind::Borrowed { var, .. } => {
                    let var = *var;
                    let addr = param_values[native];
                    self.builder.def_var(var, addr);
                    native += 1;
                }
            }
        }

        // Seed non-parameter scalar locals with a zero so a Variable is always
        // defined before use on every path (the ownership checker guarantees no
        // *semantic* read of an uninitialized local, but Cranelift's SSA
        // construction still requires a definition to exist). Stack slots
        // (aggregate or scalar) need no such seeding — a slot is always
        // addressable.
        for index in self.function.params.len()..self.function.locals.len() {
            if let LocalKind::Scalar(var, ty) = &self.kinds[index] {
                let (var, ty) = (*var, *ty);
                let zero = if ty == types::F32 {
                    self.builder.ins().f32const(0.0)
                } else if ty == types::F64 {
                    self.builder.ins().f64const(0.0)
                } else {
                    self.builder.ins().iconst(ty, 0)
                };
                self.builder.def_var(var, zero);
            }
        }

        // Lower each block's statements and terminator. Block 0's body follows
        // the parameter seeding in the same Cranelift block.
        for (index, block) in self.function.blocks.iter().enumerate() {
            if index != 0 {
                let clif_block = self.blocks[index];
                self.builder.switch_to_block(clif_block);
            }
            for statement in &block.statements {
                self.lower_statement(statement)?;
            }
            self.lower_terminator(&block.terminator)?;
        }

        self.builder.seal_all_blocks();
        Ok(())
    }

    /// Finalize the builder (consumes the lowering, which owns it).
    fn finish(self) {
        self.builder.finalize();
    }

    // ----- statements -----

    fn lower_statement(&mut self, statement: &Statement) -> Result<(), CodegenError> {
        match statement {
            Statement::Assign { place, rvalue } => self.lower_assign(place, rvalue),
            Statement::Call { dest, callee, args } => self.lower_call(dest, *callee, args),
            // A host effect (`std::rt`, ADR-0006) is refused, never
            // mis-compiled: its native lowering (the `tuo_rt_write`/
            // `tuo_rt_read_byte`/`tuo_rt_exit` runtime symbols) lands with
            // ADR-0006 Stage B. Until then the interpreter is the reference
            // for the pure core and effectful programs cannot be built.
            Statement::Effect { op, .. } => Err(CodegenError::unsupported(format!(
                "the `std::rt::{}` effect is not lowered by the Cranelift backend yet; \
                 its native lowering lands with ADR-0006 Stage B",
                op.name()
            ))),
            Statement::Drop { .. } => {
                // v0 supported values own no host resource; a Stage-1 aggregate
                // owns no heap (scalar fields only), so its drop is a no-op too,
                // exactly like a scalar's. Nothing to emit.
                Ok(())
            }
        }
    }

    /// Lower `place = rvalue`. An aggregate rvalue (`Aggregate`) is materialized
    /// in place into the destination's byte storage; a whole-aggregate `Use`
    /// (copy/move of an aggregate place) is a memcpy between slots; every scalar
    /// rvalue (including `Discriminant`, which yields a `Usize` scalar) takes the
    /// scalar path and is stored with `write_place`.
    fn lower_assign(&mut self, place: &Place, rvalue: &Rvalue) -> Result<(), CodegenError> {
        match rvalue {
            Rvalue::Aggregate { kind, fields } => self.lower_aggregate(place, kind, fields),
            // A unit-valued copy/move (a unit local, or a zero-sized aggregate
            // such as `[T; 0]`) carries no bytes: nothing to emit.
            Rvalue::Use(operand) if self.operand_is_unit(operand) => Ok(()),
            Rvalue::Use(operand) if self.operand_is_aggregate(operand) => {
                // Whole-aggregate copy/move: memcpy from the source place's slot
                // into the destination's byte storage.
                let (dest_addr, dest_ty) = self.place_address(place)?;
                let layout = self.layout(&dest_ty)?;
                let src_addr = self.operand_aggregate_address(operand)?;
                self.emit_memcpy(dest_addr, src_addr, layout);
                Ok(())
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
        let Some(&callee_id) = self.ids.get(&callee) else {
            return Err(CodegenError::unsupported(
                "call to a function outside the lowered program (v0 has no external calls)",
            ));
        };
        let callee_fn = self.function_named(callee)?;

        // Does the callee return an aggregate? If so, the first native argument
        // is an sret out-pointer to caller-owned destination storage.
        let dest_ty = self.place_type(dest);
        let ret_is_aggregate = matches!(
            classify_storage(&callee_fn.ret, self.types, &callee_fn.name)?,
            Storage::Aggregate(_)
        );

        let mut arg_values: Vec<ClifValue> = Vec::with_capacity(args.len() + 1);

        // Allocate the sret destination up front (a temporary slot unless dest is
        // a bare aggregate local, in which case its own slot is the destination).
        let sret_slot = if ret_is_aggregate {
            let layout = self.layout(&callee_fn.ret)?;
            let (slot, addr) = self.sret_destination(dest, &dest_ty, layout)?;
            arg_values.push(addr);
            Some((slot, layout))
        } else {
            None
        };

        // Marshal each argument. A scalar `Value` arg is passed by value; an
        // aggregate `Value` arg is materialized into a caller temporary and its
        // address passed. Unit arguments occupy no native slot.
        for arg in args {
            match arg {
                tuo_mir::Arg::Value(operand) => {
                    if self.operand_is_unit(operand) {
                        // A unit argument carries no native value.
                    } else if self.operand_is_aggregate(operand) {
                        let addr = self.materialize_aggregate_arg(operand)?;
                        arg_values.push(addr);
                    } else {
                        arg_values.push(self.lower_operand(operand)?);
                    }
                }
                tuo_mir::Arg::Borrow(place) | tuo_mir::Arg::BorrowMut(place) => {
                    // Borrow-mode argument: pass the ADDRESS of the caller's
                    // place (the callee reads/writes through it directly; no
                    // copy-in, no copy-back). A unit-typed borrow occupies no
                    // ABI slot, matching the signature builder.
                    let ty = self.place_type(place);
                    match classify_storage(&ty, self.types, &self.function.name)? {
                        Storage::Unit => {}
                        Storage::Scalar | Storage::Aggregate(_) => {
                            let addr = self.borrow_address(place)?;
                            arg_values.push(addr);
                        }
                    }
                }
            }
        }

        let func_ref = self
            .module
            .declare_func_in_func(callee_id, self.builder.func);
        let call = self.builder.ins().call(func_ref, &arg_values);

        if let Some((slot, layout)) = sret_slot {
            // The callee wrote the aggregate result into the sret slot. If `dest`
            // is a bare aggregate local, that slot *is* its storage and nothing
            // more is needed. If `dest` is projected, copy from the temporary
            // slot into the projected address.
            if !dest.projection.is_empty() {
                let src = self.builder.ins().stack_addr(self.pointer_type, slot, 0);
                let (dest_addr, _leaf) = self.place_address(dest)?;
                self.emit_memcpy(dest_addr, src, layout);
            }
        } else {
            let results = self.builder.inst_results(call);
            // A unit-returning callee yields no result; a scalar callee yields
            // one, stored into the (scalar) destination.
            if let Some(&result) = results.first() {
                self.write_place(dest, result)?;
            }
        }
        Ok(())
    }

    /// The sret destination for a call returning an aggregate: `(slot, address)`.
    /// When `dest` is a bare aggregate local, its own slot is reused (no post-call
    /// copy). Otherwise a fresh temporary slot is allocated and returned so the
    /// caller can copy it into the projected destination after the call.
    fn sret_destination(
        &mut self,
        dest: &Place,
        dest_ty: &Ty,
        layout: Layout,
    ) -> Result<(StackSlot, ClifValue), CodegenError> {
        if dest.projection.is_empty() {
            if let LocalKind::Aggregate { slot, .. } = &self.kinds[dest.local.0 as usize] {
                let slot = *slot;
                let addr = self.builder.ins().stack_addr(self.pointer_type, slot, 0);
                return Ok((slot, addr));
            }
        }
        // Projected (or otherwise) destination: use a fresh temporary slot.
        let _ = dest_ty;
        let slot = self.new_temp_slot(layout)?;
        let addr = self.builder.ins().stack_addr(self.pointer_type, slot, 0);
        Ok((slot, addr))
    }

    /// Materialize an aggregate call argument into a caller-owned temporary slot
    /// and return its address, per the by-pointer call ABI. The operand is a
    /// `Copy`/`Move` of an aggregate place; memcpy its slot into the temporary so
    /// the callee's copy-in cannot alias the caller's live value.
    fn materialize_aggregate_arg(&mut self, operand: &Operand) -> Result<ClifValue, CodegenError> {
        let ty = self
            .operand_ty(operand)
            .ok_or_else(|| CodegenError::backend("aggregate argument has no static type"))?;
        let layout = self.layout(&ty)?;
        let src = self.operand_aggregate_address(operand)?;
        let slot = self.new_temp_slot(layout)?;
        let dest = self.builder.ins().stack_addr(self.pointer_type, slot, 0);
        self.emit_memcpy(dest, src, layout);
        Ok(dest)
    }

    /// The address of `place` for a borrow-mode (`in`/`mut`) call argument.
    ///
    /// A bare local resolves to its own storage: a borrowed-root scalar's slot
    /// (the pre-scan forced it into memory), an aggregate's slot, or — when
    /// forwarding a borrowed parameter — the incoming pointer value itself. A
    /// projected place resolves through the shared address walk.
    fn borrow_address(&mut self, place: &Place) -> Result<ClifValue, CodegenError> {
        if place.projection.is_empty() {
            return match &self.kinds[place.local.0 as usize] {
                LocalKind::ScalarSlot(slot, _) => {
                    Ok(self.builder.ins().stack_addr(self.pointer_type, *slot, 0))
                }
                LocalKind::Aggregate { slot, .. } => {
                    Ok(self.builder.ins().stack_addr(self.pointer_type, *slot, 0))
                }
                LocalKind::Borrowed { var, .. } => Ok(self.builder.use_var(*var)),
                LocalKind::Scalar(..) => Err(CodegenError::backend(
                    "borrowed scalar local was not slot-backed (pre-scan invariant)",
                )),
                LocalKind::Unit => Err(CodegenError::backend(
                    "address of a unit local requested for a borrow argument",
                )),
            };
        }
        let (addr, _leaf) = self.place_address(place)?;
        Ok(addr)
    }

    /// Allocate a fresh temporary stack slot of `layout`.
    fn new_temp_slot(&mut self, layout: Layout) -> Result<StackSlot, CodegenError> {
        Ok(self.builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            u32::try_from(layout.size)
                .map_err(|_| CodegenError::backend("aggregate temporary larger than 4 GiB"))?,
            log2_align(layout.align),
        )))
    }

    /// The MIR function with symbol `callee`, needed to read its return type and
    /// name for the call ABI. All calls are direct and internal in v0.
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
                        self.builder.ins().return_(&[]);
                    }
                    Storage::Scalar => {
                        let value = self.lower_operand(operand)?;
                        self.builder.ins().return_(&[value]);
                    }
                    Storage::Aggregate(layout) => {
                        // Copy the operand's aggregate value into the sret
                        // out-pointer (native parameter 0), then return void.
                        let src = self.operand_aggregate_address(operand)?;
                        let sret = self.builder.block_params(self.blocks[0])[0];
                        self.emit_memcpy(sret, src, layout);
                        self.builder.ins().return_(&[]);
                    }
                }
                Ok(())
            }
            Terminator::Goto(target) => {
                let block = self.blocks[target.0 as usize];
                self.builder.ins().jump(block, &[]);
                Ok(())
            }
            Terminator::Branch {
                cond,
                then_block,
                else_block,
            } => {
                let value = self.lower_operand(cond)?;
                let then_b = self.blocks[then_block.0 as usize];
                let else_b = self.blocks[else_block.0 as usize];
                self.builder.ins().brif(value, then_b, &[], else_b, &[]);
                Ok(())
            }
            Terminator::Switch {
                discr,
                arms,
                otherwise,
            } => {
                let value = self.lower_operand(discr)?;
                // Cranelift's Switch needs a fixed-width integer; the discr is
                // an integer-classed scalar (int, char, or bool). Widen bool
                // (i8) uniformly by using the value's own type.
                let mut switch = Switch::new();
                for (case_value, target) in arms {
                    let block = self.blocks[target.0 as usize];
                    // MIR guarantees arm values fit the operand's type.
                    switch.set_entry(*case_value as u128, block);
                }
                let otherwise_block = self.blocks[otherwise.0 as usize];
                switch.emit(&mut self.builder, value, otherwise_block);
                Ok(())
            }
            Terminator::Assert { cond, trap, target } => {
                let value = self.lower_operand(cond)?;
                let ok_block = self.blocks[target.0 as usize];
                let trap_block = self.builder.create_block();
                self.builder
                    .ins()
                    .brif(value, ok_block, &[], trap_block, &[]);
                self.builder.switch_to_block(trap_block);
                self.builder.seal_block(trap_block);
                self.emit_trap_call(trap_code_of(*trap));
                Ok(())
            }
            Terminator::Trap(trap) => {
                self.emit_trap_call(trap_code_of(*trap));
                Ok(())
            }
        }
    }

    // ----- rvalues & operands -----

    fn lower_rvalue(&mut self, rvalue: &Rvalue) -> Result<ClifValue, CodegenError> {
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
                "the growable `Array[T]` (and its `Len`) is not lowered by the Cranelift \
                 backend yet",
            )),
            // The `std::str` byte operations (ADR-0006) are refused, never
            // mis-compiled: their native lowering lands with ADR-0006
            // Stage B, together with `Str`'s `{ptr, len}` value layout.
            Rvalue::StrOp { op, .. } => Err(CodegenError::unsupported(format!(
                "the `std::str::{}` string operation is not lowered by the Cranelift backend \
                 yet; its native lowering lands with ADR-0006 Stage B",
                op.name()
            ))),
        }
    }

    fn lower_operand(&mut self, operand: &Operand) -> Result<ClifValue, CodegenError> {
        match operand {
            Operand::Copy(place) | Operand::Move(place) => self.read_place(place),
            Operand::Const(constant) => self.lower_const(constant),
        }
    }

    fn lower_const(&mut self, constant: &Const) -> Result<ClifValue, CodegenError> {
        match constant {
            Const::Bool(b) => Ok(self.builder.ins().iconst(types::I8, i64::from(*b))),
            Const::Char(c) => Ok(self
                .builder
                .ins()
                .iconst(types::I32, i64::from(u32::from(*c)))),
            Const::Int(value, kind) => {
                let ty = int_type(*kind);
                // Reinterpret the mathematical value's low bits at the target
                // width; MIR guarantees it is in range for the kind.
                let bits = *value as i64;
                Ok(self.builder.ins().iconst(ty, bits))
            }
            // The MIR constant is stored at f64 width, already normalized to
            // its kind's precision, so materializing an `F32` by rounding the
            // f64 payload to f32 is exact.
            Const::Float(value, FloatKind::F32) => Ok(self.builder.ins().f32const(*value as f32)),
            Const::Float(value, FloatKind::F64) => Ok(self.builder.ins().f64const(*value)),
            Const::Unit => Err(CodegenError::unsupported(
                "a unit constant has no scalar representation to place in a value context",
            )),
            // Unreachable for locals/fields of type `Str` (classification
            // refuses those up front); kept for a bare `Str` literal in an
            // operand position.
            Const::Str(_) => Err(CodegenError::unsupported(
                "a `Str` string constant is not lowered by the Cranelift backend yet (runtime \
                 strings await ADR-0006); the interpreter (`tuo spec`/`tuo verify`) remains the \
                 reference",
            )),
        }
    }

    // ----- arithmetic -----

    fn lower_unary(
        &mut self,
        op: UnOp,
        operand: &Operand,
        value: ClifValue,
    ) -> Result<ClifValue, CodegenError> {
        match op {
            UnOp::Not => {
                // Boolean negation: xor with 1 (values are 0/1).
                Ok(self.builder.ins().bxor_imm(value, 1))
            }
            UnOp::Neg if matches!(self.operand_ty(operand), Some(Ty::Float(_))) => {
                // Float negation flips the sign bit (IEEE 754, works on NaN
                // too) and never traps — exactly the interpreter's `-v`.
                Ok(self.builder.ins().fneg(value))
            }
            UnOp::Neg => {
                let kind = self.operand_int_kind(operand)?;
                // Integer negation traps on MIN (two's complement, §24).
                let (min, _max) = int_bounds(kind);
                let ty = int_type(kind);
                let min_const = self.builder.ins().iconst(ty, min);
                let is_min = self.builder.ins().icmp(IntCC::Equal, value, min_const);
                self.guard(is_min, TrapCode::IntegerOverflow);
                Ok(self.builder.ins().ineg(value))
            }
        }
    }

    fn lower_binary(
        &mut self,
        op: BinOp,
        lhs: &Operand,
        l: ClifValue,
        r: ClifValue,
    ) -> Result<ClifValue, CodegenError> {
        // Floats take their own IEEE-754 path (never trapping); everything
        // else reduces to integer compares/arithmetic.
        if let Some(Ty::Float(kind)) = self.operand_ty(lhs) {
            return self.lower_float_binary(op, kind, l, r);
        }
        // Comparisons on chars/bools/ints all reduce to integer compares; the
        // arithmetic operators are integer-only in the supported subset.
        if let Some(cc) = comparison_code(op, self.operand_signed(lhs)) {
            // Cranelift's `icmp` yields an `i8` boolean (0 or 1), which is
            // exactly the backend's `Bool` representation — no widening needed.
            return Ok(self.builder.ins().icmp(cc, l, r));
        }

        let kind = self.operand_int_kind(lhs)?;
        match op {
            BinOp::Add => Ok(self.checked_arith(kind, l, r, ArithOp::Add)),
            BinOp::Sub => Ok(self.checked_arith(kind, l, r, ArithOp::Sub)),
            BinOp::Mul => Ok(self.checked_arith(kind, l, r, ArithOp::Mul)),
            BinOp::Div => Ok(self.checked_divrem(kind, l, r, true)),
            BinOp::Rem => Ok(self.checked_divrem(kind, l, r, false)),
            // Comparisons handled above; reaching here is impossible.
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => Err(
                CodegenError::backend("comparison fell through arithmetic path"),
            ),
        }
    }

    /// Lower a float binary operation: IEEE 754 (round to nearest even), never
    /// trapping — `x / 0.0` is an infinity or NaN, exactly as the interpreter
    /// computes with Rust `f64`/`f32` arithmetic. `Rem` has C `fmod` semantics
    /// (the sign of the dividend), which is Rust `%` — Cranelift has no `frem`
    /// instruction, so it is lowered as a call to the C library's
    /// `fmod`/`fmodf` (the exact function Rust's `%` lowers to).
    ///
    /// Comparisons use Cranelift's [`FloatCC`]: the four orderings are
    /// *ordered* (false when either side is NaN) and `NotEqual` is
    /// unordered-or-unequal (true on NaN) — exactly Rust's float comparison
    /// semantics, which the interpreter uses.
    fn lower_float_binary(
        &mut self,
        op: BinOp,
        kind: FloatKind,
        l: ClifValue,
        r: ClifValue,
    ) -> Result<ClifValue, CodegenError> {
        if let Some(cc) = float_comparison_code(op) {
            // `fcmp` yields an `i8` boolean (0 or 1), the backend's `Bool`.
            return Ok(self.builder.ins().fcmp(cc, l, r));
        }
        Ok(match op {
            BinOp::Add => self.builder.ins().fadd(l, r),
            BinOp::Sub => self.builder.ins().fsub(l, r),
            BinOp::Mul => self.builder.ins().fmul(l, r),
            BinOp::Div => self.builder.ins().fdiv(l, r),
            BinOp::Rem => {
                let func_ref = self.fmod_func_ref(kind);
                let call = self.builder.ins().call(func_ref, &[l, r]);
                self.builder.inst_results(call)[0]
            }
            // Comparisons handled above; reaching here is impossible.
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                return Err(CodegenError::backend(
                    "float comparison fell through the arithmetic path",
                ));
            }
        })
    }

    /// Declare (on demand) and reference the C library's `fmod` (f64) or
    /// `fmodf` (f32) for [`BinOp::Rem`] on floats, mirroring how the runtime
    /// trap symbol is imported. The CLI links with `-lm` so the symbol resolves
    /// on every supported host.
    fn fmod_func_ref(&mut self, kind: FloatKind) -> cranelift_codegen::ir::FuncRef {
        let float_ty = float_type(kind);
        let mut signature = Signature::new(CallConv::triple_default(self.module.isa().triple()));
        signature.params.push(AbiParam::new(float_ty));
        signature.params.push(AbiParam::new(float_ty));
        signature.returns.push(AbiParam::new(float_ty));
        let name = match kind {
            FloatKind::F32 => "fmodf",
            FloatKind::F64 => "fmod",
        };
        let id = self
            .module
            .declare_function(name, Linkage::Import, &signature)
            .expect("declaring the libm fmod symbol");
        self.module.declare_func_in_func(id, self.builder.func)
    }

    /// Lower a trapping add/sub/mul: compute at full width, then trap if the
    /// result is out of the kind's range. For the register width equal to the
    /// kind width, Cranelift's overflow-flagged ops would suffice; to keep one
    /// uniform rule across widths, we compute in a wider space by range-checking
    /// against the kind bounds using the operation's overflow instruction.
    fn checked_arith(
        &mut self,
        kind: IntKind,
        l: ClifValue,
        r: ClifValue,
        op: ArithOp,
    ) -> ClifValue {
        let signed = is_signed(kind);
        // Use Cranelift's carry/overflow-producing instructions where the
        // register width matches the kind width (the common case for i8..i64).
        let (result, overflow) = match (op, signed) {
            (ArithOp::Add, true) => self.builder.ins().sadd_overflow(l, r),
            (ArithOp::Add, false) => self.builder.ins().uadd_overflow(l, r),
            (ArithOp::Sub, true) => self.builder.ins().ssub_overflow(l, r),
            (ArithOp::Sub, false) => self.builder.ins().usub_overflow(l, r),
            (ArithOp::Mul, true) => self.builder.ins().smul_overflow(l, r),
            (ArithOp::Mul, false) => self.builder.ins().umul_overflow(l, r),
        };
        self.guard(overflow, TrapCode::IntegerOverflow);
        result
    }

    /// Lower a trapping div (`is_div`) or rem: trap on a zero divisor, and on
    /// the signed `MIN / -1` overflow case.
    fn checked_divrem(
        &mut self,
        kind: IntKind,
        l: ClifValue,
        r: ClifValue,
        is_div: bool,
    ) -> ClifValue {
        let ty = int_type(kind);
        let signed = is_signed(kind);

        // Divisor == 0 → division-by-zero trap.
        let zero = self.builder.ins().iconst(ty, 0);
        let is_zero = self.builder.ins().icmp(IntCC::Equal, r, zero);
        self.guard(is_zero, TrapCode::DivisionByZero);

        if signed {
            // MIN / -1 (and MIN % -1) overflow → integer-overflow trap.
            let (min, _max) = int_bounds(kind);
            let min_const = self.builder.ins().iconst(ty, min);
            let neg_one = self.builder.ins().iconst(ty, -1);
            let l_is_min = self.builder.ins().icmp(IntCC::Equal, l, min_const);
            let r_is_neg_one = self.builder.ins().icmp(IntCC::Equal, r, neg_one);
            let both = self.builder.ins().band(l_is_min, r_is_neg_one);
            self.guard(both, TrapCode::IntegerOverflow);
            if is_div {
                self.builder.ins().sdiv(l, r)
            } else {
                self.builder.ins().srem(l, r)
            }
        } else if is_div {
            self.builder.ins().udiv(l, r)
        } else {
            self.builder.ins().urem(l, r)
        }
    }

    fn lower_cast(
        &mut self,
        kind: CastKind,
        operand: &Operand,
        value: ClifValue,
        to: &Ty,
    ) -> Result<ClifValue, CodegenError> {
        match kind {
            CastKind::IntToInt => {
                let Ty::Int(target) = to else {
                    return Err(CodegenError::backend("int-to-int cast to a non-integer"));
                };
                let from = self.operand_int_kind(operand)?;
                Ok(self.resize_int(value, from, *target))
            }
            CastKind::IntToFloat => {
                let Ty::Float(target) = to else {
                    return Err(CodegenError::backend("int-to-float cast to a non-float"));
                };
                // Round to nearest even, by the SOURCE's signedness. The
                // interpreter computes `v as f64` and then re-rounds an `F32`
                // (`normalize_float`); this takes the same literal two-step
                // int→f64→f32 path. (The double rounding is provably
                // innocuous — 53 ≥ 2·24+2 — so it equals a direct int→f32
                // conversion; the two-step form keeps the correspondence to
                // the interpreter self-evident.)
                let from = self.operand_int_kind(operand)?;
                let wide = if is_signed(from) {
                    self.builder.ins().fcvt_from_sint(types::F64, value)
                } else {
                    self.builder.ins().fcvt_from_uint(types::F64, value)
                };
                Ok(match target {
                    FloatKind::F64 => wide,
                    FloatKind::F32 => self.builder.ins().fdemote(types::F32, wide),
                })
            }
            CastKind::FloatToInt => {
                let Ty::Int(target) = to else {
                    return Err(CodegenError::backend("float-to-int cast to a non-integer"));
                };
                // Cranelift's saturating conversions truncate toward zero,
                // saturate to the TARGET's range, and map NaN to 0 — exactly
                // the interpreter's `saturating_float_to_int`. Never traps.
                let to_ty = int_type(*target);
                Ok(if is_signed(*target) {
                    self.builder.ins().fcvt_to_sint_sat(to_ty, value)
                } else {
                    self.builder.ins().fcvt_to_uint_sat(to_ty, value)
                })
            }
            CastKind::FloatToFloat => {
                let Ty::Float(target) = to else {
                    return Err(CodegenError::backend("float-to-float cast to a non-float"));
                };
                let Some(Ty::Float(from)) = self.operand_ty(operand) else {
                    return Err(CodegenError::backend("float-to-float cast of a non-float"));
                };
                // IEEE 754 conversion: exact when widening, round to nearest
                // even when narrowing — the interpreter's `normalize_float`.
                Ok(match (from, target) {
                    (FloatKind::F32, FloatKind::F64) => {
                        self.builder.ins().fpromote(types::F64, value)
                    }
                    (FloatKind::F64, FloatKind::F32) => {
                        self.builder.ins().fdemote(types::F32, value)
                    }
                    (FloatKind::F32, FloatKind::F32) | (FloatKind::F64, FloatKind::F64) => value,
                })
            }
        }
    }

    /// Resize an integer from `from` to `target` width with two's-complement
    /// wrapping (truncate when narrowing; sign/zero-extend from the source's
    /// signedness when widening) — matching the interpreter's `wrap_int`.
    fn resize_int(&mut self, value: ClifValue, from: IntKind, target: IntKind) -> ClifValue {
        let from_bits = int_width_bits(from);
        let to_bits = int_width_bits(target);
        let to_ty = int_type(target);
        if to_bits == from_bits {
            value
        } else if to_bits < from_bits {
            self.builder.ins().ireduce(to_ty, value)
        } else if is_signed(from) {
            self.builder.ins().sextend(to_ty, value)
        } else {
            self.builder.ins().uextend(to_ty, value)
        }
    }

    // ----- places -----

    /// Read the scalar value of `place`. An empty projection on a scalar local
    /// reads its SSA variable; a non-empty projection resolves to a leaf byte
    /// address (which must be a scalar leaf in Stage 1) and loads it.
    fn read_place(&mut self, place: &Place) -> Result<ClifValue, CodegenError> {
        if place.projection.is_empty() {
            let local = place.local.0 as usize;
            return match &self.kinds[local] {
                LocalKind::Scalar(var, _) => Ok(self.builder.use_var(*var)),
                LocalKind::ScalarSlot(slot, ty) => {
                    let (slot, ty) = (*slot, *ty);
                    Ok(self.builder.ins().stack_load(ty, slot, 0))
                }
                LocalKind::Borrowed { var, ty } => {
                    // A scalar borrowed parameter: load through the caller's
                    // pointer. (A whole-aggregate read goes through the
                    // aggregate path, exactly as for `Aggregate` locals.)
                    let var = *var;
                    let ty = ty.clone();
                    let clif = require_scalar(&ty, &self.function.name).map_err(|_| {
                        CodegenError::backend(
                            "reading a whole aggregate as a scalar (should go through the \
                             aggregate path)",
                        )
                    })?;
                    let addr = self.builder.use_var(var);
                    Ok(self.builder.ins().load(clif, MemFlags::trusted(), addr, 0))
                }
                LocalKind::Unit => Err(CodegenError::backend(
                    "reading a scalar value from a unit local",
                )),
                LocalKind::Aggregate { .. } => Err(CodegenError::backend(
                    "reading a whole aggregate as a scalar (should go through the aggregate path)",
                )),
            };
        }
        // Projected read: walk to the leaf address and load the scalar there.
        let (addr, leaf_ty) = self.place_address(place)?;
        let clif = require_scalar(&leaf_ty, &self.function.name)?;
        Ok(self.builder.ins().load(clif, MemFlags::trusted(), addr, 0))
    }

    /// Write the scalar `value` to `place`. An empty projection on a scalar local
    /// defines its SSA variable; a non-empty projection resolves to a leaf byte
    /// address (a scalar leaf in Stage 1) and stores there. A unit destination
    /// carries no value.
    fn write_place(&mut self, place: &Place, value: ClifValue) -> Result<(), CodegenError> {
        if place.projection.is_empty() {
            let local = place.local.0 as usize;
            return match &self.kinds[local] {
                LocalKind::Scalar(var, _) => {
                    self.builder.def_var(*var, value);
                    Ok(())
                }
                LocalKind::ScalarSlot(slot, _) => {
                    let slot = *slot;
                    self.builder.ins().stack_store(value, slot, 0);
                    Ok(())
                }
                LocalKind::Borrowed { var, .. } => {
                    // A `mut` scalar parameter: store through the caller's
                    // pointer, so the write is visible to the caller with no
                    // copy-back.
                    let var = *var;
                    let addr = self.builder.use_var(var);
                    self.builder
                        .ins()
                        .store(MemFlags::trusted(), value, addr, 0);
                    Ok(())
                }
                // A unit-typed destination carries no value; skip it.
                LocalKind::Unit => Ok(()),
                LocalKind::Aggregate { .. } => Err(CodegenError::backend(
                    "writing a scalar into a whole aggregate local (should go through the \
                     aggregate path)",
                )),
            };
        }
        let (addr, _leaf_ty) = self.place_address(place)?;
        self.builder
            .ins()
            .store(MemFlags::trusted(), value, addr, 0);
        Ok(())
    }

    /// The shared address walk: resolve `place` to a `(byte address, leaf type)`
    /// pair by starting at the root local's slot base and advancing by each
    /// projection's ABI offset. A `Field`/`VariantField` step advances by a
    /// constant ABI offset; an `Index` step advances by the runtime index value
    /// times the element stride — **unchecked**, because MIR asserts the bounds
    /// (`Assert { IndexOutOfBounds }`) before every `Index` use, exactly like
    /// the interpreter's post-check access.
    ///
    /// This is the single place both scalar field read/write and whole-aggregate
    /// operations compute a field address, so the two code paths cannot diverge.
    fn place_address(&mut self, place: &Place) -> Result<(ClifValue, Ty), CodegenError> {
        let local = place.local.0 as usize;
        let (mut addr, mut cur_ty) = match &self.kinds[local] {
            LocalKind::Aggregate { slot, ty } => {
                let base = self.builder.ins().stack_addr(self.pointer_type, *slot, 0);
                (base, ty.clone())
            }
            // A borrowed parameter's incoming pointer is the base address of
            // the caller's place; projections advance from it exactly as from
            // a local slot.
            LocalKind::Borrowed { var, ty } => {
                let (var, ty) = (*var, ty.clone());
                (self.builder.use_var(var), ty)
            }
            // A zero-sized aggregate root (e.g. `[T; 0]`) classifies as unit
            // storage yet may still be projected in MIR — only in code the
            // preceding bounds `Assert` makes unreachable at runtime (any index
            // into a zero-length array traps first). Give it a null base; no
            // load/store computed from it can ever execute.
            LocalKind::Unit if !place.projection.is_empty() => (
                self.builder.ins().iconst(self.pointer_type, 0),
                self.function.locals[local].ty.clone(),
            ),
            LocalKind::Scalar(..) | LocalKind::ScalarSlot(..) | LocalKind::Unit
                if place.projection.is_empty() =>
            {
                return Err(CodegenError::backend(
                    "place_address called on a non-aggregate local with no projection",
                ));
            }
            LocalKind::Scalar(..) | LocalKind::ScalarSlot(..) | LocalKind::Unit => {
                // A projection on a scalar local is impossible in well-formed
                // MIR; refuse rather than mis-address.
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
                    let offset = i64::try_from(offset)
                        .map_err(|_| CodegenError::backend("field offset exceeds i64"))?;
                    addr = self.builder.ins().iadd_imm(addr, offset);
                }
                Projection::VariantField { variant, field } => {
                    let offsets = variant_field_offsets(&cur_ty, *variant as usize, self.types)
                        .map_err(|error| self.layout_error(error))?;
                    let f = *field as usize;
                    let offset = *offsets
                        .get(f)
                        .ok_or_else(|| CodegenError::backend("variant field index out of range"))?;
                    cur_ty = self.variant_field_type(&cur_ty, *variant as usize, f)?;
                    let offset = i64::try_from(offset)
                        .map_err(|_| CodegenError::backend("field offset exceeds i64"))?;
                    addr = self.builder.ins().iadd_imm(addr, offset);
                }
                Projection::Index(index_local) => {
                    // Only the fixed `[T; N]` is indexable natively; element
                    // `i` lives at `i × stride(T)` (the ABI's inline layout).
                    let element = match &cur_ty {
                        Ty::FixedArray(element, _) => (**element).clone(),
                        _ => {
                            return Err(CodegenError::unsupported(
                                "indexing the growable `Array[T]` is not lowered by the \
                                 Cranelift backend (only the fixed `[T; N]` is)",
                            ));
                        }
                    };
                    let stride = i64::try_from(self.layout(&element)?.stride())
                        .map_err(|_| CodegenError::backend("element stride exceeds i64"))?;
                    // The index is a `Usize` scalar local, pre-asserted in
                    // bounds by the MIR before this use.
                    let index_value = match &self.kinds[index_local.0 as usize] {
                        LocalKind::Scalar(var, _) => self.builder.use_var(*var),
                        LocalKind::ScalarSlot(slot, ty) => {
                            let (slot, ty) = (*slot, *ty);
                            self.builder.ins().stack_load(ty, slot, 0)
                        }
                        _ => {
                            return Err(CodegenError::backend("array index local is not a scalar"));
                        }
                    };
                    let scaled = self.builder.ins().imul_imm(index_value, stride);
                    addr = self.builder.ins().iadd(addr, scaled);
                    cur_ty = element;
                }
            }
        }
        Ok((addr, cur_ty))
    }

    /// Materialize a `Rvalue::Aggregate` in place into `dest`'s byte storage.
    ///
    /// For an enum-like type the u32 discriminant is stored at offset 0 first;
    /// for a struct/tuple no tag is written. Each field operand is then stored at
    /// its ABI offset — a scalar field with a scalar store, a nested-aggregate
    /// field with a whole-aggregate memcpy from its source slot. A fixed-array
    /// aggregate takes its own path (element `i` at `i × stride`).
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
                    "range construction is not lowered by the Cranelift backend (Stage 2)",
                ));
            }
            tuo_mir::AggregateKind::Array { element, len } => {
                return self.lower_array_aggregate(dest, element, *len, fields);
            }
        };

        // The destination base address (bare aggregate local, or a projected
        // whole-aggregate slot).
        let base = self.aggregate_dest_address(dest)?;

        // Enum-like: store the u32 tag at offset 0. Struct/tuple: no tag.
        let enum_like = matches!(ty, Ty::Enum(..) | Ty::Option(_) | Ty::Result(..));
        if enum_like {
            let tag = self.builder.ins().iconst(types::I32, i64::from(variant));
            self.builder.ins().store(MemFlags::trusted(), tag, base, 0);
        }

        // Field offsets: struct/tuple vs enum variant.
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
            // A unit / zero-sized field contributes no store.
            if self.operand_is_unit(operand) {
                continue;
            }
            let offset = i64::try_from(offsets[i])
                .map_err(|_| CodegenError::backend("field offset exceeds i64"))?;
            let field_addr = self.builder.ins().iadd_imm(base, offset);
            if self.operand_is_aggregate(operand) {
                // Nested aggregate field: memcpy from its source slot.
                let field_ty = self
                    .operand_ty(operand)
                    .ok_or_else(|| CodegenError::backend("aggregate field has no static type"))?;
                let layout = self.layout(&field_ty)?;
                let src = self.operand_aggregate_address(operand)?;
                self.emit_memcpy(field_addr, src, layout);
            } else {
                // Scalar field: evaluate and store.
                let value = self.lower_operand(operand)?;
                self.builder
                    .ins()
                    .store(MemFlags::trusted(), value, field_addr, 0);
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
    /// mirroring `lower_aggregate`'s field dispatch.
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
                .and_then(|offset| i64::try_from(offset).ok())
                .ok_or_else(|| CodegenError::backend("array element offset exceeds i64"))?;
            let element_addr = self.builder.ins().iadd_imm(base, offset);
            if self.operand_is_aggregate(operand) {
                let src = self.operand_aggregate_address(operand)?;
                self.emit_memcpy(element_addr, src, elem_layout);
            } else {
                let value = self.lower_operand(operand)?;
                self.builder
                    .ins()
                    .store(MemFlags::trusted(), value, element_addr, 0);
            }
        }
        Ok(())
    }

    /// The base address to materialize an aggregate into, for an `Assign` whose
    /// destination is an aggregate. A bare aggregate local uses its slot; a
    /// projected destination resolves through the address walk.
    fn aggregate_dest_address(&mut self, dest: &Place) -> Result<ClifValue, CodegenError> {
        if dest.projection.is_empty() {
            return match &self.kinds[dest.local.0 as usize] {
                LocalKind::Aggregate { slot, .. } => {
                    Ok(self.builder.ins().stack_addr(self.pointer_type, *slot, 0))
                }
                // A whole-aggregate write to a `mut` borrowed parameter goes
                // straight through the caller's pointer (no copy-back).
                LocalKind::Borrowed { var, .. } => Ok(self.builder.use_var(*var)),
                _ => Err(CodegenError::backend(
                    "aggregate rvalue assigned to a non-aggregate local",
                )),
            };
        }
        let (addr, _leaf) = self.place_address(dest)?;
        Ok(addr)
    }

    /// Lower `Rvalue::Discriminant`: load the u32 tag at offset 0 of the enum
    /// value and zero-extend it to a `Usize` (i64), matching the interpreter's
    /// `Value::Int(discr, Usize)`.
    fn lower_discriminant(&mut self, place: &Place) -> Result<ClifValue, CodegenError> {
        let base = if place.projection.is_empty() {
            match &self.kinds[place.local.0 as usize] {
                LocalKind::Aggregate { slot, .. } => {
                    self.builder.ins().stack_addr(self.pointer_type, *slot, 0)
                }
                // The discriminant of a borrowed enum parameter reads through
                // the caller's pointer.
                LocalKind::Borrowed { var, .. } => self.builder.use_var(*var),
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
        // Load the u32 tag, then zero-extend to i64 (Usize). Never truncate.
        let tag = self
            .builder
            .ins()
            .load(types::I32, MemFlags::trusted(), base, 0);
        Ok(self.builder.ins().uextend(types::I64, tag))
    }

    // ----- traps -----

    /// If `condition` is true, trap with `code`: branch to a fresh trap block
    /// that calls the runtime and never returns; otherwise fall through to a
    /// fresh continuation block.
    fn guard(&mut self, condition: ClifValue, code: TrapCode) {
        let trap_block = self.builder.create_block();
        let continue_block = self.builder.create_block();
        self.builder
            .ins()
            .brif(condition, trap_block, &[], continue_block, &[]);
        self.builder.switch_to_block(trap_block);
        self.builder.seal_block(trap_block);
        self.emit_trap_call(code);
        self.builder.switch_to_block(continue_block);
        self.builder.seal_block(continue_block);
    }

    /// Emit a call to the runtime trap symbol with `code`, then an unreachable
    /// terminator (the call never returns). Terminates the current block.
    fn emit_trap_call(&mut self, code: TrapCode) {
        let func_ref = self.trap_func_ref();
        let code_value = self
            .builder
            .ins()
            .iconst(types::I32, i64::from(code.as_i32()));
        self.builder.ins().call(func_ref, &[code_value]);
        self.builder
            .ins()
            .trap(cranelift_codegen::ir::TrapCode::user(1).unwrap());
    }

    /// Declare (once per body, on demand) and reference the runtime trap symbol.
    fn trap_func_ref(&mut self) -> cranelift_codegen::ir::FuncRef {
        let mut signature = Signature::new(CallConv::triple_default(self.module.isa().triple()));
        signature.params.push(AbiParam::new(types::I32));
        let trap_id = self
            .module
            .declare_function(TRAP_SYMBOL, Linkage::Import, &signature)
            .expect("declaring the runtime trap symbol");
        self.module.declare_func_in_func(trap_id, self.builder.func)
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
            Operand::Const(Const::Float(_, kind)) => Some(Ty::Float(*kind)),
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

    /// Whether an operand is unit-valued (a unit constant, or a bare-local place
    /// of unit/zero-sized type). Such an operand contributes no native value.
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

    /// The base address of the aggregate an operand names: a `Copy`/`Move` of a
    /// bare aggregate local's slot, or of a projected place whose leaf is an
    /// aggregate (e.g. a struct-typed array element), resolved through the
    /// shared address walk. Used for whole-aggregate copies/moves. A move
    /// leaves the source husk untouched: a v0 aggregate owns no heap, so no
    /// zeroing is needed (the ownership checker forbids re-reading a moved value).
    fn operand_aggregate_address(&mut self, operand: &Operand) -> Result<ClifValue, CodegenError> {
        match operand {
            Operand::Copy(place) | Operand::Move(place) if place.projection.is_empty() => {
                match &self.kinds[place.local.0 as usize] {
                    LocalKind::Aggregate { slot, .. } => {
                        Ok(self.builder.ins().stack_addr(self.pointer_type, *slot, 0))
                    }
                    // A whole-aggregate read of a borrowed parameter copies
                    // from the caller's memory through the incoming pointer.
                    LocalKind::Borrowed { var, .. } => Ok(self.builder.use_var(*var)),
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

    /// Turn an ABI [`LayoutError`](tuo_runtime::abi::LayoutError) into a backend
    /// error. Reaching one after classification means the MIR named a shape the
    /// ABI cannot lay out — a backend/verifier fault, not a user error.
    fn layout_error(&self, error: tuo_runtime::abi::LayoutError) -> CodegenError {
        CodegenError::backend(format!("ABI layout failed during lowering: {error}"))
    }

    /// Copy `layout.size` bytes from `src` to `dest`, both aggregate slot
    /// addresses, using the aggregate's alignment. A zero-sized copy is a no-op.
    fn emit_memcpy(&mut self, dest: ClifValue, src: ClifValue, layout: Layout) {
        if layout.size == 0 {
            return;
        }
        let align = u8::try_from(layout.align).unwrap_or(u8::MAX);
        self.builder.emit_small_memory_copy(
            self.frontend_config,
            dest,
            src,
            layout.size,
            align,
            align,
            true,
            MemFlags::trusted(),
        );
    }
}

/// A trapping arithmetic operation.
#[derive(Clone, Copy)]
enum ArithOp {
    Add,
    Sub,
    Mul,
}

/// The Cranelift condition code for a comparison operator, or `None` if `op` is
/// not a comparison. `signed` selects signed vs unsigned ordering.
fn comparison_code(op: BinOp, signed: bool) -> Option<IntCC> {
    Some(match op {
        BinOp::Eq => IntCC::Equal,
        BinOp::Ne => IntCC::NotEqual,
        BinOp::Lt if signed => IntCC::SignedLessThan,
        BinOp::Lt => IntCC::UnsignedLessThan,
        BinOp::Le if signed => IntCC::SignedLessThanOrEqual,
        BinOp::Le => IntCC::UnsignedLessThanOrEqual,
        BinOp::Gt if signed => IntCC::SignedGreaterThan,
        BinOp::Gt => IntCC::UnsignedGreaterThan,
        BinOp::Ge if signed => IntCC::SignedGreaterThanOrEqual,
        BinOp::Ge => IntCC::UnsignedGreaterThanOrEqual,
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => return None,
    })
}

/// The Cranelift float condition code for a comparison operator, or `None` if
/// `op` is not a comparison. The four orderings are **ordered** (false when
/// either operand is NaN) and `NotEqual` is **unordered-or-unequal** (true on
/// NaN) — exactly Rust's float comparisons, which the interpreter uses.
fn float_comparison_code(op: BinOp) -> Option<FloatCC> {
    Some(match op {
        BinOp::Eq => FloatCC::Equal,
        BinOp::Ne => FloatCC::NotEqual,
        BinOp::Lt => FloatCC::LessThan,
        BinOp::Le => FloatCC::LessThanOrEqual,
        BinOp::Gt => FloatCC::GreaterThan,
        BinOp::Ge => FloatCC::GreaterThanOrEqual,
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => return None,
    })
}

/// The base-2 logarithm of a power-of-two alignment, as the `align_shift` a
/// Cranelift [`StackSlotData`] expects. The ABI guarantees `align` is a power of
/// two ≥ 1, so `trailing_zeros` is exact.
fn log2_align(align: u64) -> u8 {
    // A power-of-two u64's trailing-zero count is at most 63.
    u8::try_from(align.max(1).trailing_zeros()).unwrap_or(0)
}

/// The runtime trap code for a MIR terminator trap.
fn trap_code_of(trap: Trap) -> TrapCode {
    match trap {
        Trap::IndexOutOfBounds => TrapCode::IndexOutOfBounds,
        Trap::Unreachable => TrapCode::Unreachable,
    }
}

/// The inclusive `(min, max)` bounds of an integer kind, as `i64` (the widest
/// register the backend uses). `Usize`/`U64`'s max exceeds `i64`, so its upper
/// bound is represented by `-1` reinterpreted at 64 bits — but the backend only
/// consults `min` for the negation/overflow checks, where this is exact.
fn int_bounds(kind: IntKind) -> (i64, i64) {
    match kind {
        IntKind::I8 => (i64::from(i8::MIN), i64::from(i8::MAX)),
        IntKind::I16 => (i64::from(i16::MIN), i64::from(i16::MAX)),
        IntKind::I32 => (i64::from(i32::MIN), i64::from(i32::MAX)),
        IntKind::I64 | IntKind::Isize => (i64::MIN, i64::MAX),
        IntKind::U8 => (0, i64::from(u8::MAX)),
        IntKind::U16 => (0, i64::from(u16::MAX)),
        IntKind::U32 => (0, i64::from(u32::MAX)),
        IntKind::U64 | IntKind::Usize => (0, -1),
    }
}

/// The exported symbol name of a MIR function, from its stable symbol id.
///
/// It is derived only from the symbol (its identity), never the source name, so
/// two functions can never collide and the name is stable across renames.
fn mangle(symbol: SymbolId) -> String {
    format!("tuo_fn_{}", symbol.as_u32())
}

/// Emit a native `main` that calls the entry function and returns its value as
/// the process exit status.
///
/// `entry_id` is the [`FuncId`] the entry was declared under in this module (so
/// the call resolves directly, with no symbol-name juggling), and `entry_kind`
/// is its integer return kind (the caller checked the entry is nullary and
/// returns an integer). `main`'s standard signature is `() -> i32`; the entry's
/// return is truncated/extended to `i32`, exactly as a C `main` return becomes
/// the exit code — and exactly the value the interpreter observes running the
/// same entry.
pub(crate) fn emit_main_shim(
    module: &mut ObjectModule,
    entry_id: FuncId,
    entry_kind: IntKind,
) -> Result<(), CodegenError> {
    let call_conv = module.isa().default_call_conv();
    let mut main_sig = Signature::new(call_conv);
    main_sig.returns.push(AbiParam::new(types::I32));

    let main_id = module
        .declare_function("main", Linkage::Export, &main_sig)
        .map_err(|error| CodegenError::backend(format!("declaring main: {error}")))?;

    let mut ctx = module.make_context();
    ctx.func.signature = main_sig;

    let mut builder_ctx = FunctionBuilderContext::new();
    let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
    let block = builder.create_block();
    builder.switch_to_block(block);
    builder.seal_block(block);

    let entry_ref = module.declare_func_in_func(entry_id, builder.func);
    let call = builder.ins().call(entry_ref, &[]);
    let value = builder.inst_results(call)[0];

    // Reduce/extend the entry's integer return to i32 for the exit code. A
    // narrower signed source sign-extends; a narrower unsigned source
    // zero-extends; a wider source truncates. (The observable exit code is the
    // low bits regardless, matching a C `main`.)
    let from_ty = int_type(entry_kind);
    let ret = if from_ty == types::I32 {
        value
    } else if from_ty.bits() > 32 {
        builder.ins().ireduce(types::I32, value)
    } else if is_signed(entry_kind) {
        builder.ins().sextend(types::I32, value)
    } else {
        builder.ins().uextend(types::I32, value)
    };
    builder.ins().return_(&[ret]);
    builder.finalize();

    module
        .define_function(main_id, &mut ctx)
        .map_err(|error| CodegenError::backend(format!("defining main: {error}")))?;
    module.clear_context(&mut ctx);
    Ok(())
}
