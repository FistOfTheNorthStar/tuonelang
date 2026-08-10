//! The MIR interpreter proper: the deterministic execution engine.
//!
//! [`Interpreter`] executes **verified** MIR by walking the control-flow
//! graph of one function, evaluating each statement and terminator exactly as
//! documented on [`tuo_mir`]'s instruction types. It is the reference
//! semantics: where a native backend and this interpreter disagree, the
//! backend is wrong.
//!
//! The engine is deterministic and total in the sense that matters: it either
//! returns a value or aborts with a structured [`RuntimeError`] — it never
//! panics on any input it accepts, and it refuses (via the mandatory
//! [`tuo_mir::verify`] gate) any input it cannot execute. Evaluation order is
//! the textual order MIR fixes; there is no undefined behavior to leave
//! unspecified.

use std::collections::HashMap;

use tuo_mir::{
    AggregateKind, Arg, BinOp, BlockId, CastKind, Const, Function, LocalId, Operand, Place,
    Program, Projection, Rvalue, Statement, Terminator, Trap, UnOp,
};
use tuo_resolve::SymbolId;
use tuo_source::Span;
use tuo_types::{FloatKind, IntKind, Ty};

use crate::config::Limits;
use crate::trace::{Trace, TraceEvent};
use crate::trap::{Frame, RuntimeError, TrapKind};
use crate::value::Value;

/// The outcome of running a function: its return value, or a structured abort.
pub type RunResult = Result<Value, RuntimeError>;

/// A prepared interpreter over one verified [`Program`].
///
/// Build it with [`Interpreter::new`] (which enforces the mandatory
/// verification gate), optionally set [`Limits`] and enable tracing, then
/// [`Interpreter::run`] an entry function by name.
pub struct Interpreter<'p> {
    program: &'p Program,
    by_symbol: HashMap<SymbolId, usize>,
    by_name: HashMap<String, usize>,
    limits: Limits,
    trace_enabled: bool,
}

impl<'p> Interpreter<'p> {
    /// Prepare to interpret `program`.
    ///
    /// The MIR **must verify**: this constructor runs [`tuo_mir::verify`] and
    /// refuses unverified MIR, exactly as every backend must. On failure it
    /// returns the verifier's structured diagnostics (never a panic), so a
    /// caller cannot accidentally execute malformed MIR.
    ///
    /// # Errors
    ///
    /// Returns the verifier diagnostics if `program` does not verify against
    /// `types`.
    pub fn new(
        program: &'p Program,
        types: &tuo_types::TypeckResult,
    ) -> Result<Self, Vec<tuo_diagnostics::Diagnostic>> {
        let problems = tuo_mir::verify(program, types);
        if !problems.is_empty() {
            return Err(problems);
        }
        let mut by_symbol = HashMap::new();
        let mut by_name = HashMap::new();
        for (index, function) in program.functions.iter().enumerate() {
            by_symbol.insert(function.symbol, index);
            by_name.insert(function.name.clone(), index);
        }
        Ok(Self {
            program,
            by_symbol,
            by_name,
            limits: Limits::default(),
            trace_enabled: false,
        })
    }

    /// Set the resource limits for subsequent runs (builder style).
    #[must_use]
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// Enable structured tracing for subsequent runs (builder style).
    #[must_use]
    pub fn with_trace(mut self) -> Self {
        self.trace_enabled = true;
        self
    }

    /// Run the entry function named `name` with `args` as its parameter
    /// values, returning its result value or a structured abort.
    ///
    /// `args` must supply exactly one value per parameter, in declaration
    /// order. Borrow-mode parameters receive their value the same way (the
    /// interpreter has no caller place to alias for a top-level entry).
    ///
    /// # Errors
    ///
    /// Returns a [`RuntimeError`] on any language trap, resource-limit abort,
    /// or internal violation (an unknown entry name, or an arity mismatch).
    pub fn run(&self, name: &str, args: Vec<Value>) -> RunResult {
        let mut trace = Trace::new();
        self.run_traced(name, args, &mut trace)
    }

    /// Like [`Self::run`], but also records a structured [`Trace`] into
    /// `trace` (only populated when the interpreter was built
    /// [`with_trace`](Self::with_trace)).
    ///
    /// # Errors
    ///
    /// As [`Self::run`].
    pub fn run_traced(&self, name: &str, args: Vec<Value>, trace: &mut Trace) -> RunResult {
        let Some(&index) = self.by_name.get(name) else {
            // No natural span for a missing entry; attribute it to the first
            // function if the program has one, else a synthetic placeholder.
            let span = self
                .program
                .functions
                .first()
                .map_or_else(placeholder_span, |function| function.span);
            return Err(RuntimeError {
                kind: TrapKind::Internal(format!("no entry function named `{name}`")),
                message: format!("the program has no function named `{name}` to run"),
                span,
                backtrace: Vec::new(),
            });
        };
        let entry = &self.program.functions[index];
        let mut machine = Machine {
            interp: self,
            fuel: self.limits.fuel,
            live_values: 0,
            trace_enabled: self.trace_enabled,
            trace: Trace::new(),
            stack: Vec::new(),
        };
        let outcome = machine.call(entry, args).map(|(value, _finals)| value);
        // The trace records the whole run regardless of outcome (including the
        // trap event, if any), so the caller can inspect a failed execution.
        *trace = std::mem::take(&mut machine.trace);
        outcome
    }

    /// Look up a called function by its symbol.
    ///
    /// The returned reference is tied to the program's lifetime `'p`, not to
    /// `&self`, so callers may hold it across `&mut self` interpreter calls.
    fn function_of(&self, symbol: SymbolId) -> Option<&'p Function> {
        self.by_symbol
            .get(&symbol)
            .map(|&index| &self.program.functions[index])
    }
}

/// The mutable machine state threaded through one whole execution.
struct Machine<'i, 'p> {
    interp: &'i Interpreter<'p>,
    /// Remaining instruction fuel; each executed statement/terminator costs 1.
    fuel: u64,
    /// The current count of live values across every stack frame — a
    /// backend-independent memory proxy.
    live_values: u64,
    trace_enabled: bool,
    /// The structured execution trace, accumulated when tracing is on (an
    /// empty, untouched `Trace` costs nothing otherwise).
    trace: Trace,
    /// The call-stack backtrace, outermost frame first; the last entry is the
    /// currently-executing frame.
    stack: Vec<Frame>,
}

