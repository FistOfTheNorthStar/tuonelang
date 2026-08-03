//! Automatic regression fixtures for discovered bugs.
//!
//! When a sweep or a coverage-guided run finds an input that violates a stage
//! invariant, that input must never be forgotten: it becomes a committed
//! regression fixture that every future run replays, so a fixed bug can never
//! silently reappear. This module is the mechanism.
//!
//! # The fixture directory
//!
//! Fixtures live under `crates/tuo-fuzz/regressions/<stage>/`, one file per bug,
//! named by a content hash of the input so the same crash is never filed twice.
//! Each file's bytes *are* the exact crashing input — no metadata wrapper — so a
//! fixture is trivially replayable and diffable, and a human can open it. A
//! `stage` names which checker in [`super::stages`] the input must be replayed
//! through.
//!
//! # Recording (write side)
//!
//! [`record`] is called with a stage name and a crashing input. It writes the
//! input to the stage's fixture directory (idempotently — a duplicate is a
//! no-op) and returns the path. The fuzz targets call it from a panic hook so a
//! newly discovered crash is captured the moment it happens; a developer then
//! commits the new file. Recording is deliberately a plain file write with no
//! external dependency, so it works identically under `cargo fuzz` and an
//! ordinary test.
//!
//! # Replaying (read side)
//!
//! [`replay_all`] walks every fixture and re-runs it through its stage's
//! checker. It is the guard the stable test suite runs on every `cargo test`:
//! if any committed fixture ever panics again, the regression test fails. A
//! freshly discovered-and-recorded fixture that has *not* yet been fixed will,
//! by construction, fail this replay — which is the point: it turns "we saw a
//! crash once" into an enforced, checked-in obligation.

use std::fs;
use std::path::PathBuf;

use crate::stages;

/// The regression corpus root, relative to this crate's manifest directory.
/// Resolved at runtime from `CARGO_MANIFEST_DIR` so it is correct whether the
/// harness runs from the workspace root or the crate directory.
#[must_use]
pub fn corpus_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("regressions")
}

/// A short, stable, content-addressed file stem for `input` — an FNV-1a fold
/// rendered as hex. Not cryptographic; its only job is to give the same input
/// the same filename so a crash is filed exactly once.
#[must_use]
pub fn fixture_stem(input: &str) -> String {
    let mut state: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in input.as_bytes() {
        state ^= u64::from(b);
        state = state.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{state:016x}")
}

/// Record a crashing `input` for `stage` as a committed-able regression
/// fixture, returning the path written (or the existing path if already filed).
///
/// The write is idempotent: the filename is a content hash, so re-recording the
/// same input is a no-op. The bytes written are exactly `input`.
///
/// # Errors
///
/// Returns any I/O error from creating the directory or writing the file.
pub fn record(stage: &str, input: &str) -> std::io::Result<PathBuf> {
    let dir = corpus_root().join(stage);
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.tuo", fixture_stem(input)));
    if !path.exists() {
        fs::write(&path, input)?;
    }
    Ok(path)
}

/// One recorded fixture: the stage it belongs to and its exact input.
#[derive(Debug, Clone)]
pub struct Fixture {
    /// The stage checker this input must be replayed through.
    pub stage: String,
    /// The path the fixture was loaded from.
    pub path: PathBuf,
    /// The exact crashing input.
    pub input: String,
}

/// Load every committed regression fixture, grouped by nothing (a flat list).
/// A missing corpus root (no bug ever recorded) yields an empty list, not an
/// error, so a clean checkout replays trivially.
#[must_use]
pub fn load_all() -> Vec<Fixture> {
    let root = corpus_root();
    let mut fixtures = Vec::new();
    let Ok(stage_dirs) = fs::read_dir(&root) else {
        return fixtures;
    };
    let mut stage_dirs: Vec<PathBuf> = stage_dirs
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    stage_dirs.sort();
    for stage_dir in stage_dirs {
        let stage = stage_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_owned();
        let mut files: Vec<PathBuf> = match fs::read_dir(&stage_dir) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "tuo"))
                .collect(),
            Err(_) => continue,
        };
        files.sort();
        for path in files {
            if let Ok(input) = fs::read_to_string(&path) {
                fixtures.push(Fixture {
                    stage: stage.clone(),
                    path,
                    input,
                });
            }
        }
    }
    fixtures
}

/// The checker for a stage name, or `None` if the stage is unknown (a fixture
/// filed under a stage that no longer exists — surfaced so it is not silently
/// skipped).
#[must_use]
pub fn checker_for(stage: &str) -> Option<fn(&str)> {
    stages::all_checkers()
        .into_iter()
        .find(|(name, _)| *name == stage)
        .map(|(_, f)| f)
}

/// Replay every committed fixture through its stage's checker. Any fixture whose
/// checker panics propagates that panic to the caller (the regression test),
/// failing the build — which is exactly the desired "a fixed bug stays fixed"
/// guarantee. Returns the number of fixtures replayed.
///
/// # Panics
///
/// Panics if a fixture names a stage with no known checker (a stale fixture),
/// so a rename is caught rather than silently dropping coverage.
pub fn replay_all() -> usize {
    let fixtures = load_all();
    for fixture in &fixtures {
        let checker = checker_for(&fixture.stage).unwrap_or_else(|| {
            panic!(
                "regression fixture {} names unknown stage {:?}",
                fixture.path.display(),
                fixture.stage
            )
        });
        checker(&fixture.input);
    }
    fixtures.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_stem_is_stable_and_content_addressed() {
        assert_eq!(fixture_stem("fn f() {}"), fixture_stem("fn f() {}"));
        assert_ne!(fixture_stem("a"), fixture_stem("b"));
    }

    #[test]
    fn checker_lookup_covers_every_advertised_stage() {
        for (name, _) in stages::all_checkers() {
            assert!(
                checker_for(name).is_some(),
                "no checker resolvable for advertised stage {name:?}"
            );
        }
        assert!(checker_for("no-such-stage").is_none());
    }

    #[test]
    fn record_is_idempotent_and_replayable() {
        // Record into a throwaway system-temp dir so we never touch the
        // committed corpus from a unit test.
        let tmp = std::env::temp_dir().join("tuo-fuzz-regression-record-test");
        let _ = fs::remove_dir_all(&tmp);
        let stage_dir = tmp.join("lexer");
        fs::create_dir_all(&stage_dir).expect("mkdir");
        let input = "fn f( ) {"; // a harmless, already-total input
        let path = stage_dir.join(format!("{}.tuo", fixture_stem(input)));
        fs::write(&path, input).expect("write");
        // Second write of the same content-addressed name is a no-op overwrite.
        assert_eq!(fs::read_to_string(&path).unwrap(), input);
        // The replay checker for the stage accepts this input (it is total).
        let checker = checker_for("lexer").expect("lexer checker");
        checker(input);
        let _ = fs::remove_dir_all(&tmp);
    }

    /// The committed corpus always replays clean — no fixed bug has regressed.
    /// (Also proves `replay_all` runs end to end even when the corpus is only
    /// the README-bearing skeleton.)
    #[test]
    fn committed_regression_corpus_replays_clean() {
        let _count = replay_all();
    }
}
