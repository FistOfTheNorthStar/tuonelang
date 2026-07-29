//! The analysis host: every language feature, answered by the shared compiler
//! queries.
//!
//! [`Analysis`] owns a single [`IncrementalSession`] — the same incremental
//! query engine the CLI and the agent protocol drive — and answers each LSP
//! request as a read-only projection of the session's semantic results. It
//! **never** parses, resolves, or type-checks: every answer is a translation of
//! what [`tuo_resolve::Resolution`], [`tuo_types::TypeckResult`], and the
//! diagnostics already computed, mapped from compiler [`Span`]s to LSP ranges at
//! the presentation edge ([`crate::convert`]).
//!
//! The correspondence is one existing compiler query per feature:
//!
//! | LSP feature            | Compiler query it reuses                                           |
//! |------------------------|--------------------------------------------------------------------|
//! | diagnostics            | [`Semantics::diagnostics`]                                          |
//! | hover                  | [`type_of`](tuo_types::TypeckResult::type_of) / [`expr_ty`](tuo_types::TypeckResult::expr_ty) |
//! | go-to-definition       | [`resolved_at`](tuo_resolve::Resolution::resolved_at) → declaration |
//! | find references        | [`references_to`](tuo_resolve::Resolution::references_to)          |
//! | rename                 | [`rename_spans`](tuo_resolve::Resolution::rename_spans)            |
//! | document symbols       | [`symbols`](tuo_resolve::Resolution::symbols)                      |
//! | completion             | [`symbols`](tuo_resolve::Resolution::symbols) (in scope) + keywords |
//! | signature help         | [`type_of`](tuo_types::TypeckResult::type_of) of the called function |
//! | semantic tokens        | declarations + [`references`](tuo_resolve::Resolution::references) |
//! | code actions           | [`tuo_diagnostics::Diagnostic`] suggestions                        |
//! | function → specs        | [`specs_for`](tuo_resolve::Resolution::specs_for)                  |
//! | spec → target function  | [`target_of`](tuo_resolve::Resolution::target_of)                 |

use tuo_compiler::{IncrementalSession, Semantics};
use tuo_diagnostics::{Confidence, Severity};
use tuo_resolve::{SymbolId, SymbolKind};
use tuo_source::{ByteOffset, FileId, SourceMap, Span};
use tuo_types::Ty;

use crate::convert::{offset_of_position, position_of_offset, range_of_span, range_within};
use crate::wire::{
    CodeAction, CompletionItem, CompletionItemKind, Diagnostic, DiagnosticRelatedInformation,
    DiagnosticSeverity, DocumentSymbol, Hover, Location, ParameterInformation, Position, Range,
    SemanticToken, SemanticTokens, SignatureHelp, SignatureInformation,
    SymbolKind as LspSymbolKind, TextDocumentEdit, TextEdit, WorkspaceEdit,
};

/// The keywords offered by completion when no better candidate applies.
const KEYWORDS: [&str; 12] = [
    "fn",
    "let",
    "var",
    "if",
    "else",
    "match",
    "return",
    "struct",
    "enum",
    "interface",
    "spec",
    "const",
];

/// A language-analysis host over a shared incremental compiler session.
pub struct Analysis {
    session: IncrementalSession,
    /// The document URIs opened so far, mapped to their compiler file ids. The
    /// session's `set_file` interns the name; we retain the mapping so requests
    /// can turn a URI back into a [`FileId`] without scanning.
    files: std::collections::BTreeMap<String, FileId>,
}

impl Default for Analysis {
    fn default() -> Self {
        Self::new()
    }
}

impl Analysis {
    /// A new, empty analysis host.
    #[must_use]
    pub fn new() -> Self {
        Self {
            session: IncrementalSession::new(),
            files: std::collections::BTreeMap::new(),
        }
    }

    /// Open or update a document. The text is fed to the shared session, whose
    /// incremental engine recomputes only what the edit affected.
    pub fn set_document(&mut self, uri: &str, text: &str) -> FileId {
        let file = self.session.set_file(uri, text);
        self.files.insert(uri.to_owned(), file);
        file
    }

