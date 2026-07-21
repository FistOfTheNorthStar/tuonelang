//! The `tuo check` command: parse, resolve, type-check, and
//! ownership-check a program without generating code.
//!
//! All given files form **one program snapshot** — files declaring the same
//! `module` path share a scope, so cross-file references resolve. Specs are
//! checked like every other item (parsed, attached to their targets,
//! type-checked, and ownership-checked, per ADR-0002); they are not
//! executed.

use std::path::PathBuf;
use std::process::ExitCode;

use tuo_compiler::source::{SourceId, SourceMap};
use tuo_compiler::{check_sources, diagnostics};

/// Check `files` as one program; report every diagnostic on stderr.
///
/// Exit status: success when the program has no errors; failure when a file
/// cannot be read or any pipeline stage rejects the program.
#[expect(
    clippy::print_stderr,
    reason = "this is the CLI presentation layer: stderr carries the diagnostics"
)]
pub(crate) fn run(files: &[PathBuf]) -> ExitCode {
    let mut map = SourceMap::new();
    let mut sources: Vec<SourceId> = Vec::new();
    for path in files {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) => {
                eprintln!("error: cannot read {}: {error}", path.display());
                return ExitCode::FAILURE;
            }
        };
        let file = map.intern_file(&path.display().to_string());
        match map.add_source(file, text.as_str()) {
            Ok(id) => sources.push(id),
            Err(error) => {
                eprintln!("error: {}: {error}", path.display());
                return ExitCode::FAILURE;
            }
        }
    }
    let result = check_sources(&map, &sources);
    if !result.diagnostics.is_empty() {
        eprint!(
            "{}",
            diagnostics::render::render_all(&result.diagnostics, &map)
        );
    }
    if result.has_errors() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
