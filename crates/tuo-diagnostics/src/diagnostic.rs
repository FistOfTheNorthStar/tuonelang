//! The canonical diagnostic data model.
//!
//! Everything in this module is plain, renderer-independent data. No type
//! here may reference terminal, color, or layout concepts — those live in the
//! rendering back ends ([`crate::render`], [`crate::json`]), which consume
//! this model read-only.

use std::fmt;

use tuo_source::Span;

use crate::DiagnosticCode;

/// How serious a diagnostic is.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Severity {
    /// The program is rejected.
    Error,
    /// The program is accepted but suspect.
    Warning,
    /// Neutral information attached to other diagnostics or tooling output.
    Note,
}

impl Severity {
    /// The lowercase name used in both human and JSON output.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Note => "note",
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A secondary span with its own message, pointing at related code.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Label {
    /// Where the label points.
    pub span: Span,
    /// What the labelled code contributes to the problem.
    pub message: String,
}

/// A machine-readable value a diagnostic compares or reports — what was
/// *expected* versus what was *actually* found.
///
/// This is deliberately a small, closed vocabulary of tagged strings rather
/// than real compiler types: `tuo-diagnostics` sits below the type system in
/// the crate layering, so later stages describe their values through this
/// neutral shape and machine consumers can still tell a type name from a
/// token or a count.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum StructuredValue {
    /// A type, rendered in tuonelang surface syntax (e.g. `List[Int]`).
    Type(String),
    /// A token or piece of syntax (e.g. `;`).
    Token(String),
    /// A symbol name (e.g. a function or variable name).
    Name(String),
    /// An integer quantity (e.g. an arity).
    Count(u64),
    /// Prose that has no more specific shape.
    Text(String),
}

impl StructuredValue {
    /// The tag machine consumers dispatch on.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Type(_) => "type",
            Self::Token(_) => "token",
            Self::Name(_) => "name",
            Self::Count(_) => "count",
            Self::Text(_) => "text",
        }
    }
}

impl fmt::Display for StructuredValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Type(s) | Self::Token(s) | Self::Name(s) => write!(f, "`{s}`"),
            Self::Count(n) => write!(f, "{n}"),
            Self::Text(s) => f.write_str(s),
        }
    }
}

/// How confident the compiler is that a [`Suggestion`] is correct.
///
/// Tools use this to decide whether an edit may be applied without a human in
/// the loop.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Confidence {
    /// The edit is known-correct and may be applied automatically.
    MachineApplicable,
    /// The edit is probably right but should be reviewed by a human (or a
    /// smarter agent) before applying.
    Probable,
    /// The edit sketches a direction; it may be incomplete or wrong.
    Speculative,
}

impl Confidence {
    /// The kebab-case name used in both human and JSON output.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::MachineApplicable => "machine-applicable",
            Self::Probable => "probable",
            Self::Speculative => "speculative",
        }
    }
}

impl fmt::Display for Confidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// One concrete text edit: replace the bytes at `span` with `replacement`.
/// An empty `span` is an insertion; an empty `replacement` is a deletion.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Edit {
    /// The bytes to replace.
    pub span: Span,
    /// The new text.
    pub replacement: String,
}

/// A suggested fix: a message, the edits that implement it, and how much the
/// compiler trusts them. All edits belong together — applying half a
/// suggestion is meaningless.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Suggestion {
    /// What the suggestion does, in imperative mood (e.g. "insert a `;`" ).
    pub message: String,
    /// The edits that implement it, in source order.
    pub edits: Vec<Edit>,
    /// How confident the compiler is in the edits.
    pub confidence: Confidence,
}

/// The canonical tuonelang diagnostic: structured data, independent of any
/// rendering.
///
/// Construct with [`Diagnostic::new`] (or the [`Diagnostic::error`] /
/// [`Diagnostic::warning`] shorthands) and attach optional parts with the
/// `with_*` builders:
///
/// ```
/// use tuo_diagnostics::{Diagnostic, DiagnosticCode, Namespace, StructuredValue};
/// use tuo_source::{SourceId, Span, TextRange};
///
/// let span = Span::new(
///     SourceId::from_raw(0),
///     TextRange::new(4u32, 6u32).expect("forward range"),
/// );
/// let diag = Diagnostic::error(
///     DiagnosticCode::new(Namespace::Type, 301),
///     "mismatched types",
///     span,
/// )
/// .with_primary_label("expected `Int`, found `String`")
/// .with_expected(StructuredValue::Type("Int".into()))
/// .with_actual(StructuredValue::Type("String".into()))
/// .with_note("`Int` and `String` never coerce into each other");
/// assert_eq!(diag.code.to_string(), "T0301");
/// ```
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Diagnostic {
    /// The stable, namespaced code identifying this error class.
    pub code: DiagnosticCode,
    /// How serious the diagnostic is.
    pub severity: Severity,
    /// The one-line human message (no trailing period, no source excerpts).
    pub message: String,
    /// The place the diagnostic is *about*.
    pub primary_span: Span,
    /// Optional message attached directly to the primary span.
    pub primary_label: Option<String>,
    /// Related places, each with its own message.
    pub secondary_labels: Vec<Label>,
    /// Free-form additional facts ("`x` was moved here because …").
    pub notes: Vec<String>,
    /// Actionable advice, when there is any.
    pub help: Option<String>,
    /// What the compiler expected, as machine-readable values.
    pub expected: Vec<StructuredValue>,
    /// What the compiler actually found, as machine-readable values.
    pub actual: Vec<StructuredValue>,
    /// Zero or more suggested fixes, each with a confidence level.
    pub suggestions: Vec<Suggestion>,
}

