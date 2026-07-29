//! Fine-grained incremental invalidation: the five edit scenarios, asserted on
//! the *exact* set of per-item queries that re-execute.
//!
//! The session records every derived-query **execution** (not a cache hit or a
//! revalidation) in an execution log. Each scenario primes a program, clears
//! the log, applies one edit, re-asks every query, and asserts which per-item
//! queries actually re-ran. Whole-program digest queries (`resolution()`,
//! `interface(...)`) may re-run and then hit **early cutoff**, so they can
//! appear in the log while still *not* cascading — the assertions therefore
//! target the per-symbol stage queries (`type_of`, `mir_of`, `spec_deps`)
//! whose re-execution is the real signal.

use tuo_compiler::IncrementalSession;
use tuo_resolve::SymbolId;

/// Two-file program: `lib.tuo` defines `add` and `double`; `main.tuo` defines
/// `main` and a spec of `add`.
const LIB: &str = "\
fn add(take a: Int, take b: Int) -> Int {
    a + b
}

fn double(take x: Int) -> Int {
    x * 2
}
";

const MAIN: &str = "\
fn main() -> Int {
    add(2, 3)
}

spec add {
    given a: Int = 2, b: Int = 3;
    then add(a, b) == 5;
}
";

/// A primed session plus the symbol ids of the functions and the spec.
struct Fixture {
    session: IncrementalSession,
    add: SymbolId,
    double: SymbolId,
    main: SymbolId,
    spec: SymbolId,
}

/// Prime the session: set both files and ask every query once so all memos
/// exist. Returns the fixture with resolved symbol ids.
fn prime() -> Fixture {
    let mut session = IncrementalSession::new();
    session.set_file("lib.tuo", LIB);
    session.set_file("main.tuo", MAIN);
    reask(&session);

    // Function symbols come back in declaration order across files
    // (lib: add, double; main: main); the spec is the sole spec symbol.
    let funcs = session.function_symbols();
    assert_eq!(funcs.len(), 3, "expected exactly add, double, main");
    Fixture {
        add: funcs[0],
        double: funcs[1],
        main: funcs[2],
        spec: session.spec_symbols()[0],
        session,
    }
}

/// Ask every per-item query once, so every memo is populated (or revalidated).
fn reask(session: &IncrementalSession) {
    session.resolution().unwrap();
    for file in session.files() {
        session.parse(file).unwrap();
    }
    for f in session.function_symbols() {
        session.type_of_function(f).unwrap();
        session.mir_of(f).unwrap();
    }
    for s in session.spec_symbols() {
        session.spec_dependencies(s).unwrap();
    }
}

/// The `mir_of(symN)` / `type_of(symN)` / `spec_deps(symN)` label for a symbol.
fn mir_label(sym: SymbolId) -> String {
    format!("mir_of(sym{})", sym.as_u32())
}
fn type_label(sym: SymbolId) -> String {
    format!("type_of(sym{})", sym.as_u32())
}
fn spec_label(sym: SymbolId) -> String {
    format!("spec_deps(sym{})", sym.as_u32())
}

fn ran(log: &[String], label: &str) -> bool {
    log.iter().any(|entry| entry == label)
}

#[test]
fn scenario_no_change_recomputes_nothing() {
    let fx = prime();
    fx.session.clear_executions();
    reask(&fx.session);
    assert_eq!(
        fx.session.executed_queries(),
        Vec::<String>::new(),
        "asking again after no edit must recompute nothing"
    );
}

#[test]
fn scenario_function_body_only_edit() {
    let fx = prime();
    fx.session.clear_executions();
    // Change only `add`'s body (a + b -> b + a); signatures untouched.
    let edited = LIB.replace("    a + b\n", "    b + a\n");
    let mut session = fx.session;
    session.set_file("lib.tuo", &edited);
    reask(&session);
    let log = session.executed_queries();

    // The edited function's MIR re-lowers.
    assert!(
        ran(&log, &mir_label(fx.add)),
        "add's MIR must re-lower: {log:?}"
    );
    // A sibling in the same file does NOT re-lower (function-level body cutoff).
    assert!(
        !ran(&log, &mir_label(fx.double)),
        "double's MIR must not re-lower for an edit to add: {log:?}"
    );
    // No signature re-checks, and the caller/spec are untouched.
    assert!(
        !ran(&log, &type_label(fx.add)),
        "add's type must not re-check: {log:?}"
    );
    assert!(
        !ran(&log, &type_label(fx.double)),
        "double's type must not re-check: {log:?}"
    );
    assert!(
        !ran(&log, &mir_label(fx.main)),
        "main's MIR must not re-lower: {log:?}"
    );
    assert!(
        !ran(&log, &spec_label(fx.spec)),
        "the spec graph must not re-run: {log:?}"
    );
}

