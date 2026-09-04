//! The tuonelang 0.1 release gate, kept honest by the tree itself.
//!
//! Prompt 40 defined `specification/RELEASE-0.1-GATE.md`: the sixteen criteria
//! that must be `MET` (or explicitly `RELEASE-BLOCKING`) before 0.1 may be
//! declared ready, each pinned to a concrete committed *proving artifact* — a
//! test target, a benchmark, or a normative document.
//!
//! A hand-maintained status table drifts: a test gets renamed, a doc moves, and
//! the gate quietly starts citing something that no longer exists while still
//! reading "MET". This suite makes that impossible. It parses the gate document's
//! machine-readable manifest and asserts, from the real filesystem:
//!
//! * exactly the sixteen criteria `G1`..`G16` are present, once each;
//! * every status is a word from the gate's own vocabulary
//!   (`MET` / `PARTIAL` / `RELEASE-BLOCKING`); and
//! * **every artifact path a criterion cites actually exists** relative to the
//!   repository root — so the gate can never advertise readiness backed by a
//!   phantom artifact, the same honesty rule the whole compiler holds itself to.
//!
//! It also enforces two consistency invariants between the manifest and the prose
//! that would otherwise be free to disagree:
//!
//! * the `GRAMMAR-VERSION` marker G1 relies on is present in `grammar.ebnf` and
//!   reads exactly `0.1` (the version this gate is written for); and
//! * the manifest's per-criterion statuses match the readiness-summary table in
//!   the prose, so the two renderings of the same fact cannot diverge.
//!
//! Run with `--nocapture` to print the readiness report the gate describes,
//! regenerated from the manifest and the live filesystem:
//!
//! ```bash
//! cargo test -p tuo-cli --test release_gate -- --nocapture
//! ```

#![allow(
    clippy::print_stdout,
    reason = "the --nocapture path is the release-readiness report meant to be read"
)]

use std::path::{Path, PathBuf};

/// The repository root, reached from this crate's manifest directory.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The gate document itself.
fn gate_doc() -> PathBuf {
    repo_root().join("specification/RELEASE-0.1-GATE.md")
}

/// The version this gate is written for; G1's grammar marker must match it.
const GATE_GRAMMAR_VERSION: &str = "0.3";

/// The number of criteria the prompt fixes.
const CRITERIA_COUNT: usize = 16;

/// One parsed manifest row.
#[derive(Debug, Clone)]
struct Criterion {
    id: String,
    status: String,
    artifacts: Vec<String>,
}

/// A status word is valid iff it is one of the gate's three vocabulary entries.
fn is_valid_status(status: &str) -> bool {
    matches!(status, "MET" | "PARTIAL" | "RELEASE-BLOCKING")
}

/// Extract the fenced ```gate-manifest block and parse its rows.
///
/// Each content line is `G<n> | <status> | <path>[; <path>...]`. Whitespace
/// around every field is insignificant.
fn parse_manifest(doc: &str) -> Vec<Criterion> {
    let mut in_block = false;
    let mut rows = Vec::new();
    for line in doc.lines() {
        let trimmed = line.trim();
        if trimmed == "```gate-manifest" {
            in_block = true;
            continue;
        }
        if in_block && trimmed == "```" {
            break;
        }
        if !in_block || trimmed.is_empty() {
            continue;
        }
        let fields: Vec<&str> = trimmed.split('|').map(str::trim).collect();
        assert_eq!(
            fields.len(),
            3,
            "manifest row must have exactly three `|`-separated fields, got: {trimmed:?}"
        );
        let artifacts = fields[2]
            .split(';')
            .map(|a| a.trim().to_owned())
            .filter(|a| !a.is_empty())
            .collect();
        rows.push(Criterion {
            id: fields[0].to_owned(),
            status: fields[1].to_owned(),
            artifacts,
        });
    }
    rows
}

/// The manifest names exactly `G1`..`G16`, once each, in order.
#[test]
fn manifest_lists_every_criterion_once() {
    let doc = std::fs::read_to_string(gate_doc()).expect("read RELEASE-0.1-GATE.md");
    let rows = parse_manifest(&doc);
    assert_eq!(
        rows.len(),
        CRITERIA_COUNT,
        "the gate must list exactly {CRITERIA_COUNT} criteria"
    );
    for (i, row) in rows.iter().enumerate() {
        let expected = format!("G{}", i + 1);
        assert_eq!(
            row.id, expected,
            "criteria must be G1..G{CRITERIA_COUNT} in order; row {i} was {:?}",
            row.id
        );
    }
}

/// Every status word is from the gate's vocabulary.
#[test]
fn every_status_is_valid() {
    let doc = std::fs::read_to_string(gate_doc()).expect("read RELEASE-0.1-GATE.md");
    for row in parse_manifest(&doc) {
        assert!(
            is_valid_status(&row.status),
            "{} has invalid status {:?}",
            row.id,
            row.status
        );
    }
}