/// A single stack frame's storage: one slot per local.
struct Locals {
    slots: Vec<Slot>,
}

/// One local slot: either holding a value or uninitialized/moved-out.
enum Slot {
    Init(Value),
    Uninit,
}

impl Locals {
    fn new(count: usize) -> Self {
        Self {
            slots: (0..count).map(|_| Slot::Uninit).collect(),
        }
    }

    fn get(&self, local: LocalId) -> Option<&Value> {
        match self.slots.get(local.0 as usize)? {
            Slot::Init(value) => Some(value),
            Slot::Uninit => None,
        }
    }

    fn get_mut(&mut self, local: LocalId) -> Option<&mut Value> {
        match self.slots.get_mut(local.0 as usize)? {
            Slot::Init(value) => Some(value),
            Slot::Uninit => None,
        }
    }

    fn set(&mut self, local: LocalId, value: Value) {
        if let Some(slot) = self.slots.get_mut(local.0 as usize) {
            *slot = Slot::Init(value);
        }
    }

    fn take(&mut self, local: LocalId) -> Option<Value> {
        let slot = self.slots.get_mut(local.0 as usize)?;
        match std::mem::replace(slot, Slot::Uninit) {
            Slot::Init(value) => Some(value),
            Slot::Uninit => None,
        }
    }
}

impl Machine<'_, '_> {
    /// Execute one call: push the frame, seed the parameter locals, run the
    /// CFG to a `Return`, then tear the frame down. Returns both the result
    /// and the callee's final parameter locals, so the caller can copy the
    /// final state of any `mut` parameter back into its own place (a borrow
    /// lives only for the call, so copy-in/copy-back is observably identical
    /// to aliasing). The entry call ignores the returned finals.
    fn call(
        &mut self,
        function: &Function,
        args: Vec<Value>,
    ) -> Result<(Value, Vec<Value>), RuntimeError> {
        // Recursion ceiling (the entry frame counts as depth 1).
        if self.stack.len() as u32 >= self.interp.limits.max_call_depth {
            return Err(self.abort(
                TrapKind::RecursionLimit,
                format!(
                    "call stack exceeded the configured depth of {}",
                    self.interp.limits.max_call_depth
                ),
                function.span,
            ));
        }
        // Arity: params occupy locals `0 .. params.len()`.
        if args.len() != function.params.len() {
            return Err(self.abort(
                TrapKind::Internal(format!(
                    "arity mismatch calling `{}`: expected {} argument(s), got {}",
                    function.name,
                    function.params.len(),
                    args.len()
                )),
                "interpreter was given the wrong number of arguments".to_owned(),
                function.span,
            ));
        }

        self.stack.push(Frame {
            function: function.name.clone(),
            span: function.span,
        });
        if self.trace_enabled {
            self.trace.push(TraceEvent::Call {
                function: function.name.clone(),
                depth: self.stack.len() as u32,
            });
        }

        let param_count = function.params.len();
        let mut locals = Locals::new(function.locals.len());
        for (index, value) in args.into_iter().enumerate() {
            self.account_value(&value, function.span)?;
            locals.set(LocalId(index as u32), value);
        }

        let result = self.run_body(function, &mut locals);

        // Snapshot the final state of the parameter locals for writeback,
        // before releasing the frame. A `mut` parameter that the callee left
        // uninitialized (moved out) writes back nothing meaningful, so a
        // moved-out slot is represented as `Unit` and simply not used.
        let finals: Vec<Value> = (0..param_count)
            .map(|index| {
                locals
                    .get(LocalId(index as u32))
                    .cloned()
                    .unwrap_or(Value::Unit)
            })
            .collect();

        // A frame's locals die with the frame (deterministically, no
        // destructors — Constitution §24); reclaim their budget.
        self.release_locals(&locals);
        self.stack.pop();
        result.map(|value| (value, finals))
    }

    /// Run a function's basic-block graph from block 0 to a `Return`.
    fn run_body(&mut self, function: &Function, locals: &mut Locals) -> RunResult {
        let mut block = BlockId(0);
        loop {
            let index = block.0 as usize;
            let Some(basic_block) = function.blocks.get(index) else {
                // The verifier proves all edges land in-range; reaching here
                // would be an internal contradiction.
                return Err(self.abort(
                    TrapKind::Internal(format!("jumped to nonexistent block bb{index}")),
                    "control transferred to a block that does not exist".to_owned(),
                    function.span,
                ));
            };
            if self.trace_enabled {
                self.trace.push(TraceEvent::Block { block: block.0 });
            }
            for statement in &basic_block.statements {
                self.charge_fuel(function, locals, statement_span(function, statement))?;
                self.exec_statement(function, locals, statement)?;
            }
            self.charge_fuel(function, locals, function.span)?;
            match self.exec_terminator(function, locals, &basic_block.terminator)? {
                Flow::Goto(next) => block = next,
                Flow::Return(value) => return Ok(value),
            }
        }
    }

    // ----- statements -----

    fn exec_statement(
        &mut self,
        function: &Function,
        locals: &mut Locals,
        statement: &Statement,
    ) -> Result<(), RuntimeError> {
        if self.trace_enabled {
            self.trace.push(TraceEvent::Statement {
                detail: render_statement(statement),
            });
        }
        match statement {
            Statement::Assign { place, rvalue } => {
                let value = self.eval_rvalue(function, locals, rvalue)?;
                self.write_place(function, locals, place, value)?;
                Ok(())
            }
            Statement::Call { dest, callee, args } => {
                self.exec_call(function, locals, dest, *callee, args)
            }
            Statement::Drop { place } => {
                // Drop ends the value's initialization. v0 has no user
                // destructors (ADR-0003) and every runtime value the
                // interpreter models owns no host resource, so the observable
                // effect is exactly de-initialization: read the place out and
                // discard it, freeing its budget.
                let value = self.read_place_moving(function, locals, place)?;
                self.release_value(&value);
                Ok(())
            }
        }
    }

