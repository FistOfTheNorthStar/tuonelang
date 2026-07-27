//! Conformance suite for the MIR interpreter.
//!
//! Each test compiles a small tuonelang program through the whole front end
//! (parse → resolve → type check → ownership check), lowers it to MIR, and
//! executes an entry function with the reference interpreter, asserting the
//! computed [`Value`] or the structured [`RuntimeError`]. This is the
//! executable definition of the language's dynamic semantics for the v0
//! subset: arithmetic, branches, recursion, structs, enums, `Option`,
//! `Result`, and ownership-driven destruction.
//!
//! The programs are front-end-clean by construction; a fixture that fails the
//! front end fails the harness immediately (with the diagnostics), because MIR
//! is only defined for accepted programs.

use tuo_ast::Ast;
use tuo_mir_interp::{Interpreter, Limits, RunResult, TrapKind, Value};
use tuo_source::SourceMap;
use tuo_types::IntKind;

/// Compile `source` through the whole front end and lower it to MIR, panicking
/// (with rendered diagnostics) on any front-end error — fixtures must be
/// accepted programs.
fn compile(source: &str) -> (tuo_mir::Program, tuo_types::TypeckResult) {
    let mut map = SourceMap::new();
    let file = map.intern_file("conformance.tuo");
    let id = map.add_source(file, source).expect("fixture fits");
    let parse = tuo_parser::parse(map.source(id));
    assert_eq!(parse.diagnostics, vec![], "fixture has parse errors");
    let asts = [Ast::new(&parse.tree, source)];
    let resolution = tuo_resolve::resolve(&asts);
    assert_eq!(
        resolution.diagnostics(),
        &[],
        "fixture has resolution errors"
    );
    let types = tuo_types::check(&asts, &resolution);
    assert_eq!(types.diagnostics(), &[], "fixture has type errors");
    let ownership = tuo_ownership::check(&asts, &resolution, &types);
    assert_eq!(ownership.diagnostics(), &[], "fixture has ownership errors");
    let hir = tuo_hir::lower(&asts, &resolution);
    let program = tuo_mir::lower(&hir, &resolution, &types);
    (program, types)
}

/// Compile, build a (verification-gated) interpreter, and run `entry(args)`.
fn run(source: &str, entry: &str, args: Vec<Value>) -> RunResult {
    let (program, types) = compile(source);
    let interp = Interpreter::new(&program, &types).expect("lowered MIR must verify");
    interp.run(entry, args)
}

/// Run and unwrap to a value (a trap is a test failure).
fn value(source: &str, entry: &str, args: Vec<Value>) -> Value {
    run(source, entry, args).expect("program returned a value")
}

/// An `Int` (`I64`) value shorthand.
fn int(value: i128) -> Value {
    Value::Int(value, IntKind::I64)
}

// ---------------------------------------------------------------------------
// Arithmetic
// ---------------------------------------------------------------------------

#[test]
fn arithmetic_evaluates_by_documented_semantics() {
    let src = "
        fn arith(in a: Int, in b: Int) -> Int {
            let sum = a + b;
            let diff = a - b;
            let product = sum * diff;
            let quotient = product / b;
            quotient % a
        }
    ";
    // sum=10, diff=4, product=40, quotient=40/3=13, 13 % 7 = 6.
    assert_eq!(value(src, "arith", vec![int(7), int(3)]), int(6));
}

#[test]
fn integer_overflow_traps_deterministically() {
    // I8 add overflow: 127 + 1 traps (two's complement, trapping, §24).
    let src = "
        fn bump(in x: I8) -> I8 {
            x + 1
        }
    ";
    let err = run(src, "bump", vec![Value::Int(127, IntKind::I8)]).expect_err("overflow must trap");
    assert_eq!(err.kind, TrapKind::IntegerOverflow);
    assert!(err.kind.is_language_trap());
    assert_eq!(err.backtrace.len(), 1);
    assert_eq!(err.backtrace[0].function, "bump");
}

