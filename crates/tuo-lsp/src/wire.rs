//! The Language Server Protocol wire vocabulary tuonelang's server speaks.
//!
//! These are plain data shapes — positions, ranges, locations, hovers, edits,
//! symbols, completions — that mirror the LSP JSON forms field-for-field
//! (camelCase where the protocol uses it) and derive [`serde`] so a transport
//! layer can serialize them directly. They deliberately carry **no** compiler
//! types: a [`Span`](tuo_source::Span) is translated to a [`Range`] at the
//! presentation edge (see [`crate::convert`]), so nothing below the LSP
//! boundary needs to know these shapes exist.
//!
//! # Positions are UTF-16, zero-based
//!
//! An LSP [`Position`] is a zero-based line and a zero-based **UTF-16 code
//! unit** column — the protocol's default `positionEncoding`. Internal compiler
//! positions are UTF-8 [`ByteOffset`](tuo_source::ByteOffset)s; the conversion
//! at both ends lives in [`crate::convert`], which is the only place the two
//! encodings meet.

use serde::{Deserialize, Serialize};

/// A zero-based position in a text document: a line and a UTF-16 code-unit
/// column (the LSP default encoding).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct Position {
    /// Zero-based line number.
    pub line: u32,
    /// Zero-based column, in UTF-16 code units from the line start.
    pub character: u32,
}

impl Position {
    /// A position at `line`, `character`.
    #[must_use]
    pub const fn new(line: u32, character: u32) -> Self {
        Self { line, character }
    }
}

/// A half-open range in a text document, `[start, end)`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Range {
    /// The (inclusive) start position.
    pub start: Position,
    /// The (exclusive) end position.
    pub end: Position,
}

impl Range {
    /// A range from `start` to `end`.
    #[must_use]
    pub const fn new(start: Position, end: Position) -> Self {
        Self { start, end }
    }

    /// Whether `position` lies within the range (start-inclusive, end-exclusive,
    /// or exactly at an empty range's point).
    #[must_use]
    pub fn contains(self, position: Position) -> bool {
        if self.start == self.end {
            return position == self.start;
        }
        position >= self.start && position < self.end
    }
}

/// A range within a specific document.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Location {
    /// The document's URI (here, its file name).
    pub uri: String,
    /// The range within that document.
    pub range: Range,
}

/// The severity of a published diagnostic (LSP's numeric scale).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(into = "u8", try_from = "u8")]
pub enum DiagnosticSeverity {
    /// An error (1).
    Error,
    /// A warning (2).
    Warning,
    /// Information (3).
    Information,
    /// A hint (4).
    Hint,
}

impl From<DiagnosticSeverity> for u8 {
    fn from(severity: DiagnosticSeverity) -> Self {
        match severity {
            DiagnosticSeverity::Error => 1,
            DiagnosticSeverity::Warning => 2,
            DiagnosticSeverity::Information => 3,
            DiagnosticSeverity::Hint => 4,
        }
    }
}

impl TryFrom<u8> for DiagnosticSeverity {
    type Error = String;

    fn try_from(value: u8) -> Result<Self, <Self as TryFrom<u8>>::Error> {
        match value {
            1 => Ok(Self::Error),
            2 => Ok(Self::Warning),
            3 => Ok(Self::Information),
            4 => Ok(Self::Hint),
            other => Err(format!("invalid diagnostic severity {other}")),
        }
    }
}

/// A related location and message attached to a diagnostic (a secondary label).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct DiagnosticRelatedInformation {
    /// Where the related code is.
    pub location: Location,
    /// What it contributes to the problem.
    pub message: String,
}

/// A published diagnostic: a ranged problem in a document.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Diagnostic {
    /// The range the diagnostic applies to.
    pub range: Range,
    /// Its severity.
    pub severity: DiagnosticSeverity,
    /// Its stable code (e.g. `T0301`).
    pub code: String,
    /// The primary message.
    pub message: String,
    /// Related locations (secondary labels), if any.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related_information: Vec<DiagnosticRelatedInformation>,
}

/// The contents of a hover response.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Hover {
    /// The rendered hover text (Markdown).
    pub contents: String,
    /// The range the hover describes, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<Range>,
}

/// A single textual edit within one document.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct TextEdit {
    /// The range to replace.
    pub range: Range,
    /// The replacement text.
    pub new_text: String,
}

/// A set of edits to one document.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct TextDocumentEdit {
    /// The document to edit (its URI / file name).
    pub uri: String,
    /// The edits to apply, in no particular order (LSP requires
    /// non-overlapping).
    pub edits: Vec<TextEdit>,
}

