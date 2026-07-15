//! Syntax-comparison fixtures: the version-controlled inputs to the tokenizer
//! lab.
//!
//! A [`FixtureFile`] holds a set of [`ComparisonGroup`]s. Each group poses one
//! syntax question (e.g. "which function keyword?") and lists the
//! [`Candidate`] spellings to compare. Fixtures are plain JSON on disk so they
//! are reviewable, diffable, and machine-readable.
//!
//! # Why fixtures, not hard-coded strings
//!
//! Keeping the candidate syntax in data (not code) means new comparisons are
//! added by editing a JSON file, and the *same* engine measures them against
//! *every* registered tokenizer. This is the mechanism that stops a syntax
//! decision from being quietly tuned to one tokenizer.
//!
//! # Token count is not the decision
//!
//! Each candidate can carry `notes` recording non-token considerations
//! (readability, ambiguity, familiarity). The harness reports token metrics; it
//! never ranks candidates for you. See the tool README's decision policy.

use serde::{Deserialize, Serialize};

/// The granularity a comparison operates at. This is exactly the set of levels
/// Prompt 2 requires the harness to measure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Level {
    /// A single syntactic construct in isolation (a keyword, a generic clause,
    /// an import line).
    Construct,
    /// A small but representative complete function.
    Function,
    /// A representative complete program.
    Program,
}

/// One candidate spelling within a [`ComparisonGroup`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    /// A short, stable label for this candidate (e.g. `"fn"`, `"func"`).
    pub label: String,
    /// The actual source text measured. For `Level::Construct` this may be just
    /// the fragment (e.g. `"fn"`); for higher levels it is complete code.
    pub source: String,
    /// Optional non-token considerations for this candidate (readability,
    /// ambiguity, prior art). Recorded so reviewers weigh more than token count.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// A single syntax question with the candidates to compare.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComparisonGroup {
    /// A stable identifier for the group (e.g. `"function-keyword"`).
    pub id: String,
    /// A human-readable question this group explores.
    pub question: String,
    /// The measurement granularity for every candidate in the group.
    pub level: Level,
    /// The Constitution section this relates to, if any (e.g. `"§8"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constitution_ref: Option<String>,
    /// The candidate spellings to compare. At least two are expected for a
    /// meaningful comparison.
    pub candidates: Vec<Candidate>,
}

/// A version-controlled file of comparison groups.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixtureFile {
    /// Free-form description of what this fixture file covers.
    pub description: String,
    /// The comparison groups.
    pub groups: Vec<ComparisonGroup>,
}

impl FixtureFile {
    /// Parse a fixture file from JSON text.
    ///
    /// # Errors
    ///
    /// Returns the underlying [`serde_json::Error`] if the text is not valid
    /// JSON in the fixture shape.
    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }

    /// Serialize this fixture file to pretty JSON.
    ///
    /// # Errors
    ///
    /// Returns the underlying [`serde_json::Error`] if serialization fails
    /// (not expected for this data shape).
    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Total number of candidates across all groups.
    #[must_use]
    pub fn candidate_count(&self) -> usize {
        self.groups.iter().map(|g| g.candidates.len()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_round_trips_through_json() {
        let file = FixtureFile {
            description: "test".into(),
            groups: vec![ComparisonGroup {
                id: "function-keyword".into(),
                question: "Which keyword introduces a function?".into(),
                level: Level::Construct,
                constitution_ref: Some("§8".into()),
                candidates: vec![
                    Candidate {
                        label: "fn".into(),
                        source: "fn".into(),
                        notes: vec!["shortest".into()],
                    },
                    Candidate {
                        label: "function".into(),
                        source: "function".into(),
                        notes: vec![],
                    },
                ],
            }],
        };
        let json = file.to_json_pretty().unwrap();
        let parsed = FixtureFile::from_json(&json).unwrap();
        assert_eq!(file, parsed);
        assert_eq!(parsed.candidate_count(), 2);
    }
}