#[test]
fn division_and_remainder_by_zero_trap() {
    let src = "
        fn div(in a: Int, in b: Int) -> Int { a / b }
        fn rem(in a: Int, in b: Int) -> Int { a % b }
    ";
    assert_eq!(
        run(src, "div", vec![int(1), int(0)]).unwrap_err().kind,
        TrapKind::DivisionByZero
    );
    assert_eq!(
        run(src, "rem", vec![int(1), int(0)]).unwrap_err().kind,
        TrapKind::DivisionByZero
    );
}

#[test]
fn negation_of_the_minimum_overflows() {
    let src = "fn neg(in x: I8) -> I8 { -x }";
    assert_eq!(
        run(src, "neg", vec![Value::Int(-128, IntKind::I8)])
            .unwrap_err()
            .kind,
        TrapKind::IntegerOverflow
    );
    // A non-minimum value negates cleanly.
    assert_eq!(
        value(src, "neg", vec![Value::Int(5, IntKind::I8)]),
        Value::Int(-5, IntKind::I8)
    );
}

#[test]
fn casts_wrap_and_saturate_as_documented() {
    let src = "
        fn narrow(in wide: Int) -> I8 { wide as I8 }
        fn to_float(in x: Int) -> F64 { x as F64 }
    ";
    // 300 as I8 wraps: 300 - 256 = 44.
    assert_eq!(
        value(src, "narrow", vec![int(300)]),
        Value::Int(44, IntKind::I8)
    );
    assert_eq!(
        value(src, "to_float", vec![int(3)]),
        Value::Float(3.0, tuo_types::FloatKind::F64)
    );
}

#[test]
fn comparisons_and_logic_short_circuit() {
    let src = "
        fn compare(in a: Int, in b: Int) -> Bool {
            let below = a < b;
            let above = a > b;
            below == above
        }
        fn logic(in p: Bool, in q: Bool) -> Bool {
            let both = p && q;
            let either = p || q;
            !both != either
        }
    ";
    assert_eq!(
        value(src, "compare", vec![int(1), int(2)]),
        Value::Bool(false)
    );
    // logic(true, false): both = false, either = true, `!both != either`
    // is `true != true` = false.
    assert_eq!(
        value(src, "logic", vec![Value::Bool(true), Value::Bool(false)]),
        Value::Bool(false)
    );
    // logic(true, true): both = true, either = true, `!both != either`
    // is `false != true` = true.
    assert_eq!(
        value(src, "logic", vec![Value::Bool(true), Value::Bool(true)]),
        Value::Bool(true)
    );
}

// ---------------------------------------------------------------------------
// Branches
// ---------------------------------------------------------------------------

#[test]
fn branches_take_the_documented_edge() {
    let src = "
        fn max(in a: Int, in b: Int) -> Int {
            if a > b { a } else { b }
        }
    ";
    assert_eq!(value(src, "max", vec![int(3), int(9)]), int(9));
    assert_eq!(value(src, "max", vec![int(9), int(3)]), int(9));
}

#[test]
fn loops_and_accumulation_run_to_a_fixpoint() {
    let src = "
        fn sum_to(in n: Int) -> Int {
            var total = 0;
            var i = 1;
            while i <= n {
                total = total + i;
                i = i + 1;
            }
            total
        }
    ";
    // 1 + 2 + ... + 5 = 15.
    assert_eq!(value(src, "sum_to", vec![int(5)]), int(15));
    assert_eq!(value(src, "sum_to", vec![int(0)]), int(0));
}

// ---------------------------------------------------------------------------
// Recursion
// ---------------------------------------------------------------------------

#[test]
fn recursion_computes_and_the_call_stack_backtraces() {
    let src = "
        fn fib(in n: Int) -> Int {
            if n < 2 { n } else { fib(n - 1) + fib(n - 2) }
        }
    ";
    assert_eq!(value(src, "fib", vec![int(10)]), int(55));
    assert_eq!(value(src, "fib", vec![int(0)]), int(0));
}

