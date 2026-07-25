//! The stable machine protocol for the `tuo` CLI.
//!
//! Human output (`--message-format=human`) is a courtesy for people at a
//! terminal and may change freely. The two machine formats are a **contract**:
//! coding agents, editors, and CI drive the compiler by parsing them, so their
//! shape is versioned by [`PROTOCOL_VERSION`] and only ever changes in a
//! backwards-compatible way (new optional fields, new event or status variants
//! consumers must tolerate) until the version is bumped.
//!
//! # Two machine encodings of one event stream
//!
//! Every command produces the same ordered stream of [`Event`]s. The encoding
//! chosen by `--message-format` decides only how that stream reaches stdout:
//!
//! - **`json`** — the whole stream is buffered and emitted as one
//!   envelope object: `{ protocol_version, command, events: [...] }`.
//!   A consumer reads stdout to EOF and parses it once. Best for a batch tool
//!   that wants the final result.
//! - **`json-lines`** — each [`Event`] is emitted as its own single-line JSON
//!   object the instant it happens, so a consumer sees a long-running
//!   operation progress in real time. Each line additionally carries
//!   `protocol_version` and `command` so a line is self-describing out of
//!   context.
//!
//! # Message shape
//!
//! Every machine message (the `json` envelope and every `json-lines` line)
//! carries the fields the protocol guarantees:
//!
//! - `protocol_version` — this module's [`PROTOCOL_VERSION`];
//! - `event` — the event-kind tag (`started`, `diagnostic`, `item`,
//!   `progress`, `finished`; a per-result `item` names its shape in a `kind`
//!   payload field — `spec_result`, `fmt_file`, `spec_skipped`);
//! - `command` — which `tuo` subcommand produced it (`check`, `spec`, …);
//! - `status` — the [`Status`] of the run or of this event;
//! - stable diagnostics — serialized with the versioned
//!   [`tuo_diagnostics::json`] schema (its own `diagnostic_schema_version`
//!   travels alongside, so the two contracts version independently);
//! - relevant IDs — e.g. a spec's stable symbol id, a source file id;
//! - source ranges — every span is the versioned diagnostic span object
//!   (byte offsets plus derived line/column).
//!
//! # Streams
//!
//! In a machine format **stdout carries protocol output only** — nothing else
//! is ever written there, so a consumer can parse stdout wholesale. Internal
//! logging goes to **stderr and only when explicitly enabled** (`--log`); by
//! default stderr is silent in machine mode. In human mode stdout/stderr keep
//! their conventional roles (see each command).

use std::io::{self, Write};

use serde_json::{Value, json};
use tuo_compiler::diagnostics::{self, Diagnostic};
use tuo_compiler::source::{SourceMap, Span};

use crate::output::OutputMode;

/// The version of the CLI machine protocol. Bumped only on a
/// backwards-incompatible change to the envelope, event, or message layout
/// documented in this module. Additive changes (new optional fields, new
/// event/status variants) do **not** bump it — machine consumers must ignore
/// unknown fields and tolerate unknown variants.
pub(crate) const PROTOCOL_VERSION: u32 = 1;

/// Which subcommand a machine message belongs to. Stable strings: a consumer
/// dispatches on them, so they never change once shipped.
#[derive(Clone, Copy, Debug)]
pub(crate) enum ProtocolCommand {
    /// `tuo check`.
    Check,
    /// `tuo spec`.
    Spec,
    /// `tuo verify`.
    Verify,
    /// `tuo fmt` (and `fmt --check`).
    Fmt,
}

impl ProtocolCommand {
    /// The stable wire name.
    const fn name(self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::Spec => "spec",
            Self::Verify => "verify",
            Self::Fmt => "fmt",
        }
    }
}

/// The overall status of a run, or the status an individual event reports.
///
/// `running` only appears on mid-stream events (`started`, `progress`); a
/// `finished` event is always `ok` or `error`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    /// The operation succeeded (no errors, no failing specs).
    Ok,
    /// The operation failed (a front-end error, a failing/ trapping spec, a
    /// non-canonical file under `--check`, or an I/O failure).
    Error,
    /// The operation is still in progress (mid-stream only).
    Running,
}

impl Status {
    /// The stable wire name.
    const fn name(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "error",
            Self::Running => "running",
        }
    }
}

/// The kind tag carried by every event's `event` field.
#[derive(Clone, Copy, Debug)]
enum EventKind {
    Started,
    Diagnostic,
    Item,
    Progress,
    Finished,
}

impl EventKind {
    const fn name(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Diagnostic => "diagnostic",
            // A per-result item (one executed spec, one formatted file, …).
            // Its own `kind` payload field says which — `spec_result`,
            // `fmt_file`, `spec_skipped`.
            Self::Item => "item",
            Self::Progress => "progress",
            Self::Finished => "finished",
        }
    }
}

