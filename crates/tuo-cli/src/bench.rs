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
//! protocol item; human mode prints the reviewer table.
//!
//! # `tuo bench run`: live generation through an OpenAI-compatible endpoint
//!
//! The harness still embeds no model. What this command adds is the one
//! [`ModelAdapter`] the CLI can honestly provide: an HTTP chat-completions
//! client ([`HttpChatModel`]) for any server speaking the OpenAI-compatible
//! `/v1/chat/completions` shape — vLLM, Ollama, LM Studio, a hosted provider
//! behind a bearer token. The request goes through the system `curl` (the same
//! kind of host-tool seam linking uses for `cc`), so the workspace gains no HTTP
//! or TLS dependency and the endpoint may be local or remote.
//!
//! The adapter is deliberately plain: a fixed system prompt asking for
//! tuonelang source and nothing else, the harness's rendered prompt as the user
//! turn, temperature `0` and a fixed seed by default so a model that *can* be
//! deterministic is. With `--prime` the system prompt additionally carries the
//! generated language brief (`tuo cheatsheet`, ADR-0018), which is the
//! experiment that brief was written for: the same model, the same tasks, with
//! and without the compiler-generated context. Everything the model was shown
//! is recorded — the system prompt's SHA-256 and whether it was primed live in
//! the run's [`ModelConfig`] — and the written run file is exactly what
//! `tuo bench report` rescores, so a live run is never trusted on its own say-so
//! either.

use std::io::Write;
use std::path::Path;
use std::process::{Command, ExitCode, Stdio};

use serde_json::{Value, json};
use tuo_codegen_bench::{
    BenchmarkRun, BenchmarkSummary, ConfigEntry, Generation, GenerationError, ModelAdapter,
    ModelConfig, Prompt, RunConfig, TaskSet, rescore, run_task,
};
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

/// Everything `tuo bench run` needs to reach a model.
pub(crate) struct LiveOptions<'a> {
    /// The pinned task-set file.
    pub tasks: &'a Path,
    /// Where to write the recorded run.
    pub output: &'a Path,
    /// The chat-completions endpoint base, e.g. `http://localhost:11434/v1`.
    pub endpoint: &'a str,
    /// The served model name, as the endpoint's `/v1/models` reports it.
    pub model: &'a str,
    /// Repair turns after the initial generation.
    pub max_repairs: usize,
    /// Sampling temperature, recorded verbatim.
    pub temperature: f64,
    /// Prime the system prompt with the generated language brief.
    pub prime: bool,
    /// The environment variable holding a bearer token, if the endpoint needs one.
    pub api_key_env: Option<&'a str>,
    /// Per-request timeout, in seconds.
    pub timeout_secs: u64,
}

