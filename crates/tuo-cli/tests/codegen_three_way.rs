//! Three-way semantic differential tests: interpreter == Cranelift == LLVM.
//!
//! tuonelang has one reference meaning: what the MIR interpreter computes for a
//! program's verified MIR. It now has *two* native backends — Cranelift (the
//! default debug build) and LLVM (`--release`) — and both must agree with that
//! reference, and therefore with each other. This suite pins all three at once:
//! for every fixture it
//!
//! 1. executes `main` on the reference [`Interpreter`](tuo_mir_interp::Interpreter),
//! 2. builds and runs the program with `tuo run` (Cranelift),
//! 3. builds and runs it with `tuo run --release` (LLVM), and
//! 4. asserts the three outcomes coincide.
//!
//! An *outcome* is what the language makes observable today: either a normal
//! return (the integer `main` yields, surfaced natively as the process exit
//! status, truncated to a byte by the v0 entry ABI exactly as a C `main` return
//! becomes the exit code) or a deterministic trap (the interpreter aborts and,
//! natively, the process terminates with the runtime's fixed trap status). A
//! disagreement between *any* pair is a compiler-correctness failure — and,
//! because a release build differs only by backend, a **release blocker**.
//!
//! This file complements the two-way suite in `codegen_differential.rs` (which
//! pins the default backend against the interpreter) and the randomized suite in
//! `differential.rs`; here the emphasis is that the *release* backend never
//! drifts from the reference or the debug backend.

use std::path::{Path, PathBuf};
use std::process::Command;

use tuo_compiler::source::SourceMap;
use tuo_mir_interp::{Interpreter, Value};

/// The runtime's fixed trap exit status (see `tuo_runtime::TRAP_EXIT_STATUS`).
const TRAP_EXIT_STATUS: i32 = 134;

/// The observable result of running a program through one engine, normalized so
/// the interpreter's wide integer and a native exit code compare on the same
/// footing. Equality *is* the differential comparison.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Outcome {
    /// `main` returned normally with this exit byte (`value & 0xff`).
    Returned(i32),
    /// The program aborted deterministically (a trap).
    Trapped,
}

/// The path to a fixture in `tests/codegen/fixtures/`.
fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/codegen/fixtures")
        .join(name)
}

/// The reference outcome of `main` for the program in `path`, from the MIR
/// interpreter.
fn interpret(path: &Path) -> Outcome {
    let text = std::fs::read_to_string(path).expect("fixture is readable");
    let mut map = SourceMap::new();
    let file = map.intern_file(&path.display().to_string());
    let id = map
        .add_source(file, text.as_str())
        .expect("fixture is valid source");

    // Front end → verified MIR (the same lowering the `build`/`run` path uses).
    let check = tuo_compiler::check_sources(&map, &[id]);
    assert!(
        !check.has_errors(),
        "fixture {} should type-check cleanly",
        path.display()
    );
    let parse = tuo_compiler::parser::parse(map.source(id));
    let asts = [tuo_compiler::ast::Ast::new(
        &parse.tree,
        map.source(id).text(),
    )];
    let lowered_hir = tuo_compiler::hir::lower(&asts, &check.resolution);
    let program = tuo_compiler::mir::lower(&lowered_hir, &check.resolution, &check.types);

    let interpreter =
        Interpreter::new(&program, &check.types).expect("lowered MIR verifies for the interpreter");
    match interpreter.run("main", Vec::new()) {
        Ok(Value::Int(value, _)) => Outcome::Returned((value & 0xff) as i32),
        Ok(other) => panic!("main returned a non-integer value: {}", other.render()),
        Err(_) => Outcome::Trapped,
    }
}

/// Build and run the fixture natively with `tuo run [--release]`, returning its
/// outcome. `release` selects the LLVM backend; otherwise the default Cranelift.
///
/// A native run either returns (an exit byte) or traps (the fixed trap status).
/// Any other exit means the program was not actually run — most likely the
/// backend *refused* it as outside the v0 subset — which is a test-precondition
/// violation, not a differential finding, so it is surfaced loudly. A refusal is
/// identified by the backend's own "subset"/"does not lower" marker on stderr,
/// never by the exit code (which a program may deliberately return).
fn run_native(path: &Path, release: bool) -> Outcome {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tuo"));
    command.arg("run");
    if release {
        command.arg("--release");
    }
    let output = command.arg(path).output().expect("the tuo binary runs");
    let code = output
        .status
        .code()
        .expect("the native process exits with a code, not a signal");

    if code == TRAP_EXIT_STATUS {
        return Outcome::Trapped;
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("subset") && !stderr.contains("does not lower"),
        "`tuo run{}` refused {} as outside the backend subset (exit {code}); every fixture in \
         this suite must be inside both backends' subset. stderr:\n{stderr}",
        if release { " --release" } else { "" },
        path.display(),
    );
    Outcome::Returned(code)
}

