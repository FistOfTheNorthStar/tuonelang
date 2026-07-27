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
//! Each MIR local becomes a Cranelift [`Variable`], read and written by index.
//! v0's supported locals are all scalars (one register), so no stack slots are
//! needed. A local that never holds a supported scalar (only aggregates, say)
//! makes the whole function unsupported.

use std::collections::HashMap;

use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{AbiParam, InstBuilder, Signature, Type, Value as ClifValue, types};
use cranelift_codegen::isa::CallConv;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Switch, Variable};
use cranelift_module::{FuncId, Linkage, Module};
use cranelift_object::ObjectModule;

use tuo_codegen::CodegenError;
use tuo_mir::{
    BinOp, CastKind, Const, Function, Operand, Place, Program, Rvalue, Statement, Terminator, Trap,
    UnOp,
};
use tuo_resolve::SymbolId;
use tuo_runtime::{TRAP_SYMBOL, TrapCode};
use tuo_types::{IntKind, Ty, TypeckResult};

use crate::abi::{int_type, int_width_bits, is_signed, scalar_type};
use crate::{CodegenCtx, FUNCTION_LINKAGE};

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
    _types: &TypeckResult,
) -> Result<HashMap<SymbolId, FuncId>, CodegenError> {
    // Pass 1: declare every function so direct calls can reference them before
    // their bodies are defined.
    let mut ids: HashMap<SymbolId, FuncId> = HashMap::new();
    for function in &program.functions {
        let signature = function_signature(module, function)?;
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
        define_function(module, &mut ctx, &mut builder_ctx, &ids, function)?;
    }
    Ok(ids)
}