    fn exec_call(
        &mut self,
        function: &Function,
        locals: &mut Locals,
        dest: &Place,
        callee: SymbolId,
        args: &[Arg],
    ) -> Result<(), RuntimeError> {
        // Look up the callee. `function_of` borrows the program, which
        // outlives every frame, so the reference is safe to hold across the
        // nested `call`.
        let Some(target) = self.interp.function_of(callee) else {
            return Err(self.abort(
                TrapKind::Internal("call to a function outside the lowered program".to_owned()),
                "the interpreter only executes functions present in the lowered MIR (v0 has \
                 no indirect or external calls)"
                    .to_owned(),
                function.span,
            ));
        };

        // Evaluate arguments left to right into by-value parameter slots.
        // Every argument — value, `in`, or `mut` — is passed as an owned copy
        // of the caller's value; a `mut` argument additionally records the
        // caller place to copy the callee's final parameter state back into.
        let mut values = Vec::with_capacity(args.len());
        let mut writeback_places: Vec<Option<Place>> = Vec::with_capacity(args.len());
        for arg in args {
            match arg {
                Arg::Value(operand) => {
                    values.push(self.eval_operand(function, locals, operand)?);
                    writeback_places.push(None);
                }
                Arg::Borrow(place) => {
                    values.push(self.read_place_copying(function, locals, place)?);
                    writeback_places.push(None);
                }
                Arg::BorrowMut(place) => {
                    values.push(self.read_place_copying(function, locals, place)?);
                    writeback_places.push(Some(place.clone()));
                }
            }
        }

        let (ret, finals) = self.call(target, values)?;

        // Copy each `mut` parameter's final state back into its caller place.
        for (position, place) in writeback_places.into_iter().enumerate() {
            if let Some(place) = place {
                let value = finals.get(position).cloned().unwrap_or(Value::Unit);
                self.write_place(function, locals, &place, value)?;
            }
        }

        self.write_place(function, locals, dest, ret)?;
        Ok(())
    }

    // ----- terminators -----

    fn exec_terminator(
        &mut self,
        function: &Function,
        locals: &mut Locals,
        terminator: &Terminator,
    ) -> Result<Flow, RuntimeError> {
        match terminator {
            Terminator::Return(operand) => {
                let value = self.eval_operand(function, locals, operand)?;
                if self.trace_enabled {
                    self.trace.push(TraceEvent::Return {
                        value: value.clone(),
                    });
                }
                Ok(Flow::Return(value))
            }
            Terminator::Goto(target) => Ok(Flow::Goto(*target)),
            Terminator::Branch {
                cond,
                then_block,
                else_block,
            } => {
                let value = self.eval_operand(function, locals, cond)?;
                let taken = value.as_bool().ok_or_else(|| {
                    self.abort(
                        TrapKind::Internal("branch condition was not a Bool".to_owned()),
                        "the verifier guarantees Bool branch conditions".to_owned(),
                        function.span,
                    )
                })?;
                Ok(Flow::Goto(if taken { *then_block } else { *else_block }))
            }
            Terminator::Switch {
                discr,
                arms,
                otherwise,
            } => {
                let value = self.eval_operand(function, locals, discr)?;
                let scrutinee = self.switch_key(function, &value)?;
                for (arm_value, target) in arms {
                    if *arm_value == scrutinee {
                        return Ok(Flow::Goto(*target));
                    }
                }
                Ok(Flow::Goto(*otherwise))
            }
            Terminator::Assert { cond, trap, target } => {
                let value = self.eval_operand(function, locals, cond)?;
                let ok = value.as_bool().ok_or_else(|| {
                    self.abort(
                        TrapKind::Internal("assert condition was not a Bool".to_owned()),
                        "the verifier guarantees Bool assert conditions".to_owned(),
                        function.span,
                    )
                })?;
                if ok {
                    Ok(Flow::Goto(*target))
                } else {
                    Err(self.trap_of(*trap, function.span))
                }
            }
            Terminator::Trap(trap) => Err(self.trap_of(*trap, function.span)),
        }
    }

    /// The integer key a `Switch` compares against: an integer's value, a
    /// char's scalar value, or an enum-like value's discriminant.
    fn switch_key(&mut self, function: &Function, value: &Value) -> Result<i128, RuntimeError> {
        match value {
            Value::Int(v, _) => Ok(*v),
            Value::Char(c) => Ok(i128::from(u32::from(*c))),
            Value::Bool(b) => Ok(i128::from(*b)),
            Value::Variant { variant, .. } => Ok(i128::from(*variant)),
            other => Err(self.abort(
                TrapKind::Internal(format!(
                    "switch on a non-integer-classed value: {}",
                    other.render()
                )),
                "the verifier guarantees switch operands are integer-classed".to_owned(),
                function.span,
            )),
        }
    }

    // ----- rvalues & operands -----

