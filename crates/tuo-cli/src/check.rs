//! The `tuo check` command: parse, resolve, type-check, and
//! ownership-check a program without generating code.
//!
//! All given files form **one program snapshot** — files declaring the same
//! `module` path share a scope, so cross-file references resolve. Specs are
//! checked like every other item (parsed, attached to their targets,
//! type-checked, and ownership-checked, per ADR-0002); they are not
//! executed.
//!
//! In `human` mode diagnostics render to stderr. In a machine format the run
//! is a [`crate::protocol`] event stream on stdout: `started`, one
//! `diagnostic` event per diagnostic, then `finished` with a `{errors,
//! warnings}` summary.

use std::path::PathBuf;
use std::process::ExitCode;

use tuo_compiler::diagnostics::Severity;
use tuo_compiler::source::{SourceId, SourceMap};
use tuo_compiler::{CheckResult, check_sources, diagnostics};

use crate::output::OutputMode;
use crate::protocol::{Event, ProtocolCommand, Status};

/// Check `files` as one program, reporting through `mode`.
///
/// Exit status: success when the program has no errors; failure when a file
/// cannot be read or any pipeline stage rejects the program.
pub(crate) fn run(files: &[PathBuf], mode: OutputMode) -> ExitCode {
    let (map, sources) = match load(files, mode) {
        Ok(loaded) => loaded,
        Err(code) => return code,
    };
    let result = check_sources(&map, &sources);
    if mode.is_machine() {
        report_machine(&result, &map, files, mode)
    } else {
        report_human(&result, &map)
    }
}

/// Load `files` into one program snapshot, reporting a read error through
/// `mode` (a `finished`/`error` protocol event in machine mode, a stderr line
/// in human mode).
fn load(files: &[PathBuf], mode: OutputMode) -> Result<(SourceMap, Vec<SourceId>), ExitCode> {
    let mut map = SourceMap::new();
    let mut sources = Vec::new();
    for path in files {
        let display = path.display().to_string();
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) => return Err(read_error(mode, &display, &error.to_string())),
        };
        let file = map.intern_file(&display);
        match map.add_source(file, text.as_str()) {
            Ok(id) => sources.push(id),
            Err(error) => return Err(read_error(mode, &display, &error.to_string())),
        }
    }
    Ok((map, sources))
}

/// Report an unreadable/unusable input file through `mode`.
#[expect(
    clippy::print_stderr,
    reason = "this is the CLI presentation layer: in human mode read errors go to stderr"
)]
fn read_error(mode: OutputMode, path: &str, error: &str) -> ExitCode {
    if mode.is_machine() {
        crate::protocol::io_error(ProtocolCommand::Check, mode, path, error);
    } else {
        eprintln!("error: cannot read {path}: {error}");
    }
    ExitCode::FAILURE
}

/// Human mode: render diagnostics to stderr, exit by error state.
#[expect(
    clippy::print_stderr,
    reason = "this is the CLI presentation layer: stderr carries the diagnostics"
)]
fn report_human(result: &CheckResult, map: &SourceMap) -> ExitCode {
    if !result.diagnostics.is_empty() {
        eprint!(
            "{}",
            diagnostics::render::render_all(&result.diagnostics, map)
        );
    }
    if result.has_errors() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Machine mode: drive the protocol event stream on stdout.
fn report_machine(
    result: &CheckResult,
    map: &SourceMap,
    files: &[PathBuf],
    mode: OutputMode,
) -> ExitCode {
    let Some(mut emitter) = mode.emitter(ProtocolCommand::Check) else {
        // `is_machine()` was true, so an emitter is always produced here; this
        // branch is unreachable but avoids an unwrap.
        return ExitCode::FAILURE;
    };
    let names: Vec<String> = files.iter().map(|p| p.display().to_string()).collect();
    let outcome = || -> std::io::Result<()> {
        emitter.emit(&Event::started(&names))?;
        let (mut errors, mut warnings) = (0u64, 0u64);
        for diagnostic in &result.diagnostics {
            match diagnostic.severity {
                Severity::Error => errors += 1,
                Severity::Warning => warnings += 1,
                Severity::Note => {}
            }
            emitter.emit(&Event::diagnostic(diagnostic, map))?;
        }
        let status = if result.has_errors() {
            Status::Error
        } else {
            Status::Ok
        };
        let summary = serde_json::json!({ "errors": errors, "warnings": warnings });
        emitter.emit(&Event::finished(status, summary))?;
        emitter.finish()
    }();
    if outcome.is_err() {
        // The consumer's pipe closed; nothing more we can say on a closed
        // stream. The check itself is what determines success.
        mode.log("protocol: stdout write failed");
    }
    if result.has_errors() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