#[test]
fn unbounded_recursion_hits_the_configured_depth_limit() {
    // A function that recurses without a base case would loop forever; the
    // recursion-depth ceiling aborts it deterministically.
    let src = "
        fn forever(in n: Int) -> Int { forever(n + 1) }
    ";
    let (program, types) = compile(src);
    let interp = Interpreter::new(&program, &types)
        .expect("verifies")
        .with_limits(Limits::default().max_depth(64));
    let err = interp.run("forever", vec![int(0)]).unwrap_err();
    assert_eq!(err.kind, TrapKind::RecursionLimit);
    // The backtrace is as deep as the ceiling.
    assert_eq!(err.backtrace.len(), 64);
}

#[test]
fn instruction_fuel_bounds_a_long_run() {
    let src = "
        fn spin(in n: Int) -> Int {
            var i = 0;
            while i < n { i = i + 1; }
            i
        }
    ";
    let (program, types) = compile(src);
    let interp = Interpreter::new(&program, &types)
        .expect("verifies")
        .with_limits(Limits::with_fuel(50));
    let err = interp.run("spin", vec![int(1_000_000)]).unwrap_err();
    assert_eq!(err.kind, TrapKind::OutOfFuel);
    // The same program with ample fuel completes.
    let interp = Interpreter::new(&program, &types).expect("verifies");
    assert_eq!(interp.run("spin", vec![int(3)]).unwrap(), int(3));
}

#[test]
fn the_memory_budget_bounds_live_values() {
    // A tiny live-value budget aborts a program that materializes more values
    // than it allows, deterministically and structurally.
    let src = "
        struct Big { a: Int, b: Int, c: Int }
        fn build(in x: Int) -> Int {
            let one = Big { a: x, b: x, c: x };
            let two = Big { a: x, b: x, c: x };
            one.a + two.b
        }
    ";
    let (program, types) = compile(src);
    let interp = Interpreter::new(&program, &types)
        .expect("verifies")
        .with_limits(Limits::default().max_values(3));
    let err = interp.run("build", vec![int(1)]).unwrap_err();
    assert_eq!(err.kind, TrapKind::MemoryBudget);
    // With a generous budget the same program completes.
    let interp = Interpreter::new(&program, &types).expect("verifies");
    assert_eq!(interp.run("build", vec![int(2)]).unwrap(), int(4));
}

// ---------------------------------------------------------------------------
// Structs
// ---------------------------------------------------------------------------

#[test]
fn structs_construct_and_project() {
    let src = "
        struct Point { x: Int, y: Int }
        fn origin() -> Point { Point { x: 0, y: 0 } }
        fn swap(in p: Point) -> Point { Point { x: p.y, y: p.x } }
        fn sum(in p: Point) -> Int { p.x + p.y }
    ";
    // origin() = (0, 0).
    let origin = value(src, "origin", vec![]);
    assert_eq!(
        origin,
        Value::Variant {
            variant: 0,
            fields: vec![int(0), int(0)]
        }
    );

    // swap projects and reconstructs.
    let swapped = value(
        src,
        "swap",
        vec![Value::Variant {
            variant: 0,
            fields: vec![int(3), int(7)],
        }],
    );
    assert_eq!(
        swapped,
        Value::Variant {
            variant: 0,
            fields: vec![int(7), int(3)]
        }
    );

    // Field access sums to 10.
    assert_eq!(
        value(
            src,
            "sum",
            vec![Value::Variant {
                variant: 0,
                fields: vec![int(4), int(6)]
            }]
        ),
        int(10)
    );
}

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

#[test]
fn enums_switch_on_the_discriminant() {
    let src = "
        enum Shape {
            Circle { radius: Int },
            Rect { width: Int, height: Int },
            Empty,
        }
        fn area(in shape: Shape) -> Int {
            match shape {
                Shape::Circle { radius } => 3 * radius * radius,
                Shape::Rect { width, height } => width * height,
                Shape::Empty => 0,
            }
        }
    ";
    // Circle{radius:2} -> 3*2*2 = 12  (variant 0).
    assert_eq!(
        value(
            src,
            "area",
            vec![Value::Variant {
                variant: 0,
                fields: vec![int(2)]
            }]
        ),
        int(12)
    );
    // Rect{4,5} -> 20 (variant 1).
    assert_eq!(
        value(
            src,
            "area",
            vec![Value::Variant {
                variant: 1,
                fields: vec![int(4), int(5)]
            }]
        ),
        int(20)
    );
    // Empty -> 0 (variant 2).
    assert_eq!(
        value(
            src,
            "area",
            vec![Value::Variant {
                variant: 2,
                fields: vec![]
            }]
        ),
        int(0)
    );
}

