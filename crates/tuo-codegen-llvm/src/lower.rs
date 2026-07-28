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
//! Each MIR local becomes an `alloca` in the function's entry block, read with
//! `load` and written with `store`. v0's supported locals are all scalars, so
//! the slot is a single integer. The standard optimization pipeline promotes
//! these to registers (`mem2reg`); at `-O0` they stay in memory, still correct.

use std::collections::HashMap;

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
    BinOp, CastKind, Const, Function, Operand, Place, Program, Rvalue, Statement, Terminator, Trap,
    UnOp,
};
use tuo_resolve::SymbolId;
use tuo_runtime::{TRAP_SYMBOL, TrapCode};
use tuo_types::{IntKind, Ty};

use crate::abi::{int_type, int_width_bits, is_signed, scalar_type};

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
) -> Result<HashMap<SymbolId, FunctionValue<'ctx>>, CodegenError> {
    // Pass 1: declare every function so direct calls can reference them before
    // their bodies are defined.
    let mut ids: HashMap<SymbolId, FunctionValue<'ctx>> = HashMap::new();
    for function in &program.functions {
        let fn_type = function_type(ctx, function)?;
        let name = mangle(function.symbol);
        let value = module.add_function(&name, fn_type, Some(FUNCTION_LINKAGE));
        ids.insert(function.symbol, value);
    }

    // Pass 2: define each body.
    let builder = ctx.create_builder();
    for function in &program.functions {
        let mut lowering = Lowering::new(ctx, module, &builder, &ids, function)?;
        lowering.run()?;
    }
    Ok(ids)
}