/// A workspace-wide edit: edits grouped by document. Used by rename.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct WorkspaceEdit {
    /// The per-document edits.
    pub document_changes: Vec<TextDocumentEdit>,
}

/// The kind of a symbol, in LSP's numeric vocabulary (the subset tuonelang
/// declares).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(into = "u8", try_from = "u8")]
pub enum SymbolKind {
    /// A module (LSP `Module` = 2).
    Module,
    /// A function (LSP `Function` = 12).
    Function,
    /// A struct (LSP `Struct` = 23).
    Struct,
    /// An enum (LSP `Enum` = 10).
    Enum,
    /// An enum member (LSP `EnumMember` = 22).
    EnumMember,
    /// An interface (LSP `Interface` = 11).
    Interface,
    /// A constant (LSP `Constant` = 14).
    Constant,
    /// A spec — rendered as LSP `Event` = 24, the closest fit for an
    /// executable specification block.
    Spec,
}

impl From<SymbolKind> for u8 {
    fn from(kind: SymbolKind) -> Self {
        match kind {
            SymbolKind::Module => 2,
            SymbolKind::Enum => 10,
            SymbolKind::Interface => 11,
            SymbolKind::Function => 12,
            SymbolKind::Constant => 14,
            SymbolKind::EnumMember => 22,
            SymbolKind::Struct => 23,
            SymbolKind::Spec => 24,
        }
    }
}

impl TryFrom<u8> for SymbolKind {
    type Error = String;

    fn try_from(value: u8) -> Result<Self, <Self as TryFrom<u8>>::Error> {
        match value {
            2 => Ok(Self::Module),
            10 => Ok(Self::Enum),
            11 => Ok(Self::Interface),
            12 => Ok(Self::Function),
            14 => Ok(Self::Constant),
            22 => Ok(Self::EnumMember),
            23 => Ok(Self::Struct),
            24 => Ok(Self::Spec),
            other => Err(format!("unsupported symbol kind {other}")),
        }
    }
}

/// One entry in a document's symbol outline (a flat [`SymbolInformation`]-style
/// item with a location).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct DocumentSymbol {
    /// The symbol's name.
    pub name: String,
    /// Its kind.
    pub kind: SymbolKind,
    /// The range of the whole declaration (here, the declaring name token).
    pub range: Range,
    /// The range of the name to select when navigating to it.
    pub selection_range: Range,
    /// A short detail string (e.g. the rendered signature), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// The kind of a completion item (LSP numeric subset).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(into = "u8", try_from = "u8")]
pub enum CompletionItemKind {
    /// A function (LSP `Function` = 3).
    Function,
    /// A struct (LSP `Struct` = 22).
    Struct,
    /// An enum (LSP `Enum` = 13).
    Enum,
    /// An enum member (LSP `EnumMember` = 20).
    EnumMember,
    /// An interface (LSP `Interface` = 8).
    Interface,
    /// A constant (LSP `Constant` = 21).
    Constant,
    /// A variable — a local or parameter (LSP `Variable` = 6).
    Variable,
    /// A keyword (LSP `Keyword` = 14).
    Keyword,
}

impl From<CompletionItemKind> for u8 {
    fn from(kind: CompletionItemKind) -> Self {
        match kind {
            CompletionItemKind::Variable => 6,
            CompletionItemKind::Interface => 8,
            CompletionItemKind::Function => 3,
            CompletionItemKind::Enum => 13,
            CompletionItemKind::Keyword => 14,
            CompletionItemKind::EnumMember => 20,
            CompletionItemKind::Constant => 21,
            CompletionItemKind::Struct => 22,
        }
    }
}

impl TryFrom<u8> for CompletionItemKind {
    type Error = String;

    fn try_from(value: u8) -> Result<Self, <Self as TryFrom<u8>>::Error> {
        match value {
            6 => Ok(Self::Variable),
            8 => Ok(Self::Interface),
            3 => Ok(Self::Function),
            13 => Ok(Self::Enum),
            14 => Ok(Self::Keyword),
            20 => Ok(Self::EnumMember),
            21 => Ok(Self::Constant),
            22 => Ok(Self::Struct),
            other => Err(format!("unsupported completion item kind {other}")),
        }
    }
}

/// A single completion candidate.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct CompletionItem {
    /// The label shown and inserted.
    pub label: String,
    /// The candidate's kind.
    pub kind: CompletionItemKind,
    /// A short detail (e.g. the rendered type), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// One parameter within a signature.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ParameterInformation {
    /// The parameter's rendered label (e.g. `take a: Int`).
    pub label: String,
}

/// One callable signature.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SignatureInformation {
    /// The whole signature, rendered.
    pub label: String,
    /// Its parameters, in order.
    pub parameters: Vec<ParameterInformation>,
}