#[test]
fn enum_match_guards_select_the_right_arm() {
    let src = "
        enum Shape {
            Circle { radius: Int },
            Rect { width: Int, height: Int },
            Empty,
        }
        fn is_wide(in shape: Shape) -> Bool {
            match shape {
                Shape::Rect { width, height } if width > height => true,
                other => false,
            }
        }
    ";
    assert_eq!(
        value(
            src,
            "is_wide",
            vec![Value::Variant {
                variant: 1,
                fields: vec![int(5), int(2)]
            }]
        ),
        Value::Bool(true)
    );
    assert_eq!(
        value(
            src,
            "is_wide",
            vec![Value::Variant {
                variant: 1,
                fields: vec![int(2), int(5)]
            }]
        ),
        Value::Bool(false)
    );
    assert_eq!(
        value(
            src,
            "is_wide",
            vec![Value::Variant {
                variant: 0,
                fields: vec![int(9)]
            }]
        ),
        Value::Bool(false)
    );
}

// ---------------------------------------------------------------------------
// Option
// ---------------------------------------------------------------------------

#[test]
fn option_constructs_matches_and_propagates() {
    let src = "
        fn find(in haystack: Int, in needle: Int) -> Option[Int] {
            if haystack == needle {
                Some { value: needle }
            } else {
                None
            }
        }
        fn unwrap_or(in maybe: Option[Int], in fallback: Int) -> Int {
            match maybe {
                Some { value } => value,
                fallthrough => fallback,
            }
        }
        fn checked_sum(in a: Int, in b: Int) -> Option[Int] {
            let first = find(a, a)?;
            let second = find(b, b)?;
            Some { value: first + second }
        }
    ";
    // find(3,3) = Some(3) (variant 0); find(3,4) = None (variant 1).
    assert_eq!(
        value(src, "find", vec![int(3), int(3)]),
        Value::Variant {
            variant: 0,
            fields: vec![int(3)]
        }
    );
    assert_eq!(
        value(src, "find", vec![int(3), int(4)]),
        Value::Variant {
            variant: 1,
            fields: vec![]
        }
    );
    // unwrap_or picks the payload or the fallback.
    assert_eq!(
        value(
            src,
            "unwrap_or",
            vec![
                Value::Variant {
                    variant: 0,
                    fields: vec![int(9)]
                },
                int(0)
            ]
        ),
        int(9)
    );
    assert_eq!(
        value(
            src,
            "unwrap_or",
            vec![
                Value::Variant {
                    variant: 1,
                    fields: vec![]
                },
                int(42)
            ]
        ),
        int(42)
    );
    // `?` propagates: checked_sum(2,3) sums the found values.
    assert_eq!(
        value(src, "checked_sum", vec![int(2), int(3)]),
        Value::Variant {
            variant: 0,
            fields: vec![int(5)]
        }
    );
}

#[test]
fn a_bare_unit_variant_pattern_tests_the_discriminant() {
    // Regression: a bare `None` arm is the *variant* pattern, not a catch-all
    // binding named "None". It must test the discriminant so `Some` and `None`
    // reach different arms — the earlier lowering treated `None` as an
    // irrefutable binding and every value fell into that arm.
    let src = "
        fn tag(in v: Option[Int]) -> Int {
            match v {
                None => 0,
                Some { value } => value,
            }
        }
    ";
    // `Some { value: 7 }` (variant 0) selects the `Some` arm and yields 7.
    assert_eq!(
        value(
            src,
            "tag",
            vec![Value::Variant {
                variant: 0,
                fields: vec![int(7)]
            }]
        ),
        int(7)
    );
    // `None` (variant 1) selects the `None` arm and yields 0.
    assert_eq!(
        value(
            src,
            "tag",
            vec![Value::Variant {
                variant: 1,
                fields: vec![]
            }]
        ),
        int(0)
    );
}

