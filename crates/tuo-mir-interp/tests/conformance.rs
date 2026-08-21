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
            fields: vec![Value::Str(b"bad".to_vec())]
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
    assert_eq!(value(src, "chain", vec![]), Value::Str(b"hello".to_vec()));
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

// ---------------------------------------------------------------------------
// ADR-0006 Stage A: `std::str` byte operations and the effect sandbox
// ---------------------------------------------------------------------------

#[test]
fn str_len_counts_bytes_including_multibyte_utf8() {
    let src = r#"
        fn ascii_len() -> Int { std::str::len("hello") }
        fn accented_len() -> Int { std::str::len("héllo") }
        fn empty_len() -> Int { std::str::len("") }
    "#;
    assert_eq!(value(src, "ascii_len", vec![]), int(5));
    // `é` is two UTF-8 bytes: len is 6, not 5 code points.
    assert_eq!(value(src, "accented_len", vec![]), int(6));
    assert_eq!(value(src, "empty_len", vec![]), int(0));
}

#[test]
fn str_byte_at_reads_bytes_and_traps_out_of_bounds() {
    let src = r#"
        fn probe(take index: Int) -> Int { std::str::byte_at("héllo", index) }
    "#;
    // 'h' = 0x68; the first byte of the two-byte `é` is 0xC3 = 195.
    assert_eq!(value(src, "probe", vec![int(0)]), int(0x68));
    assert_eq!(value(src, "probe", vec![int(1)]), int(0xC3));
    for bad in [-1, 6, 100] {
        let error = run(src, "probe", vec![int(bad)]).expect_err("out of bounds traps");
        assert_eq!(
            error.kind,
            TrapKind::IndexOutOfBounds,
            "byte_at({bad}) must trap IndexOutOfBounds"
        );
    }
}

#[test]
fn str_slice_takes_byte_ranges_and_traps_out_of_range() {
    let src = r#"
        fn cut(take start: Int, take end: Int) -> Str { std::str::slice("héllo", start, end) }
        fn roundtrip() -> Bool { std::str::slice("abcd", 1, 3) == "bc" }
        fn empty_ok() -> Bool { std::str::slice("abc", 3, 3) == "" }
    "#;
    assert_eq!(value(src, "roundtrip", vec![]), Value::Bool(true));
    assert_eq!(value(src, "empty_ok", vec![]), Value::Bool(true));
    // A byte slice may split the two-byte `é` (the documented v0 contract):
    // bytes [1, 3) of "héllo" are exactly é's two bytes.
    assert_eq!(
        value(src, "cut", vec![int(1), int(3)]),
        Value::Str(vec![0xC3, 0xA9])
    );
    // start < 0, start > end, end > len each trap.
    for (start, end) in [(-1, 2), (3, 1), (0, 7)] {
        let error = run(src, "cut", vec![int(start), int(end)]).expect_err("range traps");
        assert_eq!(
            error.kind,
            TrapKind::IndexOutOfBounds,
            "slice({start}, {end}) must trap IndexOutOfBounds"
        );
    }
}

#[test]
fn str_ops_compose_with_equality_on_sliced_values() {
    // len and byte_at agree on a value slice produced (all byte-wise).
    let src = r#"
        fn check() -> Bool {
            let tail = std::str::slice("héllo", 3, 6);
            std::str::len(tail) == 3 && std::str::byte_at(tail, 0) == 108
        }
    "#;
    assert_eq!(value(src, "check", vec![]), Value::Bool(true));
}

#[test]
fn an_effect_reaching_the_interpreter_is_an_internal_error_not_io() {
    // An effectful program is front-end-clean (`main` may be effectful) and
    // lowers to an `Effect` statement — but the interpreter's sandbox never
    // performs I/O: executing the statement is a structured internal error,
    // not a silent no-op and not a real write.
    let src = r#"
        fn main() -> Int { std::rt::write(1, "never printed") }
    "#;
    let error = run(src, "main", vec![]).expect_err("the sandbox refuses effects");
    match &error.kind {
        TrapKind::Internal(detail) => {
            assert!(
                detail.contains("effect `write`"),
                "the internal error names the effect: {detail}"
            );
        }
        other => panic!("expected TrapKind::Internal, got {other:?}"),
    }
    assert!(
        error.message.contains("no host effects"),
        "the message states the sandbox rule: {}",
        error.message
    );
}