/// Assert the interpreter, the Cranelift backend, and the LLVM backend all
/// produce the same outcome for `name`. Any mismatch is a compiler-correctness
/// failure and a release blocker.
fn assert_three_way_agreement(name: &str) {
    let path = fixture(name);
    let reference = interpret(&path);
    let cranelift = run_native(&path, false);
    let llvm = run_native(&path, true);

    assert_eq!(
        reference, cranelift,
        "{name}: the Cranelift backend disagrees with the reference interpreter \
         (interpreter {reference:?}, cranelift {cranelift:?})"
    );
    assert_eq!(
        reference, llvm,
        "{name}: the LLVM (--release) backend disagrees with the reference interpreter \
         (interpreter {reference:?}, llvm {llvm:?}) — a release blocker"
    );
    // Redundant given the two above, but stated so a failure names the backends
    // that drifted from each other directly.
    assert_eq!(
        cranelift, llvm,
        "{name}: the two native backends disagree (cranelift {cranelift:?}, llvm {llvm:?}) — \
         a release blocker"
    );
}

#[test]
fn arithmetic_agrees_across_all_three_engines() {
    assert_three_way_agreement("arithmetic.tuo");
}

#[test]
fn comparison_and_branching_agree_across_all_three_engines() {
    assert_three_way_agreement("comparison.tuo");
}

#[test]
fn recursion_agrees_across_all_three_engines() {
    assert_three_way_agreement("recursion.tuo");
}

#[test]
fn direct_calls_agree_across_all_three_engines() {
    assert_three_way_agreement("calls.tuo");
}

#[test]
fn integer_division_agrees_across_all_three_engines() {
    assert_three_way_agreement("division.tuo");
}

#[test]
fn a_deterministic_trap_agrees_across_all_three_engines() {
    // The interpreter traps on division by zero; both native backends must also
    // abort with the runtime's fixed trap status. `assert_three_way_agreement`
    // compares `Trapped` on every side.
    assert_three_way_agreement("trap_div_zero.tuo");
}

/// The ADR-0004 Stage 1 aggregate lowering: product types (struct, tuple, enum,
/// `Option`, `Result`) whose fields are all scalars, crossing call boundaries by
/// pointer/sret. Each of these agrees across all three engines by construction —
/// every offset comes from the one runtime ABI both backends consult.
#[test]
fn stage1_aggregates_agree_across_all_three_engines() {
    for name in [
        "unsupported_struct.tuo", // struct field read (now supported)
        "agg_struct_param.tuo",   // aggregate parameter by pointer + copy-in
        "agg_struct_return.tuo",  // aggregate return via sret
        "agg_struct_both.tuo",    // sret + aggregate param ordering together
        "agg_enum_match.tuo",     // enum discriminant feeding a Switch
        "agg_enum_payload.tuo",   // enum variant payload fields
        "agg_option.tuo",         // Option[Int] payload (Some = variant 0)
        "agg_result.tuo",         // Result[Int, Int] both variants
        "agg_nested.tuo",         // a struct field that is itself a struct
        "agg_move.tuo",           // whole-aggregate move through a call
    ] {
        assert_three_way_agreement(name);
    }
}

/// ADR-0004 Stage 2 fixed-capacity arrays `[T; N]`: inline construction (both
/// literal forms), indexing (constant and loop-counter), for-folds, nesting,
/// whole-array copy, and array parameters/returns through the by-pointer/sret
/// call ABI. Each agrees across all three engines by construction — every
/// element offset is `i × stride(T)` from the one runtime ABI both backends
/// consult, and the bounds `Assert` is in the shared MIR before every index.
#[test]
fn stage2_fixed_arrays_agree_across_all_three_engines() {
    for name in [
        "arr_literal_index.tuo", // literal construction + constant index
        "arr_repeat_fold.tuo",   // repeat construction + for-fold
        "arr_param_return.tuo",  // array parameter + array return (sret)
        "arr_nested.tuo",        // `[[Int; 2]; 3]` nesting + double index
    ] {
        assert_three_way_agreement(name);
    }
}