    fn eval_rvalue(
        &mut self,
        function: &Function,
        locals: &mut Locals,
        rvalue: &Rvalue,
    ) -> RunResult {
        match rvalue {
            Rvalue::Use(operand) => self.eval_operand(function, locals, operand),
            Rvalue::Unary { op, operand } => {
                let value = self.eval_operand(function, locals, operand)?;
                self.eval_unary(function, *op, value)
            }
            Rvalue::Binary { op, lhs, rhs } => {
                let l = self.eval_operand(function, locals, lhs)?;
                let r = self.eval_operand(function, locals, rhs)?;
                self.eval_binary(function, *op, l, r)
            }
            Rvalue::Cast { kind, operand, to } => {
                let value = self.eval_operand(function, locals, operand)?;
                self.eval_cast(function, *kind, value, to)
            }
            Rvalue::Aggregate { kind, fields } => {
                let mut values = Vec::with_capacity(fields.len());
                for field in fields {
                    values.push(self.eval_operand(function, locals, field)?);
                }
                Ok(match kind {
                    AggregateKind::Adt { variant, .. } => Value::Variant {
                        variant: *variant,
                        fields: values,
                    },
                    AggregateKind::Range => Value::Aggregate(values),
                    // A fixed-size array `[T; N]` (ADR-0004 Stage 2): the
                    // operands are its elements in index order. Indexing,
                    // drops, and `value_cost` over `Value::Array` already
                    // apply; `Rvalue::Len` never sees one (the verifier
                    // rejects it), so the `Internal`-trap guard there stays
                    // honest.
                    AggregateKind::Array { .. } => Value::Array(values),
                })
            }
            Rvalue::Discriminant(place) => {
                let value = self.read_place_copying(function, locals, place)?;
                let discr = match &value {
                    Value::Variant { variant, .. } => i128::from(*variant),
                    other => {
                        return Err(self.abort(
                            TrapKind::Internal(format!(
                                "discriminant of a non-enum value: {}",
                                other.render()
                            )),
                            "the verifier guarantees Discriminant operands are enum-like"
                                .to_owned(),
                            function.span,
                        ));
                    }
                };
                Ok(Value::Int(discr, IntKind::Usize))
            }
            Rvalue::Len(place) => {
                let value = self.read_place_copying(function, locals, place)?;
                let len = match &value {
                    Value::Array(elements) => elements.len() as i128,
                    other => {
                        return Err(self.abort(
                            TrapKind::Internal(format!(
                                "length of a non-array value: {}",
                                other.render()
                            )),
                            "the verifier guarantees Len operands are arrays".to_owned(),
                            function.span,
                        ));
                    }
                };
                Ok(Value::Int(len, IntKind::Usize))
            }
        }
    }

    fn eval_operand(
        &mut self,
        function: &Function,
        locals: &mut Locals,
        operand: &Operand,
    ) -> RunResult {
        match operand {
            Operand::Copy(place) => self.read_place_copying(function, locals, place),
            Operand::Move(place) => self.read_place_moving(function, locals, place),
            Operand::Const(constant) => Ok(self.eval_const(constant)),
        }
    }

    fn eval_const(&self, constant: &Const) -> Value {
        match constant {
            Const::Unit => Value::Unit,
            Const::Bool(b) => Value::Bool(*b),
            Const::Int(v, kind) => Value::Int(*v, *kind),
            Const::Float(v, kind) => Value::Float(normalize_float(*v, *kind), *kind),
            Const::Char(c) => Value::Char(*c),
            Const::Str(s) => Value::Str(s.clone()),
        }
    }

    // ----- arithmetic -----

    fn eval_unary(&mut self, function: &Function, op: UnOp, value: Value) -> RunResult {
        match op {
            UnOp::Not => match value {
                Value::Bool(b) => Ok(Value::Bool(!b)),
                other => Err(self.type_bug(function, "unary `!` on a non-Bool", &other)),
            },
            UnOp::Neg => match value {
                Value::Int(v, kind) => {
                    // Two's-complement negation traps on the minimum (§24).
                    let (min, _max) = int_bounds(kind);
                    if v == min {
                        return Err(self.abort(
                            TrapKind::IntegerOverflow,
                            format!("negating the minimum {} value overflows", kind.name()),
                            function.span,
                        ));
                    }
                    Ok(Value::Int(-v, kind))
                }
                Value::Float(v, kind) => Ok(Value::Float(-v, kind)),
                other => Err(self.type_bug(function, "unary `-` on a non-number", &other)),
            },
        }
    }

    fn eval_binary(&mut self, function: &Function, op: BinOp, lhs: Value, rhs: Value) -> RunResult {
        match (lhs, rhs) {
            (Value::Int(a, ka), Value::Int(b, kb)) => {
                debug_assert_eq!(ka, kb, "verifier guarantees matching operand kinds");
                self.int_binary(function, op, a, b, ka)
            }
            (Value::Float(a, ka), Value::Float(b, _kb)) => Ok(self.float_binary(op, a, b, ka)),
            (Value::Bool(a), Value::Bool(b)) => match op {
                BinOp::Eq => Ok(Value::Bool(a == b)),
                BinOp::Ne => Ok(Value::Bool(a != b)),
                _ => Err(self.type_bug(function, "unsupported Bool operator", &Value::Bool(a))),
            },
            (Value::Char(a), Value::Char(b)) => Ok(Value::Bool(match op {
                BinOp::Eq => a == b,
                BinOp::Ne => a != b,
                BinOp::Lt => a < b,
                BinOp::Le => a <= b,
                BinOp::Gt => a > b,
                BinOp::Ge => a >= b,
                _ => {
                    return Err(self.type_bug(
                        function,
                        "unsupported Char operator",
                        &Value::Char(a),
                    ));
                }
            })),
            (Value::Str(a), Value::Str(b)) => Ok(Value::Bool(match op {
                BinOp::Eq => a == b,
                BinOp::Ne => a != b,
                BinOp::Lt => a < b,
                BinOp::Le => a <= b,
                BinOp::Gt => a > b,
                BinOp::Ge => a >= b,
                _ => {
                    return Err(self.type_bug(
                        function,
                        "unsupported Str operator",
                        &Value::Str(a),
                    ));
                }
            })),
            (a, _b) => Err(self.type_bug(function, "binary operands of mismatched class", &a)),
        }
    }