#[test]
fn exit_reaching_the_interpreter_is_also_an_internal_error() {
    let src = "fn main() -> Int { std::rt::exit(3) }\n";
    let error = run(src, "main", vec![]).expect_err("the sandbox refuses exit");
    assert!(matches!(&error.kind, TrapKind::Internal(detail) if detail.contains("effect `exit`")));
}

// ---------------------------------------------------------------------------
// ADR-0009 Stage A: owned String and growable Array[Int] heap operations
// ---------------------------------------------------------------------------

/// A `Value::Str` from a byte string, for asserting owned-String results.
fn bytes(text: &str) -> Value {
    Value::Str(text.as_bytes().to_vec())
}

#[test]
fn string_constructors_and_pure_queries() {
    // empty / from_str / concat / len / byte_at, all pure — usable directly.
    let src = r#"
        fn empty_len() -> Int { std::string::len(std::string::empty()) }
        fn from_len(in s: Str) -> Int { std::string::len(std::string::from_str(s)) }
        fn concat_len(in a: Str, in b: Str) -> Int {
            std::string::len(std::string::concat(a, b))
        }
        fn first_byte(in s: Str) -> Int {
            std::string::byte_at(std::string::from_str(s), 0)
        }
    "#;
    assert_eq!(value(src, "empty_len", vec![]), int(0));
    assert_eq!(value(src, "from_len", vec![bytes("abcd")]), int(4));
    // UTF-8 length is byte length: "héllo" is 6 bytes.
    assert_eq!(
        value(src, "concat_len", vec![bytes("hé"), bytes("llo")]),
        int(6)
    );
    assert_eq!(value(src, "first_byte", vec![bytes("A")]), int(65));
}

#[test]
fn string_slice_copies_out_a_new_owned_string() {
    // `slice` copies the byte range; the result is a new owned String.
    let src = r#"
        fn mid(in s: Str) -> String { std::string::slice(std::string::from_str(s), 1, 3) }
    "#;
    assert_eq!(value(src, "mid", vec![bytes("abcd")]), bytes("bc"));
}

#[test]
fn string_as_str_views_the_whole_string() {
    // ADR-0010: `as_str` yields a `Str` view of the whole String. The
    // interpreter models it as a byte copy (sound: the ownership checker keeps
    // the source pinned while the view is live), so the view's length equals
    // the String's and its bytes compare equal to the original text.
    let src = r#"
        fn view_len(in s: Str) -> Int {
            std::str::len(std::string::as_str(std::string::from_str(s)))
        }
        fn view_bytes(in s: Str) -> Str {
            std::string::as_str(std::string::from_str(s))
        }
        fn view_eq_literal() -> Bool {
            std::string::as_str(std::string::from_str("hi")) == "hi"
        }
    "#;
    assert_eq!(value(src, "view_len", vec![bytes("héllo")]), int(6));
    assert_eq!(value(src, "view_bytes", vec![bytes("abc")]), bytes("abc"));
    assert_eq!(value(src, "view_eq_literal", vec![]), Value::Bool(true));
}

#[test]
fn string_slice_may_split_a_code_point_byte_level() {
    // Byte-level slicing: "héllo" bytes are [h, 0xC3, 0xA9, l, l, o]; slicing
    // [1, 2) yields the single 0xC3 byte (not valid UTF-8 alone).
    let src = r#"
        fn split(in s: Str) -> Int {
            let piece = std::string::slice(std::string::from_str(s), 1, 2);
            std::string::byte_at(piece, 0)
        }
    "#;
    assert_eq!(value(src, "split", vec![bytes("héllo")]), int(0xC3));
}

#[test]
fn string_byte_at_traps_out_of_bounds() {
    let src = r#"
        fn at(in s: Str, take i: Int) -> Int { std::string::byte_at(std::string::from_str(s), i) }
    "#;
    assert_eq!(
        run(src, "at", vec![bytes("ab"), int(2)]).unwrap_err().kind,
        TrapKind::IndexOutOfBounds
    );
    assert_eq!(
        run(src, "at", vec![bytes("ab"), int(-1)]).unwrap_err().kind,
        TrapKind::IndexOutOfBounds
    );
}