/// The Cranelift signature of a MIR function (v0 scalar ABI).
fn function_signature(
    module: &ObjectModule,
    function: &Function,
) -> Result<Signature, CodegenError> {
    let call_conv = module.isa().default_call_conv();
    let mut signature = Signature::new(call_conv);
    // Every parameter must be a supported scalar passed by value. Borrow-mode
    // parameters are not lowered yet (they alias caller memory).
    for (index, mode) in function.params.iter().enumerate() {
        if *mode != tuo_mir::PassMode::Value {
            return Err(CodegenError::unsupported(format!(
                "`{}` takes a borrow-mode parameter, which the Cranelift backend does not \
                 lower yet",
                function.name
            )));
        }
        let ty = &function.locals[index].ty;
        let clif = require_scalar(ty, &function.name)?;
        signature.params.push(AbiParam::new(clif));
    }
    // A unit return is modelled as no return value; any other non-scalar is
    // unsupported.
    if !matches!(function.ret, Ty::Unit) {
        let clif = require_scalar(&function.ret, &function.name)?;
        signature.returns.push(AbiParam::new(clif));
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
) -> Result<(), CodegenError> {
    let signature = function_signature(module, function)?;
    let self_id = ids[&function.symbol];

    ctx.context_mut().func.signature = signature;
    let mut lowering = Lowering::new(module, ctx.context_mut(), builder_ctx, ids, function)?;
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
    /// The Cranelift block for each MIR block index.
    blocks: Vec<cranelift_codegen::ir::Block>,
    /// The register type of each local (index = local id), for supported
    /// scalar locals; `None` for a `Unit` local (which carries no value).
    local_types: Vec<Option<Type>>,
    /// The Cranelift variable backing each MIR local (index = local id).
    locals: Vec<Variable>,
}

impl<'a> Lowering<'a> {
    fn new(
        module: &'a mut ObjectModule,
        context: &'a mut cranelift_codegen::Context,
        builder_ctx: &'a mut FunctionBuilderContext,
        ids: &'a HashMap<SymbolId, FuncId>,
        function: &'a Function,
    ) -> Result<Self, CodegenError> {
        // Resolve the register type of every local up front; a non-scalar,
        // non-unit local makes the function unsupported.
        let mut local_types = Vec::with_capacity(function.locals.len());
        for local in &function.locals {
            if matches!(local.ty, Ty::Unit) {
                local_types.push(None);
            } else {
                local_types.push(Some(require_scalar(&local.ty, &function.name)?));
            }
        }

        let builder = FunctionBuilder::new(&mut context.func, builder_ctx);
        Ok(Self {
            builder,
            module,
            ids,
            function,
            blocks: Vec::new(),
            local_types,
            locals: Vec::new(),
        })
    }

    /// Lower the whole body.
    fn run(&mut self) -> Result<(), CodegenError> {
        // Create a Cranelift block per MIR block.
        self.blocks = (0..self.function.blocks.len())
            .map(|_| self.builder.create_block())
            .collect();

        // Declare every local as a Variable of its register type. `Unit` locals
        // get a placeholder i8 variable that is never read meaningfully. The
        // returned Variables are stored so later code addresses a local by its
        // MIR index.
        let local_types = self.local_types.clone();
        self.locals = local_types
            .iter()
            .map(|local_type| self.builder.declare_var(local_type.unwrap_or(types::I8)))
            .collect();

        // Entry block: append the parameters and seed the parameter locals.
        let entry = self.blocks[0];
        self.builder.append_block_params_for_function_params(entry);
        self.builder.switch_to_block(entry);
        let param_values: Vec<ClifValue> = self.builder.block_params(entry).to_vec();
        for (index, value) in param_values.into_iter().enumerate() {
            let var = self.locals[index];
            self.builder.def_var(var, value);
        }

        // Seed non-parameter locals with a zero of their type so a Variable is
        // always defined before use on every path (the ownership checker
        // guarantees no *semantic* read of an uninitialized local, but
        // Cranelift's SSA construction still requires a definition to exist).
        for index in self.function.params.len()..self.function.locals.len() {
            if let Some(ty) = self.local_types[index] {
                let zero = self.builder.ins().iconst(ty, 0);
                let var = self.locals[index];
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
            Statement::Assign { place, rvalue } => {
                let value = self.lower_rvalue(rvalue)?;
                self.write_place(place, value)?;
                Ok(())
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
        let Some(&callee_id) = self.ids.get(&callee) else {
            return Err(CodegenError::unsupported(
                "call to a function outside the lowered program (v0 has no external calls)",
            ));
        };
        let mut arg_values = Vec::with_capacity(args.len());
        for arg in args {
            match arg {
                tuo_mir::Arg::Value(operand) => arg_values.push(self.lower_operand(operand)?),
                tuo_mir::Arg::Borrow(_) | tuo_mir::Arg::BorrowMut(_) => {
                    return Err(CodegenError::unsupported(
                        "borrow-mode call arguments are not lowered by the Cranelift backend yet",
                    ));
                }
            }
        }
        let func_ref = self
            .module
            .declare_func_in_func(callee_id, self.builder.func);
        let call = self.builder.ins().call(func_ref, &arg_values);
        let results = self.builder.inst_results(call);
        // A unit-returning callee yields no result; a scalar callee yields one.
        if let Some(&result) = results.first() {
            self.write_place(dest, result)?;
        }
        Ok(())
    }

    // ----- terminators -----

    fn lower_terminator(&mut self, terminator: &Terminator) -> Result<(), CodegenError> {
        match terminator {
            Terminator::Return(operand) => {
                if matches!(self.function.ret, Ty::Unit) {
                    self.builder.ins().return_(&[]);
                } else {
                    let value = self.lower_operand(operand)?;
                    self.builder.ins().return_(&[value]);
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
            Rvalue::Aggregate { .. } => Err(CodegenError::unsupported(
                "aggregate construction is not lowered by the Cranelift backend yet",
            )),
            Rvalue::Discriminant(_) => Err(CodegenError::unsupported(
                "enum discriminants are not lowered by the Cranelift backend yet",
            )),
            Rvalue::Len(_) => Err(CodegenError::unsupported(
                "array length is not lowered by the Cranelift backend yet",
            )),
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
            Const::Unit => Err(CodegenError::unsupported(
                "a unit constant has no scalar representation to place in a value context",
            )),
            Const::Float(..) => Err(CodegenError::unsupported(
                "float constants are not lowered by the Cranelift backend yet",
            )),
            Const::Str(_) => Err(CodegenError::unsupported(
                "string constants are not lowered by the Cranelift backend yet",
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
            CastKind::IntToFloat | CastKind::FloatToInt | CastKind::FloatToFloat => {
                Err(CodegenError::unsupported(
                    "float casts are not lowered by the Cranelift backend yet",
                ))
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

    fn read_place(&mut self, place: &Place) -> Result<ClifValue, CodegenError> {
        if !place.projection.is_empty() {
            return Err(CodegenError::unsupported(
                "projected places (fields, indices) are not lowered by the Cranelift backend yet",
            ));
        }
        let var = self.locals[place.local.0 as usize];
        Ok(self.builder.use_var(var))
    }

    fn write_place(&mut self, place: &Place, value: ClifValue) -> Result<(), CodegenError> {
        if !place.projection.is_empty() {
            return Err(CodegenError::unsupported(
                "projected places (fields, indices) are not lowered by the Cranelift backend yet",
            ));
        }
        // A unit-typed destination carries no value; skip it.
        if self.local_types[place.local.0 as usize].is_none() {
            return Ok(());
        }
        let var = self.locals[place.local.0 as usize];
        self.builder.def_var(var, value);
        Ok(())
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
