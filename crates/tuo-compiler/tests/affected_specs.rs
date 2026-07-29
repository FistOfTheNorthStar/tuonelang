//! Affected-spec selection: soundness (every spec whose dependency closure
//! touches a changed symbol is selected), precision (a spec whose closure is
//! disjoint from the change is not), and agreement (running only the affected
//! set yields the same verdicts as running all specs on that subset).

use tuo_compiler::IncrementalSession;
use tuo_source::SourceMap;
use tuo_spec::{Limits, RunOutcome, Selection};

/// `a.tuo` defines `add` + a spec of it; `b.tuo` defines `double` + a spec of
/// it. The two specs' dependency closures are disjoint.
const A: &str = "\
fn add(take a: Int, take b: Int) -> Int {
    a + b
}

spec add {
    then add(2, 3) == 5;
}
";

const B: &str = "\
fn double(take x: Int) -> Int {
    x * 2
}

spec double {
    then double(21) == 42;
}
";

fn session() -> (IncrementalSession, tuo_source::FileId, tuo_source::FileId) {
    let mut session = IncrementalSession::new();
    let a = session.set_file("a.tuo", A);
    let b = session.set_file("b.tuo", B);
    // Prime the graph.
    session.resolution().unwrap();
    for s in session.spec_symbols() {
        session.spec_dependencies(s).unwrap();
    }
    (session, a, b)
}

#[test]
fn selects_specs_touching_a_changed_file() {
    let (session, a, b) = session();

    let affected_a = session.affected_specs(&[a]).unwrap();
    let affected_b = session.affected_specs(&[b]).unwrap();

    // Exactly one spec is affected by each file, and they differ.
    assert_eq!(affected_a.len(), 1, "a.tuo affects exactly its own spec");
    assert_eq!(affected_b.len(), 1, "b.tuo affects exactly its own spec");
    assert_ne!(
        affected_a, affected_b,
        "the two files affect different specs"
    );
}

#[test]
fn a_disjoint_spec_is_not_selected() {
    let (session, a, _) = session();
    let all: Vec<_> = session.spec_symbols();
    let affected = session.affected_specs(&[a]).unwrap();

    // Precision: not every spec is selected — the `double` spec is disjoint
    // from a change to a.tuo.
    assert!(
        affected.len() < all.len(),
        "affected ({}) must be a strict subset of all ({})",
        affected.len(),
        all.len()
    );
    // The selected spec is the one whose closure includes `add` (defined in a).
    let add_spec = affected[0];
    let deps = session.spec_dependencies(add_spec).unwrap();
    let add_symbols = session.symbols_in_file(a);
    assert!(
        deps.iter().any(|d| add_symbols.contains(d)) || add_symbols.contains(&add_spec),
        "the selected spec's closure touches a symbol defined in a.tuo"
    );
}

#[test]
fn selecting_all_changed_files_selects_all_specs() {
    let (session, a, b) = session();
    let affected = session.affected_specs(&[a, b]).unwrap();
    let all = session.spec_symbols();
    // Changing every file affects every spec.
    let mut affected_sorted = affected;
    affected_sorted.sort();
    let mut all_sorted = all;
    all_sorted.sort();
    assert_eq!(affected_sorted, all_sorted);
}

#[test]
fn affected_subset_agrees_with_running_all() {
    // Load the same two files into a SourceMap and run specs both ways; the
    // affected subset's verdicts must match the corresponding runs in the full
    // run. (Here every spec passes, so this checks the affected set is a
    // faithful subset that does not change any spec's outcome.)
    let mut map = SourceMap::new();
    let fa = map.intern_file("a.tuo");
    let fb = map.intern_file("b.tuo");
    let sa = map.add_source(fa, A).unwrap();
    let sb = map.add_source(fb, B).unwrap();
    let sources = [sa, sb];

    // Compute the affected set for an edit to a.tuo via a session.
    let (session, a_file, _) = session();
    let affected = session.affected_specs(&[a_file]).unwrap();

    let all_run = tuo_spec::run(&map, &sources, &Selection::All, Limits::default());
    let affected_run = tuo_spec::run(
        &map,
        &sources,
        &Selection::Affected(affected.clone()),
        Limits::default(),
    );

    let (RunOutcome::Ran(all_report), RunOutcome::Ran(affected_report)) = (&all_run, &affected_run)
    else {
        panic!("both runs must execute (the program checks)");
    };

    // The affected run executes exactly the affected specs — a subset of all.
    assert_eq!(
        affected_report.runs.len(),
        affected.len(),
        "the affected run executes exactly the affected specs"
    );
    assert!(
        affected_report.runs.len() < all_report.runs.len(),
        "the affected run is a strict subset of the full run"
    );

    // Every spec run in the affected set has the same verdict as in the full
    // run (matched by spec symbol).
    for affected_spec in &affected_report.runs {
        let full = all_report
            .runs
            .iter()
            .find(|r| r.symbol == affected_spec.symbol)
            .expect("affected spec also runs in the full run");
        assert_eq!(
            affected_spec.passed(),
            full.passed(),
            "verdict for a spec must agree between affected and full runs"
        );
    }
}