#[test]
fn string_slice_traps_out_of_bounds() {
    let src = r#"
        fn sl(in s: Str, take a: Int, take b: Int) -> String {
            std::string::slice(std::string::from_str(s), a, b)
        }
    "#;
    // end past len, and start > end, both trap.
    assert_eq!(
        run(src, "sl", vec![bytes("ab"), int(0), int(3)])
            .unwrap_err()
            .kind,
        TrapKind::IndexOutOfBounds
    );
    assert_eq!(
        run(src, "sl", vec![bytes("ab"), int(2), int(1)])
            .unwrap_err()
            .kind,
        TrapKind::IndexOutOfBounds
    );
}

#[test]
fn string_push_byte_grows_in_place() {
    let src = r#"
        fn build() -> Int {
            var s = std::string::empty();
            std::string::push_byte(s, 104);
            std::string::push_byte(s, 105);
            std::string::len(s)
        }
        fn readback() -> Int {
            var s = std::string::from_str("h");
            std::string::push_byte(s, 105);
            std::string::byte_at(s, 1)
        }
    "#;
    assert_eq!(value(src, "build", vec![]), int(2));
    assert_eq!(value(src, "readback", vec![]), int(105));
}

#[test]
fn string_push_byte_traps_on_an_out_of_range_byte() {
    // A byte argument outside 0..=255 traps InvalidByte — never masked.
    let src = r#"
        fn push(take b: Int) -> Int {
            var s = std::string::empty();
            std::string::push_byte(s, b);
            std::string::len(s)
        }
    "#;
    assert_eq!(
        run(src, "push", vec![int(256)]).unwrap_err().kind,
        TrapKind::InvalidByte
    );
    assert_eq!(
        run(src, "push", vec![int(-1)]).unwrap_err().kind,
        TrapKind::InvalidByte
    );
    // The boundary values are accepted.
    assert_eq!(value(src, "push", vec![int(0)]), int(1));
    assert_eq!(value(src, "push", vec![int(255)]), int(1));
}

#[test]
fn string_append_grows_in_place() {
    let src = r#"
        fn build(in seed: Str, in tail: Str) -> Int {
            var s = std::string::from_str(seed);
            std::string::append(s, tail);
            std::string::len(s)
        }
    "#;
    assert_eq!(value(src, "build", vec![bytes("ab"), bytes("cde")]), int(5));
}

#[test]
fn string_equality_is_byte_wise_content_equality() {
    // ADR-0009: `String == String` is byte-wise content equality (over the
    // same buffer `Str` uses), and consumes neither operand.
    let src = r#"
        fn same(in a: Str, in b: Str) -> Bool {
            std::string::from_str(a) == std::string::from_str(b)
        }
    "#;
    assert_eq!(
        value(src, "same", vec![bytes("ab"), bytes("ab")]),
        Value::Bool(true)
    );
    assert_eq!(
        value(src, "same", vec![bytes("ab"), bytes("ac")]),
        Value::Bool(false)
    );
}

#[test]
fn array_push_pop_round_trips() {
    let src = r#"
        fn build() -> Int {
            var xs = std::array::empty();
            std::array::push(xs, 10);
            std::array::push(xs, 20);
            std::array::push(xs, 30);
            std::array::len(xs)
        }
        fn last() -> Int {
            var xs = std::array::empty();
            std::array::push(xs, 7);
            std::array::push(xs, 8);
            let popped = std::array::pop(xs);
            match popped {
                Some { value: v } => v,
                None => -1,
            }
        }
        fn pop_empty() -> Int {
            var xs = std::array::empty();
            let popped = std::array::pop(xs);
            match popped {
                Some { value: v } => v,
                None => -1,
            }
        }
    "#;
    assert_eq!(value(src, "build", vec![]), int(3));
    assert_eq!(value(src, "last", vec![]), int(8));
    // pop of an empty array is None (-1 sentinel here).
    assert_eq!(value(src, "pop_empty", vec![]), int(-1));
}

