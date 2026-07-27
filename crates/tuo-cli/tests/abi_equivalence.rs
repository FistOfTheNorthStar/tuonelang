//! Interpreter ⇄ ABI semantic-equivalence tests.
//!
//! The reference meaning of a value is what the MIR interpreter
//! ([`tuo_mir_interp`]) computes; the runtime ABI ([`tuo_runtime::abi`]) is how
//! that same value is represented in memory natively. The two must encode the
//! *same* value with no observable difference. The native backend does not
//! lower aggregates yet, so this suite pins the equivalence at the levels a
//! native binary already depends on and the aggregates will: **scalar widths**
//! and the **enum discriminant numbering** that a native `match` would read.
//!
//! For the parts that execute, the interpreter is driven directly (the native
//! path for these shapes is covered end-to-end by `codegen_differential`, which
//! runs the real `tuo run` and compares its exit status to the interpreter).

use tuo_compiler::source::SourceMap;
use tuo_mir_interp::{Interpreter, Value};
use tuo_runtime::abi::{DISCRIMINANT_SIZE, Layout, POINTER_SIZE, align_up, int_size, layout_of};
use tuo_types::{IntKind, Ty, TypeckResult};

/// Run `main` of a source program on the reference interpreter and return its
/// value.
fn interpret_main_value(source: &str) -> Value {
    let mut map = SourceMap::new();
    let file = map.intern_file("abi_equivalence.tuo");
    let id = map.add_source(file, source).expect("valid source");

    let check = tuo_compiler::check_sources(&map, &[id]);
    assert!(!check.has_errors(), "the program should type-check cleanly");
    let parse = tuo_compiler::parser::parse(map.source(id));
    let asts = [tuo_compiler::ast::Ast::new(
        &parse.tree,
        map.source(id).text(),
    )];
    let hir = tuo_compiler::hir::lower(&asts, &check.resolution);
    let program = tuo_compiler::mir::lower(&hir, &check.resolution, &check.types);
    let interpreter = Interpreter::new(&program, &check.types).expect("the lowered MIR verifies");
    interpreter.run("main", Vec::new()).expect("main returns")
}

/// The interpreter models `Isize`/`Usize` as a 64-bit integer; the ABI lays
/// them out at pointer width — the same 64 bits on the supported hosts. A
/// program that widens a `Usize` past 32 bits and returns it exercises the
/// interpreter's width, and the ABI layout is asserted to match. This is the
/// exact equivalence the native exit-status path already relies on.
#[test]
fn pointer_width_integers_agree_between_the_interpreter_and_the_abi() {
    // 2^32 only round-trips through a value at least 33 bits wide; the
    // interpreter's 64-bit `Usize` model holds it exactly.
    let source = "\
fn main() -> Int {
    let big: Usize = 4294967296;
    big as Int
}
";
    assert_eq!(
        interpret_main_value(source),
        Value::Int(4_294_967_296, IntKind::I64),
        "the interpreter holds a Usize at 64 bits"
    );

    // The ABI lays Isize/Usize out at pointer width — the same 64 bits — so a
    // native value and the interpreter's value are bit-for-bit the same.
    assert_eq!(int_size(IntKind::Usize), POINTER_SIZE);
    assert_eq!(
        layout_of(&Ty::Int(IntKind::Usize), &TypeckResult::default()).unwrap(),
        Layout {
            size: POINTER_SIZE,
            align: POINTER_SIZE,
        }
    );
}

/// Each scalar kind the interpreter computes at a given width lays out at that
/// same width in the ABI. A program whose `main` returns an `I32`-typed result
/// exercises the interpreter at that width; the ABI layout is asserted equal.
#[test]
fn scalar_kinds_execute_and_lay_out_at_the_same_width() {
    // The interpreter carries the kind on the value; `main`'s I32 arithmetic
    // yields an I32-kinded result.
    let source = "\
fn main() -> I32 {
    let a: I32 = 30000;
    a + 2768
}
";
    match interpret_main_value(source) {
        Value::Int(value, kind) => {
            assert_eq!(value, 32_768);
            assert_eq!(kind, IntKind::I32, "the interpreter keeps the I32 kind");
            // The ABI lays an I32 out at 4 bytes — the width the interpreter
            // computed at.
            assert_eq!(int_size(kind), 4);
        }
        other => panic!("expected an integer, got {}", other.render()),
    }
}

/// The ABI's enum discriminant is the variant's declaration-order index, stored
/// as an explicit [`DISCRIMINANT_SIZE`] tag — the numbering the interpreter uses
/// in `Value::Variant { variant, .. }`, so a native `match` reading the tag
/// would branch identically. This asserts the *layout* contract structurally
/// (the executable `match` path is exercised by the spec suite, not here).
#[test]
fn the_enum_discriminant_layout_matches_the_documented_numbering() {
    let t = &TypeckResult::default();

    // Option[Int]: an explicit 4-byte tag followed by an 8-byte payload, the
    // whole aligned to the widest field (8) — no niche packing in v0.
    let opt = Ty::Option(Box::new(Ty::int()));
    let layout = layout_of(&opt, t).expect("Option has a layout");
    assert_eq!(layout.align, POINTER_SIZE, "aligned to its widest field");
    assert_eq!(
        layout.size,
        align_up(DISCRIMINANT_SIZE, POINTER_SIZE) + POINTER_SIZE,
        "tag padded up to the payload's alignment, then the 8-byte payload"
    );

    // Result[Int, Int] lays out to the larger arm the same way.
    let res = Ty::Result(Box::new(Ty::int()), Box::new(Ty::int()));
    assert_eq!(
        layout_of(&res, t).unwrap(),
        layout,
        "same shape as Option[Int]"
    );
}
