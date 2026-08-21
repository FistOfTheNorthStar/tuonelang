//! The performance laboratory's contract: the committed benchmark repository is
//! honest and consistent with the harness, and the harness itself upholds the
//! prompt's rules.
//!
//! These tests guard four things:
//!
//! 1. **The committed programs are the source of record.** Every supported
//!    workload's `.tuo` (and its C peer) on disk equals the string the harness
//!    embeds — they cannot drift.
//! 2. **The workloads really compile through the front end.** Each supported
//!    program is run through the *real* `check_sources` and must be accepted,
//!    with the expected entry present — so the lab never lists a "supported"
//!    workload the compiler would reject.
//! 3. **The honesty rules hold.** Unsupported workloads carry reasons and no
//!    source; the human report contains no superlative; comparisons exist only
//!    for supported workloads.
//! 4. **The committed example report is valid and regenerable.**
//!
//! A final `--nocapture` measurement test prints the full report (driving the
//! real compiler) so a reviewer can see the numbers a run produces.

use std::path::{Path, PathBuf};

use tuo_bench::lab::compare::{PeerLanguage, Verdict, comparison_for, comparison_for_peer};
use tuo_bench::lab::compiler::{self, ColdStage, Edit, measure_cold, measure_edit};
use tuo_bench::lab::env::Environment;
use tuo_bench::lab::report::{ComparisonEntry, LabReport, render_human};
use tuo_bench::lab::runtime::{Support, workloads};
use tuo_bench::lab::timing::SystemClock;
use tuo_compiler::check_sources;
use tuo_compiler::source::SourceMap;

/// Repository root (two levels up from this crate's manifest dir).
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("tuo-bench should live under <root>/crates/")
        .to_path_buf()
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()))
}

/// (1) Every supported workload's committed `.tuo` file equals the embedded
/// source, and every C peer's committed `.c` file equals the embedded peer.
#[test]
fn committed_programs_match_the_embedded_sources() {
    let root = repo_root();
    for workload in workloads() {
        let Support::Supported { source, .. } = &workload.support else {
            continue;
        };
        let tuo_path = root
            .join("benchmarks/runtime/programs/tuo")
            .join(format!("{}.tuo", workload.label));
        assert_eq!(
            &read(&tuo_path),
            source,
            "committed {} drifted from the embedded workload source",
            tuo_path.display()
        );

        let comparison = comparison_for(&workload).expect("supported workloads compare");
        let c_path = root
            .join("benchmarks/runtime/programs/c")
            .join(format!("{}.c", workload.label));
        assert_eq!(
            read(&c_path),
            comparison.peer_source,
            "committed {} drifted from the embedded peer source",
            c_path.display()
        );

        let go_comparison = comparison_for_peer(&workload, PeerLanguage::Go)
            .expect("supported workloads have a Go peer");
        let go_path = root
            .join("benchmarks/runtime/programs/go")
            .join(format!("{}.go", workload.label));
        assert_eq!(
            read(&go_path),
            go_comparison.peer_source,
            "committed {} drifted from the embedded Go peer source",
            go_path.display()
        );
    }
}

/// (2) Every supported workload is accepted by the real front end, and declares
/// an entry named `main`. A "supported" claim the compiler would reject is a lie.
#[test]
fn supported_workloads_pass_the_real_front_end() {
    for workload in workloads() {
        let Support::Supported { source, .. } = &workload.support else {
            continue;
        };
        let mut map = SourceMap::new();
        let file = map.intern_file("workload.tuo");
        let id = map
            .add_source(file, source.as_str())
            .expect("workload source fits");
        let result = check_sources(&map, &[id]);
        assert!(
            !result.has_errors(),
            "supported workload `{}` must pass the front end, diagnostics: {:?}",
            workload.label,
            result.diagnostics
        );
        assert!(
            source.contains("fn main"),
            "supported workload `{}` must define a `main` entry",
            workload.label
        );
    }
}

/// (3) Honesty: unsupported workloads carry a reason and no source; supported
/// workloads carry a source; comparisons exist for exactly the supported set.
#[test]
fn honesty_rules_hold_over_the_catalog() {
    let mut supported = 0;
    let mut unsupported = 0;
    for workload in workloads() {
        match &workload.support {
            Support::Supported { source, .. } => {
                supported += 1;
                assert!(!source.trim().is_empty());
                assert!(comparison_for(&workload).is_some());
            }
            Support::Unsupported { reason } => {
                unsupported += 1;
                assert!(!reason.trim().is_empty(), "must explain the gap");
                assert!(
                    comparison_for(&workload).is_none(),
                    "an unexpressible workload cannot be compared"
                );
            }
        }
    }
    // The v0 story: the four scalar-core workloads plus the fixed-array
    // collections workload (ADR-0004 Stage 2), the borrowed-`Str`
    // string-processing workload (ADR-0006), the allocator-core allocation
    // workload (ADR-0009), and the function-value indirect-calls workload
    // (ADR-0008 Tier 1) run; only networking is honestly recorded as
    // not-yet-expressible.
    assert_eq!(supported, 8);
    assert_eq!(unsupported, 1);
}

/// (3, cont.) The human report never markets. No superlative may appear, and
/// unsupported workloads must show their reason rather than a number.
#[test]
fn human_report_carries_no_superlative() {
    let mut report = LabReport::new(Environment::capture());
    // Fill it with real edit scenarios so the check runs against populated data.
    report.edit_scenarios = vec![measure_edit(Edit::NoOpCheck)];
    let text = render_human(&report).to_lowercase();
    for banned in [
        "blazing",
        "blazingly",
        "fastest",
        "lightning fast",
        "lightning-fast",
        "screaming fast",
        "world's fastest",
    ] {
        assert!(
            !text.contains(banned),
            "the human report must never contain the marketing phrase `{banned}`"
        );
    }
    assert!(text.contains("not yet expressible"));
}

