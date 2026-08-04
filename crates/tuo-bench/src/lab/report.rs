//! The lab report: the single, versioned, machine-readable record a run
//! produces, plus a human rendering that states its limits plainly.
//!
//! A [`LabReport`] is the whole `Record:` the prompt asks for, in one object:
//! the [`Environment`](super::env::Environment) (hardware, OS, versions), the
//! compiler-scenario results, the runtime-workload catalog (supported *and*
//! unsupported, with reasons), and any cross-language comparisons. Both the JSON
//! (via [`LabReport::to_json_pretty`]) and the human table (via [`render_human`])
//! come from this one object, so they cannot disagree.
//!
//! The report carries **no summary verdict and no superlative**. There is no
//! "blazing fast" field, and the renderer is written so a caller cannot smuggle
//! one in: it prints measured numbers, unmeasured workloads with their reasons,
//! and comparisons only where both sides produced a real figure. The claim the
//! repository is *capable* of proving is exactly the claim the data supports —
//! no more.

use serde::{Deserialize, Serialize};

use crate::SCHEMA_VERSION;
use crate::lab::compare::{ComparisonWorkload, Verdict};
use crate::lab::compiler::{ColdResult, EditResult};
use crate::lab::env::Environment;
use crate::lab::runtime::{RuntimeWorkload, Support};

/// One cross-language comparison entry: the workload, its peer program, and the
/// verdict (a measured figure with provenance, or a recorded skip).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComparisonEntry {
    /// The workload/peer pairing measured.
    pub workload: ComparisonWorkload,
    /// The tuonelang native exit status for this workload, when it was run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tuonelang_exit: Option<i32>,
    /// The peer-language verdict.
    pub peer: Verdict,
}

/// The complete output of one performance-laboratory run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LabReport {
    /// Output schema version ([`crate::SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// Where the run happened (hardware, OS, toolchain versions).
    pub environment: Environment,
    /// Cold, from-scratch compiler-stage timings.
    pub cold_stages: Vec<ColdResult>,
    /// Incremental edit-scenario re-execution records.
    pub edit_scenarios: Vec<EditResult>,
    /// Every runtime workload the lab tracks, supported or not.
    pub runtime_workloads: Vec<RuntimeWorkload>,
    /// Cross-language comparisons, where a peer with equivalent semantics exists.
    pub comparisons: Vec<ComparisonEntry>,
}

impl LabReport {
    /// A report with the current environment and the standard workload catalog,
    /// and no measurements yet. Callers fill the measurement vectors.
    #[must_use]
    pub fn new(environment: Environment) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            environment,
            cold_stages: Vec::new(),
            edit_scenarios: Vec::new(),
            runtime_workloads: crate::lab::runtime::workloads(),
            comparisons: Vec::new(),
        }
    }

    /// Serialize to pretty JSON for a committed result file.
    ///
    /// # Errors
    ///
    /// Returns the underlying [`serde_json::Error`] if serialization fails.
    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Parse a report from JSON.
    ///
    /// # Errors
    ///
    /// Returns the underlying [`serde_json::Error`] on malformed input.
    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }

    /// How many runtime workloads are measurable on the current language.
    #[must_use]
    pub fn supported_workload_count(&self) -> usize {
        self.runtime_workloads
            .iter()
            .filter(|w| w.is_supported())
            .count()
    }
}