    fn int_binary(
        &mut self,
        function: &Function,
        op: BinOp,
        a: i128,
        b: i128,
        kind: IntKind,
    ) -> RunResult {
        let (min, max) = int_bounds(kind);
        let checked = |value: i128, this: &mut Self| -> Result<Value, RuntimeError> {
            if value < min || value > max {
                return Err(this.abort(
                    TrapKind::IntegerOverflow,
                    format!("{} arithmetic overflowed", kind.name()),
                    function.span,
                ));
            }
            Ok(Value::Int(value, kind))
        };
        match op {
            BinOp::Add => checked(a + b, self),
            BinOp::Sub => checked(a - b, self),
            BinOp::Mul => checked(a * b, self),
            BinOp::Div => {
                if b == 0 {
                    return Err(self.abort(
                        TrapKind::DivisionByZero,
                        "integer division by zero".to_owned(),
                        function.span,
                    ));
                }
                // `MIN / -1` overflows (the quotient is out of range).
                checked(a.wrapping_div(b), self)
            }
            BinOp::Rem => {
                if b == 0 {
                    return Err(self.abort(
                        TrapKind::DivisionByZero,
                        "integer remainder by zero".to_owned(),
                        function.span,
                    ));
                }
                // `MIN % -1` is defined as 0, but the corresponding division
                // overflows; the language traps this case (§24).
                if a == min && b == -1 {
                    return Err(self.abort(
                        TrapKind::IntegerOverflow,
                        format!("{} remainder overflowed (MIN % -1)", kind.name()),
                        function.span,
                    ));
                }
                Ok(Value::Int(a % b, kind))
            }
            BinOp::Eq => Ok(Value::Bool(a == b)),
            BinOp::Ne => Ok(Value::Bool(a != b)),
            BinOp::Lt => Ok(Value::Bool(a < b)),
            BinOp::Le => Ok(Value::Bool(a <= b)),
            BinOp::Gt => Ok(Value::Bool(a > b)),
            BinOp::Ge => Ok(Value::Bool(a >= b)),
        }
    }

    fn float_binary(&self, op: BinOp, a: f64, b: f64, kind: FloatKind) -> Value {
        match op {
            BinOp::Add => Value::Float(normalize_float(a + b, kind), kind),
            BinOp::Sub => Value::Float(normalize_float(a - b, kind), kind),
            BinOp::Mul => Value::Float(normalize_float(a * b, kind), kind),
            BinOp::Div => Value::Float(normalize_float(a / b, kind), kind),
            BinOp::Rem => Value::Float(normalize_float(a % b, kind), kind),
            BinOp::Eq => Value::Bool(a == b),
            BinOp::Ne => Value::Bool(a != b),
            BinOp::Lt => Value::Bool(a < b),
            BinOp::Le => Value::Bool(a <= b),
            BinOp::Gt => Value::Bool(a > b),
            BinOp::Ge => Value::Bool(a >= b),
        }
    }

    fn eval_cast(
        &mut self,
        function: &Function,
        kind: CastKind,
        value: Value,
        to: &Ty,
    ) -> RunResult {
        match (kind, value) {
            (CastKind::IntToInt, Value::Int(v, _)) => {
                let target = int_kind_of(to).ok_or_else(|| {
                    self.type_bug(function, "int-to-int cast to a non-integer", &Value::Unit)
                })?;
                Ok(Value::Int(wrap_int(v, target), target))
            }
            (CastKind::IntToFloat, Value::Int(v, _)) => {
                let target = float_kind_of(to).ok_or_else(|| {
                    self.type_bug(function, "int-to-float cast to a non-float", &Value::Unit)
                })?;
                Ok(Value::Float(normalize_float(v as f64, target), target))
            }
            (CastKind::FloatToInt, Value::Float(v, _)) => {
                let target = int_kind_of(to).ok_or_else(|| {
                    self.type_bug(function, "float-to-int cast to a non-integer", &Value::Unit)
                })?;
                Ok(Value::Int(saturating_float_to_int(v, target), target))
            }
            (CastKind::FloatToFloat, Value::Float(v, _)) => {
                let target = float_kind_of(to).ok_or_else(|| {
                    self.type_bug(function, "float-to-float cast to a non-float", &Value::Unit)
                })?;
                Ok(Value::Float(normalize_float(v, target), target))
            }
            (_, other) => Err(self.type_bug(function, "cast of a mismatched value", &other)),
        }
    }

    // ----- places -----

    /// Read a place by copying (leaves the source initialized). Used for
    /// `Copy` operands, borrows, discriminants, lengths, and writeback
    /// captures.
    fn read_place_copying(
        &mut self,
        function: &Function,
        locals: &mut Locals,
        place: &Place,
    ) -> RunResult {
        let root = locals.get(place.local).ok_or_else(|| {
            self.abort(
                TrapKind::Internal(format!("read of uninitialized local _{}", place.local.0)),
                "the verifier guarantees no reads of uninitialized locals".to_owned(),
                function.span,
            )
        })?;
        self.project(function, locals, root.clone(), &place.projection)
    }

    /// Read a place by moving (de-initializes the *root* local). Used for
    /// `Move` operands and `Drop`. A bare move de-initializes the local; a
    /// **projected** (partial) move extracts one sub-field and leaves the
    /// local's *other* fields readable — lowering emits partial moves
    /// (`_1 = move _0.0; _2 = copy _0.1`), so de-initializing the whole root
    /// would wrongly poison the siblings. The moved sub-field itself is never
    /// read again (the ownership checker guarantees it), so it is replaced
    /// with a `Unit` husk rather than tracked with per-field init flags.
    fn read_place_moving(
        &mut self,
        function: &Function,
        locals: &mut Locals,
        place: &Place,
    ) -> RunResult {
        if place.projection.is_empty() {
            return locals.take(place.local).ok_or_else(|| {
                self.abort(
                    TrapKind::Internal(format!("move of uninitialized local _{}", place.local.0)),
                    "the verifier guarantees no moves of uninitialized locals".to_owned(),
                    function.span,
                )
            });
        }
        // Projected move: read the sub-value out, then blank *that field path*
        // to a husk, leaving sibling fields of the root intact.
        let value = self.read_place_copying(function, locals, place)?;
        self.write_place(function, locals, place, Value::Unit)?;
        Ok(value)
    }