/// One protocol event: the semantic unit both machine formats encode.
///
/// An event owns its already-serialized payload [`Value`] (built against a
/// [`SourceMap`] so spans are fully resolved) and the metadata every message
/// carries. The command and protocol version are attached by the [`Emitter`]
/// at write time, not stored here.
pub(crate) struct Event {
    kind: EventKind,
    status: Status,
    /// The event-specific fields, merged into the message object.
    payload: Value,
}

impl Event {
    /// The `started` event: the run has begun. Its payload names the inputs.
    pub(crate) fn started(files: &[String]) -> Self {
        Self {
            kind: EventKind::Started,
            status: Status::Running,
            payload: json!({ "files": files }),
        }
    }

    /// A single diagnostic, serialized with the versioned diagnostic schema.
    /// Its `id` is the stable diagnostic code (e.g. `T0301`); its span travels
    /// as the versioned span object inside `diagnostic`.
    pub(crate) fn diagnostic(diagnostic: &Diagnostic, map: &SourceMap) -> Self {
        let status = match diagnostic.severity {
            diagnostics::Severity::Error => Status::Error,
            diagnostics::Severity::Warning | diagnostics::Severity::Note => Status::Ok,
        };
        Self {
            kind: EventKind::Diagnostic,
            status,
            payload: json!({
                "id": diagnostic.code.to_string(),
                "diagnostic_schema_version": diagnostics::json::SCHEMA_VERSION,
                "diagnostic": diagnostics::json::diagnostic_to_json(diagnostic, map),
            }),
        }
    }

    /// A per-result `item` event with an arbitrary payload object (e.g. a spec
    /// result or a per-file `fmt` outcome). The payload's own `kind` field
    /// distinguishes the item shape. The payload must be a JSON object.
    pub(crate) fn item(status: Status, payload: Value) -> Self {
        debug_assert!(payload.is_object(), "event payloads are objects");
        Self {
            kind: EventKind::Item,
            status,
            payload,
        }
    }

    /// A free-form progress event for a long-running operation: a stable
    /// `stage` name and an optional human `message`. Carries no result, only
    /// forward motion a `json-lines` consumer can display.
    pub(crate) fn progress(stage: &str, message: impl Into<String>) -> Self {
        Self {
            kind: EventKind::Progress,
            status: Status::Running,
            payload: json!({ "stage": stage, "message": message.into() }),
        }
    }

    /// The terminal `finished` event: the run's final status and an optional
    /// summary object. Always the last event of a stream.
    pub(crate) fn finished(status: Status, summary: Value) -> Self {
        Self {
            kind: EventKind::Finished,
            status,
            payload: json!({ "summary": summary }),
        }
    }

    /// This event as a wire object, tagged with `command` and
    /// `protocol_version` (present on every line in `json-lines`; the `json`
    /// envelope hoists the two shared fields to the top and omits them
    /// per-event).
    fn to_message(&self, command: ProtocolCommand, tagged: bool) -> Value {
        let mut object = serde_json::Map::new();
        if tagged {
            object.insert("protocol_version".into(), json!(PROTOCOL_VERSION));
            object.insert("command".into(), json!(command.name()));
        }
        object.insert("event".into(), json!(self.kind.name()));
        object.insert("status".into(), json!(self.status.name()));
        if let Value::Object(fields) = &self.payload {
            for (key, value) in fields {
                object.insert(key.clone(), value.clone());
            }
        }
        Value::Object(object)
    }
}

/// A machine encoding of the event stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MachineFormat {
    /// Buffer the whole stream and emit one envelope object.
    Json,
    /// Emit each event as its own single-line JSON object, immediately.
    JsonLines,
}

/// Writes the event stream in one of the machine formats.
///
/// `json-lines` writes and flushes each event as it arrives (streaming);
/// `json` accumulates events and writes the single envelope on [`finish`].
/// Either way, **only** protocol output reaches the wrapped writer.
///
/// [`finish`]: Emitter::finish
pub(crate) struct Emitter<W: Write> {
    writer: W,
    format: MachineFormat,
    command: ProtocolCommand,
    /// Buffered events for `json`; unused (empty) for `json-lines`.
    buffered: Vec<Value>,
}

impl<W: Write> Emitter<W> {
    /// A new emitter writing `command`'s events to `writer` in `format`.
    pub(crate) fn new(writer: W, format: MachineFormat, command: ProtocolCommand) -> Self {
        Self {
            writer,
            format,
            command,
            buffered: Vec::new(),
        }
    }

