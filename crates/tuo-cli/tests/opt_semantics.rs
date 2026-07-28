//! Semantic differential and measurement for the MIR optimization passes.
//!
//! Optimization is only ever a **meaning-preserving rewrite** of MIR: the
//! reference interpreter's result on the *unoptimized* MIR is the program's
//! meaning, and the optimized MIR a backend consumes must compute exactly the
//! same thing. This suite pins that invariant directly at the MIR level —
//! interpret the raw MIR, interpret the optimized MIR, and require identical
//! outcomes (same return value, or the same trap) — over both the dedicated
//! optimization fixtures and the shared codegen fixtures. It is the semantic
//! twin of the `tuo-mir` before/after golden suite, and lives here because the
//! interpreter sits above `tuo-mir` in the dependency layering.
//!
//! It also **measures** the two effects the prompt asks for, and prints them
//! (visible with `cargo test -- --nocapture`):
//!
//! * **compile-time cost** — how long the pass pipeline takes, and
//! * **generated-code effect** — the reduction in MIR size (statements +
//!   locals + blocks), a backend-independent proxy for how much less code the
//!   backends have to lower.
//!
//! Trap preservation is a first-class case here: a program that divides by
//! zero must still trap after optimization, never be folded to a value.

use std::path::{Path, PathBuf};
use std::time::Instant;

use tuo_compiler::ast::Ast;
use tuo_compiler::source::SourceMap;
use tuo_compiler::{hir, mir, parser};
use tuo_mir::Program;
use tuo_mir_interp::{Interpreter, RuntimeError, Value};
use tuo_types::TypeckResult;

/// A normalized run outcome for cross-program comparison.
#[derive(PartialEq, Eq, Debug)]
enum Outcome {
    Returned(String),
    Trapped(String),
}

/// Lower an accepted program to raw MIR and its type-check result. Panics on a
/// front-end error (all fixtures here are accepted programs).
fn lower(name: &str, text: &str) -> (Program, TypeckResult) {
    let mut map = SourceMap::new();
    let file = map.intern_file(name);
    let id = map.add_source(file, text).expect("fixture fits");
    let check = tuo_compiler::check_sources(&map, &[id]);
    assert!(!check.has_errors(), "{name}: front-end errors");
    let parse = parser::parse(map.source(id));
    let asts = [Ast::new(&parse.tree, map.source(id).text())];
    let lowered_hir = hir::lower(&asts, &check.resolution);
    let program = mir::lower(&lowered_hir, &check.resolution, &check.types);
    assert!(
        mir::verify(&program, &check.types).is_empty(),
        "{name}: raw MIR failed verification"
    );
    (program, check.types)
}

/// Run a program's `main` through the interpreter, normalizing to an outcome.
fn run(program: &Program, types: &TypeckResult) -> Outcome {
    let interpreter = Interpreter::new(program, types).expect("verified MIR runs");
    match interpreter.run("main", Vec::new()) {
        Ok(value) => Outcome::Returned(render(&value)),
        Err(RuntimeError { kind, .. }) => Outcome::Trapped(kind.label().to_owned()),
    }
}

fn render(value: &Value) -> String {
    value.render()
}

/// A crude backend-independent size of a program: statements + locals +
/// blocks over every function. Smaller means less for a backend to lower.
fn program_size(program: &Program) -> usize {
    program
        .functions
        .iter()
        .map(|function| {
            function.locals.len()
                + function.blocks.len()
                + function
                    .blocks
                    .iter()
                    .map(|block| block.statements.len())
                    .sum::<usize>()
        })
        .sum()
}