    /// Apply a projection path to a value, resolving `Index` locals through
    /// the frame.
    fn project(
        &mut self,
        function: &Function,
        locals: &Locals,
        mut value: Value,
        projection: &[Projection],
    ) -> RunResult {
        for step in projection {
            value = match (step, value) {
                // `Field` names a struct/tuple/range field. Structs lower to a
                // single-variant ADT, so their storage is a `Variant`; tuples
                // and ranges use `Aggregate`. Both are positional.
                (Projection::Field(index), Value::Aggregate(mut fields))
                | (Projection::Field(index), Value::Variant { mut fields, .. }) => {
                    field_at(&mut fields, *index).ok_or_else(|| {
                        self.abort(
                            TrapKind::Internal(format!("field {index} out of range")),
                            "the verifier guarantees in-range field projections".to_owned(),
                            function.span,
                        )
                    })?
                }
                (Projection::VariantField { field, .. }, Value::Variant { fields, .. }) => {
                    let mut fields = fields;
                    field_at(&mut fields, *field).ok_or_else(|| {
                        self.abort(
                            TrapKind::Internal(format!("variant field {field} out of range")),
                            "the verifier guarantees valid variant-field projections".to_owned(),
                            function.span,
                        )
                    })?
                }
                (Projection::Index(index_local), Value::Array(elements)) => {
                    let index = locals.get(*index_local).and_then(Value::as_int);
                    let Some((raw, _)) = index else {
                        return Err(self.abort(
                            TrapKind::Internal("index local is not an integer".to_owned()),
                            "the verifier guarantees Index locals are Usize".to_owned(),
                            function.span,
                        ));
                    };
                    if raw < 0 || raw as usize >= elements.len() {
                        return Err(self.abort(
                            TrapKind::IndexOutOfBounds,
                            format!(
                                "index {raw} out of bounds for array of length {}",
                                elements.len()
                            ),
                            function.span,
                        ));
                    }
                    elements[raw as usize].clone()
                }
                (step, other) => {
                    return Err(self.abort(
                        TrapKind::Internal(format!(
                            "projection {step:?} does not apply to {}",
                            other.render()
                        )),
                        "the verifier guarantees well-typed projections".to_owned(),
                        function.span,
                    ));
                }
            };
        }
        Ok(value)
    }

    /// Write `value` into `place`, following its projection into the existing
    /// aggregate/array/variant at the root local.
    fn write_place(
        &mut self,
        function: &Function,
        locals: &mut Locals,
        place: &Place,
        value: Value,
    ) -> Result<(), RuntimeError> {
        if place.projection.is_empty() {
            self.account_value(&value, function.span)?;
            locals.set(place.local, value);
            return Ok(());
        }
        // Resolve the array indices up front (immutable borrow), then mutate.
        let mut indices = Vec::new();
        for step in &place.projection {
            if let Projection::Index(index_local) = step {
                let raw = locals.get(*index_local).and_then(Value::as_int);
                let Some((raw, _)) = raw else {
                    return Err(self.abort(
                        TrapKind::Internal("index local is not an integer".to_owned()),
                        "the verifier guarantees Index locals are Usize".to_owned(),
                        function.span,
                    ));
                };
                indices.push(raw);
            }
        }
        let root = locals.get_mut(place.local).ok_or_else(|| RuntimeError {
            kind: TrapKind::Internal(format!("write to uninitialized local _{}", place.local.0)),
            message: "the verifier guarantees the destination root is live".to_owned(),
            span: function.span,
            backtrace: Vec::new(),
        })?;
        let span = function.span;
        let backtrace = self.stack.clone();
        write_projected(root, &place.projection, &indices, value, span, &backtrace)
    }

    // ----- traps & accounting -----

    fn trap_of(&mut self, trap: Trap, span: Span) -> RuntimeError {
        let (kind, message) = match trap {
            Trap::IndexOutOfBounds => (
                TrapKind::IndexOutOfBounds,
                "array index out of bounds".to_owned(),
            ),
            Trap::Unreachable => (
                TrapKind::Unreachable,
                "reached a point proved unreachable".to_owned(),
            ),
        };
        self.abort(kind, message, span)
    }

    /// Build a structured abort, snapshotting the current backtrace and
    /// recording a trace event if tracing is on.
    fn abort(&mut self, kind: TrapKind, message: String, span: Span) -> RuntimeError {
        if self.trace_enabled {
            self.trace.push(TraceEvent::Trap {
                label: kind.label().to_owned(),
                span,
            });
        }
        let mut backtrace = self.stack.clone();
        // Innermost frame first.
        backtrace.reverse();
        // Attribute the innermost frame to the failing span.
        if let Some(frame) = backtrace.first_mut() {
            frame.span = span;
        }
        RuntimeError {
            kind,
            message,
            span,
            backtrace,
        }
    }

    fn type_bug(&mut self, function: &Function, what: &str, value: &Value) -> RuntimeError {
        self.abort(
            TrapKind::Internal(format!("{what}: {}", value.render())),
            "the verifier should have rejected this MIR".to_owned(),
            function.span,
        )
    }

    /// Charge one unit of fuel or trap. `span` attributes the abort.
    fn charge_fuel(
        &mut self,
        _function: &Function,
        _locals: &Locals,
        span: Span,
    ) -> Result<(), RuntimeError> {
        match self.fuel.checked_sub(1) {
            Some(remaining) => {
                self.fuel = remaining;
                Ok(())
            }
            None => Err(self.abort(
                TrapKind::OutOfFuel,
                "instruction-fuel budget exhausted".to_owned(),
                span,
            )),
        }
    }

    /// Account for a newly live value, or trap if the budget is exceeded.
    fn account_value(&mut self, value: &Value, span: Span) -> Result<(), RuntimeError> {
        let cost = value_cost(value);
        self.live_values = self.live_values.saturating_add(cost);
        if self.live_values > self.interp.limits.max_live_values {
            return Err(self.abort(
                TrapKind::MemoryBudget,
                format!(
                    "live-value budget of {} exceeded",
                    self.interp.limits.max_live_values
                ),
                span,
            ));
        }
        Ok(())
    }

    fn release_value(&mut self, value: &Value) {
        let cost = value_cost(value);
        self.live_values = self.live_values.saturating_sub(cost);
    }

    fn release_locals(&mut self, locals: &Locals) {
        for slot in &locals.slots {
            if let Slot::Init(value) = slot {
                self.release_value(value);
            }
        }
    }
}

/// Control flow out of a block.
enum Flow {
    Goto(BlockId),
    Return(Value),
}

// ----- free helpers -----

/// Remove and return the `index`-th field of an aggregate/variant field list
/// by value (cloning it, since projection reads do not consume the source).
fn field_at(fields: &mut [Value], index: u32) -> Option<Value> {
    fields.get(index as usize).cloned()
}

