//! Incrementality tests for the query database: memoized reuse, precise
//! invalidation, independence of unrelated files, early cutoff, and
//! cancellation safety.
//!
//! The tests observe *executions* (actual recomputations) through the
//! database's instrumentation log; cache hits and revalidations do not
//! appear in it.

use tuo_db::{Database, QueryError};
use tuo_source::FileId;

/// A database with two files of known line counts.
fn fixture() -> (Database, FileId, FileId) {
    let mut db = Database::new();
    let main = db.intern_file("main.tuo");
    let lib = db.intern_file("lib.tuo");
    db.set_source_text(main, "fn main() {\n    greet();\n}\n")
        .expect("fixture fits"); // 4 lines (trailing newline opens line 4)
    db.set_source_text(lib, "fn greet() {}\n")
        .expect("fixture fits"); // 2 lines
    (db, main, lib)
}

#[test]
fn unchanged_inputs_reuse_results() {
    let (db, main, _) = fixture();

    assert_eq!(db.line_count(main), Ok(4));
    assert_eq!(db.executions(), ["line_count(file#0)"]);

    // Asking again — and asking anything that depends on it — recomputes
    // nothing for `main`.
    db.clear_executions();
    assert_eq!(db.line_count(main), Ok(4));
    assert_eq!(db.line_count(main), Ok(4));
    assert_eq!(db.executions(), Vec::<String>::new());
}

#[test]
fn derived_queries_compose_and_reuse_lower_memos() {
    let (db, main, _) = fixture();

    // Prime one per-file memo first.
    assert_eq!(db.line_count(main), Ok(4));
    db.clear_executions();

    // The total executes itself and the missing per-file query only.
    assert_eq!(db.total_line_count(), Ok(6));
    assert_eq!(
        db.executions(),
        ["line_count(file#1)", "total_line_count()"]
    );
}

#[test]
fn changed_source_invalidates_dependents() {
    let (mut db, main, _) = fixture();
    assert_eq!(db.line_count(main), Ok(4));
    assert_eq!(db.total_line_count(), Ok(6));

    db.set_source_text(main, "fn main() {}\n").expect("fits");
    db.clear_executions();

    assert_eq!(db.line_count(main), Ok(2));
    assert_eq!(db.total_line_count(), Ok(4));
    assert_eq!(
        db.executions(),
        ["line_count(file#0)", "total_line_count()"]
    );
}

#[test]
fn unrelated_files_do_not_invalidate_each_other() {
    let (mut db, main, lib) = fixture();
    assert_eq!(db.line_count(main), Ok(4));
    assert_eq!(db.line_count(lib), Ok(2));

    // Change only `lib`.
    db.set_source_text(lib, "fn greet() {}\nfn wave() {}\n")
        .expect("fits");
    db.clear_executions();

    // `main`'s query revalidates against its unchanged input without
    // re-executing.
    assert_eq!(db.line_count(main), Ok(4));
    assert_eq!(db.executions(), Vec::<String>::new());

    // `lib`'s query does re-execute.
    assert_eq!(db.line_count(lib), Ok(3));
    assert_eq!(db.executions(), ["line_count(file#1)"]);
}

#[test]
fn early_cutoff_shields_downstream_queries() {
    let (mut db, main, _) = fixture();
    assert_eq!(db.total_line_count(), Ok(6));

    // Rewrite `main` with different text but the same number of lines: the
    // per-file query must re-execute, but its equal result keeps its old
    // changed_at, so the total is revalidated without re-execution.
    db.set_source_text(main, "fn main() {\n    wave();\n}\n")
        .expect("fits");
    db.clear_executions();
    assert_eq!(db.total_line_count(), Ok(6));
    assert_eq!(db.executions(), ["line_count(file#0)"]);
}

#[test]
fn resetting_identical_text_is_cutoff_at_the_first_derived_layer() {
    let (mut db, main, _) = fixture();
    assert_eq!(db.total_line_count(), Ok(6));

    // Same bytes again: a new snapshot identity, but semantically equal
    // source. line_count re-executes and is cut off; the total never runs.
    db.set_source_text(main, "fn main() {\n    greet();\n}\n")
        .expect("fits");
    db.clear_executions();
    assert_eq!(db.total_line_count(), Ok(6));
    assert_eq!(db.executions(), ["line_count(file#0)"]);
}

#[test]
fn files_without_text_error_instead_of_guessing() {
    let mut db = Database::new();
    let ghost = db.intern_file("ghost.tuo");
    assert_eq!(
        db.line_count(ghost),
        Err(QueryError::NoText { file: ghost })
    );
    // Interned-but-textless files also do not participate in totals.
    assert_eq!(db.total_line_count(), Ok(0));
}

#[test]
fn cancellation_aborts_cleanly_and_resumes_without_corruption() {
    let (db, main, lib) = fixture();

    // Prime one memo, then cancel.
    assert_eq!(db.line_count(main), Ok(4));
    db.request_cancellation();

    // Everything — memoized or not — reports cancellation while the flag is
    // set, and nothing is computed.
    db.clear_executions();
    assert_eq!(db.line_count(main), Err(QueryError::Cancelled));
    assert_eq!(db.line_count(lib), Err(QueryError::Cancelled));
    assert_eq!(db.total_line_count(), Err(QueryError::Cancelled));
    assert_eq!(db.executions(), Vec::<String>::new());

    // After clearing, results are correct and the pre-cancellation memo is
    // still valid: only the never-computed queries execute.
    db.clear_cancellation();
    assert_eq!(db.total_line_count(), Ok(6));
    assert_eq!(
        db.executions(),
        ["line_count(file#1)", "total_line_count()"]
    );
    assert_eq!(db.line_count(main), Ok(4));
}

#[test]
fn cancellation_mid_graph_leaves_completed_subresults_valid() {
    let (mut db, main, lib) = fixture();
    assert_eq!(db.total_line_count(), Ok(6));

    // Invalidate everything, then cancel before re-asking: the stale memos
    // must survive the aborted pass untouched.
    db.set_source_text(main, "fn main() {}\n").expect("fits");
    db.set_source_text(lib, "fn greet() { wave(); }\n")
        .expect("fits");
    db.request_cancellation();
    assert_eq!(db.total_line_count(), Err(QueryError::Cancelled));

    db.clear_cancellation();
    db.clear_executions();
    assert_eq!(db.total_line_count(), Ok(4));
    assert_eq!(
        db.executions(),
        [
            "line_count(file#0)",
            "line_count(file#1)",
            "total_line_count()",
        ]
    );
}

#[test]
fn source_text_exposes_the_current_snapshot() {
    let (mut db, main, _) = fixture();
    let before = db.source_text(main).expect("text is set");
    assert_eq!(before.text(), "fn main() {\n    greet();\n}\n");

    db.set_source_text(main, "fn main() {}\n").expect("fits");
    let after = db.source_text(main).expect("text is set");
    assert_eq!(after.text(), "fn main() {}\n");
    // Snapshots are immutable: the old handle still shows the old text.
    assert_eq!(before.text(), "fn main() {\n    greet();\n}\n");
    assert!(before.revision() < after.revision());
}
