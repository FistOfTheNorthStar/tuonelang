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

#[test]
fn both_backends_refuse_an_unsupported_program_rather_than_miscompile() {
    // A program outside the scalar subset (an aggregate) must be *refused* by
    // both backends with a failure exit and an explanatory message, never
    // silently mis-compiled. The interpreter remains the reference and can still
    // run it. This asserts the two backends agree on the *boundary* of what they
    // lower, not just on results inside it.
    let path = fixture("unsupported_struct.tuo");
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