    /// The [`FileId`] of an open document, if any.
    #[must_use]
    pub fn file_of(&self, uri: &str) -> Option<FileId> {
        self.files.get(uri).copied()
    }

    // ------------------------------------------------------------------
    // Diagnostics.
    // ------------------------------------------------------------------

    /// Every diagnostic in `uri`, as LSP diagnostics with ranges and related
    /// information. Reuses the front end's collected diagnostics verbatim.
    #[must_use]
    pub fn diagnostics(&self, uri: &str) -> Vec<Diagnostic> {
        self.session.with_semantics(|sema| {
            let Some(file) = file_id_by_name(sema.map, uri) else {
                return Vec::new();
            };
            sema.diagnostics
                .iter()
                .filter(|diagnostic| span_file(sema.map, diagnostic.primary_span) == Some(file))
                .filter_map(|diagnostic| lsp_diagnostic(sema.map, diagnostic))
                .collect()
        })
    }

    // ------------------------------------------------------------------
    // Hover.
    // ------------------------------------------------------------------

    /// The hover for the position in `uri`: the type and kind of the symbol (or
    /// expression) under the cursor.
    #[must_use]
    pub fn hover(&self, uri: &str, position: Position) -> Option<Hover> {
        self.session.with_semantics(|sema| {
            let (span, _) = self.locate(&sema, uri, position)?;
            // Prefer a resolved symbol under the cursor; fall back to the type
            // of the innermost expression span at that exact token.
            if let Some(symbol) = sema.resolution.resolved_at(span) {
                let sym = sema.resolution.symbol(symbol);
                let mut contents = format!("```tuo\n{}", sym.kind.noun());
                if let Some(ty) = sema.types.type_of(symbol) {
                    contents.push_str(&format!(" {}: {}", sym.name, ty.render(sema.resolution)));
                } else {
                    contents.push_str(&format!(" {}", sym.name));
                }
                contents.push_str("\n```");
                return Some(Hover {
                    contents,
                    range: range_of_span(sema.map, span),
                });
            }
            // No resolved name; try the expression type at this token.
            let ty = sema.types.expr_ty(span)?;
            Some(Hover {
                contents: format!("```tuo\n{}\n```", ty.render(sema.resolution)),
                range: range_of_span(sema.map, span),
            })
        })
    }

    // ------------------------------------------------------------------
    // Go to definition.
    // ------------------------------------------------------------------

    /// The definition location of the symbol under the cursor in `uri`.
    #[must_use]
    pub fn goto_definition(&self, uri: &str, position: Position) -> Option<Location> {
        self.session.with_semantics(|sema| {
            let (span, _) = self.locate(&sema, uri, position)?;
            let symbol = sema.resolution.resolved_at(span)?;
            let declaration = sema.resolution.symbol(symbol).declaration?;
            location_of_span(sema.map, declaration)
        })
    }

    // ------------------------------------------------------------------
    // Find references.
    // ------------------------------------------------------------------

    /// Every reference to the symbol under the cursor in `uri`, optionally
    /// including its declaration.
    #[must_use]
    pub fn references(
        &self,
        uri: &str,
        position: Position,
        include_declaration: bool,
    ) -> Vec<Location> {
        self.session.with_semantics(|sema| {
            let Some((span, _)) = self.locate(&sema, uri, position) else {
                return Vec::new();
            };
            let Some(symbol) = sema.resolution.resolved_at(span) else {
                return Vec::new();
            };
            let mut spans: Vec<Span> = Vec::new();
            if include_declaration {
                spans.extend(sema.resolution.symbol(symbol).declaration);
            }
            spans.extend(sema.resolution.references_to(symbol));
            spans
                .into_iter()
                .filter_map(|span| location_of_span(sema.map, span))
                .collect()
        })
    }

