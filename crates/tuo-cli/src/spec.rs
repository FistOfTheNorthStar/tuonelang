//! The `tuo spec` and `tuo verify` commands.
//!
//! `tuo spec` **executes** a program's specs: it runs the front end, lowers
//! each spec to verified MIR, and drives its assertions through the reference
//! interpreter in a deterministic sandbox (bounded instruction fuel, recursion
//! depth, and memory). `tuo spec --target <name>` narrows execution to the
//! specs of one function (or a free-standing spec of that name). `tuo verify`
//! performs every static check *and* runs the specs — a superset of both
//! `tuo check` and `tuo spec`.
//!
//! Neither command promises any particular latency; both report the measured
//! execution time so it can be observed. A program with front-end errors is
//! refused (a broken spec does not run — ADR-0002).
//!
//! In `human` mode results render to stderr. In a machine format the run is a
//! [`crate::protocol`] event stream on stdout: `started`, a `progress` event
//! per stage (spec execution is potentially long-running), one `item` event
//! per executed spec as it finishes (payload `kind: "spec_result"`), then
//! `finished` with a summary. Because `json-lines` flushes each event
//! immediately, a consumer watches specs complete in real time.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use serde_json::{Value, json};
use tuo_compiler::diagnostics;
use tuo_compiler::source::{SourceId, SourceMap, Span};
use tuo_spec::report::{AssertionResult, Outcome, SkippedSpec, SpecReport, SpecRun, TrapReport};
use tuo_spec::{Limits, RunOutcome, Selection};

use crate::output::OutputMode;
use crate::protocol::{self, Emitter, Event, ProtocolCommand, Status};

/// Load `files` into one program snapshot, reporting a read error through
/// `mode`.
#[expect(
    clippy::print_stderr,
    reason = "this is the CLI presentation layer: in human mode read errors go to stderr"
)]
fn load(
    files: &[PathBuf],
    command: ProtocolCommand,
    mode: OutputMode,
) -> Result<(SourceMap, Vec<SourceId>), ExitCode> {
    let mut map = SourceMap::new();
    let mut sources = Vec::new();
    for path in files {
        let display = path.display().to_string();
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) => {
                if mode.is_machine() {
                    protocol::io_error(command, mode, &display, &error.to_string());
                } else {
                    eprintln!("error: cannot read {display}: {error}");
                }
                return Err(ExitCode::FAILURE);
            }
        };
        let file = map.intern_file(&display);
        match map.add_source(file, text.as_str()) {
            Ok(id) => sources.push(id),
            Err(error) => {
                if mode.is_machine() {
                    protocol::io_error(command, mode, &display, &error.to_string());
                } else {
                    eprintln!("error: {display}: {error}");
                }
                return Err(ExitCode::FAILURE);
            }
        }
    }
    Ok((map, sources))
}

/// `tuo spec [--target] <files>`: execute the selected specs.
pub(crate) fn run(target: Option<String>, files: &[PathBuf], mode: OutputMode) -> ExitCode {
    let (map, sources) = match load(files, ProtocolCommand::Spec, mode) {
        Ok(loaded) => loaded,
        Err(code) => return code,
    };
    let selection = target.map_or(Selection::All, Selection::Target);
    execute(
        &map,
        &sources,
        &selection,
        files,
        ProtocolCommand::Spec,
        mode,
    )
}

/// `tuo verify <files>`: all static checks, then run every spec.
///
/// `tuo spec` already runs the front end and refuses a program with errors,
/// so it *is* the static-plus-dynamic check; `verify` is its whole-program
/// form (no target narrowing) and exists as the named command the workflow
/// expects.
pub(crate) fn verify(files: &[PathBuf], mode: OutputMode) -> ExitCode {
    let (map, sources) = match load(files, ProtocolCommand::Verify, mode) {
        Ok(loaded) => loaded,
        Err(code) => return code,
    };
    execute(
        &map,
        &sources,
        &Selection::All,
        files,
        ProtocolCommand::Verify,
        mode,
    )
}

/// Run the specs and present the outcome; the shared body of `spec`/`verify`.
fn execute(
    map: &SourceMap,
    sources: &[SourceId],
    selection: &Selection,
    files: &[PathBuf],
    command: ProtocolCommand,
    mode: OutputMode,
) -> ExitCode {
    let outcome = tuo_spec::run(map, sources, selection, Limits::default());
    if mode.is_machine() {
        report_machine(&outcome, map, files, command, mode)
    } else {
        report_human(&outcome, map)
    }
}