impl Diagnostic {
    /// A diagnostic with the mandatory parts; everything else starts empty.
    #[must_use]
    pub fn new(
        code: DiagnosticCode,
        severity: Severity,
        message: impl Into<String>,
        primary_span: Span,
    ) -> Self {
        Self {
            code,
            severity,
            message: message.into(),
            primary_span,
            primary_label: None,
            secondary_labels: Vec::new(),
            notes: Vec::new(),
            help: None,
            expected: Vec::new(),
            actual: Vec::new(),
            suggestions: Vec::new(),
        }
    }

    /// Shorthand for a [`Severity::Error`] diagnostic.
    #[must_use]
    pub fn error(code: DiagnosticCode, message: impl Into<String>, primary_span: Span) -> Self {
        Self::new(code, Severity::Error, message, primary_span)
    }

    /// Shorthand for a [`Severity::Warning`] diagnostic.
    #[must_use]
    pub fn warning(code: DiagnosticCode, message: impl Into<String>, primary_span: Span) -> Self {
        Self::new(code, Severity::Warning, message, primary_span)
    }

    /// Attach a message directly to the primary span.
    #[must_use]
    pub fn with_primary_label(mut self, label: impl Into<String>) -> Self {
        self.primary_label = Some(label.into());
        self
    }

    /// Add a secondary labelled span.
    #[must_use]
    pub fn with_secondary_label(mut self, span: Span, message: impl Into<String>) -> Self {
        self.secondary_labels.push(Label {
            span,
            message: message.into(),
        });
        self
    }

    /// Add a note.
    #[must_use]
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    /// Set the help text.
    #[must_use]
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    /// Add an expected value.
    #[must_use]
    pub fn with_expected(mut self, value: StructuredValue) -> Self {
        self.expected.push(value);
        self
    }

    /// Add an actually-found value.
    #[must_use]
    pub fn with_actual(mut self, value: StructuredValue) -> Self {
        self.actual.push(value);
        self
    }

    /// Add a suggested fix.
    #[must_use]
    pub fn with_suggestion(
        mut self,
        message: impl Into<String>,
        edits: Vec<Edit>,
        confidence: Confidence,
    ) -> Self {
        self.suggestions.push(Suggestion {
            message: message.into(),
            edits,
            confidence,
        });
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Namespace;
    use tuo_source::{SourceId, TextRange};

    fn span(start: u32, end: u32) -> Span {
        Span::new(
            SourceId::from_raw(0),
            TextRange::new(start, end).expect("test range must be forward"),
        )
    }

    #[test]
    fn builder_accumulates_all_parts() {
        let d = Diagnostic::error(
            DiagnosticCode::new(Namespace::Ownership, 12),
            "use after move",
            span(10, 11),
        )
        .with_primary_label("used here after move")
        .with_secondary_label(span(4, 5), "value moved here")
        .with_note("`x` does not implement `Copy`")
        .with_help("clone the value before moving it")
        .with_expected(StructuredValue::Text("an initialized value".into()))
        .with_actual(StructuredValue::Text("a moved value".into()))
        .with_suggestion(
            "clone `x`",
            vec![Edit {
                span: span(4, 5),
                replacement: "x.clone()".into(),
            }],
            Confidence::Probable,
        );

        assert_eq!(d.code.to_string(), "O0012");
        assert_eq!(d.severity, Severity::Error);
        assert_eq!(d.secondary_labels.len(), 1);
        assert_eq!(d.notes.len(), 1);
        assert!(d.help.is_some());
        assert_eq!(d.expected.len(), 1);
        assert_eq!(d.actual.len(), 1);
        assert_eq!(d.suggestions.len(), 1);
        assert_eq!(d.suggestions[0].confidence, Confidence::Probable);
    }

    #[test]
    fn structured_values_expose_stable_kinds() {
        let cases: [(StructuredValue, &str); 5] = [
            (StructuredValue::Type("Int".into()), "type"),
            (StructuredValue::Token(";".into()), "token"),
            (StructuredValue::Name("fibonacci".into()), "name"),
            (StructuredValue::Count(2), "count"),
            (StructuredValue::Text("hi".into()), "text"),
        ];
        for (value, kind) in cases {
            assert_eq!(value.kind(), kind);
        }
    }
}
