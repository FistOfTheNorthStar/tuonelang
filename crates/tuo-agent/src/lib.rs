//! The native tuonelang agent protocol.
//!
//! `tuo-agent` exposes the compiler's intelligence to a coding agent over a
//! versioned, JSON-lines request/response protocol. It is the third consumer of
//! the shared semantic engine — alongside the CLI and the LSP — and, like them,
//! it **reimplements no compiler stage**: every answer is a read-only
//! projection of what [`tuo_compiler::IncrementalSession`] already computed, so
//! the CLI, the LSP, and the agent all drive the *same* compiler queries.
//!
//! # A compiler-intelligence protocol, not an AI model
//!
//! Nothing here names, embeds, or assumes any particular LLM provider or model.
//! The protocol answers *compiler* questions — diagnostics, types, definitions,
//! references, symbols, signatures, importable names, specs — for whatever
//! agent is on the other end. The agent decides *what* to ask; this crate is
//! the compiler's honest answer.
//!
//! # Shape
//!
//! Three layers, mirroring the LSP's:
//!
//! - [`protocol`] — the wire vocabulary: the versioned [`Request`] /
//!   [`Response`] envelopes and error codes, carrying no compiler types.
//! - [`convert`] — the one place wire positions (byte offset *or* one-based
//!   `line:column`) meet compiler byte offsets, both directions, clamping
//!   out-of-range input rather than failing.
//! - [`session`] — the [`Session`], a long-lived database over
//!   [`IncrementalSession`](tuo_compiler::IncrementalSession) that answers every
//!   method, and [`server`]'s [`Server`], the transport-agnostic dispatcher.
//! - [`generation`] — the compiler-guided *generation* queries
//!   ([`GenerationQueries`]), what an agent asks before writing the next token:
//!   the expected type, visible names, valid members, and — kept strictly
//!   separate and flagged non-exhaustive — a lexical read of syntactic context.
//!   Semantic answers project the shared [`Semantics`](tuo_compiler::Semantics);
//!   syntactic answers are conservative heuristics, never a claim that the
//!   compiler enumerates every valid next token.
//!
//! # Database reuse across requests
//!
//! A [`Server`] owns one [`Session`], which owns one incremental compiler
//! database, kept alive for the server's whole life. Opening or editing a
//! document feeds that shared session; its red-green engine recomputes only what
//! the edit affected. A subsequent request reads the memoized result — the
//! compiler is **not** restarted per edit, which is the whole point of a
//! long-lived agent server.
//!
//! # Determinism
//!
//! Every response is deterministic where the underlying compiler operation is:
//! the same request against the same open-document state produces the same
//! `result`. The only non-deterministic field is a *measured* duration (spec
//! timing), reported as an observation in its own clearly-named field, never as
//! a promise.
//!
//! # Transport
//!
//! The transport — `tuo agent --stdio`, a line reader/writer over the process's
//! stdio — lives in the `tuo` CLI, which also supplies the canonical formatter
//! (`tuo-fmt`, which sits at this crate's own dependency layer and so cannot be
//! imported here) through the [`Formatter`](session::Formatter) seam. This crate
//! is the pure, testable protocol core; no byte-stream server is advertised here
//! before the transport exists.

pub mod convert;
pub mod generation;
pub mod protocol;
pub mod server;
pub mod session;

pub use generation::GenerationQueries;
pub use protocol::{ErrorCode, PROTOCOL_VERSION, Request, Response, ResponseError};
pub use server::Server;
pub use session::{FormatResult, Formatter, Session};
