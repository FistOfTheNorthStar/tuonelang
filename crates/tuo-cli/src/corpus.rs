//! `tuo corpus validate`: run a program through the compiler-validated corpus
//! pipeline and report its per-stage results and metadata.
//!
//! The corpus pipeline ([`tuo_corpus`]) drives the real compiler stages —
//! format → parse → resolve → type check → ownership → MIR verify →
//! specs/tests — and this command supplies the one stage the corpus crate
//! cannot perform alone: **native execution**, injected through the
//! [`NativeExecutor`](tuo_corpus::NativeExecutor) seam and backed by the CLI's
//! own compile-link-run machinery ([`crate::codegen::native_run`]).
//!
//! The command validates the given files as one candidate. `--category`
//! declares which corpus contract to check against (default `correct`); the
//! command *proves* the candidate meets that contract (a correct program passes
//! everything; a repair program fails at exactly its named stage) rather than
//! trusting the label. In a machine format it emits the full
//! [`EntryMetadata`](tuo_corpus::EntryMetadata) as a protocol item; in human
//! mode it prints a per-stage summary.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::{Value, json};
use tuo_compiler::source::{SourceId, SourceMap};
use tuo_corpus::{
    Candidate, Category, Config, NativeExecutor, NativeRun, Origin, Rejection, SourceFile, admit,
};

use crate::codegen::{self, NativeRunResult};
use crate::output::OutputMode;
use crate::protocol::{Event, ProtocolCommand, Status};

/// The CLI's native-execution seam for the corpus pipeline: compile, link, and
/// run a program through the real backend + `cc`, reporting the outcome.
struct CliNativeExecutor;

impl NativeExecutor for CliNativeExecutor {
    fn compile_and_run(&self, map: &SourceMap, sources: &[SourceId]) -> NativeRun {
        match codegen::native_run(map, sources) {
            NativeRunResult::Ran { exit_status } => NativeRun::Ran { exit_status },
            NativeRunResult::Failed { reason } => NativeRun::Failed { reason },
        }
    }
}

/// `tuo corpus validate [--category <c>] [--origin <o>] <files>`.
#[expect(
    clippy::print_stderr,
    reason = "the CLI presentation layer reports validation results on stderr in human mode"
)]
pub(crate) fn validate(
    category: CorpusCategory,
    origin: CorpusOrigin,
    files: &[PathBuf],
    mode: OutputMode,
) -> ExitCode {
    if files.is_empty() {
        return error(mode, "no source files given to validate");
    }

    // Read the candidate's files from disk.
    let mut source_files = Vec::with_capacity(files.len());
    for path in files {
        match std::fs::read_to_string(path) {
            Ok(text) => source_files.push(SourceFile::new(display(path), text)),
            Err(io) => {
                return error(mode, &format!("could not read {}: {io}", display(path)));
            }
        }
    }

    let candidate = Candidate {
        category: category.into(),
        origin: origin.into(),
        files: source_files,
        fixed: None,
    };

    let native = CliNativeExecutor;
    match admit(&candidate, Config::default(), Some(&native)) {
        Ok(entry) => {
            if mode.is_machine() {
                emit_admitted(mode, category, &entry);
            } else {
                report_admitted_human(category, &entry);
            }
            ExitCode::SUCCESS
        }
        Err(rejection) => {
            if mode.is_machine() {
                emit_rejected(mode, category, &rejection);
            } else {
                eprintln!(
                    "rejected from the `{}` corpus: {}",
                    category.id(),
                    describe(&rejection)
                );
            }
            ExitCode::FAILURE
        }
    }
}

/// Emit the admitted entry's metadata as a single protocol item.
fn emit_admitted(mode: OutputMode, category: CorpusCategory, entry: &tuo_corpus::AdmittedEntry) {
    let metadata = serde_json::to_value(&entry.metadata).unwrap_or(Value::Null);
    let payload = json!({
        "kind": "corpus_entry",
        "category": category.id(),
        "admitted": true,
        "metadata": metadata,
    });
    emit_item(mode, Status::Ok, payload);
}

/// Emit a rejection as a single protocol item.
fn emit_rejected(mode: OutputMode, category: CorpusCategory, rejection: &Rejection) {
    let payload = json!({
        "kind": "corpus_entry",
        "category": category.id(),
        "admitted": false,
        "reason": describe(rejection),
    });
    emit_item(mode, Status::Error, payload);
}

/// Emit a started → item → finished exchange for one validation.
fn emit_item(mode: OutputMode, status: Status, payload: Value) {
    let Some(mut emitter) = mode.emitter(ProtocolCommand::Corpus) else {
        return;
    };
    let write = (|| -> std::io::Result<()> {
        emitter.emit(&Event::started(&[] as &[String]))?;
        emitter.emit(&Event::item(status, payload))?;
        emitter.emit(&Event::finished(status, json!({})))?;
        emitter.finish()
    })();
    if write.is_err() {
        mode.log("protocol: stdout write failed");
    }
}