#[test]
fn scenario_function_signature_edit() {
    let fx = prime();
    fx.session.clear_executions();
    // Add a parameter to `add`: a signature change.
    let edited = LIB.replace(
        "fn add(take a: Int, take b: Int) -> Int {\n    a + b\n}",
        "fn add(take a: Int, take b: Int, take c: Int) -> Int {\n    a + b + c\n}",
    );
    let mut session = fx.session;
    session.set_file("lib.tuo", &edited);
    reask(&session);
    let log = session.executed_queries();

    // The edited function's signature re-checks and its MIR re-lowers.
    assert!(
        ran(&log, &type_label(fx.add)),
        "add's type must re-check: {log:?}"
    );
    assert!(
        ran(&log, &mir_label(fx.add)),
        "add's MIR must re-lower: {log:?}"
    );
    // The spec of `add` has `add` in its closure, so its dependency graph re-runs.
    assert!(
        ran(&log, &spec_label(fx.spec)),
        "add's spec graph must re-run: {log:?}"
    );
}

#[test]
fn scenario_unrelated_file_edit() {
    let fx = prime();
    fx.session.clear_executions();
    // Edit `main.tuo`'s body only; `lib.tuo` is untouched.
    let edited = MAIN.replace("    add(2, 3)\n", "    add(3, 4)\n");
    let mut session = fx.session;
    session.set_file("main.tuo", &edited);
    reask(&session);
    let log = session.executed_queries();

    // Nothing defined in the untouched lib.tuo re-executes.
    assert!(
        !ran(&log, &mir_label(fx.add)),
        "add is in the untouched file: {log:?}"
    );
    assert!(
        !ran(&log, &mir_label(fx.double)),
        "double is in the untouched file: {log:?}"
    );
    assert!(
        !ran(&log, &type_label(fx.add)),
        "add's type must not re-check: {log:?}"
    );
    // The edited file's own function re-lowers.
    assert!(
        ran(&log, &mir_label(fx.main)),
        "main's MIR must re-lower: {log:?}"
    );
}

#[test]
fn scenario_spec_only_edit_changing_dependencies() {
    // A spec that depends on `add`; the edit repoints it at `double`, changing
    // its semantic dependency set — the one spec edit that a dependency-graph
    // query must react to.
    let mut session = IncrementalSession::new();
    session.set_file("lib.tuo", LIB);
    session.set_file(
        "main.tuo",
        "fn main() -> Int {\n    add(2, 3)\n}\n\nspec \"s\" {\n    then add(1, 1) == 2;\n}\n",
    );
    reask(&session);
    let funcs = session.function_symbols();
    let (add, double, main) = (funcs[0], funcs[1], funcs[2]);
    let spec = session.spec_symbols()[0];

    session.clear_executions();
    // Edit only the spec: call `double` instead of `add` — a dependency change.
    session.set_file(
        "main.tuo",
        "fn main() -> Int {\n    add(2, 3)\n}\n\nspec \"s\" {\n    then double(21) == 42;\n}\n",
    );
    reask(&session);
    let log = session.executed_queries();

    // The edited spec's dependency graph re-runs (its dependency set changed).
    assert!(
        ran(&log, &spec_label(spec)),
        "the edited spec's dependency graph must re-run: {log:?}"
    );
    // No function's type re-checks or MIR re-lowers from a spec-only edit —
    // functions' bodies and signatures are untouched.
    assert!(
        !ran(&log, &type_label(add)),
        "add's type must not re-check: {log:?}"
    );
    assert!(
        !ran(&log, &mir_label(add)),
        "add's MIR must not re-lower: {log:?}"
    );
    assert!(
        !ran(&log, &type_label(double)),
        "double's type must not re-check: {log:?}"
    );
    assert!(
        !ran(&log, &mir_label(double)),
        "double's MIR must not re-lower: {log:?}"
    );
    assert!(
        !ran(&log, &mir_label(main)),
        "main's MIR must not re-lower: {log:?}"
    );
}

#[test]
fn scenario_spec_value_only_edit_is_a_clean_cutoff() {
    // Editing only the *values* an assertion tests (not which symbols it
    // references) leaves the spec's dependency graph unchanged — so even the
    // `spec_deps` query cuts off. This is correct: the dependency graph did not
    // change, so nothing that *tracks* it needs to re-run. (Re-*executing* the
    // spec is `tuo spec`'s job and is orthogonal to dependency tracking.)
    let mut session = IncrementalSession::new();
    session.set_file("lib.tuo", LIB);
    session.set_file(
        "main.tuo",
        "fn main() -> Int {\n    add(2, 3)\n}\n\nspec add {\n    then add(1, 1) == 2;\n}\n",
    );
    reask(&session);
    let spec = session.spec_symbols()[0];

    session.clear_executions();
    session.set_file(
        "main.tuo",
        "fn main() -> Int {\n    add(2, 3)\n}\n\nspec add {\n    then add(2, 2) == 4;\n}\n",
    );
    reask(&session);
    let log = session.executed_queries();

    // The dependency set is unchanged (still just `add`), so the graph cuts off.
    assert!(
        !ran(&log, &spec_label(spec)),
        "an assertion-value-only edit leaves the dependency graph unchanged: {log:?}"
    );
}