/// ADR-0012 Stage A: the growable `Array[T]` element type is generic in the
/// reference interpreter — the array is a `Vec<Value>` that already holds any
/// element (`Str`, `String`, `Bool`, a struct/enum `Variant`), and `push`/`get`/
/// `pop` move those values without assuming `Int`. This pins that a non-`Int`
/// element round-trips: build → get → read, and `pop` transfers an owned
/// `String` element out.
#[test]
fn array_generic_elements_round_trip() {
    let src = r#"
        // Array[Str]: get returns the stored slice; its length is read back.
        fn str_get() -> Int {
            var xs = std::array::empty();
            std::array::push(xs, "hello");
            std::array::push(xs, "worlds");
            std::str::len(std::array::get(xs, 1))
        }
        // Array[String]: pop transfers the owned element out as Option[String].
        fn string_pop() -> Int {
            var xs = std::array::empty();
            std::array::push(xs, std::string::from_str("abc"));
            std::array::push(xs, std::string::from_str("de"));
            match std::array::pop(xs) {
                Some { value: v } => std::string::len(v),
                None => -1,
            }
        }
        // Array[Bool]: the element is a bool, returned as-is by get.
        fn bool_get() -> Bool {
            var xs = std::array::empty();
            std::array::push(xs, true);
            std::array::push(xs, false);
            std::array::get(xs, 0)
        }
    "#;
    // len("worlds") == 6.
    assert_eq!(value(src, "str_get", vec![]), int(6));
    // pop returns the last pushed "de"; len == 2.
    assert_eq!(value(src, "string_pop", vec![]), int(2));
    assert_eq!(value(src, "bool_get", vec![]), Value::Bool(true));
}

#[test]
fn array_get_reads_and_traps_out_of_bounds() {
    let src = r#"
        fn get(take i: Int) -> Int {
            var xs = std::array::empty();
            std::array::push(xs, 100);
            std::array::push(xs, 200);
            std::array::get(xs, i)
        }
    "#;
    assert_eq!(value(src, "get", vec![int(0)]), int(100));
    assert_eq!(value(src, "get", vec![int(1)]), int(200));
    assert_eq!(
        run(src, "get", vec![int(2)]).unwrap_err().kind,
        TrapKind::IndexOutOfBounds
    );
    assert_eq!(
        run(src, "get", vec![int(-1)]).unwrap_err().kind,
        TrapKind::IndexOutOfBounds
    );
}

#[test]
fn the_memory_budget_counts_byte_buffer_growth() {
    // Growing a String past a tiny budget aborts deterministically: the push
    // charges the grown value's cost (1 + byte length), so the budget bounds
    // heap growth exactly as it bounds aggregate materialization.
    let src = r#"
        fn grow(take n: Int) -> Int {
            var s = std::string::empty();
            var i = 0;
            loop {
                if i >= n { break; }
                std::string::push_byte(s, 97);
                i = i + 1;
            }
            std::string::len(s)
        }
    "#;
    let (program, types) = compile(src);
    // A small budget aborts a long push loop with MemoryBudget.
    let interp = Interpreter::new(&program, &types)
        .expect("verifies")
        .with_limits(Limits::default().max_values(8));
    let err = interp.run("grow", vec![int(100)]).unwrap_err();
    assert_eq!(err.kind, TrapKind::MemoryBudget);
    // A generous budget completes and reports the full length.
    let interp = Interpreter::new(&program, &types).expect("verifies");
    assert_eq!(interp.run("grow", vec![int(10)]).unwrap(), int(10));
}

#[test]
fn array_growth_is_counted_by_the_budget() {
    let src = r#"
        fn grow(take n: Int) -> Int {
            var xs = std::array::empty();
            var i = 0;
            loop {
                if i >= n { break; }
                std::array::push(xs, i);
                i = i + 1;
            }
            std::array::len(xs)
        }
    "#;
    let (program, types) = compile(src);
    let interp = Interpreter::new(&program, &types)
        .expect("verifies")
        .with_limits(Limits::default().max_values(8));
    assert_eq!(
        interp.run("grow", vec![int(100)]).unwrap_err().kind,
        TrapKind::MemoryBudget
    );
}

#[test]
fn a_moved_out_heap_value_is_dropped_exactly_once() {
    // Drop placement for heap values (gating ADR-0009 Stage B's native free):
    // when a `String` is moved into another binding, only the destination is
    // dropped — the moved-from source is not dropped again. The interpreter
    // executes drops on owned trees, so a double-drop would surface as an
    // internal error and a leak as a budget miscount; a clean run over a tight
    // budget confirms exactly-once release.
    let src = r#"
        fn move_and_use(in seed: Str) -> Int {
            var s = std::string::from_str(seed);
            std::string::push_byte(s, 33);
            let t = s;            // move: `s` is de-initialized, only `t` owns
            std::string::len(t)
        }
    "#;
    let (program, types) = compile(src);
    // A budget that admits one live String (seed 3 bytes + push = "abc!" cost
    // 5, plus scalars) but would be blown by a second live copy.
    let interp = Interpreter::new(&program, &types)
        .expect("verifies")
        .with_limits(Limits::default().max_values(64));
    assert_eq!(
        interp.run("move_and_use", vec![bytes("abc")]).unwrap(),
        int(4)
    );
}