    // ------------------------------------------------------------------
    // Rename.
    // ------------------------------------------------------------------

    /// The workspace edit renaming the symbol under the cursor to `new_name`.
    /// Rewrites the declaring name token and every use, across files.
    #[must_use]
    pub fn rename(&self, uri: &str, position: Position, new_name: &str) -> Option<WorkspaceEdit> {
        self.session.with_semantics(|sema| {
            let (span, _) = self.locate(&sema, uri, position)?;
            let symbol = sema.resolution.resolved_at(span)?;
            let spans = sema.resolution.rename_spans(symbol);
            if spans.is_empty() {
                return None;
            }
            Some(workspace_edit(sema.map, &spans, new_name))
        })
    }

    // ------------------------------------------------------------------
    // Document symbols.
    // ------------------------------------------------------------------

    /// The outline of `uri`: its module-level items, with kinds and signatures.
    #[must_use]
    pub fn document_symbols(&self, uri: &str) -> Vec<DocumentSymbol> {
        self.session.with_semantics(|sema| {
            let Some(file) = file_id_by_name(sema.map, uri) else {
                return Vec::new();
            };
            let mut symbols: Vec<DocumentSymbol> = sema
                .resolution
                .symbols()
                .filter(|(_, symbol)| is_outline_kind(symbol.kind))
                .filter_map(|(id, symbol)| {
                    let declaration = symbol.declaration?;
                    if span_file(sema.map, declaration) != Some(file) {
                        return None;
                    }
                    let range = range_of_span(sema.map, declaration)?;
                    Some(DocumentSymbol {
                        name: symbol.name.clone(),
                        kind: lsp_symbol_kind(symbol.kind),
                        range,
                        selection_range: range,
                        detail: sema.types.type_of(id).map(|ty| ty.render(sema.resolution)),
                    })
                })
                .collect();
            symbols.sort_by_key(|s| (s.range.start.line, s.range.start.character));
            symbols
        })
    }

    // ------------------------------------------------------------------
    // Completion.
    // ------------------------------------------------------------------

    /// Completion candidates for the position in `uri`: the module-level names
    /// in scope, plus the language keywords. (v0: unfiltered — the client
    /// filters by the typed prefix.)
    #[must_use]
    pub fn completion(&self, uri: &str, _position: Position) -> Vec<CompletionItem> {
        self.session.with_semantics(|sema| {
            let mut items: Vec<CompletionItem> = sema
                .resolution
                .symbols()
                .filter(|(_, symbol)| is_completion_kind(symbol.kind))
                .map(|(id, symbol)| CompletionItem {
                    label: symbol.name.clone(),
                    kind: completion_kind(symbol.kind),
                    detail: sema.types.type_of(id).map(|ty| ty.render(sema.resolution)),
                })
                .collect();
            let _ = uri;
            items.extend(KEYWORDS.iter().map(|kw| CompletionItem {
                label: (*kw).to_owned(),
                kind: CompletionItemKind::Keyword,
                detail: None,
            }));
            // Deterministic order: named items first (by name), then keywords.
            items.sort_by(|a, b| {
                let key = |i: &CompletionItem| {
                    (
                        matches!(i.kind, CompletionItemKind::Keyword),
                        i.label.clone(),
                    )
                };
                key(a).cmp(&key(b))
            });
            items
        })
    }

    // ------------------------------------------------------------------
    // Signature help.
    // ------------------------------------------------------------------

