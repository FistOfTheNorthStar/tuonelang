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

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use tuo_compiler::diagnostics;
use tuo_compiler::source::{SourceId, SourceMap, Span};
use tuo_spec::report::{Outcome, SpecReport, SpecRun, TrapReport};
use tuo_spec::{Limits, RunOutcome, Selection};

/// Load `files` into one program snapshot, reporting a read error to stderr.
#[expect(
    clippy::print_stderr,
    reason = "this is the CLI presentation layer: stderr carries the diagnostics"
)]
fn load(files: &[PathBuf]) -> Result<(SourceMap, Vec<SourceId>), ExitCode> {
    let mut map = SourceMap::new();
    let mut sources = Vec::new();
    for path in files {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) => {
                eprintln!("error: cannot read {}: {error}", path.display());
                return Err(ExitCode::FAILURE);
            }
        };
        let file = map.intern_file(&path.display().to_string());
        match map.add_source(file, text.as_str()) {
            Ok(id) => sources.push(id),
            Err(error) => {
                eprintln!("error: {}: {error}", path.display());
                return Err(ExitCode::FAILURE);
            }
        }
    }
    Ok((map, sources))
}

/// `tuo spec [target] <files>`: execute the selected specs.
pub(crate) fn run(target: Option<String>, files: &[PathBuf]) -> ExitCode {
    let (map, sources) = match load(files) {
        Ok(loaded) => loaded,
        Err(code) => return code,
    };
    let selection = target.map_or(Selection::All, Selection::Target);
    execute(&map, &sources, &selection)
}

/// `tuo verify <files>`: all static checks, then run every spec.
///
/// `tuo spec` already runs the front end and refuses a program with errors,
/// so it *is* the static-plus-dynamic check; `verify` is its whole-program
/// form (no target narrowing) and exists as the named command the workflow
/// expects.
pub(crate) fn verify(files: &[PathBuf]) -> ExitCode {
    let (map, sources) = match load(files) {
        Ok(loaded) => loaded,
        Err(code) => return code,
    };
    execute(&map, &sources, &Selection::All)
}

/// Run the specs and present the outcome; the shared body of `spec`/`verify`.
#[expect(
    clippy::print_stderr,
    reason = "this is the CLI presentation layer: stderr carries diagnostics and results"
)]
fn execute(map: &SourceMap, sources: &[SourceId], selection: &Selection) -> ExitCode {
    match tuo_spec::run(map, sources, selection, Limits::default()) {
        RunOutcome::NotChecked(problems) => {
            if !problems.is_empty() {
                eprint!("{}", diagnostics::render::render_all(&problems, map));
            }
            eprintln!("error: cannot run specs: the program has front-end errors");
            ExitCode::FAILURE
        }
        RunOutcome::Ran(report) => {
            eprint!("{}", present(&report, map));
            if report.passed() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
    }
}

/// Render a report as human-readable text (deterministic; not a protocol).
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