fn opt_fixtures() -> Vec<PathBuf> {
    collect_tuo(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/mir/opt/fixtures"))
}

fn codegen_fixtures() -> Vec<PathBuf> {
    collect_tuo(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/codegen/fixtures"))
}

fn collect_tuo(root: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(root)
        .expect("fixture dir exists")
        .map(|entry| entry.expect("readable entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "tuo"))
        .collect();
    entries.sort();
    entries
}

/// Interpret raw vs optimized MIR; they must agree. Returns `(before, after)`
/// sizes for measurement, or `None` if the fixture has no runnable `main`
/// (some codegen fixtures are deliberately outside the subset).
fn check_equivalence(name: &str, text: &str) -> Option<(usize, usize)> {
    let (raw, types) = lower(name, text);
    // Only fixtures with a `main` are runnable here.
    if !raw.functions.iter().any(|f| f.name == "main") {
        return None;
    }
    let before = run(&raw, &types);

    let mut optimized = raw.clone();
    let _ = mir::optimize(&mut optimized, &types);
    assert!(
        mir::verify(&optimized, &types).is_empty(),
        "{name}: optimized MIR failed verification"
    );
    let after = run(&optimized, &types);

    assert_eq!(
        before, after,
        "{name}: optimization changed observable meaning (before={before:?}, after={after:?})"
    );
    Some((program_size(&raw), program_size(&optimized)))
}

#[test]
fn optimized_mir_agrees_with_the_interpreter_over_opt_fixtures() {
    let fixtures = opt_fixtures();
    assert!(fixtures.len() >= 4, "opt fixture corpus went missing");
    for path in fixtures {
        let name = path
            .file_name()
            .expect("name")
            .to_string_lossy()
            .into_owned();
        let text = std::fs::read_to_string(&path).expect("readable");
        check_equivalence(&name, &text);
    }
}

#[test]
fn optimized_mir_agrees_with_the_interpreter_over_codegen_fixtures() {
    for path in codegen_fixtures() {
        let name = path
            .file_name()
            .expect("name")
            .to_string_lossy()
            .into_owned();
        let text = std::fs::read_to_string(&path).expect("readable");
        // Some codegen fixtures are intentionally outside the runnable subset
        // (they test refusal); skip those, exercise the rest.
        let mut map = SourceMap::new();
        let file = map.intern_file(&name);
        let Ok(id) = map.add_source(file, text.as_str()) else {
            continue;
        };
        if tuo_compiler::check_sources(&map, &[id]).has_errors() {
            continue;
        }
        check_equivalence(&name, &text);
    }
}

#[test]
fn preserves_a_division_by_zero_trap() {
    // The load-bearing soundness case, stated on its own: a program that traps
    // must still trap after optimization — the constant divisor must NOT be
    // folded away.
    let text = "fn main() -> I64 {\n    let z: I64 = 0;\n    return 10 / z;\n}\n";
    let (raw, types) = lower("trap.tuo", text);
    assert_eq!(
        run(&raw, &types),
        Outcome::Trapped("division_by_zero".to_owned())
    );
    let mut optimized = raw.clone();
    let _ = mir::optimize(&mut optimized, &types);
    assert_eq!(
        run(&optimized, &types),
        Outcome::Trapped("division_by_zero".to_owned()),
        "the trap must survive optimization"
    );
}

#[test]
#[expect(
    clippy::print_stdout,
    reason = "a measurement test: it reports compile-time cost and code-size effect on stdout"
)]
fn measures_compile_time_cost_and_code_size_effect() {
    // Aggregate the two measurements the prompt asks for across every runnable
    // fixture, and print a small report (visible with `--nocapture`).
    let mut fixtures = opt_fixtures();
    fixtures.extend(codegen_fixtures());

    let mut total_before = 0usize;
    let mut total_after = 0usize;
    let mut total_opt_time = std::time::Duration::ZERO;
    let mut measured = 0usize;

    println!("\nMIR optimization measurement (per fixture):");
    for path in fixtures {
        let name = path
            .file_name()
            .expect("name")
            .to_string_lossy()
            .into_owned();
        let text = std::fs::read_to_string(&path).expect("readable");

        let mut map = SourceMap::new();
        let file = map.intern_file(&name);
        let Ok(id) = map.add_source(file, text.as_str()) else {
            continue;
        };
        if tuo_compiler::check_sources(&map, &[id]).has_errors() {
            continue;
        }
        let (raw, types) = lower(&name, &text);
        let before = program_size(&raw);

        let mut optimized = raw.clone();
        let start = Instant::now();
        let _ = mir::optimize(&mut optimized, &types);
        let elapsed = start.elapsed();

        let after = program_size(&optimized);
        total_before += before;
        total_after += after;
        total_opt_time += elapsed;
        measured += 1;

        println!(
            "  {name:<28} size {before:>4} -> {after:<4} ({:>3}% smaller)  opt {:>8.1?}",
            percent_smaller(before, after),
            elapsed
        );
    }

    assert!(measured > 0, "no runnable fixtures were measured");
    println!(
        "  {:-<28} total {total_before:>4} -> {total_after:<4} ({:>3}% smaller)  opt {:>8.1?}\n",
        "",
        percent_smaller(total_before, total_after),
        total_opt_time
    );

    // The measurement is only meaningful if optimization actually shrinks the
    // aggregate MIR — a guard against the pipeline silently becoming a no-op.
    assert!(
        total_after < total_before,
        "optimization did not reduce aggregate MIR size ({total_before} -> {total_after})"
    );
}

/// Percent reduction from `before` to `after` (0 if `before` is 0).
fn percent_smaller(before: usize, after: usize) -> usize {
    if before == 0 {
        return 0;
    }
    (before.saturating_sub(after)) * 100 / before
}
