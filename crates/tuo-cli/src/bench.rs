//! `tuo bench report`: score a recorded code-generation benchmark run by
//! **re-compiling the model's outputs** and reporting the metrics.
//!
//! The evaluation harness ([`tuo_codegen_bench`]) embeds no LLM: a model is
//! reached through a host-implemented adapter, and an external runner records its
//! outputs into a [`BenchmarkRun`](tuo_codegen_bench::BenchmarkRun) result file.
//! This command takes such a run together with the **pinned task set** it was
//! produced against and *proves* its metrics: it recompiles every recorded
//! generation through the real front end and spec runner
//! ([`tuo_codegen_bench::rescore`]) and computes the summary from those verdicts,
//! never from the recorded booleans. The task set's content digests are verified
//! first, so a silently-edited benchmark is rejected before any scoring.
//!
//! Both reports come from the one summary: the machine format emits it as a
//! protocol item; human mode prints the reviewer table. The command never
//! advertises live generation — it has none to offer.

use std::path::Path;
use std::process::ExitCode;

use serde_json::{Value, json};
use tuo_codegen_bench::{BenchmarkRun, BenchmarkSummary, TaskSet, rescore};
use tuo_spec::Limits;

use crate::output::OutputMode;
use crate::protocol::{Event, ProtocolCommand, Status};

/// `tuo bench report <tasks> <run>`: re-score `run`'s recorded outputs against
/// the pinned `tasks` and report the metrics.
pub(crate) fn report(tasks_path: &Path, run_path: &Path, mode: OutputMode) -> ExitCode {
    // Load and digest-verify the task set. A stale pin means the benchmark was
    // changed without re-pinning — a silent change, which we refuse.
    let task_set = match load_task_set(tasks_path) {
        Ok(set) => set,
        Err(message) => return fail(mode, &message),
    };
    let tasks = match task_set.tasks() {
        Ok(tasks) => tasks,
        Err(mismatch) => {
            return fail(
                mode,
                &format!(
                    "task `{}` was changed without updating its digest pin \
                     (recorded {}, actual {}) — the benchmark changed silently",
                    mismatch.task_id, mismatch.recorded, mismatch.actual
                ),
            );
        }
    };

    // Load the recorded run (the model's outputs and provenance).
    let run = match load_run(run_path) {
        Ok(run) => run,
        Err(message) => return fail(mode, &message),
    };

    // Re-score by really compiling every recorded output, then summarize.
    let rescored = rescore(&run, &tasks, Limits::default());
    let verified = BenchmarkRun::new(run.model.clone(), run.task_set_digest.clone(), rescored);
    let summary = BenchmarkSummary::from_run(&verified);

    if mode.is_machine() {
        emit(mode, &verified, &summary);
    } else {
        report_human(&verified, &summary);
    }
    ExitCode::SUCCESS
}

/// Load a pinned task set from disk.
fn load_task_set(path: &Path) -> Result<TaskSet, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("could not read {}: {e}", display(path)))?;
    TaskSet::from_json(&text).map_err(|e| format!("invalid task set {}: {e}", display(path)))
}

/// Load a recorded benchmark run from disk.
fn load_run(path: &Path) -> Result<BenchmarkRun, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("could not read {}: {e}", display(path)))?;
    BenchmarkRun::from_json(&text)
        .map_err(|e| format!("invalid benchmark run {}: {e}", display(path)))
}

/// Emit the summary (and provenance) as a single protocol item.
fn emit(mode: OutputMode, run: &BenchmarkRun, summary: &BenchmarkSummary) {
    let payload = json!({
        "kind": "bench_summary",
        "model": run.model.id,
        "compiler_version": run.compiler_version,
        "language_version": run.language_version,
        "task_set_digest": run.task_set_digest,
        "summary": serde_json::to_value(summary).unwrap_or(Value::Null),
    });
    let Some(mut emitter) = mode.emitter(ProtocolCommand::Bench) else {
        return;
    };
    let write = (|| -> std::io::Result<()> {
        emitter.emit(&Event::started(&[] as &[String]))?;
        emitter.emit(&Event::item(Status::Ok, payload))?;
        emitter.emit(&Event::finished(Status::Ok, json!({})))?;
        emitter.finish()
    })();
    if write.is_err() {
        mode.log("protocol: stdout write failed");
    }
}

/// Print the human-readable report to stdout.
#[expect(
    clippy::print_stdout,
    reason = "the CLI presentation layer prints the human benchmark report to stdout"
)]
fn report_human(run: &BenchmarkRun, summary: &BenchmarkSummary) {
    print!("{}", tuo_codegen_bench::render_human(run, summary));
}

/// Report a usage/IO/validation failure through the active mode.
#[expect(
    clippy::print_stderr,
    reason = "the CLI presentation layer reports errors on stderr in human mode"
)]
fn fail(mode: OutputMode, message: &str) -> ExitCode {
    if mode.is_machine() {
        let Some(mut emitter) = mode.emitter(ProtocolCommand::Bench) else {
            return ExitCode::FAILURE;
        };
        let write = (|| -> std::io::Result<()> {
            emitter.emit(&Event::started(&[] as &[String]))?;
            emitter.emit(&Event::finished(Status::Error, json!({ "error": message })))?;
            emitter.finish()
        })();
        if write.is_err() {
            mode.log("protocol: stdout write failed");
        }
    } else {
        eprintln!("error: {message}");
    }
    ExitCode::FAILURE
}

/// A file path as a display string.
fn display(path: &Path) -> String {
    path.display().to_string()
}
