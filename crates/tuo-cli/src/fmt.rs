//! The `tuo fmt` command: rewrite files into tuonelang's canonical form, or
//! verify them with `--check`.

use std::path::PathBuf;
use std::process::ExitCode;

use tuo_compiler::source::SourceMap;

/// Format `files` in place, or with `check` only report the ones that are
/// not canonical.
///
/// Exit status: success when every file is (now) canonical; failure when
/// `--check` found non-canonical files, a file could not be read or
/// written, or the formatter refused a file (self-check bail-out).
#[expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "this is the CLI presentation layer: stdout lists files, stderr carries errors"
)]
pub(crate) fn run(check: bool, files: &[PathBuf]) -> ExitCode {
    let mut failed = false;
    let mut dirty = false;
    for path in files {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) => {
                eprintln!("error: cannot read {}: {error}", path.display());
                failed = true;
                continue;
            }
        };
        let mut map = SourceMap::new();
        let file = map.intern_file(&path.display().to_string());
        let id = match map.add_source(file, text.as_str()) {
            Ok(id) => id,
            Err(error) => {
                eprintln!("error: {}: {error}", path.display());
                failed = true;
                continue;
            }
        };
        let outcome = tuo_fmt::format_source(map.source(id));
        if !outcome.safe {
            eprintln!(
                "error: {}: refusing to format — the formatter could not verify that the \
                 result preserves the file's meaning (this is a formatter limitation; the \
                 file was left untouched)",
                path.display()
            );
            failed = true;
            continue;
        }
        if !outcome.changed {
            continue;
        }
        if check {
            println!("would reformat: {}", path.display());
            dirty = true;
        } else if let Err(error) = std::fs::write(path, &outcome.text) {
            eprintln!("error: cannot write {}: {error}", path.display());
            failed = true;
        }
    }
    if failed || dirty {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