    /// Signature help for the call the cursor is inside: the signature of the
    /// nearest function reference at or before the cursor on its line.
    ///
    /// Without a positional CST index, the active call is approximated by the
    /// last resolved function reference at or before the cursor — sufficient for
    /// a v0 that has no overloading.
    #[must_use]
    pub fn signature_help(&self, uri: &str, position: Position) -> Option<SignatureHelp> {
        self.session.with_semantics(|sema| {
            let file = file_id_by_name(sema.map, uri)?;
            let text = sema.map.get_source(sema.map.latest(file)?)?;
            let cursor = offset_of_position(text, position);
            // The nearest function reference beginning at or before the cursor.
            let called = sema
                .resolution
                .references()
                .iter()
                .filter(|reference| {
                    span_file(sema.map, reference.span) == Some(file)
                        && reference.span.range().start() <= cursor
                        && sema.resolution.symbol(reference.symbol).kind == SymbolKind::Function
                })
                .max_by_key(|reference| reference.span.range().start().as_u32())?;
            let ty = sema.types.type_of(called.symbol)?;
            let Ty::Fn(fn_ty) = ty else { return None };
            let name = &sema.resolution.symbol(called.symbol).name;
            let params: Vec<ParameterInformation> = fn_ty
                .params
                .iter()
                .map(|param| ParameterInformation {
                    label: param.render(sema.resolution),
                })
                .collect();
            let rendered = params
                .iter()
                .map(|p| p.label.clone())
                .collect::<Vec<_>>()
                .join(", ");
            let label = format!(
                "fn {name}({rendered}) -> {}",
                fn_ty.ret.render(sema.resolution)
            );
            Some(SignatureHelp {
                signatures: vec![SignatureInformation {
                    label,
                    parameters: params,
                }],
                active_signature: 0,
                active_parameter: 0,
            })
        })
    }

    // ------------------------------------------------------------------
    // Semantic tokens.
    // ------------------------------------------------------------------

    /// Semantic tokens for `uri`: every declaration name and resolved reference,
    /// classified by symbol kind, delta-encoded for LSP.
    #[must_use]
    pub fn semantic_tokens(&self, uri: &str) -> SemanticTokens {
        self.session.with_semantics(|sema| {
            let Some(file) = file_id_by_name(sema.map, uri) else {
                return SemanticTokens::default();
            };
            let mut tokens: Vec<SemanticToken> = Vec::new();
            // Declarations.
            for (_, symbol) in sema.resolution.symbols() {
                if let Some(span) = symbol.declaration {
                    if span_file(sema.map, span) == Some(file) {
                        if let Some(token) = semantic_token(&sema, span, symbol.kind) {
                            tokens.push(token);
                        }
                    }
                }
            }
            // Uses.
            for reference in sema.resolution.references() {
                if span_file(sema.map, reference.span) == Some(file) {
                    let kind = sema.resolution.symbol(reference.symbol).kind;
                    if let Some(token) = semantic_token(&sema, reference.span, kind) {
                        tokens.push(token);
                    }
                }
            }
            // LSP requires document order and non-overlap.
            tokens.sort_by_key(|t| (t.line, t.start_char));
            tokens.dedup_by_key(|t| (t.line, t.start_char));
            SemanticTokens::encode(&tokens)
        })
    }

    // ------------------------------------------------------------------
    // Code actions.
    // ------------------------------------------------------------------

    /// Safe quick-fix code actions for diagnostics overlapping `range` in
    /// `uri`. Only compiler-authored, machine-applicable suggestions are
    /// offered — the LSP never invents a fix the compiler did not vouch for.
    #[must_use]
    pub fn code_actions(&self, uri: &str, range: Range) -> Vec<CodeAction> {
        self.session.with_semantics(|sema| {
            let Some(file) = file_id_by_name(sema.map, uri) else {
                return Vec::new();
            };
            let mut actions = Vec::new();
            for diagnostic in sema.diagnostics {
                if span_file(sema.map, diagnostic.primary_span) != Some(file) {
                    continue;
                }
                let Some(diag_range) = range_of_span(sema.map, diagnostic.primary_span) else {
                    continue;
                };
                if !ranges_overlap(diag_range, range) {
                    continue;
                }
                for suggestion in &diagnostic.suggestions {
                    // Offer only fixes the compiler marks safe to apply.
                    if suggestion.confidence != Confidence::MachineApplicable {
                        continue;
                    }
                    let Some(edit) = suggestion_edit(sema.map, suggestion) else {
                        continue;
                    };
                    actions.push(CodeAction {
                        title: suggestion.message.clone(),
                        kind: "quickfix".to_owned(),
                        edit,
                        is_preferred: true,
                    });
                }
            }
            actions
        })
    }