/// (4) The committed example report parses, round-trips, and matches a fresh
/// regeneration of its deterministic parts (the edit scenarios and the workload
/// catalog). The environment block is illustrative, so it is not re-derived.
#[test]
fn committed_example_report_is_valid_and_regenerable() {
    let path = repo_root().join("benchmarks/runtime/results/example-report.json");
    let committed = LabReport::from_json(&read(&path)).expect("example report parses");

    // Schema + catalog invariants.
    assert_eq!(committed.schema_version, tuo_bench::SCHEMA_VERSION);
    assert_eq!(committed.runtime_workloads, workloads());
    assert_eq!(committed.supported_workload_count(), 8);

    // The deterministic edit scenarios regenerate identically.
    let fresh_edits = vec![
        measure_edit(Edit::NoOpCheck),
        measure_edit(Edit::FunctionBody),
        measure_edit(Edit::FunctionSignature),
        measure_edit(Edit::SingleSpec),
        measure_edit(Edit::AffectedSpecVerify),
    ];
    assert_eq!(
        committed.edit_scenarios, fresh_edits,
        "example report's edit scenarios are stale; regenerate the example"
    );

    // The example's comparisons are all recorded as skipped (no live toolchain
    // is assumed for the committed file) and cover exactly the supported set,
    // once per peer language — 8 supported workloads × 2 peers (C and Go) = 16.
    assert_eq!(committed.comparisons.len(), 16);
    for entry in &committed.comparisons {
        assert!(matches!(entry.peer, Verdict::Skipped { .. }));
    }
    let c_count = committed
        .comparisons
        .iter()
        .filter(|e| e.workload.peer == PeerLanguage::C)
        .count();
    let go_count = committed
        .comparisons
        .iter()
        .filter(|e| e.workload.peer == PeerLanguage::Go)
        .count();
    assert_eq!(c_count, 8, "every supported workload has a C peer entry");
    assert_eq!(go_count, 8, "every supported workload has a Go peer entry");

    // Round-trip.
    let reserialized = committed.to_json_pretty().expect("serialize");
    let back = LabReport::from_json(&reserialized).expect("re-parse");
    assert_eq!(committed, back);
}

/// The no-op re-check re-executes nothing, and a signature edit re-executes
/// strictly more than an independent body edit — the fine-grained invalidation
/// the compiler benchmarks are meant to demonstrate.
#[test]
fn incremental_scenarios_show_fine_grained_invalidation() {
    let no_op = measure_edit(Edit::NoOpCheck);
    let body = measure_edit(Edit::FunctionBody);
    let signature = measure_edit(Edit::FunctionSignature);

    assert_eq!(no_op.reexecuted_count, 0, "a no-op must cost nothing");
    // A signature change invalidates callers' typing/MIR that an independent body
    // change does not — so it re-executes strictly more queries.
    assert!(
        signature.reexecuted_count > body.reexecuted_count,
        "signature edit ({}) should re-execute more than body edit ({})",
        signature.reexecuted_count,
        body.reexecuted_count
    );
    // The signature edit specifically re-runs some caller typing; the body edit
    // does not touch type_of/mir_of at all.
    assert!(
        signature.reexecuted.iter().any(|q| q.contains("type_of")),
        "a signature edit must re-check some function typing, got {:?}",
        signature.reexecuted
    );
    assert!(
        !body.reexecuted.iter().any(|q| q.contains("type_of")),
        "an independent body edit must not re-check any function typing, got {:?}",
        body.reexecuted
    );
}

/// A `--nocapture` measurement: build the full report by driving the real
/// compiler and print it. Not an assertion of any number — the report states
/// plainly that its figures are measurements, not promises.
#[test]
#[expect(
    clippy::print_stdout,
    reason = "a measurement test: print the lab report under --nocapture for review"
)]
fn measured_lab_report_prints_under_nocapture() {
    let clock = SystemClock::new();
    let mut report = LabReport::new(Environment::capture());

    // Cold front-end stages over the representative aggregate/loop program
    // (struct + fixed array + bounded for, per ADR-0004), so the cold cost of
    // the new lowering is what the lab tracks.
    for stage in [ColdStage::Lex, ColdStage::Parse, ColdStage::Check] {
        if let Some(result) = measure_cold(&clock, stage, compiler::COLD_AGGREGATE, 50, 20) {
            report.cold_stages.push(result);
        }
    }

    // Incremental edits (deterministic query counts).
    report.edit_scenarios = vec![
        measure_edit(Edit::NoOpCheck),
        measure_edit(Edit::FunctionBody),
        measure_edit(Edit::FunctionSignature),
        measure_edit(Edit::SingleSpec),
        measure_edit(Edit::AffectedSpecVerify),
    ];

    // Comparisons: skipped here (this in-crate test injects no toolchain — the
    // CLI seam test drives live C and Go). One entry per peer (C and Go), so the
    // render exercises the full peer set. The point is to exercise the full
    // render, not to claim a number.
    report.comparisons = workloads()
        .iter()
        .flat_map(tuo_bench::lab::compare::comparisons_for)
        .map(|cmp| ComparisonEntry {
            workload: cmp,
            tuonelang_exit: None,
            peer: Verdict::Skipped {
                reason: "no toolchain injected in this in-crate test".to_string(),
            },
        })
        .collect();

    println!("\n{}", render_human(&report));
}