#[test]
fn a_bare_none_arm_ordered_first_still_discriminates() {
    // The arm order must not matter: with `None` written before `Some`, a
    // `Some` value must still skip the `None` arm and reach `Some`.
    let src = "
        fn is_some(in v: Option[Int]) -> Int {
            match v {
                None => 0,
                Some { value } => 1,
            }
        }
    ";
    assert_eq!(
        value(
            src,
            "is_some",
            vec![Value::Variant {
                variant: 0,
                fields: vec![int(99)]
            }]
        ),
        int(1),
        "a Some value reaches the Some arm even when None is written first"
    );
    assert_eq!(
        value(
            src,
            "is_some",
            vec![Value::Variant {
                variant: 1,
                fields: vec![]
            }]
        ),
        int(0)
    );
}

// ---------------------------------------------------------------------------
// Result
// ---------------------------------------------------------------------------

#[test]
fn result_constructs_and_question_mark_short_circuits() {
    let src = r#"
        fn parse_pair(in good: Bool) -> Result[Int, Str] {
            if good {
                Ok { value: 7 }
            } else {
                Err { error: "bad" }
            }
        }
        fn double_parse(in good: Bool) -> Result[Int, Str] {
            let value = parse_pair(good)?;
            Ok { value: value * 2 }
        }
    "#;
    // Ok(7) doubles to Ok(14) (variant 0).
    assert_eq!(
        value(src, "double_parse", vec![Value::Bool(true)]),
        Value::Variant {
            variant: 0,
            fields: vec![int(14)]
        }
    );
    // Err("bad") propagates unchanged (variant 1).
    assert_eq!(
        value(src, "double_parse", vec![Value::Bool(false)]),
        Value::Variant {
            variant: 1,
            fields: vec![Value::Str("bad".to_owned())]
        }
    );
}

// ---------------------------------------------------------------------------
// Ownership-driven destruction
// ---------------------------------------------------------------------------

#[test]
fn moves_transfer_and_dropped_values_do_not_corrupt_results() {
    // `var slot = first; slot = second;` lowers to a `drop` of the old value
    // followed by an assignment. The interpreter destroys the old value
    // (de-initializes it, no destructor — §24) and the program still computes
    // the correct final result, proving drop elaboration executes cleanly.
    let src = "
        struct Holder { tag: Int }
        fn reassign(in a: Int, in b: Int) -> Int {
            var slot = Holder { tag: a };
            slot = Holder { tag: b };
            slot.tag
        }
    ";
    assert_eq!(value(src, "reassign", vec![int(1), int(2)]), int(2));
}

#[test]
fn a_moved_value_flows_through_a_chain_of_locals() {
    // A String is moved local-to-local and finally returned; each move
    // transfers ownership and de-initializes the source, and the value
    // arrives intact.
    let src = r#"
        fn chain() -> Str {
            let a = "hello";
            let b = a;
            let c = b;
            c
        }
    "#;
    assert_eq!(value(src, "chain", vec![]), Value::Str("hello".to_owned()));
}

#[test]
fn borrow_mut_writes_are_visible_after_the_call() {
    // A `mut` parameter aliases the caller's place for the call's duration;
    // the interpreter models this as copy-in/copy-back, so a write through the
    // borrow is visible to the caller afterward.
    let src = "
        struct Cell { value: Int }
        fn bump(mut c: Cell) {
            c = Cell { value: c.value + 1 };
        }
        fn use_bump(in start: Int) -> Int {
            var cell = Cell { value: start };
            bump(cell);
            cell.value
        }
    ";
    assert_eq!(value(src, "use_bump", vec![int(41)]), int(42));
}

// ---------------------------------------------------------------------------
// The verification gate
// ---------------------------------------------------------------------------