    // ------------------------------------------------------------------
    // Spec ↔ function navigation.
    // ------------------------------------------------------------------

    /// The specs attached to the function under the cursor in `uri` — navigation
    /// from a function to its colocated specs.
    #[must_use]
    pub fn specs_for_function(&self, uri: &str, position: Position) -> Vec<Location> {
        self.session.with_semantics(|sema| {
            let Some((span, _)) = self.locate(&sema, uri, position) else {
                return Vec::new();
            };
            let Some(symbol) = sema.resolution.resolved_at(span) else {
                return Vec::new();
            };
            // Resolve to the function whether the cursor is on the function or a
            // spec targeting it.
            let function = match sema.resolution.symbol(symbol).kind {
                SymbolKind::Function => symbol,
                SymbolKind::Spec => match sema.resolution.target_of(symbol) {
                    Some(target) => target,
                    None => return Vec::new(),
                },
                _ => return Vec::new(),
            };
            sema.resolution
                .specs_for(function)
                .into_iter()
                .filter_map(|spec| spec_location(&sema, spec))
                .collect()
        })
    }

    /// The target function of the spec under the cursor in `uri` — navigation
    /// from a spec to the function it specifies. `None` for a free-standing
    /// (string-named) spec.
    #[must_use]
    pub fn target_of_spec(&self, uri: &str, position: Position) -> Option<Location> {
        self.session.with_semantics(|sema| {
            let (span, offset) = self.locate(&sema, uri, position)?;
            // A spec's own identity: either the cursor is on the spec name
            // (a target reference resolving to the spec) or inside the block.
            let spec = sema
                .resolution
                .resolved_at(span)
                .filter(|&s| sema.resolution.symbol(s).kind == SymbolKind::Spec)
                .or_else(|| enclosing_spec(&sema, span.source(), offset))?;
            let target = sema.resolution.target_of(spec)?;
            let declaration = sema.resolution.symbol(target).declaration?;
            location_of_span(sema.map, declaration)
        })
    }

    // ------------------------------------------------------------------
    // Cursor location.
    // ------------------------------------------------------------------

    /// Locate the exact name-token span at `position` in `uri`, plus the byte
    /// offset. The span is a declaration or reference token — the exact input
    /// [`Resolution::resolved_at`] and [`TypeckResult::expr_ty`] expect.
    fn locate(
        &self,
        sema: &Semantics<'_>,
        uri: &str,
        position: Position,
    ) -> Option<(Span, ByteOffset)> {
        let file = file_id_by_name(sema.map, uri)?;
        let source_id = sema.map.latest(file)?;
        let text = sema.map.get_source(source_id)?;
        let offset = offset_of_position(text, position);

        // Find the innermost name token (declaration or reference) whose range
        // contains the offset. References and declarations both carry exact
        // name-token spans; the smallest containing one wins.
        let mut best: Option<Span> = None;
        let mut consider = |span: Span| {
            if span.source() == source_id && span.range().contains(offset) {
                let better = best.is_none_or(|current| span.range().len() < current.range().len());
                if better {
                    best = Some(span);
                }
            }
        };
        for reference in sema.resolution.references() {
            consider(reference.span);
        }
        for (_, symbol) in sema.resolution.symbols() {
            if let Some(span) = symbol.declaration {
                consider(span);
            }
        }
        best.map(|span| (span, offset))
    }
}

// ----------------------------------------------------------------------
// Free helpers.
// ----------------------------------------------------------------------

/// The [`FileId`] whose name is `uri` in this (snapshot) map, if any.
fn file_id_by_name(map: &SourceMap, uri: &str) -> Option<FileId> {
    map.file_id(uri)
}

