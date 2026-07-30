//! The versioned wire protocol: request and response envelopes.
//!
//! The agent protocol is a **contract**, exactly like the CLI's
//! [machine protocol](../../tuo_cli/protocol/index.html): a coding agent drives
//! the compiler by parsing these shapes, so they are versioned by
//! [`PROTOCOL_VERSION`] and only ever change in a backwards-compatible way (new
//! optional fields, new method or response variants a consumer must tolerate)
//! until the version is bumped.
//!
//! # One request, one response, correlated by id
//!
//! The transport is JSON-lines over a duplex stream: the client writes one
//! [`Request`] object per line and reads one [`Response`] object per line. Every
//! request carries a client-chosen [`Request::id`]; the response to it echoes
//! that id, so a client may pipeline requests and match replies out of order.
//!
//! ```text
//! → {"protocol_version":1,"id":1,"method":"initialize","params":{}}
//! ← {"protocol_version":1,"id":1,"ok":true,"result":{ … }}
//! ```
//!
//! # Determinism
//!
//! Where the underlying compiler operation is deterministic the response is
//! byte-for-byte deterministic: the same request against the same open-document
//! state yields the same `result`. The one field that is *not* promised
//! deterministic is a measured duration (spec timing) — it is reported as an
//! observation, never a guarantee, and lives in its own clearly-named field.
//!
//! # This is a compiler-intelligence protocol
//!
//! No field here names, embeds, or assumes any particular LLM provider or
//! model. The protocol answers *compiler* questions — diagnostics, types,
//! definitions, references, specs — for whatever agent is on the other end.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The version of the agent wire protocol. Bumped only on a
/// backwards-incompatible change to the request/response layout documented in
/// this module. Additive changes (new optional fields, new method or result
/// variants) do **not** bump it — a consumer must ignore unknown fields and
/// tolerate unknown variants.
pub const PROTOCOL_VERSION: u32 = 1;

/// A client request: a correlation id, a method name, and the method's
/// parameters.
///
/// `params` is an untyped [`Value`] so the envelope parses even for a method
/// this build does not know; the dispatcher validates the shape per method and
/// answers an unknown method with a structured error rather than a parse
/// failure.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Request {
    /// The protocol version the client speaks. Optional on the wire (a client
    /// may omit it); when present it lets a server detect a version skew.
    #[serde(default)]
    pub protocol_version: Option<u32>,
    /// The client-chosen correlation id, echoed in the matching [`Response`].
    pub id: u64,
    /// The method name (`initialize`, `check`, `type_at`, …).
    pub method: String,
    /// The method parameters. Absent parameters default to `null`.
    #[serde(default)]
    pub params: Value,
}

/// A server response: the request's id, whether it succeeded, and the payload.
///
/// Exactly one of `result` (on success) or `error` (on failure) is populated,
/// discriminated by `ok`. Both variants echo the request `id` and carry the
/// [`PROTOCOL_VERSION`] so a line is self-describing out of context.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Response {
    /// This module's [`PROTOCOL_VERSION`].
    pub protocol_version: u32,
    /// The id of the request this answers.
    pub id: u64,
    /// `true` for a `result`, `false` for an `error`.
    pub ok: bool,
    /// The success payload (present iff `ok`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// The failure payload (present iff not `ok`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ResponseError>,
}

impl Response {
    /// A success response echoing `id` and carrying `result`.
    #[must_use]
    pub fn ok(id: u64, result: Value) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            id,
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    /// A failure response echoing `id` and carrying a structured error.
    #[must_use]
    pub fn err(id: u64, error: ResponseError) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            id,
            ok: false,
            result: None,
            error: Some(error),
        }
    }
}

/// A structured error: a stable machine `code`, a human `message`, and optional
/// structured `data`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ResponseError {
    /// A stable error code a client dispatches on (never changes once shipped).
    pub code: ErrorCode,
    /// A human-readable explanation (may change freely; not a contract).
    pub message: String,
    /// Optional structured detail (e.g. the offending method name).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl ResponseError {
    /// An error with a code and message and no extra data.
    #[must_use]
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    /// Attach structured data.
    #[must_use]
    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }
}

/// A stable error code. Serialized as a lowercase string a client dispatches
/// on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The request line was not valid JSON, or not a well-formed [`Request`].
    ParseError,
    /// The `method` is not one this server implements.
    UnknownMethod,
    /// The `params` were the wrong shape for the method.
    InvalidParams,
    /// The method named a document that has not been opened.
    UnknownDocument,
    /// The requested operation is not applicable (e.g. `type_at` on a position
    /// resolving to nothing, or `run_spec` on a program with front-end errors).
    Unavailable,
    /// The compiler query was cancelled (cooperative cancellation).
    Cancelled,
    /// An internal error the server could not attribute to the request.
    Internal,
}