/// A signature-help response.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SignatureHelp {
    /// The applicable signatures (tuonelang has no overloading, so at most one).
    pub signatures: Vec<SignatureInformation>,
    /// The index of the active signature.
    pub active_signature: u32,
    /// The index of the active parameter within it.
    pub active_parameter: u32,
}

/// The semantic-token type legend the server emits, in the index order LSP
/// `SemanticTokens` data refers to. Kept small and stable.
pub const SEMANTIC_TOKEN_TYPES: [&str; 7] = [
    "function",   // 0
    "type",       // 1  (struct, enum, interface)
    "enumMember", // 2
    "variable",   // 3  (local)
    "parameter",  // 4
    "property",   // 5  (constant)
    "namespace",  // 6  (module)
];

/// A decoded semantic token (before LSP's delta-encoding). The server produces
/// these in document order; [`SemanticTokens`] carries their flat encoding.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SemanticToken {
    /// Zero-based start line.
    pub line: u32,
    /// Zero-based start character (UTF-16).
    pub start_char: u32,
    /// The token's length in UTF-16 code units.
    pub length: u32,
    /// Index into [`SEMANTIC_TOKEN_TYPES`].
    pub token_type: u32,
}

/// A semantic-tokens response: the flat delta-encoded `data` array LSP expects
/// (five integers per token: deltaLine, deltaStart, length, tokenType,
/// tokenModifiers).
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct SemanticTokens {
    /// The delta-encoded token data.
    pub data: Vec<u32>,
}

impl SemanticTokens {
    /// Delta-encode `tokens` (which must be in document order) into the flat
    /// LSP `data` array.
    #[must_use]
    pub fn encode(tokens: &[SemanticToken]) -> Self {
        let mut data = Vec::with_capacity(tokens.len() * 5);
        let mut prev_line = 0;
        let mut prev_start = 0;
        for token in tokens {
            let delta_line = token.line - prev_line;
            let delta_start = if delta_line == 0 {
                token.start_char - prev_start
            } else {
                token.start_char
            };
            data.extend_from_slice(&[delta_line, delta_start, token.length, token.token_type, 0]);
            prev_line = token.line;
            prev_start = token.start_char;
        }
        Self { data }
    }
}

/// A code action offering a safe fix (an LSP `quickfix`).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct CodeAction {
    /// The user-facing title.
    pub title: String,
    /// The action kind — always `"quickfix"` here.
    pub kind: String,
    /// The edit to apply.
    pub edit: WorkspaceEdit,
    /// Whether the action is safe to apply automatically (machine-applicable).
    pub is_preferred: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_contains_is_half_open() {
        let range = Range::new(Position::new(0, 1), Position::new(0, 3));
        assert!(range.contains(Position::new(0, 1)));
        assert!(range.contains(Position::new(0, 2)));
        assert!(!range.contains(Position::new(0, 3)), "end is exclusive");
        assert!(!range.contains(Position::new(0, 0)));
    }

    #[test]
    fn semantic_tokens_delta_encode() {
        let tokens = [
            SemanticToken {
                line: 0,
                start_char: 3,
                length: 3,
                token_type: 0,
            },
            // Same line, later start: delta-start from the previous start.
            SemanticToken {
                line: 0,
                start_char: 10,
                length: 1,
                token_type: 4,
            },
            // Next line: delta-line 2, absolute start.
            SemanticToken {
                line: 2,
                start_char: 4,
                length: 6,
                token_type: 1,
            },
        ];
        let encoded = SemanticTokens::encode(&tokens);
        assert_eq!(
            encoded.data,
            vec![
                0, 3, 3, 0, 0, // first token
                0, 7, 1, 4, 0, // delta-start 10-3=7
                2, 4, 6, 1, 0, // delta-line 2, absolute start 4
            ]
        );
    }

    #[test]
    fn severity_serializes_to_the_lsp_scale() {
        assert_eq!(
            serde_json::to_string(&DiagnosticSeverity::Error).unwrap(),
            "1"
        );
        assert_eq!(
            serde_json::to_string(&DiagnosticSeverity::Warning).unwrap(),
            "2"
        );
        let round: DiagnosticSeverity = serde_json::from_str("4").unwrap();
        assert_eq!(round, DiagnosticSeverity::Hint);
    }

    #[test]
    fn symbol_kind_uses_lsp_numeric_codes() {
        assert_eq!(serde_json::to_string(&SymbolKind::Function).unwrap(), "12");
        assert_eq!(serde_json::to_string(&SymbolKind::Struct).unwrap(), "23");
    }
}