#[test]
fn a_reassigned_heap_value_drops_the_old_buffer_first() {
    // Assignment drops the old value before storing the new one (the ABI
    // contract): the emitted MIR is `drop _s; _s = move _new`, so no old
    // *heap buffer* leaks past a reassignment, and the interpreter releases
    // the old String's budget on that drop (heap growth/shrink is accounted
    // exactly — see the budget-growth tests above, and a whole-local `Move`
    // is now budget-neutral). This asserts the drop-old-first semantics: the
    // loop terminates with the final length and never double-drops.
    //
    // (The live-value *proxy* still slowly accumulates on repeated *scalar*
    // `Copy` reassignment — overwriting a `Copy` local re-charges the new
    // value without releasing the old, a pre-existing behavior orthogonal to
    // heap accounting — so the loop counter, not the String, is what a very
    // tight ceiling would eventually catch. Heap accounting itself is exact.)
    let src = r#"
        fn churn(take n: Int) -> Int {
            var s = std::string::empty();
            var i = 0;
            loop {
                if i >= n { break; }
                s = std::string::from_str("xxxx");  // drops the previous `s`
                i = i + 1;
            }
            std::string::len(s)
        }
    "#;
    assert_eq!(value(src, "churn", vec![int(1000)]), int(4));
}

// ---------------------------------------------------------------------------
// First-class function values (ADR-0008 Tier 1)
// ---------------------------------------------------------------------------

#[test]
fn an_indirect_call_dispatches_like_a_direct_call() {
    // `apply(add, 2, 3)` calls `add` indirectly through a function value; the
    // result equals the direct call `add(2, 3)`.
    let src = "\
        fn add(take a: Int, take b: Int) -> Int { a + b }\n\
        fn apply(take f: fn(take Int, take Int) -> Int, take a: Int, take b: Int) -> Int {\n\
            f(a, b)\n\
        }\n\
        fn indirect() -> Int { apply(add, 2, 3) }\n\
        fn direct() -> Int { add(2, 3) }\n";
    assert_eq!(value(src, "indirect", vec![]), int(5));
    assert_eq!(value(src, "direct", vec![]), int(5));
    assert_eq!(value(src, "indirect", vec![]), value(src, "direct", vec![]));
}

#[test]
fn a_function_value_bound_to_a_local_is_callable() {
    let src = "\
        fn triple(take n: Int) -> Int { n + n + n }\n\
        fn go(take n: Int) -> Int { var g = triple; g(n) }\n";
    assert_eq!(value(src, "go", vec![int(7)]), int(21));
}

#[test]
fn a_function_value_is_copy_and_callable_twice() {
    // Copying a function value (it is `Copy`) leaves the source usable: two
    // indirect calls through copies of the same value both dispatch.
    let src = "\
        fn sq(take n: Int) -> Int { n * n }\n\
        fn twice(take n: Int) -> Int {\n\
            var f = sq;\n\
            var g = f;\n\
            f(n) + g(n)\n\
        }\n";
    assert_eq!(value(src, "twice", vec![int(4)]), int(32));
}

#[test]
fn a_function_value_selected_by_a_branch_dispatches_at_runtime() {
    // The function value flows through a variable whose value a branch picks,
    // then is called — the interpreter dispatches on the runtime value.
    let src = "\
        fn inc(take n: Int) -> Int { n + 1 }\n\
        fn dec(take n: Int) -> Int { n - 1 }\n\
        fn pick(take up: Bool, take n: Int) -> Int {\n\
            var f = inc;\n\
            if up { f = inc; } else { f = dec; }\n\
            f(n)\n\
        }\n";
    assert_eq!(
        value(src, "pick", vec![Value::Bool(true), int(10)]),
        int(11)
    );
    assert_eq!(
        value(src, "pick", vec![Value::Bool(false), int(10)]),
        int(9)
    );
}