/// Native float support: IEEE-754 arithmetic (never trapping), `%` with C
/// `fmod` semantics (Cranelift calls libm's `fmod`/`fmodf`, LLVM emits
/// `frem`), Rust-semantics NaN comparisons, saturating float→int casts
/// (NaN → 0, via `fcvt_to_*_sat` and the `llvm.fpto*i.sat` intrinsics),
/// genuine f32 arithmetic, and floats inside aggregates. All three engines
/// must agree bit for bit on the observable exit.
#[test]
fn floats_agree_across_all_three_engines() {
    for name in [
        "flt_arith.tuo",    // f64 + - * / and unary negation
        "flt_rem.tuo",      // f64 and f32 remainder (fmod/fmodf vs frem)
        "flt_compare.tuo",  // all six comparisons, including NaN cases
        "flt_cast_sat.tuo", // float→int saturation high/low, NaN → 0
        "flt_f32.tuo",      // F32 arithmetic + F32↔F64 casts
        "flt_struct.tuo",   // a Float field in a struct, passed `take`
    ] {
        assert_three_way_agreement(name);
    }
}

/// Borrow-mode (`in`/`mut`) call arguments: both backends pass the address of
/// the caller's place and the callee works through the pointer directly (no
/// copy-in, no copy-back) — observably identical to the interpreter's
/// copy-in/copy-back because the borrow checker forbids aliasing and the
/// borrow lasts only for the call.
#[test]
fn borrow_mode_calls_agree_across_all_three_engines() {
    for name in [
        "brw_scalar_in.tuo",  // scalar read through `in`
        "brw_scalar_mut.tuo", // scalar write-back observed through `mut`
        "brw_agg_in.tuo",     // struct fields read through `in` (twice — no move)
        "brw_agg_mut.tuo",    // struct field written through `mut`
        "brw_arr_in.tuo",     // `[Int; 4]` borrowed `in`, folded by `for`
        "brw_forward.tuo",    // an `in` parameter forwarded as an `in` argument
    ] {
        assert_three_way_agreement(name);
    }
}

/// An out-of-bounds index traps identically on all three engines: the
/// interpreter aborts with `IndexOutOfBounds`, and both backends abort with the
/// runtime's fixed trap status (the bounds `Assert` is lowered before the
/// unchecked address arithmetic).
#[test]
fn a_fixed_array_out_of_bounds_trap_agrees_across_all_three_engines() {
    assert_three_way_agreement("arr_trap_oob.tuo");
}

/// ADR-0006 Stage B strings: the `Str` fat pointer over static data, the
/// `std::str` byte operations, byte-wise equality via `memcmp`, and `Str`
/// crossing every call boundary shape. Both backends must agree with the
/// interpreter's byte semantics (UTF-8 is bytes: `len("héllo") == 6`) — and
/// with each other — including the deterministic `IndexOutOfBounds` trap on
/// an out-of-range `byte_at`.
#[test]
fn strings_agree_across_all_three_engines() {
    for name in [
        "str_len.tuo",        // multi-byte literal length (bytes, not chars)
        "str_eq.tuo",         // equal / unequal / empty-string comparisons
        "str_slice_scan.tuo", // slice + byte_at folded over a while loop
        "str_param.tuo",      // Str take/in params and an sret Str return
        "str_in_struct.tuo",  // a Str field inside a struct, passed take
        "str_trap_oob.tuo",   // byte_at out of bounds traps on all three
    ] {
        assert_three_way_agreement(name);
    }
}

#[test]
fn both_backends_refuse_an_unsupported_program_rather_than_miscompile() {
    // A program still outside the native subset (a function with an
    // owned-`String` local, which awaits the allocator ADR — `Str` itself is
    // lowered since ADR-0006 Stage B) must be *refused* by both backends with
    // a failure exit and an explanatory message, never silently mis-compiled.
    // The interpreter remains the reference and can still run it. This asserts
    // the two backends agree on the *boundary* of what they lower, not just on
    // results inside it.
    let path = fixture("unsupported_string.tuo");
    for release in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_tuo"));
        command.arg("build");
        if release {
            command.arg("--release");
        }
        let output = command
            .arg("-o")
            .arg(
                std::env::temp_dir()
                    .join(format!("tuo-three-way-unsupported-{}", u8::from(release))),
            )
            .arg(&path)
            .output()
            .expect("the tuo binary runs");
        let which = if release { "llvm" } else { "cranelift" };
        assert!(
            !output.status.success(),
            "the {which} backend must refuse an unsupported program, not emit a wrong binary"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("subset") || stderr.contains("does not lower"),
            "the {which} refusal should explain the program is outside the backend subset; \
             got: {stderr}"
        );
    }
}
