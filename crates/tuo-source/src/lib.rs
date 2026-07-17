//! Source-text infrastructure for the tuonelang compiler.
//!
//! This crate owns the representation of source files, byte offsets, text
//! ranges, spans, and the source map that the rest of the compiler references
//! when reporting locations. It is the foundation of the pipeline and depends
//! on no other tuonelang crate.
//!
//! # Design
//!
//! - **Byte offsets.** All internal positions are UTF-8 **byte** offsets
//!   ([`ByteOffset`]), never character or line/column positions. Line and
//!   column values ([`LineCol`]) are derived on demand, for presentation only.
//! - **Strong identities.** [`FileId`] names a file, [`SourceId`] names one
//!   immutable snapshot of a file's text, and [`Revision`] orders the
//!   snapshots of a single file. Snapshots are never mutated — a change to a
//!   file produces a new [`SourceId`] at the next [`Revision`], which is the
//!   hook later incremental compilation builds on.
//! - **Immutable, cheaply shared text.** [`SourceText`] holds its text as
//!   `Arc<str>` and is itself handed out as `Arc<SourceText>` by the
//!   [`SourceMap`]; cloning either is O(1).
//! - **No silent invalid ranges.** Constructing a backwards [`TextRange`],
//!   mapping an out-of-bounds or non-char-boundary offset, or adding a text
//!   larger than the offset space are all `Result`s ([`InvalidRange`],
//!   [`LocateError`], [`SourceTooLarge`]) — never a silent wrap or clamp.
//! - **Policy-free.** Any Unicode text is accepted, and CRLF line terminators
//!   map correctly. tuonelang's *source preconditions* (UTF-8, no BOM, LF-only,
//!   no tabs — `specification/lexical-grammar.md` §1) are enforced by the
//!   lexer, not here: this crate must be able to represent *invalid* input in
//!   order to report diagnostics about it.
//!
//! No parser-specific concepts (tokens, trees, nodes) belong in this crate.

mod ids;
mod map;
mod offset;
mod range;
mod span;
mod text;

pub use ids::{FileId, Revision, SourceId};
pub use map::SourceMap;
pub use offset::ByteOffset;
pub use range::{InvalidRange, TextRange};
pub use span::Span;
pub use text::{LineCol, LocateError, SourceText, SourceTooLarge};