/// Print a human-readable per-stage summary of an admitted entry.
#[expect(
    clippy::print_stdout,
    reason = "the CLI presentation layer prints the human summary to stdout"
)]
fn report_admitted_human(category: CorpusCategory, entry: &tuo_corpus::AdmittedEntry) {
    println!("admitted to the `{}` corpus", category.id());
    println!(
        "  language version: {}",
        entry.metadata.language_version.as_str()
    );
    println!("  origin: {}", entry.metadata.origin.id());
    let features: Vec<&str> = entry.metadata.features.iter().map(|f| f.id()).collect();
    println!(
        "  features: {}",
        if features.is_empty() {
            "(none)".to_string()
        } else {
            features.join(", ")
        }
    );
    let c = &entry.metadata.complexity;
    println!(
        "  complexity: {} bytes, {} lines, {} items, {} specs, {} features",
        c.bytes, c.lines, c.items, c.specs, c.features
    );
    println!("  validation:");
    for (name, status) in entry.metadata.validation.stages() {
        println!("    {name:<18} {}", stage_word(status));
    }
    if !entry.metadata.token_counts.is_empty() {
        let counts: Vec<String> = entry
            .metadata
            .token_counts
            .iter()
            .map(|tc| format!("{}={}", tc.tokenizer, tc.tokens))
            .collect();
        println!("  tokens: {}", counts.join(", "));
    }
}

/// A human word for a stage status.
fn stage_word(status: tuo_corpus::StageStatus) -> &'static str {
    match status {
        tuo_corpus::StageStatus::Passed => "passed",
        tuo_corpus::StageStatus::Failed => "failed",
        tuo_corpus::StageStatus::Skipped => "skipped",
    }
}

/// A human description of a rejection.
fn describe(rejection: &Rejection) -> String {
    match rejection {
        Rejection::NotClean { stage } => {
            format!("expected a correct program, but the `{stage}` stage failed")
        }
        Rejection::ExpectedFailureButClean => {
            "expected a broken program, but it passed the whole pipeline".to_string()
        }
        Rejection::WrongFailureStage { expected, actual } => {
            format!("expected the failure at `{expected}`, but it failed at `{actual}`")
        }
        Rejection::FixNotClean { stage } => {
            format!("the attached fix does not validate: its `{stage}` stage failed")
        }
        Rejection::Empty => "the candidate has no source files".to_string(),
    }
}

/// Report a usage/IO error through the active mode.
#[expect(
    clippy::print_stderr,
    reason = "the CLI presentation layer reports usage errors on stderr in human mode"
)]
fn error(mode: OutputMode, message: &str) -> ExitCode {
    if mode.is_machine() {
        emit_item(
            mode,
            Status::Error,
            json!({ "kind": "corpus_entry", "admitted": false, "reason": message }),
        );
    } else {
        eprintln!("error: {message}");
    }
    ExitCode::FAILURE
}

/// A file path as a display string.
fn display(path: &Path) -> String {
    path.display().to_string()
}

/// The corpus category, as a CLI value.
#[derive(Clone, Copy, PartialEq, Eq, Debug, clap::ValueEnum)]
pub(crate) enum CorpusCategory {
    /// Programs that pass the whole pipeline.
    Correct,
    /// Programs with a syntax error, for syntax-error repair.
    SyntaxRepair,
    /// Programs that fail type checking, for type-error repair.
    TypeRepair,
    /// Programs that fail ownership checking, for ownership-error repair.
    OwnershipRepair,
    /// Statically valid programs with a failing spec, for failing-spec repair.
    SpecRepair,
    /// A repository-level (multi-file) change validated as a whole.
    RepositoryChange,
}

impl CorpusCategory {
    fn id(self) -> &'static str {
        Category::from(self).id()
    }
}

impl From<CorpusCategory> for Category {
    fn from(value: CorpusCategory) -> Self {
        match value {
            CorpusCategory::Correct => Category::Correct,
            CorpusCategory::SyntaxRepair => Category::SyntaxRepair,
            CorpusCategory::TypeRepair => Category::TypeRepair,
            CorpusCategory::OwnershipRepair => Category::OwnershipRepair,
            CorpusCategory::SpecRepair => Category::SpecRepair,
            CorpusCategory::RepositoryChange => Category::RepositoryChange,
        }
    }
}

/// The candidate's stated origin, as a CLI value.
#[derive(Clone, Copy, PartialEq, Eq, Debug, clap::ValueEnum)]
pub(crate) enum CorpusOrigin {
    /// Written by a human.
    Human,
    /// Emitted by a deterministic generator.
    Generator,
    /// Produced by an LLM.
    Llm,
    /// Adapted from an external benchmark task.
    TransformedBenchmark,
}

impl From<CorpusOrigin> for Origin {
    fn from(value: CorpusOrigin) -> Self {
        match value {
            CorpusOrigin::Human => Origin::Human,
            CorpusOrigin::Generator => Origin::Generator,
            CorpusOrigin::Llm => Origin::Llm,
            CorpusOrigin::TransformedBenchmark => Origin::TransformedBenchmark,
        }
    }
}