/// Write `value` into a nested place inside `root`, following `projection`.
/// `indices` are the pre-resolved array indices, in projection order.
fn write_projected(
    root: &mut Value,
    projection: &[Projection],
    indices: &[i128],
    value: Value,
    span: Span,
    backtrace: &[Frame],
) -> Result<(), RuntimeError> {
    let internal = |message: String| RuntimeError {
        kind: TrapKind::Internal(message),
        message: "the verifier guarantees well-typed place writes".to_owned(),
        span,
        backtrace: {
            let mut bt = backtrace.to_vec();
            bt.reverse();
            bt
        },
    };
    let mut target = root;
    let mut index_cursor = 0usize;
    for (position, step) in projection.iter().enumerate() {
        let last = position + 1 == projection.len();
        match step {
            // `Field` addresses a positional field of a tuple/range
            // (`Aggregate`) or a struct (a single-variant `Variant`).
            Projection::Field(field) => {
                let fields = match target {
                    Value::Aggregate(fields) | Value::Variant { fields, .. } => fields,
                    _ => {
                        return Err(internal("field projection on a non-aggregate".to_owned()));
                    }
                };
                let slot = fields
                    .get_mut(*field as usize)
                    .ok_or_else(|| internal(format!("field {field} out of range")))?;
                if last {
                    *slot = value;
                    return Ok(());
                }
                target = slot;
            }
            Projection::VariantField { field, .. } => match target {
                Value::Variant { fields, .. } => {
                    let slot = fields
                        .get_mut(*field as usize)
                        .ok_or_else(|| internal(format!("variant field {field} out of range")))?;
                    if last {
                        *slot = value;
                        return Ok(());
                    }
                    target = slot;
                }
                _ => {
                    return Err(internal(
                        "variant-field projection on a non-enum".to_owned(),
                    ));
                }
            },
            Projection::Index(_) => {
                let raw = *indices
                    .get(index_cursor)
                    .ok_or_else(|| internal("missing resolved index".to_owned()))?;
                index_cursor += 1;
                match target {
                    Value::Array(elements) => {
                        if raw < 0 || raw as usize >= elements.len() {
                            return Err(RuntimeError {
                                kind: TrapKind::IndexOutOfBounds,
                                message: format!(
                                    "index {raw} out of bounds for array of length {}",
                                    elements.len()
                                ),
                                span,
                                backtrace: {
                                    let mut bt = backtrace.to_vec();
                                    bt.reverse();
                                    bt
                                },
                            });
                        }
                        let slot = &mut elements[raw as usize];
                        if last {
                            *slot = value;
                            return Ok(());
                        }
                        target = slot;
                    }
                    _ => return Err(internal("index projection on a non-array".to_owned())),
                }
            }
        }
    }
    // An empty projection is handled by the caller; reaching here means the
    // path had length 0, which cannot happen.
    Err(internal("empty projection in write_projected".to_owned()))
}

/// The inclusive `(min, max)` value range of an integer kind, as `i128`.
fn int_bounds(kind: IntKind) -> (i128, i128) {
    match kind {
        IntKind::I8 => (i128::from(i8::MIN), i128::from(i8::MAX)),
        IntKind::I16 => (i128::from(i16::MIN), i128::from(i16::MAX)),
        IntKind::I32 => (i128::from(i32::MIN), i128::from(i32::MAX)),
        // Isize is modelled as 64-bit (the interpreter's target-independent
        // choice; documented as such).
        IntKind::I64 | IntKind::Isize => (i128::from(i64::MIN), i128::from(i64::MAX)),
        IntKind::U8 => (0, i128::from(u8::MAX)),
        IntKind::U16 => (0, i128::from(u16::MAX)),
        IntKind::U32 => (0, i128::from(u32::MAX)),
        IntKind::U64 | IntKind::Usize => (0, i128::from(u64::MAX)),
    }
}

/// Wrap `value` into `kind`'s range (two's-complement truncation), for casts.
fn wrap_int(value: i128, kind: IntKind) -> i128 {
    let (min, max) = int_bounds(kind);
    let width = max - min + 1;
    if width == 0 {
        return value;
    }
    let mut wrapped = (value - min).rem_euclid(width) + min;
    // rem_euclid keeps it in range for non-degenerate widths.
    if wrapped > max {
        wrapped -= width;
    }
    wrapped
}

/// Truncate `value` toward zero then saturate to `kind`'s range; NaN → 0.
fn saturating_float_to_int(value: f64, kind: IntKind) -> i128 {
    let (min, max) = int_bounds(kind);
    if value.is_nan() {
        return 0;
    }
    let truncated = value.trunc();
    if truncated <= min as f64 {
        return min;
    }
    if truncated >= max as f64 {
        return max;
    }
    let as_i128 = truncated as i128;
    as_i128.clamp(min, max)
}

/// Round a float to its kind's precision (an `F32` stored at `f64` precision).
fn normalize_float(value: f64, kind: FloatKind) -> f64 {
    match kind {
        FloatKind::F64 => value,
        FloatKind::F32 => f64::from(value as f32),
    }
}

/// The integer kind named by a type, if it is an integer type.
fn int_kind_of(ty: &Ty) -> Option<IntKind> {
    match ty {
        Ty::Int(kind) => Some(*kind),
        _ => None,
    }
}

/// The float kind named by a type, if it is a float type.
fn float_kind_of(ty: &Ty) -> Option<FloatKind> {
    match ty {
        Ty::Float(kind) => Some(*kind),
        _ => None,
    }
}

/// A backend-independent "size" of a value for the live-value budget: one per
/// scalar, plus the sum of the fields for aggregates.
fn value_cost(value: &Value) -> u64 {
    match value {
        Value::Unit | Value::Bool(_) | Value::Int(..) | Value::Float(..) | Value::Char(_) => 1,
        Value::Str(s) => 1 + s.len() as u64,
        Value::Aggregate(fields) | Value::Variant { fields, .. } => {
            1 + fields.iter().map(value_cost).sum::<u64>()
        }
        Value::Array(elements) => 1 + elements.iter().map(value_cost).sum::<u64>(),
    }
}

/// The span attributed to a statement (all statements use the function span
/// today; kept as a hook for finer source mapping later).
fn statement_span(function: &Function, _statement: &Statement) -> Span {
    function.span
}

