//! The `tuo agent --stdio` transport: a JSON-lines request/response loop over
//! the process's standard streams.
//!
//! This module is *only* the transport. The protocol itself — every method, the
//! shared compiler database, determinism — lives in [`tuo_agent`], which is
//! transport-agnostic and directly testable. Here the CLI reads one request per
//! line from stdin, hands it to [`tuo_agent::Server::handle_line`], and writes
//! the response as one line to stdout. It also injects the canonical formatter
//! (`tuo-fmt`), which the agent crate cannot depend on directly because the two
//! sit at the same dependency layer.
//!
//! **stdout carries protocol output only.** Exactly one JSON response line is
//! written per request line, so a consumer parses stdout wholesale — matching
//! the discipline of the CLI's other machine output.

use std::io::{self, BufRead, Write};

use tuo_agent::Server;
use tuo_agent::session::{FormatResult, Formatter};
use tuo_compiler::source::SourceMap;

use crate::output::OutputMode;

/// Run the agent protocol server over stdio until stdin reaches EOF.
///
/// Reads one JSON request per line, writes one JSON response per line. A blank
/// line is skipped. A line that is not a valid request is answered with a
/// structured `parse_error` response (the loop never aborts on bad input). The
/// exit status is success on a clean EOF; a stdout write failure ends the loop
/// with a failure status.
pub(crate) fn run(mode: OutputMode) -> std::process::ExitCode {
    let mut server = Server::new(Box::new(FmtFormatter));
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                mode.log(&format!("agent: stdin read failed: {error}"));
                return std::process::ExitCode::FAILURE;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let response = server.handle_line(&line);
        // Serialization of a `Response` cannot fail (it is plain data); fall back
        // to nothing rather than panicking if it somehow did.
        let encoded = serde_json::to_string(&response).unwrap_or_default();
        if writeln!(out, "{encoded}")
            .and_then(|()| out.flush())
            .is_err()
        {
            // The consumer's pipe closed; there is nothing more to say.
            mode.log("agent: stdout write failed");
            return std::process::ExitCode::FAILURE;
        }
    }
    std::process::ExitCode::SUCCESS
}

/// The canonical formatter, backing the agent's `format` method with `tuo-fmt`.
struct FmtFormatter;

impl Formatter for FmtFormatter {
    fn format(&self, text: &str) -> FormatResult {
        // `format_source` needs a `SourceText`; build a throwaway map holding
        // just this text. The formatter is pure, so a fresh map per call is
        // correct (and formatting is not on any hot path).
        let mut map = SourceMap::new();
        let file = map.intern_file("<agent>");
        match map.add_source(file, text) {
            Ok(id) => {
                let outcome = tuo_fmt::format_source(map.source(id));
                FormatResult {
                    text: outcome.text,
                    changed: outcome.changed,
                    safe: outcome.safe,
                }
            }
            // The text did not fit the 32-bit offset space: return it unchanged
            // and mark it unsafe, so the caller knows nothing was verified.
            Err(_) => FormatResult {
                text: text.to_owned(),
                changed: false,
                safe: false,
            },
        }
    }
}