/// Human mode: render diagnostics or results to stderr; exit by outcome.
#[expect(
    clippy::print_stderr,
    reason = "this is the CLI presentation layer: stderr carries diagnostics and results"
)]
fn report_human(outcome: &RunOutcome, map: &SourceMap) -> ExitCode {
    match outcome {
        RunOutcome::NotChecked(problems) => {
            if !problems.is_empty() {
                eprint!("{}", diagnostics::render::render_all(problems, map));
            }
            eprintln!("error: cannot run specs: the program has front-end errors");
            ExitCode::FAILURE
        }
        RunOutcome::Ran(report) => {
            eprint!("{}", present(report, map));
            if report.passed() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
    }
}

/// Machine mode: drive the protocol event stream on stdout.
fn report_machine(
    outcome: &RunOutcome,
    map: &SourceMap,
    files: &[PathBuf],
    command: ProtocolCommand,
    mode: OutputMode,
) -> ExitCode {
    let Some(mut emitter) = mode.emitter(command) else {
        return ExitCode::FAILURE;
    };
    let names: Vec<String> = files.iter().map(|p| p.display().to_string()).collect();
    let write = (|| -> std::io::Result<()> {
        emitter.emit(&Event::started(&names))?;
        match outcome {
            RunOutcome::NotChecked(problems) => emit_not_checked(&mut emitter, problems, map),
            RunOutcome::Ran(report) => emit_ran(&mut emitter, report, map),
        }
    })();
    if write.is_err() {
        mode.log("protocol: stdout write failed");
    }
    let passed = match outcome {
        RunOutcome::NotChecked(_) => false,
        RunOutcome::Ran(report) => report.passed(),
    };
    if passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Emit the stream for a program the front end rejected: each diagnostic as a
/// `diagnostic` event, then `finished`/`error`.
fn emit_not_checked(
    emitter: &mut Emitter<std::io::Stdout>,
    problems: &[diagnostics::Diagnostic],
    map: &SourceMap,
) -> std::io::Result<()> {
    for problem in problems {
        emitter.emit(&Event::diagnostic(problem, map))?;
    }
    let summary = json!({ "checked": false, "reason": "front-end errors" });
    emitter.emit(&Event::finished(Status::Error, summary))?;
    emitter.finish()
}

/// Emit the stream for an executed program: a `progress` event, one `item`
/// event per run (and per skipped spec), then a `finished` summary.
fn emit_ran(
    emitter: &mut Emitter<std::io::Stdout>,
    report: &SpecReport,
    map: &SourceMap,
) -> std::io::Result<()> {
    emitter.emit(&Event::progress(
        "executing",
        format!("running {} spec(s)", report.runs.len()),
    ))?;
    for run in &report.runs {
        let status = if run.passed() {
            Status::Ok
        } else {
            Status::Error
        };
        emitter.emit(&Event::item(status, spec_run_payload(run, map)))?;
    }
    for skipped in &report.skipped {
        emitter.emit(&Event::item(Status::Ok, skipped_payload(skipped, map)))?;
    }
    let ran = report.ran();
    let failed = report.failures();
    let summary = json!({
        "passed": ran - failed,
        "failed": failed,
        "ran": ran,
        "skipped": report.skipped.len(),
        "duration_micros": report.total_duration().as_micros(),
    });
    let status = if report.passed() {
        Status::Ok
    } else {
        Status::Error
    };
    emitter.emit(&Event::finished(status, summary))?;
    emitter.finish()
}

/// The `item` event payload for one executed spec: its stable identity, source
/// range, per-assertion results, and measured duration.
fn spec_run_payload(run: &SpecRun, map: &SourceMap) -> Value {
    json!({
        "kind": "spec_result",
        "id": run.symbol.to_string(),
        "name": run.name,
        "target": run.target.map(|symbol| symbol.to_string()),
        "range": protocol::range(run.span, map),
        "duration_micros": run.duration.as_micros(),
        "passed": run.passed(),
        "assertions": run
            .assertions
            .iter()
            .map(|assertion| assertion_payload(assertion, map))
            .collect::<Vec<_>>(),
    })
}

/// The payload for one assertion result.
fn assertion_payload(assertion: &AssertionResult, map: &SourceMap) -> Value {
    let (status, detail) = match &assertion.outcome {
        Outcome::Passed => ("passed", Value::Null),
        Outcome::Failed(failure) => (
            "failed",
            json!({
                "expected": failure.expected,
                "actual": failure.actual,
            }),
        ),
        Outcome::Errored(trap) => ("errored", trap_payload(trap, map)),
    };
    json!({
        "assertion": assertion.kind.keyword(),
        "source": assertion.source,
        "range": protocol::range(assertion.span, map),
        "outcome": status,
        "detail": detail,
    })
}

/// The payload for a trap: its label, message, span, and TDG call trace.
fn trap_payload(trap: &TrapReport, map: &SourceMap) -> Value {
    json!({
        "trap": trap.label,
        "message": trap.message,
        "range": protocol::range(trap.span, map),
        "trace": trap
            .trace
            .iter()
            .map(|frame| json!({
                "function": frame.function,
                "range": protocol::range(frame.span, map),
            }))
            .collect::<Vec<_>>(),
    })
}

/// The payload for a skipped (not-lowered) spec.
fn skipped_payload(skipped: &SkippedSpec, map: &SourceMap) -> Value {
    json!({
        "kind": "spec_skipped",
        "id": skipped.symbol.to_string(),
        "name": skipped.name,
        "reason": skipped.reason,
        "range": protocol::range(skipped.span, map),
    })
}

// ---- Human rendering (deterministic text; not a protocol) ----

/// Render a report as human-readable text.
fn present(report: &SpecReport, map: &SourceMap) -> String {
    let mut out = String::new();
    for run in &report.runs {
        present_run(&mut out, run, map);
    }
    for skipped in &report.skipped {
        out.push_str(&format!(
            "skip {} — {} ({})\n",
            skipped.name,
            skipped.reason,
            location(map, skipped.span)
        ));
    }
    out.push_str(&summary(report));
    out
}

/// Render one spec's assertions and per-spec timing.
fn present_run(out: &mut String, run: &SpecRun, map: &SourceMap) {
    let status = if run.passed() { "ok" } else { "FAILED" };
    out.push_str(&format!(
        "{status} {} ({}, {})\n",
        run.name,
        location(map, run.span),
        format_duration(run.duration)
    ));
    for assertion in &run.assertions {
        match &assertion.outcome {
            Outcome::Passed => {}
            Outcome::Failed(failure) => {
                out.push_str(&format!(
                    "  {} failed: {} ({})\n",
                    assertion.kind.keyword(),
                    assertion.source,
                    location(map, assertion.span)
                ));
                if let (Some(expected), Some(actual)) = (&failure.expected, &failure.actual) {
                    out.push_str(&format!("    expected: {expected}\n"));
                    out.push_str(&format!("    actual:   {actual}\n"));
                }
            }
            Outcome::Errored(trap) => {
                out.push_str(&format!(
                    "  {} trapped: {} ({})\n",
                    assertion.kind.keyword(),
                    assertion.source,
                    location(map, assertion.span)
                ));
                present_trap(out, trap, map);
            }
        }
    }
}

/// Render a trap's cause and TDG call trace.
fn present_trap(out: &mut String, trap: &TrapReport, map: &SourceMap) {
    out.push_str(&format!(
        "    trap {}: {} ({})\n",
        trap.label,
        trap.message,
        location(map, trap.span)
    ));
    for frame in &trap.trace {
        out.push_str(&format!(
            "      at {} ({})\n",
            frame.function,
            location(map, frame.span)
        ));
    }
}

/// A one-line pass/fail/timing summary.
fn summary(report: &SpecReport) -> String {
    let ran = report.ran();
    let failed = report.failures();
    let passed = ran - failed;
    let mut line = format!(
        "\n{passed} passed, {failed} failed of {ran} spec{} in {}",
        if ran == 1 { "" } else { "s" },
        format_duration(report.total_duration())
    );
    if !report.skipped.is_empty() {
        line.push_str(&format!("; {} skipped", report.skipped.len()));
    }
    line.push('\n');
    line
}

/// `file:line:col` for a span, or `?` if it cannot be located.
fn location(map: &SourceMap, span: Span) -> String {
    let Some(text) = map.get_source(span.source()) else {
        return "?".to_owned();
    };
    let file = map.file_name(text.file());
    match text.line_col(span.range().start()) {
        Ok(lc) => format!("{file}:{}:{}", lc.line, lc.column),
        Err(_) => file.to_owned(),
    }
}

/// A stable, human-readable duration rendering.
fn format_duration(duration: Duration) -> String {
    let micros = duration.as_micros();
    if micros < 1_000 {
        format!("{micros}µs")
    } else if micros < 1_000_000 {
        format!("{:.3}ms", duration.as_secs_f64() * 1_000.0)
    } else {
        format!("{:.3}s", duration.as_secs_f64())
    }
}