/// A synthetic span used only when there is genuinely no source location to
/// attribute an internal error to (e.g. an empty program with no functions).
fn placeholder_span() -> Span {
    Span::new(
        tuo_source::SourceId::from_raw(0),
        tuo_source::TextRange::new(0, 0).expect("empty range is forward"),
    )
}

/// A short, deterministic rendering of a statement for the trace.
fn render_statement(statement: &Statement) -> String {
    match statement {
        Statement::Assign { place, .. } => format!("assign _{}", place.local.0),
        Statement::Call { dest, .. } => format!("call -> _{}", dest.local.0),
        Statement::Drop { place } => format!("drop _{}", place.local.0),
    }
}

#[cfg(test)]
mod tests {
    use super::{Value, int_bounds, saturating_float_to_int, value_cost, wrap_int};
    use tuo_types::IntKind;

    #[test]
    fn a_partial_move_leaves_sibling_fields_readable() {
        // Mirror the shape lowering emits for a partial move (see the golden
        // `partial_move` fixture): a struct/tuple param whose first field is
        // *moved* out and whose second field is then *copied* — proving the
        // move de-initializes only the moved sub-field, not the whole root.
        //   fn partial(_0: (Int, Int)) -> Int {
        //     _1 = move _0.0
        //     _2 = copy _0.1
        //     return copy _2
        //   }
        use super::Interpreter;
        use tuo_mir::{
            BasicBlock, Function, LocalDecl, LocalId, Operand, Place, Program, Projection, Rvalue,
            Statement, Terminator,
        };
        use tuo_resolve::SymbolId;
        use tuo_source::{SourceId, Span, TextRange};
        use tuo_types::Ty;

        let span = Span::new(
            SourceId::from_raw(0),
            TextRange::new(0, 1).expect("forward"),
        );
        // Field 0 is an `Array` (non-`Copy`), so moving it is legal MIR (the
        // verifier forbids moving a `Copy` value); field 1 is a `Copy` `Int`
        // that is *copied* out afterward.
        let moved_ty = Ty::Array(Box::new(Ty::int()));
        let field_ty = Ty::int();
        let pair_ty = Ty::Tuple(vec![moved_ty.clone(), field_ty.clone()]);
        let decl = |ty: &Ty| LocalDecl {
            ty: ty.clone(),
            name: None,
            span,
        };
        let field = |index| Place {
            local: LocalId(0),
            projection: vec![Projection::Field(index)],
        };
        let function = Function {
            symbol: SymbolId::from_raw(0),
            name: "partial".to_owned(),
            params: vec![tuo_mir::PassMode::Value],
            locals: vec![decl(&pair_ty), decl(&moved_ty), decl(&field_ty)],
            blocks: vec![BasicBlock {
                statements: vec![
                    Statement::Assign {
                        place: Place::local(LocalId(1)),
                        rvalue: Rvalue::Use(Operand::Move(field(0))),
                    },
                    Statement::Assign {
                        place: Place::local(LocalId(2)),
                        rvalue: Rvalue::Use(Operand::Copy(field(1))),
                    },
                ],
                terminator: Terminator::Return(Operand::Copy(Place::local(LocalId(2)))),
            }],
            ret: field_ty.clone(),
            span,
        };
        let program = Program {
            functions: vec![function],
            skipped: vec![],
        };
        let types = tuo_types::TypeckResult::default();
        let interp = Interpreter::new(&program, &types).unwrap_or_else(|d| {
            panic!(
                "hand-built MIR verifies: {}",
                d.iter()
                    .map(|x| format!("{}: {}", x.code, x.message))
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        });
        let arg = Value::Aggregate(vec![
            Value::Array(vec![Value::Int(7, IntKind::I64)]),
            Value::Int(99, IntKind::I64),
        ]);
        // The sibling copy of field 1 must still succeed after field 0 moved.
        assert_eq!(
            interp.run("partial", vec![arg]).unwrap(),
            Value::Int(99, IntKind::I64)
        );
    }

    #[test]
    fn int_bounds_match_the_kind_widths() {
        assert_eq!(int_bounds(IntKind::I8), (-128, 127));
        assert_eq!(int_bounds(IntKind::U8), (0, 255));
        assert_eq!(int_bounds(IntKind::U16), (0, 65_535));
        assert_eq!(int_bounds(IntKind::I32).1, i128::from(i32::MAX));
    }

    #[test]
    fn int_to_int_cast_wraps_two_s_complement() {
        // 300 into I8 wraps to 44 (300 - 256).
        assert_eq!(wrap_int(300, IntKind::I8), 44);
        // -1 into U8 wraps to 255.
        assert_eq!(wrap_int(-1, IntKind::U8), 255);
        // 256 into U8 wraps to 0.
        assert_eq!(wrap_int(256, IntKind::U8), 0);
        // An in-range value is unchanged.
        assert_eq!(wrap_int(42, IntKind::I32), 42);
    }

    #[test]
    fn float_to_int_saturates_and_maps_nan_to_zero() {
        assert_eq!(saturating_float_to_int(3.9, IntKind::I32), 3);
        assert_eq!(saturating_float_to_int(-3.9, IntKind::I32), -3);
        // Above the max saturates to the max.
        assert_eq!(saturating_float_to_int(1e30, IntKind::I8), 127);
        // Below the min saturates to the min.
        assert_eq!(saturating_float_to_int(-1e30, IntKind::U8), 0);
        // NaN maps to 0.
        assert_eq!(saturating_float_to_int(f64::NAN, IntKind::I32), 0);
    }

    #[test]
    fn value_cost_counts_scalars_and_recurses_into_aggregates() {
        assert_eq!(value_cost(&Value::Int(0, IntKind::I64)), 1);
        // A two-field aggregate costs 1 (self) + 1 + 1.
        assert_eq!(
            value_cost(&Value::Aggregate(vec![Value::Bool(true), Value::Unit,])),
            3
        );
        // A string costs 1 + its byte length.
        assert_eq!(value_cost(&Value::Str("abc".to_owned())), 4);
    }
}
