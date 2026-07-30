//! The request dispatcher: one [`Request`] in, one [`Response`] out.
//!
//! [`Server`] owns a [`Session`] and routes each method to it. It is transport
//! agnostic — it neither reads nor writes a byte stream. A transport (the `tuo`
//! CLI's `agent --stdio`) reads a JSON-lines request, hands the parsed
//! [`Request`] to [`Server::handle`], and writes the returned [`Response`] as a
//! line. Keeping the transport out of this crate keeps the protocol logic pure
//! and directly testable.
//!
//! Every response is deterministic where the underlying compiler operation is:
//! the same request against the same open-document state produces the same
//! `result`.

use serde::Deserialize;
use serde_json::Value;

use crate::convert::{Position, Range};
use crate::protocol::{ErrorCode, PROTOCOL_VERSION, Request, Response, ResponseError};
use crate::session::{Formatter, Session};

/// A dispatcher over a long-lived [`Session`].
pub struct Server {
    session: Session,
}

impl Server {
    /// A new server with the given canonical formatter (see [`Formatter`]).
    #[must_use]
    pub fn new(formatter: Box<dyn Formatter>) -> Self {
        Self {
            session: Session::new(formatter),
        }
    }

    /// Handle one request, producing its response. Never panics: a malformed
    /// request or an inapplicable operation becomes a structured error response
    /// echoing the request id.
    #[must_use]
    pub fn handle(&mut self, request: Request) -> Response {
        let id = request.id;
        match self.dispatch(&request) {
            Ok(result) => Response::ok(id, result),
            Err(error) => Response::err(id, error),
        }
    }

    /// Parse a raw request line and handle it. A line that is not a well-formed
    /// [`Request`] yields a [`ErrorCode::ParseError`] response with id `0` (the
    /// id could not be recovered).
    #[must_use]
    pub fn handle_line(&mut self, line: &str) -> Response {
        match serde_json::from_str::<Request>(line) {
            Ok(request) => self.handle(request),
            Err(error) => Response::err(
                0,
                ResponseError::new(ErrorCode::ParseError, format!("invalid request: {error}")),
            ),
        }
    }

    /// Route one request to the session.
    fn dispatch(&mut self, request: &Request) -> Result<Value, ResponseError> {
        match request.method.as_str() {
            "initialize" => Ok(self.initialize()),
            "open" | "did_open" | "did_change" | "set_document" => self.set_document(request),
            "check" => Ok(self.session.check()),
            "verify" => {
                let params = OptionalAffected::parse(&request.params)?;
                self.session.verify(params.affected_by.as_deref())
            }
            "format" => self.format(request),
            "diagnostics" => {
                let params = DocParams::parse(&request.params)?;
                self.session.diagnostics(&params.uri)
            }
            "type_at" => {
                let params = PositionParams::parse(&request.params)?;
                self.session.type_at(&params.uri, params.position)
            }
            "definition" => {
                let params = PositionParams::parse(&request.params)?;
                self.session.definition(&params.uri, params.position)
            }
            "references" => {
                let params = ReferenceParams::parse(&request.params)?;
                self.session
                    .references(&params.uri, params.position, params.include_declaration)
            }
            "symbols" => {
                let params = DocParams::parse(&request.params)?;
                self.session.symbols(&params.uri)
            }
            "signature" => {
                let params = PositionParams::parse(&request.params)?;
                self.session.signature(&params.uri, params.position)
            }
            "members" => {
                let params = PositionParams::parse(&request.params)?;
                self.session.members(&params.uri, params.position)
            }
            "available_imports" => Ok(self.session.available_imports()),
            "specs_for" => {
                let params = PositionParams::parse(&request.params)?;
                self.session.specs_for(&params.uri, params.position)
            }
            "run_spec" => {
                let params = RunSpecParams::parse(&request.params)?;
                self.session.run_spec(params.target.as_deref())
            }
            "apply_safe_fix" => {
                let params = FixParams::parse(&request.params)?;
                self.session.apply_safe_fix(&params.uri, params.range)
            }
            other => Err(ResponseError::new(
                ErrorCode::UnknownMethod,
                format!("unknown method `{other}`"),
            )
            .with_data(serde_json::json!({ "method": other }))),
        }
    }

    /// The `initialize` handshake: the server's protocol version, the methods it
    /// implements, and a self-description. Purely informational — it opens no
    /// document and is deterministic.
    fn initialize(&self) -> Value {
        serde_json::json!({
            "protocol_version": PROTOCOL_VERSION,
            "server": "tuo-agent",
            "description": "tuonelang compiler-intelligence protocol (not an AI model)",
            "methods": METHODS,
        })
    }