    /// Emit one event. In `json-lines` this writes and flushes a line now; in
    /// `json` it is buffered until [`finish`](Self::finish).
    pub(crate) fn emit(&mut self, event: &Event) -> io::Result<()> {
        match self.format {
            MachineFormat::JsonLines => {
                let line = event.to_message(self.command, true);
                writeln!(self.writer, "{line}")?;
                // Flush per line so a streaming consumer sees progress live.
                self.writer.flush()
            }
            MachineFormat::Json => {
                self.buffered.push(event.to_message(self.command, false));
                Ok(())
            }
        }
    }

    /// Finish the stream. In `json` this writes the single envelope object; in
    /// `json-lines` the events were already written, so this only flushes.
    /// Call exactly once, after the terminal `finished` event; the buffered
    /// events are drained so a second call is a harmless flush.
    pub(crate) fn finish(&mut self) -> io::Result<()> {
        if let MachineFormat::Json = self.format {
            let envelope = json!({
                "protocol_version": PROTOCOL_VERSION,
                "command": self.command.name(),
                "events": std::mem::take(&mut self.buffered),
            });
            writeln!(self.writer, "{envelope}")?;
        }
        self.writer.flush()
    }
}

/// The versioned span object for `span`, resolved against `map` — the same
/// shape the diagnostic schema uses, so every source range in the protocol has
/// one representation. Re-exported here so payload builders in the command
/// modules can attach ranges without reaching into `tuo-diagnostics`.
pub(crate) fn range(span: Span, map: &SourceMap) -> Value {
    diagnostics::json::span_to_json(span, map)
}

/// Emit a complete, minimal error stream for an I/O failure (an unreadable
/// input file) that stops a command before it can run. The stream is a
/// `started` event followed by a `finished`/`error` event whose summary
/// carries the failing `path` and `error` message — so a machine consumer sees
/// a well-formed, terminated stream even for a pre-run failure.
///
/// Best-effort: a failed write is logged (under `--log`) and otherwise
/// ignored, since the command is failing regardless.
pub(crate) fn io_error(command: ProtocolCommand, mode: OutputMode, path: &str, error: &str) {
    let Some(mut emitter) = mode.emitter(command) else {
        return;
    };
    let summary = json!({ "io_error": { "path": path, "message": error } });
    let write = (|| -> io::Result<()> {
        emitter.emit(&Event::started(std::slice::from_ref(&path.to_owned())))?;
        emitter.emit(&Event::finished(Status::Error, summary))?;
        emitter.finish()
    })();
    if write.is_err() {
        mode.log("protocol: stdout write failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run a small stream (started → progress → finished) into an in-memory
    /// writer and return the raw bytes.
    fn drive(format: MachineFormat) -> String {
        let mut buffer: Vec<u8> = Vec::new();
        {
            let mut emitter = Emitter::new(&mut buffer, format, ProtocolCommand::Check);
            emitter
                .emit(&Event::started(&["a.tuo".to_owned()]))
                .unwrap();
            emitter.emit(&Event::progress("stage", "note")).unwrap();
            emitter
                .emit(&Event::finished(Status::Ok, json!({ "ok": true })))
                .unwrap();
            emitter.finish().unwrap();
        }
        String::from_utf8(buffer).expect("utf-8")
    }

    #[test]
    fn json_buffers_the_whole_stream_into_one_envelope() {
        let text = drive(MachineFormat::Json);
        // Exactly one line: the envelope.
        assert_eq!(text.lines().count(), 1, "json is a single object");
        let value: Value = serde_json::from_str(text.trim()).expect("valid JSON");
        assert_eq!(value["protocol_version"], PROTOCOL_VERSION);
        assert_eq!(value["command"], "check");
        let events = value["events"].as_array().expect("events array");
        assert_eq!(events.len(), 3);
        // The shared header is hoisted to the envelope, not repeated per event.
        assert!(events[0].get("protocol_version").is_none());
        assert_eq!(events[0]["event"], "started");
        assert_eq!(events[2]["event"], "finished");
    }

    #[test]
    fn json_lines_emits_one_self_describing_object_per_event() {
        let text = drive(MachineFormat::JsonLines);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "one line per event");
        for line in &lines {
            let value: Value = serde_json::from_str(line).expect("each line is JSON");
            // Every line repeats the header so it is self-describing.
            assert_eq!(value["protocol_version"], PROTOCOL_VERSION);
            assert_eq!(value["command"], "check");
            assert!(value["event"].is_string());
            assert!(value["status"].is_string());
        }
    }

    #[test]
    fn the_two_formats_encode_the_same_event_sequence() {
        let json: Value = serde_json::from_str(drive(MachineFormat::Json).trim()).unwrap();
        let json_events: Vec<&str> = json["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|event| event["event"].as_str().unwrap())
            .collect();
        let line_events: Vec<String> = drive(MachineFormat::JsonLines)
            .lines()
            .map(|line| {
                serde_json::from_str::<Value>(line).unwrap()["event"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(json_events, line_events, "same event stream, two encodings");
    }
}
