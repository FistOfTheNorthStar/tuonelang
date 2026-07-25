//! The `tuo fmt` command: rewrite files into tuonelang's canonical form, or
//! verify them with `--check`.
//!
//! In `human` mode `--check` lists non-canonical files on stdout and errors go
//! to stderr. In a machine format the run is a [`crate::protocol`] event
//! stream on stdout: `started`, one `item` event per file (payload
//! `kind: "fmt_file"`, with its outcome and path), then `finished` with a
//! summary.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::{Value, json};
use tuo_compiler::source::SourceMap;

use crate::output::OutputMode;
use crate::protocol::{Event, ProtocolCommand, Status};

/// One file's formatting outcome.
enum FileOutcome {
    /// Already canonical; nothing to do.
    Unchanged,
    /// Rewritten in place (not `--check`).
    Reformatted,
    /// Would be rewritten (`--check` found it non-canonical).
    WouldReformat,
    /// The formatter could not verify meaning-preservation; left untouched.
    Unsafe,
    /// The file could not be read or written.
    Failed(String),
}

impl FileOutcome {
    /// The stable wire tag.
    const fn tag(&self) -> &'static str {
        match self {
            Self::Unchanged => "unchanged",
            Self::Reformatted => "reformatted",
            Self::WouldReformat => "would-reformat",
            Self::Unsafe => "unsafe",
            Self::Failed(_) => "error",
        }
    }

    /// Does this outcome make the whole run fail?
    const fn is_failure(&self) -> bool {
        matches!(self, Self::WouldReformat | Self::Unsafe | Self::Failed(_))
    }
}

/// Format `files` in place, or with `check` only report the ones that are not
/// canonical, reporting through `mode`.
///
/// Exit status: success when every file is (now) canonical; failure when
/// `--check` found non-canonical files, a file could not be read or
/// written, or the formatter refused a file (self-check bail-out).
pub(crate) fn run(check: bool, files: &[PathBuf], mode: OutputMode) -> ExitCode {
    if mode.is_machine() {
        run_machine(check, files, mode)
    } else {
        run_human(check, files)
    }
}

/// Compute one file's outcome without any I/O side effects beyond the write a
/// non-`--check` reformat performs.
fn process(path: &Path, check: bool) -> FileOutcome {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => return FileOutcome::Failed(format!("cannot read: {error}")),
    };
    let mut map = SourceMap::new();
    let file = map.intern_file(&path.display().to_string());
    let id = match map.add_source(file, text.as_str()) {
        Ok(id) => id,
        Err(error) => return FileOutcome::Failed(error.to_string()),
    };
    let outcome = tuo_fmt::format_source(map.source(id));
    if !outcome.safe {
        return FileOutcome::Unsafe;
    }
    if !outcome.changed {
        return FileOutcome::Unchanged;
    }
    if check {
        FileOutcome::WouldReformat
    } else if let Err(error) = std::fs::write(path, &outcome.text) {
        FileOutcome::Failed(format!("cannot write: {error}"))
    } else {
        FileOutcome::Reformatted
    }
}

/// Human mode: keep the conventional stdout-lists / stderr-errors behavior.
#[expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "this is the CLI presentation layer: stdout lists files, stderr carries errors"
)]
fn run_human(check: bool, files: &[PathBuf]) -> ExitCode {
    let mut failed = false;
    for path in files {
        match process(path, check) {
            FileOutcome::Unchanged | FileOutcome::Reformatted => {}
            FileOutcome::WouldReformat => {
                println!("would reformat: {}", path.display());
                failed = true;
            }
            FileOutcome::Unsafe => {
                eprintln!(
                    "error: {}: refusing to format — the formatter could not verify that the \
                     result preserves the file's meaning (this is a formatter limitation; the \
                     file was left untouched)",
                    path.display()
                );
                failed = true;
            }
            FileOutcome::Failed(message) => {
                eprintln!("error: {}: {message}", path.display());
                failed = true;
            }
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Machine mode: drive the protocol event stream on stdout.
fn run_machine(check: bool, files: &[PathBuf], mode: OutputMode) -> ExitCode {
    let Some(mut emitter) = mode.emitter(ProtocolCommand::Fmt) else {
        return ExitCode::FAILURE;
    };
    let names: Vec<String> = files.iter().map(|p| p.display().to_string()).collect();
    let mut any_failure = false;
    let mut reformatted = 0u64;
    let mut would = 0u64;
    let write = (|| -> std::io::Result<()> {
        emitter.emit(&Event::started(&names))?;
        for path in files {
            let outcome = process(path, check);
            match outcome {
                FileOutcome::Reformatted => reformatted += 1,
                FileOutcome::WouldReformat => would += 1,
                _ => {}
            }
            any_failure |= outcome.is_failure();
            let status = if outcome.is_failure() {
                Status::Error
            } else {
                Status::Ok
            };
            emitter.emit(&Event::item(status, file_payload(path, &outcome)))?;
        }
        let summary = json!({
            "check": check,
            "reformatted": reformatted,
            "would_reformat": would,
        });
        let status = if any_failure {
            Status::Error
        } else {
            Status::Ok
        };
        emitter.emit(&Event::finished(status, summary))?;
        emitter.finish()
    })();
    if write.is_err() {
        mode.log("protocol: stdout write failed");
    }
    if any_failure {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// The `item` event payload for one formatted file.
fn file_payload(path: &Path, outcome: &FileOutcome) -> Value {
    let mut object = json!({
        "kind": "fmt_file",
        "file": path.display().to_string(),
        "outcome": outcome.tag(),
    });
    if let FileOutcome::Failed(message) = outcome {
        object["message"] = json!(message);
    }
    object
}
