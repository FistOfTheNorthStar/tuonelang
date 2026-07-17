//! Structured diagnostics for the tuonelang compiler.
//!
//! tuonelang treats diagnostics as first-class, machine-readable **data**. The
//! canonical representation is the plain [`Diagnostic`] struct tree defined in
//! this crate — TDG-owned types with no rendering concepts in them. Rendering
//! is layered *on top* of that data, in two independent back ends:
//!
//! - [`json`] — machine output with an explicit, versioned schema
//!   ([`json::SCHEMA_VERSION`]);
//! - [`render`] — a deterministic plain-text renderer for human terminals.
//!
//! A third-party human renderer (e.g. `ariadne`, which handles labelled and
//! multiline spans well) may back [`render`] in the future, but its types must
//! never become the canonical representation: everything the compiler stores
//! or transports is the [`Diagnostic`] data model, and any renderer consumes
//! it read-only.
//!
//! # Diagnostic codes
//!
//! Every diagnostic carries a stable [`DiagnosticCode`] such as `T0301`,
//! namespaced by pipeline stage ([`Namespace`]):
//!
//! | Prefix | Stage |
//! |--------|-------|
//! | `Lxxxx` | lexical |
//! | `Pxxxx` | parser |
//! | `Rxxxx` | resolution |
//! | `Txxxx` | type |
//! | `Oxxxx` | ownership |
//! | `Mxxxx` | MIR |
//! | `Sxxxx` | specification |
//! | `Cxxxx` | code generation |
//!
//! Once a code has shipped, its number is never reused for a different error.
//!
//! This crate depends only on [`tuo_source`] (for spans) and must not learn
//! about parsing or later semantic stages: stage crates *construct*
//! diagnostics; this crate only models and renders them.

mod code;
mod diagnostic;
pub mod json;
pub mod render;

pub use code::{DiagnosticCode, InvalidCode, Namespace};
pub use diagnostic::{Confidence, Diagnostic, Edit, Label, Severity, StructuredValue, Suggestion};