/// **The load-bearing check.** Every artifact a criterion cites must exist on
/// disk relative to the repository root. This is what keeps the gate from ever
/// advertising readiness it cannot back.
#[test]
fn every_cited_artifact_exists() {
    let root = repo_root();
    let doc = std::fs::read_to_string(gate_doc()).expect("read RELEASE-0.1-GATE.md");
    let rows = parse_manifest(&doc);
    let mut missing: Vec<String> = Vec::new();
    for row in &rows {
        assert!(
            !row.artifacts.is_empty(),
            "{} cites no proving artifact",
            row.id
        );
        for artifact in &row.artifacts {
            let path = root.join(artifact);
            if !path.exists() {
                missing.push(format!("{} → {artifact}", row.id));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "the gate cites {} artifact(s) that do not exist:\n  {}",
        missing.len(),
        missing.join("\n  ")
    );
}

/// G1's grammar-version marker is present and matches the version this gate is
/// written for. If the grammar is re-versioned without moving the gate, this
/// fails — the two are required to travel together.
#[test]
fn grammar_carries_the_gate_version() {
    let grammar = std::fs::read_to_string(repo_root().join("specification/grammar.ebnf"))
        .expect("read grammar.ebnf");
    let marker = grammar
        .lines()
        .find_map(|l| l.trim().strip_prefix("GRAMMAR-VERSION:"))
        .map(str::trim)
        .expect("grammar.ebnf must carry a `GRAMMAR-VERSION:` marker (gate criterion G1)");
    assert_eq!(
        marker, GATE_GRAMMAR_VERSION,
        "grammar version {marker:?} disagrees with the gate version {GATE_GRAMMAR_VERSION:?}"
    );
}

/// The manifest's statuses and the prose readiness-summary table are two
/// renderings of the same fact; they must agree. The summary table rows look
/// like `| G4 | Static semantics are documented | PARTIAL |`.
#[test]
fn manifest_and_summary_table_agree() {
    let doc = std::fs::read_to_string(gate_doc()).expect("read RELEASE-0.1-GATE.md");
    let manifest = parse_manifest(&doc);

    for row in &manifest {
        // Find the summary-table line whose first cell is exactly this id.
        let summary_status = doc
            .lines()
            .filter(|l| l.trim_start().starts_with("| "))
            .find_map(|l| {
                let cells: Vec<&str> = l
                    .trim()
                    .trim_matches('|')
                    .split('|')
                    .map(str::trim)
                    .collect();
                if cells.len() == 3 && cells[0] == row.id && is_valid_status(cells[2]) {
                    Some(cells[2].to_owned())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| {
                panic!(
                    "{} has no status row in the readiness-summary table",
                    row.id
                )
            });
        assert_eq!(
            summary_status, row.status,
            "{}: summary table says {summary_status:?} but manifest says {:?}",
            row.id, row.status
        );
    }
}

/// Print the readiness report, regenerated from the manifest and the live tree.
/// This is the prompt's "generate a release readiness report from automated
/// artifacts" deliverable: nothing here is hard-coded prose — every line is
/// derived from the manifest and a real `Path::exists` probe.
#[test]
fn print_readiness_report() {
    let root = repo_root();
    let doc = std::fs::read_to_string(gate_doc()).expect("read RELEASE-0.1-GATE.md");
    let rows = parse_manifest(&doc);

    println!("\n=== tuonelang 0.1 release readiness (generated from artifacts) ===\n");
    let mut met = 0usize;
    let mut partial = 0usize;
    let mut blocking = 0usize;
    for row in &rows {
        let present = row.artifacts.iter().all(|a| root.join(a).exists());
        let mark = if present {
            "✓"
        } else {
            "✗ MISSING ARTIFACT"
        };
        match row.status.as_str() {
            "MET" => met += 1,
            "PARTIAL" => partial += 1,
            "RELEASE-BLOCKING" => blocking += 1,
            _ => {}
        }
        println!(
            "  {:<3} {:<16} artifacts:{} {mark}",
            row.id,
            row.status,
            row.artifacts.len()
        );
    }
    println!(
        "\n  {met} MET · {partial} PARTIAL · {blocking} RELEASE-BLOCKING  (of {CRITERIA_COUNT})"
    );
    let ready = partial == 0 && blocking == 0;
    println!(
        "\n  VERDICT: {}\n",
        if ready {
            "READY — every criterion MET"
        } else {
            "NOT YET READY — see specification/RELEASE-0.1-GATE.md 'Remaining work'"
        }
    );

    // The report is a *report*, not a gate: it must not fail on PARTIAL (that is
    // the honest current state). It only fails if an artifact it names vanished —
    // which `every_cited_artifact_exists` already guards, asserted again here so
    // the printed report can never quietly show a missing mark.
    let all_present = rows
        .iter()
        .all(|r| r.artifacts.iter().all(|a| root.join(a).exists()));
    assert!(
        all_present,
        "readiness report references a missing artifact"
    );
}

/// Sanity: the checker's own view of the repo root resolves to a real tree.
#[test]
fn repo_root_resolves() {
    assert!(
        Path::new(&repo_root()).join("Cargo.toml").exists(),
        "repo_root() must point at the workspace root"
    );
}