/// `tuo bench run`: generate fresh outputs for every pinned task through a live
/// endpoint, record the run, and report it exactly as `report` would.
pub(crate) fn run_live(options: &LiveOptions<'_>, mode: OutputMode) -> ExitCode {
    let task_set = match load_task_set(options.tasks) {
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
    let system = match system_prompt(options.prime) {
        Ok(system) => system,
        Err(message) => return fail(mode, &message),
    };
    let api_key = match options.api_key_env {
        Some(var) => match std::env::var(var) {
            Ok(key) => Some(key),
            Err(_) => return fail(mode, &format!("environment variable `{var}` is not set")),
        },
        None => None,
    };
    let model = HttpChatModel {
        endpoint: options.endpoint.trim_end_matches('/').to_owned(),
        model: options.model.to_owned(),
        temperature: options.temperature,
        system,
        primed: options.prime,
        api_key,
        timeout_secs: options.timeout_secs,
    };
    let config = RunConfig {
        max_repairs: options.max_repairs,
        limits: Limits::default(),
    };

    // Every task, in its default spelling and in each declared variant, so a
    // syntax-variant question gets its data from the same model and session.
    let mut runs = Vec::new();
    for task in &tasks {
        mode.log(&format!("bench: task `{}`", task.id));
        runs.push(run_task(&model, task, None, config));
        for variant in &task.variants {
            mode.log(&format!(
                "bench: task `{}` variant `{}`",
                task.id, variant.label
            ));
            runs.push(run_task(&model, task, Some(variant), config));
        }
    }
    let run = BenchmarkRun::new(model.config(), task_set_digest(&task_set), runs);
    let text = match run.to_json_pretty() {
        Ok(text) => text,
        Err(error) => return fail(mode, &format!("could not serialize the run: {error}")),
    };
    if let Some(parent) = options.output.parent() {
        if !parent.as_os_str().is_empty() && std::fs::create_dir_all(parent).is_err() {
            return fail(mode, &format!("could not create {}", display(parent)));
        }
    }
    if let Err(error) = std::fs::write(options.output, text) {
        return fail(
            mode,
            &format!("could not write {}: {error}", display(options.output)),
        );
    }

    let summary = BenchmarkSummary::from_run(&run);
    if mode.is_machine() {
        emit(mode, &run, &summary);
    } else {
        report_human(&run, &summary);
    }
    ExitCode::SUCCESS
}

/// The digest that labels a whole task set in a run: the pinned per-task
/// digests joined in order, so any task change changes it.
fn task_set_digest(set: &TaskSet) -> String {
    set.tasks
        .iter()
        .map(|pinned| pinned.digest.as_str())
        .collect::<Vec<_>>()
        .join("+")
}

/// The fixed instruction every turn opens with. Short and literal: the point
/// of the benchmark is what the model does with the *task*, not with a prompt
/// tuned per model.
const SYSTEM_PROMPT: &str = "You write tuonelang, a statically typed language with colocated \
executable specs. Reply with the complete tuonelang source for the task and nothing else: no \
prose, no explanation. If you use a code fence, use ```tuo. When compiler feedback is given, \
return the whole corrected program.";

/// The system prompt, optionally primed with the generated language brief.
fn system_prompt(prime: bool) -> Result<String, String> {
    if !prime {
        return Ok(SYSTEM_PROMPT.to_owned());
    }
    let brief = crate::cheatsheet::render()?;
    Ok(format!(
        "{SYSTEM_PROMPT}\n\nThe following is the tuonelang language brief, generated from the \
         compiler. It is the authority on syntax and on which standard-library functions exist.\n\n\
         {brief}"
    ))
}

/// A [`ModelAdapter`] over an OpenAI-compatible chat-completions endpoint.
struct HttpChatModel {
    endpoint: String,
    model: String,
    temperature: f64,
    system: String,
    primed: bool,
    api_key: Option<String>,
    timeout_secs: u64,
}

/// The fixed sampling seed sent with every request, so a server that honors it
/// reproduces a run.
const SEED: u64 = 7;

impl ModelAdapter for HttpChatModel {
    fn config(&self) -> ModelConfig {
        let entry = |key: &str, value: String| ConfigEntry {
            key: key.to_owned(),
            value,
        };
        ModelConfig {
            id: format!(
                "{}@t{}{}",
                self.model,
                self.temperature,
                if self.primed { "+primed" } else { "" }
            ),
            provider: Some(self.endpoint.clone()),
            temperature: Some(self.temperature.to_string()),
            extra: vec![
                entry("seed", SEED.to_string()),
                entry("primed", self.primed.to_string()),
                entry(
                    "system_prompt_sha256",
                    tuo_package::sha256::hex(self.system.as_bytes()),
                ),
                entry("transport", "curl".to_owned()),
            ],
        }
    }

    fn generate(&self, prompt: &Prompt<'_>) -> Result<Generation, GenerationError> {
        let body = json!({
            "model": self.model,
            "temperature": self.temperature,
            "seed": SEED,
            "messages": [
                { "role": "system", "content": self.system },
                { "role": "user", "content": render_user_turn(prompt) },
            ],
        });
        let response = self.post(&body)?;
        let content = response["choices"][0]["message"]["content"]
            .as_str()
            .ok_or_else(|| {
                GenerationError::new(match response["error"]["message"].as_str() {
                    Some(message) => format!("endpoint error: {message}"),
                    None => format!("no `choices[0].message.content` in response: {response}"),
                })
            })?;
        let tokens = response["usage"]["completion_tokens"].as_u64().unwrap_or(0);
        Ok(Generation::new(extract_source(content), tokens))
    }
}

impl HttpChatModel {
    /// POST `body` to the chat-completions route and parse the JSON reply.
    fn post(&self, body: &Value) -> Result<Value, GenerationError> {
        let url = format!("{}/chat/completions", self.endpoint);
        let mut command = Command::new("curl");
        command
            .arg("--silent")
            .arg("--show-error")
            .arg("--max-time")
            .arg(self.timeout_secs.to_string())
            .arg("-H")
            .arg("Content-Type: application/json")
            .arg("--data-binary")
            .arg("@-")
            .arg(&url)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(key) = &self.api_key {
            command
                .arg("-H")
                .arg(format!("Authorization: Bearer {key}"));
        }
        let mut child = command
            .spawn()
            .map_err(|error| GenerationError::new(format!("cannot run `curl`: {error}")))?;
        let payload = serde_json::to_vec(body)
            .map_err(|error| GenerationError::new(format!("request encoding: {error}")))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(&payload)
                .map_err(|error| GenerationError::new(format!("curl stdin: {error}")))?;
        }
        let output = child
            .wait_with_output()
            .map_err(|error| GenerationError::new(format!("curl: {error}")))?;
        if !output.status.success() {
            return Err(GenerationError::new(format!(
                "curl failed ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        serde_json::from_slice(&output.stdout).map_err(|error| {
            GenerationError::new(format!(
                "endpoint returned non-JSON ({error}): {}",
                String::from_utf8_lossy(&output.stdout)
                    .chars()
                    .take(200)
                    .collect::<String>()
            ))
        })
    }
}

/// The user turn: the harness's prompt, in the same layout `render_prompt`
/// records, so the run file shows what the model saw.
fn render_user_turn(prompt: &Prompt<'_>) -> String {
    let mut out = String::from(prompt.instruction);
    if let Some(variant) = prompt.variant {
        out.push_str(&format!("\n\nWrite it in this style: {variant}."));
    }
    if let Some(previous) = prompt.previous_source {
        out.push_str("\n\nYour previous attempt:\n```tuo\n");
        out.push_str(previous);
        if !previous.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("```");
    }
    if !prompt.feedback.is_empty() {
        out.push_str("\n\nThe compiler reported:\n");
        out.push_str(&prompt.feedback.join("\n"));
        out.push_str("\n\nReturn the whole corrected program.");
    }
    out
}

/// The tuonelang source in a model reply: the first fenced block if there is
/// one (any info string), else the whole reply with any `<think>…</think>`
/// reasoning removed. The harness compiles whatever this returns, so a model
/// that wraps its code in prose simply scores as not parsing.
fn extract_source(content: &str) -> String {
    let content = strip_think(content);
    let mut lines = content.lines();
    let mut out: Option<String> = None;
    for line in lines.by_ref() {
        if line.trim_start().starts_with("```") {
            out = Some(String::new());
            break;
        }
    }
    let Some(mut source) = out else {
        return content.trim().to_owned() + "\n";
    };
    for line in lines {
        if line.trim_start().starts_with("```") {
            break;
        }
        source.push_str(line);
        source.push('\n');
    }
    source
}

/// Remove `<think>…</think>` blocks a reasoning model may leave inline.
fn strip_think(content: &str) -> String {
    let mut rest = content;
    let mut out = String::new();
    while let Some(start) = rest.find("<think>") {
        out.push_str(&rest[..start]);
        match rest[start..].find("</think>") {
            Some(end) => rest = &rest[start + end + "</think>".len()..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
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

#[cfg(test)]
mod tests {
    use super::{extract_source, strip_think};

    #[test]
    fn a_fenced_block_is_extracted_whatever_its_info_string() {
        let reply = "Here you go:\n```rust\nfn a() -> Int {\n    1\n}\n```\nDone.";
        assert_eq!(extract_source(reply), "fn a() -> Int {\n    1\n}\n");
    }

    #[test]
    fn an_unfenced_reply_is_taken_whole_minus_reasoning() {
        let reply = "<think>maybe x + x</think>fn a() -> Int {\n    1\n}";
        assert_eq!(extract_source(reply), "fn a() -> Int {\n    1\n}\n");
    }

    #[test]
    fn an_unterminated_think_block_yields_nothing_after_it() {
        assert_eq!(strip_think("code <think>never closed"), "code ");
    }
}