/// The LLVM function type of a MIR function (v0 scalar ABI).
fn function_type<'ctx>(
    ctx: &'ctx Context,
    function: &Function,
) -> Result<inkwell::types::FunctionType<'ctx>, CodegenError> {
    // Every parameter must be a supported scalar passed by value. Borrow-mode
    // parameters are not lowered yet (they alias caller memory).
    let mut params: Vec<BasicMetadataTypeEnum<'ctx>> = Vec::with_capacity(function.params.len());
    for (index, mode) in function.params.iter().enumerate() {
        if *mode != tuo_mir::PassMode::Value {
            return Err(CodegenError::unsupported(format!(
                "`{}` takes a borrow-mode parameter, which the LLVM backend does not lower yet",
                function.name
            )));
        }
        let ty = &function.locals[index].ty;
        params.push(require_scalar(ctx, ty, &function.name)?.into());
    }

    // A unit return is modelled as `void`; any other non-scalar is unsupported.
    Ok(if matches!(function.ret, Ty::Unit) {
        ctx.void_type().fn_type(&params, false)
    } else {
        require_scalar(ctx, &function.ret, &function.name)?.fn_type(&params, false)
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
    value: FunctionValue<'ctx>,
    /// The LLVM block for each MIR block index.
    blocks: Vec<LlvmBlock<'ctx>>,
    /// The register type of each local (index = local id), for supported scalar
    /// locals; `None` for a `Unit` local (which carries no value).
    local_types: Vec<Option<IntType<'ctx>>>,
    /// The stack slot backing each MIR local (index = local id). A `Unit` local
    /// gets a placeholder `i8` slot that is never read meaningfully.
    slots: Vec<PointerValue<'ctx>>,
}

impl<'a, 'ctx> Lowering<'a, 'ctx> {
    fn new(
        ctx: &'ctx Context,
        module: &'a Module<'ctx>,
        builder: &'a Builder<'ctx>,
        ids: &'a HashMap<SymbolId, FunctionValue<'ctx>>,
        function: &'a Function,
    ) -> Result<Self, CodegenError> {
        // Resolve the register type of every local up front; a non-scalar,
        // non-unit local makes the function unsupported.
        let mut local_types = Vec::with_capacity(function.locals.len());
        for local in &function.locals {
            if matches!(local.ty, Ty::Unit) {
                local_types.push(None);
            } else {
                local_types.push(Some(require_scalar(ctx, &local.ty, &function.name)?));
            }
        }
        let value = ids[&function.symbol];
        Ok(Self {
            ctx,
            module,
            builder,
            ids,
            function,
            value,
            blocks: Vec::new(),
            local_types,
            slots: Vec::new(),
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

        let i8 = self.ctx.i8_type();
        self.slots = self
            .local_types
            .iter()
            .enumerate()
            .map(|(index, local_type)| {
                let ty = local_type.unwrap_or(i8);
                self.builder
                    .build_alloca(ty, &format!("local{index}"))
                    .map_err(builder_err("allocating a local slot"))
            })
            .collect::<Result<_, _>>()?;

        // Store each incoming parameter into its slot.
        for index in 0..self.function.params.len() {
            let param = self
                .value
                .get_nth_param(index as u32)
                .ok_or_else(|| CodegenError::backend("missing LLVM parameter"))?
                .into_int_value();
            self.builder
                .build_store(self.slots[index], param)
                .map_err(builder_err("storing a parameter"))?;
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
            Statement::Assign { place, rvalue } => {
                let value = self.lower_rvalue(rvalue)?;
                self.write_place(place, value)
            }
            Statement::Call { dest, callee, args } => self.lower_call(dest, *callee, args),
            Statement::Drop { .. } => {
                // v0 supported values own no host resource and every drop of a
                // scalar is a no-op (the interpreter's drop of a scalar frees
                // only its budget). Nothing to emit.
                Ok(())
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
        let mut arg_values = Vec::with_capacity(args.len());
        for arg in args {
            match arg {
                tuo_mir::Arg::Value(operand) => {
                    arg_values.push(self.lower_operand(operand)?.into())
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
        // A unit-returning callee yields no result; a scalar callee yields one.
        if let inkwell::values::ValueKind::Basic(value) = call.try_as_basic_value() {
            self.write_place(dest, value.into_int_value())?;
        }
        Ok(())
    }

    // ----- terminators -----

    fn lower_terminator(&mut self, terminator: &Terminator) -> Result<(), CodegenError> {
        match terminator {
            Terminator::Return(operand) => {
                if matches!(self.function.ret, Ty::Unit) {
                    self.builder
                        .build_return(None)
                        .map_err(builder_err("emitting a unit return"))?;
                } else {
                    let value = self.lower_operand(operand)?;
                    self.builder
                        .build_return(Some(&value as &dyn BasicValue))
                        .map_err(builder_err("emitting a return"))?;
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
            Rvalue::Aggregate { .. } => Err(CodegenError::unsupported(
                "aggregate construction is not lowered by the LLVM backend yet",
            )),
            Rvalue::Discriminant(_) => Err(CodegenError::unsupported(
                "enum discriminants are not lowered by the LLVM backend yet",
            )),
            Rvalue::Len(_) => Err(CodegenError::unsupported(
                "array length is not lowered by the LLVM backend yet",
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

    fn read_place(&mut self, place: &Place) -> Result<IntValue<'ctx>, CodegenError> {
        if !place.projection.is_empty() {
            return Err(CodegenError::unsupported(
                "projected places (fields, indices) are not lowered by the LLVM backend yet",
            ));
        }
        let index = place.local.0 as usize;
        let ty = self.local_types[index].unwrap_or_else(|| self.ctx.i8_type());
        let slot = self.slots[index];
        Ok(self
            .builder
            .build_load(ty, slot, "load")
            .map_err(builder_err("loading a local"))?
            .into_int_value())
    }

    fn write_place(&mut self, place: &Place, value: IntValue<'ctx>) -> Result<(), CodegenError> {
        if !place.projection.is_empty() {
            return Err(CodegenError::unsupported(
                "projected places (fields, indices) are not lowered by the LLVM backend yet",
            ));
        }
        let index = place.local.0 as usize;
        // A unit-typed destination carries no value; skip it.
        if self.local_types[index].is_none() {
            return Ok(());
        }
        self.builder
            .build_store(self.slots[index], value)
            .map_err(builder_err("storing a local"))?;
        Ok(())
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

    /// The static type of an operand, from the local's declared type or the
    /// constant's kind. Only the scalar shapes the backend supports are
    /// resolved; others return `None`.
    fn operand_ty(&self, operand: &Operand) -> Option<Ty> {
        match operand {
            Operand::Copy(place) | Operand::Move(place) if place.projection.is_empty() => {
                Some(self.function.locals[place.local.0 as usize].ty.clone())
            }
            Operand::Const(Const::Int(_, kind)) => Some(Ty::Int(*kind)),
            Operand::Const(Const::Bool(_)) => Some(Ty::Bool),
            Operand::Const(Const::Char(_)) => Some(Ty::Char),
            _ => None,
        }
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