/// Render a report as a human-readable, plain-text table.
///
/// This deliberately reports *only what was measured*: cold timings as min/median
/// microseconds (never a promise), incremental scenarios as the count of queries
/// that re-ran, runtime workloads split into measured and "not yet expressible"
/// (with the reason), and comparisons only where a real peer number exists. It
/// contains no superlative and no aggregate verdict.
#[must_use]
pub fn render_human(report: &LabReport) -> String {
    let mut out = String::new();
    out.push_str("tuonelang performance laboratory\n");
    out.push_str("================================\n\n");

    out.push_str("Environment\n-----------\n");
    out.push_str(&format!("  {}\n", report.environment.summary_line()));
    out.push_str(&format!(
        "  target: {}   cpu: {}   rustc: {}\n\n",
        report.environment.target_triple,
        report.environment.cpu_model.display(),
        report.environment.rustc_version.display(),
    ));

    out.push_str("Compiler — cold stages (per-op, microseconds)\n");
    out.push_str("---------------------------------------------\n");
    if report.cold_stages.is_empty() {
        out.push_str("  (none measured)\n");
    }
    for stage in &report.cold_stages {
        out.push_str(&format!(
            "  {:<12} min {:>9.3}  median {:>9.3}   [{}]\n",
            stage.label,
            stage.timing.min_micros(),
            stage.timing.median_micros(),
            stage.command,
        ));
    }
    out.push('\n');

    out.push_str("Compiler — incremental edits (queries re-executed)\n");
    out.push_str("--------------------------------------------------\n");
    if report.edit_scenarios.is_empty() {
        out.push_str("  (none measured)\n");
    }
    for edit in &report.edit_scenarios {
        out.push_str(&format!(
            "  {:<26} {:>3} re-executed   [{}]\n",
            edit.label, edit.reexecuted_count, edit.command,
        ));
    }
    out.push('\n');

    out.push_str("Runtime workloads\n-----------------\n");
    for workload in &report.runtime_workloads {
        match &workload.support {
            Support::Supported { .. } => {
                let exit = report
                    .comparisons
                    .iter()
                    .find(|c| c.workload.label == workload.label)
                    .and_then(|c| c.tuonelang_exit);
                let exit_note =
                    exit.map_or_else(|| "not run".to_string(), |code| format!("exit {code}"));
                out.push_str(&format!(
                    "  {:<20} measured ({})\n",
                    workload.label, exit_note
                ));
            }
            Support::Unsupported { reason } => {
                out.push_str(&format!(
                    "  {:<20} not yet expressible in v0 — {}\n",
                    workload.label, reason
                ));
            }
        }
    }
    out.push('\n');

    out.push_str("Cross-language comparison (equivalent semantics only)\n");
    out.push_str("-----------------------------------------------------\n");
    if report.comparisons.is_empty() {
        out.push_str("  (none run)\n");
    }
    for entry in &report.comparisons {
        match &entry.peer {
            Verdict::Measured {
                exit_status,
                compiler_version,
                ..
            } => {
                out.push_str(&format!(
                    "  {:<20} vs {}: peer exit {} [{}]\n",
                    entry.workload.label,
                    entry.workload.peer.label(),
                    exit_status,
                    compiler_version,
                ));
            }
            Verdict::Skipped { reason } => {
                out.push_str(&format!(
                    "  {:<20} vs {}: skipped — {}\n",
                    entry.workload.label,
                    entry.workload.peer.label(),
                    reason,
                ));
            }
        }
    }
    out.push('\n');
    out.push_str(
        "Note: figures are measurements on the environment above, not guarantees. \
         Unmeasured workloads report why, not a number.\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lab::env::Environment;

    #[test]
    fn new_report_carries_the_full_workload_catalog() {
        let report = LabReport::new(Environment::capture());
        assert_eq!(report.schema_version, SCHEMA_VERSION);
        assert_eq!(report.runtime_workloads.len(), 8);
        assert_eq!(report.supported_workload_count(), 4);
    }

    #[test]
    fn report_round_trips_through_json() {
        let report = LabReport::new(Environment::capture());
        let json = report.to_json_pretty().expect("serialize");
        let back = LabReport::from_json(&json).expect("deserialize");
        assert_eq!(report, back);
    }

    #[test]
    fn human_render_names_unsupported_reasons_and_no_superlative() {
        let report = LabReport::new(Environment::capture());
        let text = render_human(&report);
        // Every unsupported workload's reason surfaces.
        assert!(text.contains("not yet expressible in v0"));
        // No forbidden marketing language.
        let lowered = text.to_lowercase();
        for banned in ["blazing", "fastest", "blazingly", "lightning-fast"] {
            assert!(
                !lowered.contains(banned),
                "human report must not contain the superlative `{banned}`"
            );
        }
    }

    #[test]
    fn human_render_reports_missing_measurements_as_such() {
        let report = LabReport::new(Environment::capture());
        let text = render_human(&report);
        // With no cold stages / edits / comparisons filled in, the renderer says
        // so rather than inventing numbers.
        assert!(text.contains("(none measured)"));
        assert!(text.contains("(none run)"));
    }
}