/// The file a span belongs to, via its source snapshot.
fn span_file(map: &SourceMap, span: Span) -> Option<FileId> {
    map.get_source(span.source()).map(|text| text.file())
}

/// Convert a compiler diagnostic to an LSP diagnostic (dropping it if its
/// primary span cannot be located).
fn lsp_diagnostic(map: &SourceMap, diagnostic: &tuo_diagnostics::Diagnostic) -> Option<Diagnostic> {
    let range = range_of_span(map, diagnostic.primary_span)?;
    let related = diagnostic
        .secondary_labels
        .iter()
        .filter_map(|label| {
            Some(DiagnosticRelatedInformation {
                location: location_of_span(map, label.span)?,
                message: label.message.clone(),
            })
        })
        .collect();
    Some(Diagnostic {
        range,
        severity: lsp_severity(diagnostic.severity),
        code: diagnostic.code.to_string(),
        message: diagnostic.message.clone(),
        related_information: related,
    })
}

/// Map a diagnostic severity to the LSP scale.
fn lsp_severity(severity: Severity) -> DiagnosticSeverity {
    match severity {
        Severity::Error => DiagnosticSeverity::Error,
        Severity::Warning => DiagnosticSeverity::Warning,
        Severity::Note => DiagnosticSeverity::Information,
    }
}

/// Build a [`Location`] for a span, or `None` if it cannot be located.
fn location_of_span(map: &SourceMap, span: Span) -> Option<Location> {
    let text = map.get_source(span.source())?;
    Some(Location {
        uri: map.file_name(text.file()).to_owned(),
        range: range_within(text, span.range()),
    })
}

/// A single-name workspace edit rewriting every span to `new_name`, grouped by
/// document.
fn workspace_edit(map: &SourceMap, spans: &[Span], new_name: &str) -> WorkspaceEdit {
    let mut by_uri: std::collections::BTreeMap<String, Vec<TextEdit>> =
        std::collections::BTreeMap::new();
    for &span in spans {
        let Some(text) = map.get_source(span.source()) else {
            continue;
        };
        let uri = map.file_name(text.file()).to_owned();
        by_uri.entry(uri).or_default().push(TextEdit {
            range: range_within(text, span.range()),
            new_text: new_name.to_owned(),
        });
    }
    WorkspaceEdit {
        document_changes: by_uri
            .into_iter()
            .map(|(uri, edits)| TextDocumentEdit { uri, edits })
            .collect(),
    }
}

/// Convert a compiler suggestion to a workspace edit, or `None` if any edit
/// span cannot be located.
fn suggestion_edit(
    map: &SourceMap,
    suggestion: &tuo_diagnostics::Suggestion,
) -> Option<WorkspaceEdit> {
    let mut by_uri: std::collections::BTreeMap<String, Vec<TextEdit>> =
        std::collections::BTreeMap::new();
    for edit in &suggestion.edits {
        let text = map.get_source(edit.span.source())?;
        let uri = map.file_name(text.file()).to_owned();
        by_uri.entry(uri).or_default().push(TextEdit {
            range: range_within(text, edit.span.range()),
            new_text: edit.replacement.clone(),
        });
    }
    Some(WorkspaceEdit {
        document_changes: by_uri
            .into_iter()
            .map(|(uri, edits)| TextDocumentEdit { uri, edits })
            .collect(),
    })
}

/// A semantic token for a name span classified by symbol kind, or `None` if the
/// kind carries no highlight or the span cannot be located.
fn semantic_token(sema: &Semantics<'_>, span: Span, kind: SymbolKind) -> Option<SemanticToken> {
    let token_type = semantic_type(kind)?;
    let text = sema.map.get_source(span.source())?;
    let start = position_of_offset(text, span.range().start());
    let end = position_of_offset(text, span.range().end());
    // A name token never spans lines.
    if start.line != end.line {
        return None;
    }
    Some(SemanticToken {
        line: start.line,
        start_char: start.character,
        length: end.character - start.character,
        token_type,
    })
}