    /// Open or update a document from an `{uri, text}` params object.
    fn set_document(&mut self, request: &Request) -> Result<Value, ResponseError> {
        let params = DocumentParams::parse(&request.params)?;
        let file = self.session.set_document(&params.uri, &params.text);
        Ok(serde_json::json!({ "uri": params.uri, "file_id": file.as_raw() }))
    }

    /// `format`: either inline `{text}` or a document reference `{uri}`.
    fn format(&self, request: &Request) -> Result<Value, ResponseError> {
        let params = FormatParams::parse(&request.params)?;
        let text = match (params.text, params.uri) {
            (Some(text), _) => text,
            (None, Some(uri)) => self.session.document_text(&uri)?,
            (None, None) => {
                return Err(ResponseError::new(
                    ErrorCode::InvalidParams,
                    "format needs either `text` or `uri`",
                ));
            }
        };
        Ok(self.session.format(&text))
    }
}

/// The methods this server implements (for the `initialize` handshake).
const METHODS: &[&str] = &[
    "initialize",
    "set_document",
    "check",
    "verify",
    "format",
    "diagnostics",
    "type_at",
    "definition",
    "references",
    "symbols",
    "signature",
    "members",
    "available_imports",
    "specs_for",
    "run_spec",
    "apply_safe_fix",
];

// ----------------------------------------------------------------------
// Per-method parameter shapes. Each parses a `serde_json::Value` and maps a
// shape mismatch to an `InvalidParams` error.
// ----------------------------------------------------------------------

/// Parse `params` into `T`, mapping a deserialization failure to a structured
/// [`ErrorCode::InvalidParams`] error.
fn parse_params<T: for<'de> Deserialize<'de>>(params: &Value) -> Result<T, ResponseError> {
    serde_json::from_value(params.clone())
        .map_err(|error| ResponseError::new(ErrorCode::InvalidParams, error.to_string()))
}

/// `{uri, text}` — open/update a document.
#[derive(Deserialize)]
struct DocumentParams {
    uri: String,
    text: String,
}

impl DocumentParams {
    fn parse(params: &Value) -> Result<Self, ResponseError> {
        parse_params(params)
    }
}

/// `{uri}` — a whole-document query.
#[derive(Deserialize)]
struct DocParams {
    uri: String,
}

impl DocParams {
    fn parse(params: &Value) -> Result<Self, ResponseError> {
        parse_params(params)
    }
}

/// `{uri, position}` — a positional query.
#[derive(Deserialize)]
struct PositionParams {
    uri: String,
    position: Position,
}

impl PositionParams {
    fn parse(params: &Value) -> Result<Self, ResponseError> {
        parse_params(params)
    }
}

/// `{uri, position, include_declaration?}` — find references.
#[derive(Deserialize)]
struct ReferenceParams {
    uri: String,
    position: Position,
    #[serde(default = "default_true")]
    include_declaration: bool,
}

impl ReferenceParams {
    fn parse(params: &Value) -> Result<Self, ResponseError> {
        parse_params(params)
    }
}

/// `{uri, range?}` — collect safe fixes (whole file when `range` is omitted).
#[derive(Deserialize)]
struct FixParams {
    uri: String,
    #[serde(default)]
    range: Option<Range>,
}

impl FixParams {
    fn parse(params: &Value) -> Result<Self, ResponseError> {
        parse_params(params)
    }
}

/// `{target?}` — run the specs of a target (all when omitted).
#[derive(Deserialize)]
struct RunSpecParams {
    #[serde(default)]
    target: Option<String>,
}

impl RunSpecParams {
    fn parse(params: &Value) -> Result<Self, ResponseError> {
        if params.is_null() {
            return Ok(Self { target: None });
        }
        parse_params(params)
    }
}

/// `{affected_by?}` — verify, optionally restricted to an edited file.
#[derive(Deserialize)]
struct OptionalAffected {
    #[serde(default)]
    affected_by: Option<String>,
}

impl OptionalAffected {
    fn parse(params: &Value) -> Result<Self, ResponseError> {
        if params.is_null() {
            return Ok(Self { affected_by: None });
        }
        parse_params(params)
    }
}

/// `{text?, uri?}` — format inline text or an open document.
#[derive(Deserialize)]
struct FormatParams {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    uri: Option<String>,
}

impl FormatParams {
    fn parse(params: &Value) -> Result<Self, ResponseError> {
        parse_params(params)
    }
}

/// The default for `include_declaration`.
const fn default_true() -> bool {
    true
}