#[test]
fn the_interpreter_refuses_unverified_mir() {
    // Hand-build malformed MIR (a return whose block target is dangling would
    // be structural; here we make a function that returns a moved-and-copied
    // Copy value incorrectly is hard to fabricate — instead we corrupt a
    // block target). We build the smallest malformed program the verifier
    // rejects and assert the interpreter refuses it rather than running.
    use tuo_mir::{BasicBlock, BlockId, Function, Program, Terminator};
    use tuo_resolve::SymbolId;
    use tuo_source::{SourceId, Span, TextRange};
    use tuo_types::Ty;

    let span = Span::new(
        SourceId::from_raw(0),
        TextRange::new(0, 1).expect("forward"),
    );
    let function = Function {
        symbol: SymbolId::from_raw(0),
        name: "bad".to_owned(),
        params: vec![],
        locals: vec![],
        // A single block whose terminator jumps to a nonexistent block.
        blocks: vec![BasicBlock {
            statements: vec![],
            terminator: Terminator::Goto(BlockId(7)),
        }],
        ret: Ty::Unit,
        span,
    };
    let program = Program {
        functions: vec![function],
        skipped: vec![],
    };
    let types = tuo_types::TypeckResult::default();

    let refused = Interpreter::new(&program, &types);
    let diagnostics = refused.err().expect("malformed MIR must be refused");
    assert!(
        !diagnostics.is_empty(),
        "refusal must carry the verifier's structured diagnostics"
    );
}

// ---------------------------------------------------------------------------
// Structured execution trace
// ---------------------------------------------------------------------------

#[test]
fn a_trace_is_produced_on_request_and_is_deterministic() {
    use tuo_mir_interp::Trace;

    let src = "
        fn add(in a: Int, in b: Int) -> Int { a + b }
    ";
    let (program, types) = compile(src);
    let interp = Interpreter::new(&program, &types)
        .expect("verifies")
        .with_trace();

    let mut trace = Trace::new();
    let result = interp.run_traced("add", vec![int(2), int(3)], &mut trace);
    assert_eq!(result.unwrap(), int(5));
    // The trace records the call, the block, the statements, and the return.
    let rendered = trace.render();
    assert!(rendered.contains("call add @depth 1"), "trace: {rendered}");
    assert!(rendered.contains("return 5I64"), "trace: {rendered}");

    // Determinism: the same run traces identically.
    let mut again = Trace::new();
    interp
        .run_traced("add", vec![int(2), int(3)], &mut again)
        .unwrap();
    assert_eq!(rendered, again.render());

    // Without `with_trace`, no events are recorded.
    let quiet = Interpreter::new(&program, &types).expect("verifies");
    let mut empty = Trace::new();
    quiet
        .run_traced("add", vec![int(2), int(3)], &mut empty)
        .unwrap();
    assert!(empty.events().is_empty());
}

#[test]
fn a_trap_appears_in_the_trace() {
    use tuo_mir_interp::Trace;

    let src = "fn bad(in a: Int, in b: Int) -> Int { a / b }";
    let (program, types) = compile(src);
    let interp = Interpreter::new(&program, &types)
        .expect("verifies")
        .with_trace();
    let mut trace = Trace::new();
    let err = interp
        .run_traced("bad", vec![int(1), int(0)], &mut trace)
        .unwrap_err();
    assert_eq!(err.kind, TrapKind::DivisionByZero);
    assert!(
        trace.render().contains("trap division_by_zero"),
        "trace: {}",
        trace.render()
    );
}

// ---------------------------------------------------------------------------
// Determinism
// ---------------------------------------------------------------------------

#[test]
fn execution_is_deterministic() {
    let src = "
        fn fib(in n: Int) -> Int {
            if n < 2 { n } else { fib(n - 1) + fib(n - 2) }
        }
    ";
    let (program, types) = compile(src);
    let interp = Interpreter::new(&program, &types).expect("verifies");
    let first = interp.run("fib", vec![int(12)]).unwrap();
    let second = interp.run("fib", vec![int(12)]).unwrap();
    assert_eq!(first, second);
    assert_eq!(first, int(144));
}
