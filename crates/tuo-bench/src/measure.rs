//! The measurement engine.
//!
//! Given a [`Registry`] of tokenizers and a [`FixtureFile`] of syntax
//! candidates, the engine produces a [`MeasurementReport`]: for every
//! (candidate, tokenizer) pair it records token count and bytes-per-token, and
//! because each candidate declares its [`Level`], the same numbers are the
//! tokens-per-construct, tokens-per-function, and tokens-per-program figures
//! Prompt 2 asks for.
//!
//! The engine is tokenizer-agnostic: it only calls [`Tokenizer`] methods, so it
//! never changes when adapters are added.

use serde::{Deserialize, Serialize};

use crate::SCHEMA_VERSION;
use crate::fixtures::{FixtureFile, Level};
use crate::tokenizer::{Registry, Tokenizer};

/// The metrics for one candidate under one tokenizer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Measurement {
    /// The tokenizer's stable id.
    pub tokenizer: String,
    /// Number of tokens the candidate source encodes to.
    pub token_count: usize,
    /// Number of UTF-8 bytes in the candidate source.
    pub byte_count: usize,
    /// Bytes per token (`byte_count / token_count`); `0.0` for empty input.
    /// Higher is "denser" — more source bytes carried per token.
    pub bytes_per_token: f64,
}

/// All measurements for one candidate (across every tokenizer).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateResult {
    /// The candidate's label within its group.
    pub label: String,
    /// The measured source text (echoed for traceability of the result file).
    pub source: String,
    /// Per-tokenizer measurements, in registry order.
    pub measurements: Vec<Measurement>,
}

/// All results for one comparison group.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupResult {
    /// The group's stable id.
    pub id: String,
    /// The group's question.
    pub question: String,
    /// The measurement level for this group.
    pub level: Level,
    /// Per-candidate results.
    pub candidates: Vec<CandidateResult>,
}

/// The complete, machine-readable output of a measurement run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeasurementReport {
    /// Output schema version ([`crate::SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// The ids of the tokenizers used, in order — so a reader knows exactly
    /// which tokenizers a result reflects.
    pub tokenizers: Vec<String>,
    /// Results per comparison group.
    pub groups: Vec<GroupResult>,
}

/// Compute bytes-per-token, guarding against division by zero for empty input.
fn bytes_per_token(byte_count: usize, token_count: usize) -> f64 {
    if token_count == 0 {
        0.0
    } else {
        byte_count as f64 / token_count as f64
    }
}

/// Measure a single source string under one tokenizer.
#[must_use]
pub fn measure_one(tokenizer: &dyn Tokenizer, source: &str) -> Measurement {
    let token_count = tokenizer.token_count(source);
    let byte_count = source.len();
    Measurement {
        tokenizer: tokenizer.id().to_string(),
        token_count,
        byte_count,
        bytes_per_token: bytes_per_token(byte_count, token_count),
    }
}

/// Run every candidate in `fixtures` through every tokenizer in `registry`.
#[must_use]
pub fn run(registry: &Registry, fixtures: &FixtureFile) -> MeasurementReport {
    let tokenizers: Vec<String> = registry.iter().map(|t| t.id().to_string()).collect();

    let groups = fixtures
        .groups
        .iter()
        .map(|group| {
            let candidates = group
                .candidates
                .iter()
                .map(|candidate| {
                    let measurements = registry
                        .iter()
                        .map(|tok| measure_one(tok, &candidate.source))
                        .collect();
                    CandidateResult {
                        label: candidate.label.clone(),
                        source: candidate.source.clone(),
                        measurements,
                    }
                })
                .collect();
            GroupResult {
                id: group.id.clone(),
                question: group.question.clone(),
                level: group.level,
                candidates,
            }
        })
        .collect();

    MeasurementReport {
        schema_version: SCHEMA_VERSION,
        tokenizers,
        groups,
    }
}

impl MeasurementReport {
    /// Serialize the report to pretty JSON for a committed result file.
    ///
    /// # Errors
    ///
    /// Returns the underlying [`serde_json::Error`] if serialization fails.
    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{Candidate, ComparisonGroup};

    fn sample_fixtures() -> FixtureFile {
        FixtureFile {
            description: "t".into(),
            groups: vec![ComparisonGroup {
                id: "function-keyword".into(),
                question: "Which function keyword?".into(),
                level: Level::Construct,
                constitution_ref: None,
                candidates: vec![
                    Candidate {
                        label: "fn".into(),
                        source: "fn".into(),
                        notes: vec![],
                    },
                    Candidate {
                        label: "function".into(),
                        source: "function".into(),
                        notes: vec![],
                    },
                ],
            }],
        }
    }

    #[test]
    fn bytes_per_token_handles_empty_input() {
        assert_eq!(bytes_per_token(0, 0), 0.0);
        assert_eq!(bytes_per_token(8, 2), 4.0);
    }

    #[test]
    fn run_measures_every_candidate_under_every_tokenizer() {
        let registry = Registry::with_builtin_adapters();
        let report = run(&registry, &sample_fixtures());

        assert_eq!(report.schema_version, SCHEMA_VERSION);
        assert_eq!(report.tokenizers.len(), registry.len());
        assert_eq!(report.groups.len(), 1);

        let group = &report.groups[0];
        assert_eq!(group.candidates.len(), 2);
        for cand in &group.candidates {
            // One measurement per registered tokenizer.
            assert_eq!(cand.measurements.len(), registry.len());
            for m in &cand.measurements {
                assert_eq!(m.byte_count, cand.source.len());
                assert!(m.token_count >= 1);
            }
        }
    }

    #[test]
    fn byte_tokenizer_token_count_equals_byte_count() {
        let registry = Registry::with_builtin_adapters();
        let report = run(&registry, &sample_fixtures());
        let function = &report.groups[0].candidates[1]; // "function"
        let byte_m = function
            .measurements
            .iter()
            .find(|m| m.tokenizer == "bytes")
            .expect("byte tokenizer present");
        assert_eq!(byte_m.token_count, "function".len());
        assert!((byte_m.bytes_per_token - 1.0).abs() < f64::EPSILON);
    }
}