/// The semantic-token-type index for a symbol kind (see
/// [`crate::wire::SEMANTIC_TOKEN_TYPES`]), or `None` for kinds not highlighted.
fn semantic_type(kind: SymbolKind) -> Option<u32> {
    Some(match kind {
        SymbolKind::Function => 0,
        SymbolKind::Struct | SymbolKind::Enum | SymbolKind::Interface | SymbolKind::TypeParam => 1,
        SymbolKind::Variant => 2,
        SymbolKind::Local => 3,
        SymbolKind::Param => 4,
        SymbolKind::Const => 5,
        SymbolKind::Module => 6,
        SymbolKind::Spec => return None,
    })
}

/// The location of a spec's block (its own symbol's span).
fn spec_location(sema: &Semantics<'_>, spec: SymbolId) -> Option<Location> {
    // A spec block's identity span: the (span, symbol) pair that maps to it.
    let span = sema
        .resolution
        .spec_symbols()
        .find(|(_, id)| *id == spec)
        .map(|(span, _)| span)
        .or_else(|| sema.resolution.symbol(spec).declaration)?;
    location_of_span(sema.map, span)
}

/// The spec whose block span contains `offset` in `source`, if any.
fn enclosing_spec(
    sema: &Semantics<'_>,
    source: tuo_source::SourceId,
    offset: ByteOffset,
) -> Option<SymbolId> {
    sema.resolution
        .spec_symbols()
        .filter(|(span, _)| span.source() == source && span.range().contains(offset))
        .min_by_key(|(span, _)| span.range().len())
        .map(|(_, id)| id)
}

/// Whether a symbol kind appears in the document outline.
fn is_outline_kind(kind: SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Function
            | SymbolKind::Struct
            | SymbolKind::Enum
            | SymbolKind::Interface
            | SymbolKind::Const
            | SymbolKind::Spec
    )
}

/// Whether a symbol kind is offered as a completion candidate.
fn is_completion_kind(kind: SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Function
            | SymbolKind::Struct
            | SymbolKind::Enum
            | SymbolKind::Variant
            | SymbolKind::Interface
            | SymbolKind::Const
    )
}

/// Map a resolve symbol kind to the LSP document-symbol kind.
fn lsp_symbol_kind(kind: SymbolKind) -> LspSymbolKind {
    match kind {
        SymbolKind::Function => LspSymbolKind::Function,
        SymbolKind::Struct => LspSymbolKind::Struct,
        SymbolKind::Enum => LspSymbolKind::Enum,
        SymbolKind::Variant => LspSymbolKind::EnumMember,
        SymbolKind::Interface => LspSymbolKind::Interface,
        SymbolKind::Const => LspSymbolKind::Constant,
        SymbolKind::Spec => LspSymbolKind::Spec,
        SymbolKind::Module => LspSymbolKind::Module,
        // Scoped kinds never reach the outline; map defensively.
        SymbolKind::TypeParam | SymbolKind::Param | SymbolKind::Local => LspSymbolKind::Constant,
    }
}

/// Map a resolve symbol kind to the LSP completion-item kind.
fn completion_kind(kind: SymbolKind) -> CompletionItemKind {
    match kind {
        SymbolKind::Function => CompletionItemKind::Function,
        SymbolKind::Struct => CompletionItemKind::Struct,
        SymbolKind::Enum => CompletionItemKind::Enum,
        SymbolKind::Variant => CompletionItemKind::EnumMember,
        SymbolKind::Interface => CompletionItemKind::Interface,
        SymbolKind::Const => CompletionItemKind::Constant,
        SymbolKind::Param | SymbolKind::Local => CompletionItemKind::Variable,
        SymbolKind::Module | SymbolKind::Spec | SymbolKind::TypeParam => {
            CompletionItemKind::Variable
        }
    }
}

/// Whether two ranges overlap (touching endpoints count).
fn ranges_overlap(a: Range, b: Range) -> bool {
    a.start <= b.end && b.start <= a.end
}
