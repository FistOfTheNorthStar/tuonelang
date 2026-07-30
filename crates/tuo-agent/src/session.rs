//! The agent session: one long-lived compiler database answering every method.
//!
//! [`Session`] owns a single [`IncrementalSession`] — the *same* incremental
//! query engine the CLI and the LSP drive — and keeps it alive across every
//! request. Opening or editing a document feeds the shared session, whose
//! red-green engine recomputes only what the edit affected; a subsequent
//! request reads the memoized result. This is the "database reuse across
//! requests" the protocol requires: the compiler is **not** restarted per edit.
//!
//! Every semantic answer is a read-only projection of what the compiler already
//! computed — [`tuo_resolve::Resolution`], [`tuo_types::TypeckResult`], the
//! collected diagnostics — mapped from compiler [`Span`](tuo_source::Span)s to
//! wire positions at the edge ([`crate::convert`]). The session **reimplements
//! no stage**: it never parses, resolves, or type-checks on its own. Spec
//! execution reuses the shared [`tuo_spec`] runner; formatting is delegated to
//! an injected [`Formatter`], since the canonical formatter (`tuo-fmt`) sits at
//! this crate's own dependency layer and cannot be imported here.

use serde_json::{Value, json};
use tuo_compiler::{IncrementalSession, Semantics};
use tuo_diagnostics::{Confidence, Diagnostic, Severity, Suggestion};
use tuo_resolve::{Resolution, SymbolId, SymbolKind};
use tuo_source::{ByteOffset, FileId, SourceMap, Span};
use tuo_spec::{Limits, RunOutcome, Selection};
use tuo_types::Ty;

use crate::convert::{Position, Range};
use crate::protocol::{ErrorCode, ResponseError};

/// The canonical formatter, injected by the transport.
///
/// `tuo-agent` and `tuo-fmt` sit at the same dependency layer, so the agent
/// cannot call the formatter directly. The transport that owns the agent
/// server (the `tuo` CLI, which *can* depend on `tuo-fmt`) supplies an
/// implementation, so the `format` method reuses the one canonical formatter
/// rather than duplicating it.
pub trait Formatter {
    /// Format `text` into canonical form, returning `(canonical_text, changed,
    /// safe)`: `safe` is `false` when the formatter could not verify the result
    /// preserves meaning (in which case `text` is returned unchanged).
    fn format(&self, text: &str) -> FormatResult;
}

/// The outcome of a [`Formatter::format`] call — a layer-clean mirror of
/// `tuo_fmt::FormatOutcome`.
#[derive(Clone, Debug)]
pub struct FormatResult {
    /// The canonical text (or the untouched input when `safe` is `false`).
    pub text: String,
    /// Did formatting change anything?
    pub changed: bool,
    /// `false` only if the meaning-preservation self-check failed.
    pub safe: bool,
}

/// A long-lived agent session over one shared incremental compiler database.
pub struct Session {
    /// The shared incremental engine — reused across every request.
    session: IncrementalSession,
    /// The formatter the transport injected (for the `format` method).
    formatter: Box<dyn Formatter>,
}

impl Session {
    /// A new session with the given canonical formatter.
    #[must_use]
    pub fn new(formatter: Box<dyn Formatter>) -> Self {
        Self {
            session: IncrementalSession::new(),
            formatter,
        }
    }

    // ------------------------------------------------------------------
    // Document lifecycle.
    // ------------------------------------------------------------------

    /// Open or update `uri`'s text, feeding the shared incremental session.
    /// Returns the compiler [`FileId`] the document was interned under.
    pub fn set_document(&mut self, uri: &str, text: &str) -> FileId {
        self.session.set_file(uri, text)
    }

    /// The [`FileId`] of an open document, if any.
    #[must_use]
    fn file_of(&self, uri: &str) -> Option<FileId> {
        self.session.with_semantics(|sema| sema.map.file_id(uri))
    }

    // ------------------------------------------------------------------
    // Whole-program operations: check, verify, format, diagnostics.
    // ------------------------------------------------------------------

    /// `check`: the front-end verdict — every diagnostic across all open
    /// documents, plus whether the program is accepted.
    #[must_use]
    pub fn check(&self) -> Value {
        self.session.with_semantics(|sema| {
            let diagnostics: Vec<Value> = sema
                .diagnostics
                .iter()
                .map(|d| diagnostic_json(sema.map, d))
                .collect();
            json!({
                "accepted": sema.accepted,
                "errors": count(sema.diagnostics, Severity::Error),
                "warnings": count(sema.diagnostics, Severity::Warning),
                "diagnostics": diagnostics,
            })
        })
    }

    /// `diagnostics`: the diagnostics for one document, with ranges. (A subset
    /// of `check`, scoped to a single file the editor is displaying.)
    ///
    /// # Errors
    ///
    /// [`ErrorCode::UnknownDocument`] if `uri` was never opened.
    pub fn diagnostics(&self, uri: &str) -> Result<Value, ResponseError> {
        let file = self.require_file(uri)?;
        Ok(self.session.with_semantics(|sema| {
            let diagnostics: Vec<Value> = sema
                .diagnostics
                .iter()
                .filter(|d| span_file(sema.map, d.primary_span) == Some(file))
                .map(|d| diagnostic_json(sema.map, d))
                .collect();
            json!({ "uri": uri, "diagnostics": diagnostics })
        }))
    }

    /// `format`: the canonical form of `text` (or of an open document's current
    /// text when `text` is omitted), via the injected formatter.
    #[must_use]
    pub fn format(&self, text: &str) -> Value {
        let outcome = self.formatter.format(text);
        json!({
            "text": outcome.text,
            "changed": outcome.changed,
            "safe": outcome.safe,
        })
    }

    /// The current text of an open document, for a `format` request that names a
    /// document rather than sending inline text.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::UnknownDocument`] if `uri` was never opened.
    pub fn document_text(&self, uri: &str) -> Result<String, ResponseError> {
        let file = self.require_file(uri)?;
        self.session
            .with_semantics(|sema| {
                sema.map
                    .latest(file)
                    .and_then(|id| sema.map.get_source(id))
                    .map(|text| text.text().to_owned())
            })
            .ok_or_else(|| {
                ResponseError::new(ErrorCode::UnknownDocument, format!("no text for `{uri}`"))
            })
    }

    /// `verify`: run the program's specs through the reference interpreter,
    /// optionally restricted to the specs an edit to `affected_by` could have
    /// changed (the incremental affected-set). Refuses a program with front-end
    /// errors, mirroring `tuo verify`.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::Unavailable`] if the program has front-end errors (the
    /// diagnostics are attached as error data).
    pub fn verify(&self, affected_by: Option<&str>) -> Result<Value, ResponseError> {
        let selection = match affected_by {
            Some(uri) => {
                let file = self.require_file(uri)?;
                match self.session.affected_specs(&[file]) {
                    Ok(symbols) => Selection::Affected(symbols),
                    Err(_) => Selection::All,
                }
            }
            None => Selection::All,
        };
        self.run_specs(&selection)
    }

    /// `run_spec`: run the specs of one target function (or the free-standing
    /// spec of that name), or every spec when `target` is omitted.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::Unavailable`] if the program has front-end errors.
    pub fn run_spec(&self, target: Option<&str>) -> Result<Value, ResponseError> {
        let selection = target.map_or(Selection::All, |t| Selection::Target(t.to_owned()));
        self.run_specs(&selection)
    }

    // ------------------------------------------------------------------
    // Positional queries: type_at, definition, references, signature,
    // members, specs_for.
    // ------------------------------------------------------------------

    /// `type_at`: the type and kind of the symbol (or expression) at a position.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::Unavailable`] if nothing is resolvable at the position.
    pub fn type_at(&self, uri: &str, position: Position) -> Result<Value, ResponseError> {
        let file = self.require_file(uri)?;
        self.session
            .with_semantics(|sema| {
                let (span, _) = locate(&sema, file, position)?;
                if let Some(symbol) = sema.resolution.resolved_at(span) {
                    let sym = sema.resolution.symbol(symbol);
                    let ty = sema
                        .types
                        .type_of(symbol)
                        .map(|t| t.render(sema.resolution));
                    return Some(json!({
                        "name": sym.name,
                        "kind": sym.kind.noun(),
                        "type": ty,
                        "range": range_json(sema.map, span),
                    }));
                }
                let ty = sema.types.expr_ty(span)?;
                Some(json!({
                    "name": Value::Null,
                    "kind": "expression",
                    "type": ty.render(sema.resolution),
                    "range": range_json(sema.map, span),
                }))
            })
            .ok_or_else(|| {
                ResponseError::new(
                    ErrorCode::Unavailable,
                    "no type information at this position",
                )
            })
    }

    /// `definition`: the declaration location of the symbol at a position.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::Unavailable`] if nothing resolvable, or the symbol has no
    /// written declaration.
    pub fn definition(&self, uri: &str, position: Position) -> Result<Value, ResponseError> {
        let file = self.require_file(uri)?;
        self.session
            .with_semantics(|sema| {
                let (span, _) = locate(&sema, file, position)?;
                let symbol = sema.resolution.resolved_at(span)?;
                let declaration = sema.resolution.symbol(symbol).declaration?;
                location_json(sema.map, declaration)
            })
            .ok_or_else(|| {
                ResponseError::new(ErrorCode::Unavailable, "no definition at this position")
            })
    }

    /// `references`: every use of the symbol at a position (the declaration
    /// included when `include_declaration`).
    ///
    /// # Errors
    ///
    /// [`ErrorCode::Unavailable`] if nothing resolvable at the position.
    pub fn references(
        &self,
        uri: &str,
        position: Position,
        include_declaration: bool,
    ) -> Result<Value, ResponseError> {
        let file = self.require_file(uri)?;
        self.session
            .with_semantics(|sema| {
                let (span, _) = locate(&sema, file, position)?;
                let symbol = sema.resolution.resolved_at(span)?;
                let mut spans: Vec<Span> = Vec::new();
                if include_declaration {
                    spans.extend(sema.resolution.symbol(symbol).declaration);
                }
                spans.extend(sema.resolution.references_to(symbol));
                let locations: Vec<Value> = spans
                    .into_iter()
                    .filter_map(|s| location_json(sema.map, s))
                    .collect();
                Some(json!({ "references": locations }))
            })
            .ok_or_else(|| ResponseError::new(ErrorCode::Unavailable, "no symbol at this position"))
    }

    /// `signature`: the signature of the function called at (or just before) a
    /// position — the nearest resolved function reference at/before the cursor.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::Unavailable`] if no called function is in scope there.
    pub fn signature(&self, uri: &str, position: Position) -> Result<Value, ResponseError> {
        let file = self.require_file(uri)?;
        self.session
            .with_semantics(|sema| {
                let source = sema.map.latest(file)?;
                let text = sema.map.get_source(source)?;
                let cursor = position.to_offset(text);
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
                let name = sema.resolution.symbol(called.symbol).name.clone();
                let params: Vec<String> = fn_ty
                    .params
                    .iter()
                    .map(|p| p.render(sema.resolution))
                    .collect();
                let label = format!(
                    "fn {name}({}) -> {}",
                    params.join(", "),
                    fn_ty.ret.render(sema.resolution)
                );
                Some(json!({
                    "label": label,
                    "name": name,
                    "parameters": params,
                    "return_type": fn_ty.ret.render(sema.resolution),
                }))
            })
            .ok_or_else(|| ResponseError::new(ErrorCode::Unavailable, "no callable in scope here"))
    }

    /// `members`: the member names reachable through the symbol at a position —
    /// today, the variants of an enum (struct fields and methods are the type
    /// checker's, not resolution's, so v0 exposes what resolution knows:
    /// enum variants).
    ///
    /// # Errors
    ///
    /// [`ErrorCode::Unavailable`] if nothing resolvable at the position.
    pub fn members(&self, uri: &str, position: Position) -> Result<Value, ResponseError> {
        let file = self.require_file(uri)?;
        self.session
            .with_semantics(|sema| {
                let (span, _) = locate(&sema, file, position)?;
                let symbol = sema.resolution.resolved_at(span)?;
                let members: Vec<Value> = sema
                    .resolution
                    .variants_of(symbol)
                    .iter()
                    .map(|&variant| {
                        let sym = sema.resolution.symbol(variant);
                        json!({
                            "name": sym.name,
                            "kind": sym.kind.noun(),
                            "id": variant.as_u32(),
                        })
                    })
                    .collect();
                Some(json!({
                    "of": sema.resolution.symbol(symbol).name,
                    "members": members,
                }))
            })
            .ok_or_else(|| ResponseError::new(ErrorCode::Unavailable, "no symbol at this position"))
    }

    /// `specs_for`: the specs attached to the function at a position (or, if the
    /// cursor is on a spec, the specs of its target function).
    ///
    /// # Errors
    ///
    /// [`ErrorCode::Unavailable`] if the position names no function/spec.
    pub fn specs_for(&self, uri: &str, position: Position) -> Result<Value, ResponseError> {
        let file = self.require_file(uri)?;
        self.session
            .with_semantics(|sema| {
                let (span, _) = locate(&sema, file, position)?;
                let symbol = sema.resolution.resolved_at(span)?;
                let function = match sema.resolution.symbol(symbol).kind {
                    SymbolKind::Function => symbol,
                    SymbolKind::Spec => sema.resolution.target_of(symbol)?,
                    _ => return None,
                };
                let specs: Vec<Value> = sema
                    .resolution
                    .specs_for(function)
                    .into_iter()
                    .filter_map(|spec| spec_json(&sema, spec))
                    .collect();
                Some(json!({
                    "function": sema.resolution.symbol(function).name,
                    "specs": specs,
                }))
            })
            .ok_or_else(|| {
                ResponseError::new(
                    ErrorCode::Unavailable,
                    "no function or spec at this position",
                )
            })
    }

    // ------------------------------------------------------------------
    // Whole-program listings: symbols, available_imports.
    // ------------------------------------------------------------------

    /// `symbols`: the module-level outline of one document (functions, types,
    /// constants, specs), each with its kind, signature, and range.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::UnknownDocument`] if `uri` was never opened.
    pub fn symbols(&self, uri: &str) -> Result<Value, ResponseError> {
        let file = self.require_file(uri)?;
        Ok(self.session.with_semantics(|sema| {
            let mut symbols: Vec<(u32, u32, Value)> = sema
                .resolution
                .symbols()
                .filter(|(_, s)| is_outline_kind(s.kind))
                .filter_map(|(id, s)| {
                    let declaration = s.declaration?;
                    if span_file(sema.map, declaration) != Some(file) {
                        return None;
                    }
                    let range = range_json(sema.map, declaration);
                    let start = declaration.range().start().as_u32();
                    Some((
                        start,
                        id.as_u32(),
                        json!({
                            "name": s.name,
                            "kind": s.kind.noun(),
                            "id": id.as_u32(),
                            "signature": sema.types.type_of(id).map(|t| t.render(sema.resolution)),
                            "range": range,
                        }),
                    ))
                })
                .collect();
            symbols.sort_by_key(|(start, id, _)| (*start, *id));
            let list: Vec<Value> = symbols.into_iter().map(|(_, _, v)| v).collect();
            json!({ "uri": uri, "symbols": list })
        }))
    }

    /// `available_imports`: every public, module-level name the program defines
    /// — the entities another module could import. Each carries its name, kind,
    /// module path, and signature, so an agent can suggest an import without
    /// guessing.
    #[must_use]
    pub fn available_imports(&self) -> Value {
        self.session.with_semantics(|sema| {
            let mut imports: Vec<(String, Value)> = sema
                .resolution
                .symbols()
                .filter(|(_, s)| s.is_pub && is_importable_kind(s.kind))
                .map(|(id, s)| {
                    let module = module_path(sema.resolution, s.module);
                    let path = if module.is_empty() {
                        s.name.clone()
                    } else {
                        format!("{module}::{}", s.name)
                    };
                    (
                        path.clone(),
                        json!({
                            "name": s.name,
                            "kind": s.kind.noun(),
                            "module": module,
                            "path": path,
                            "signature": sema.types.type_of(id).map(|t| t.render(sema.resolution)),
                        }),
                    )
                })
                .collect();
            imports.sort_by(|a, b| a.0.cmp(&b.0));
            let list: Vec<Value> = imports.into_iter().map(|(_, v)| v).collect();
            json!({ "imports": list })
        })
    }

    // ------------------------------------------------------------------
    // apply_safe_fix.
    // ------------------------------------------------------------------

    /// `apply_safe_fix`: the compiler-authored, machine-applicable fixes for
    /// diagnostics overlapping a range in `uri`. Only fixes the compiler marks
    /// safe to apply are returned — the agent never invents a fix the compiler
    /// did not vouch for. Each fix carries its title and the exact text edits.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::UnknownDocument`] if `uri` was never opened.
    pub fn apply_safe_fix(&self, uri: &str, range: Option<Range>) -> Result<Value, ResponseError> {
        let file = self.require_file(uri)?;
        Ok(self.session.with_semantics(|sema| {
            let mut fixes = Vec::new();
            for diagnostic in sema.diagnostics {
                if span_file(sema.map, diagnostic.primary_span) != Some(file) {
                    continue;
                }
                if let Some(range) = range {
                    let diag_range = (
                        diagnostic.primary_span.range().start().as_u32(),
                        diagnostic.primary_span.range().end().as_u32(),
                    );
                    if !overlaps(diag_range, range_offsets(sema.map, file, range)) {
                        continue;
                    }
                }
                for suggestion in &diagnostic.suggestions {
                    if suggestion.confidence != Confidence::MachineApplicable {
                        continue;
                    }
                    if let Some(fix) = fix_json(sema.map, diagnostic, suggestion) {
                        fixes.push(fix);
                    }
                }
            }
            json!({ "uri": uri, "fixes": fixes })
        }))
    }

    // ------------------------------------------------------------------
    // Cancellation.
    // ------------------------------------------------------------------

    /// Request cooperative cancellation of the in-flight compiler query.
    pub fn request_cancellation(&self) {
        self.session.request_cancellation();
    }

    /// Clear a previously requested cancellation.
    pub fn clear_cancellation(&self) {
        self.session.clear_cancellation();
    }

    // ------------------------------------------------------------------
    // Internals.
    // ------------------------------------------------------------------

    /// Read the shared compiler semantics through the incremental session's
    /// scoped accessor. Exposed to the crate's [`generation`](crate::generation)
    /// module so the generation queries drive the *same* engine.
    pub(crate) fn with_semantics<R>(&self, f: impl FnOnce(Semantics<'_>) -> R) -> R {
        self.session.with_semantics(f)
    }

    /// The [`FileId`] of `uri`, or an [`ErrorCode::UnknownDocument`] error.
    pub(crate) fn require_file(&self, uri: &str) -> Result<FileId, ResponseError> {
        self.file_of(uri).ok_or_else(|| {
            ResponseError::new(
                ErrorCode::UnknownDocument,
                format!("document `{uri}` is not open"),
            )
        })
    }

    /// Run the selected specs, building the wire report. The specs run over a
    /// fresh source map built from the session's current document texts — the
    /// same texts the shared session already type-checked, so the front-end
    /// verdict agrees; affected-spec *selection* is what the shared incremental
    /// engine contributes.
    fn run_specs(&self, selection: &Selection) -> Result<Value, ResponseError> {
        let outcome = self
            .with_program(|map, sources| tuo_spec::run(map, sources, selection, Limits::default()));
        match outcome {
            RunOutcome::NotChecked(problems) => {
                let data = self.with_program(|map, _| {
                    let diagnostics: Vec<Value> =
                        problems.iter().map(|d| diagnostic_json(map, d)).collect();
                    json!({ "diagnostics": diagnostics })
                });
                Err(ResponseError::new(
                    ErrorCode::Unavailable,
                    "cannot run specs: the program has front-end errors",
                )
                .with_data(data))
            }
            RunOutcome::Ran(report) => {
                Ok(self.with_program(|map, _| spec_report_json(&report, map)))
            }
        }
    }

    /// Build a source map from the session's current documents and run `f` over
    /// it. This reconstructs the exact program the session holds so a MIR
    /// consumer (the spec runner) can drive it.
    fn with_program<R>(&self, f: impl FnOnce(&SourceMap, &[tuo_source::SourceId]) -> R) -> R {
        let mut map = SourceMap::new();
        let mut sources = Vec::new();
        // Reconstruct from the session's semantics map (its snapshot holds every
        // file's current text).
        let texts: Vec<(String, String)> = self.session.with_semantics(|sema| {
            self.session
                .files()
                .iter()
                .filter_map(|&file| {
                    let name = sema.map.file_name(file).to_owned();
                    let id = sema.map.latest(file)?;
                    let text = sema.map.get_source(id)?.text().to_owned();
                    Some((name, text))
                })
                .collect()
        });
        for (name, text) in &texts {
            let file = map.intern_file(name);
            if let Ok(id) = map.add_source(file, text.as_str()) {
                sources.push(id);
            }
        }
        f(&map, &sources)
    }
}

// ----------------------------------------------------------------------
// Cursor location.
// ----------------------------------------------------------------------

/// The smallest name-token span (a declaration or a reference) in `file`
/// containing `position`, plus its byte offset — the exact input
/// [`Resolution::resolved_at`] and [`tuo_types::TypeckResult::expr_ty`] expect.
fn locate(sema: &Semantics<'_>, file: FileId, position: Position) -> Option<(Span, ByteOffset)> {
    let source = sema.map.latest(file)?;
    let text = sema.map.get_source(source)?;
    let offset = position.to_offset(text);

    let mut best: Option<Span> = None;
    let mut consider = |span: Span| {
        if span.source() == source && span.range().contains(offset) {
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

// ----------------------------------------------------------------------
// JSON helpers.
// ----------------------------------------------------------------------

/// The number of diagnostics of `severity`.
fn count(diagnostics: &[Diagnostic], severity: Severity) -> usize {
    diagnostics
        .iter()
        .filter(|d| d.severity == severity)
        .count()
}

/// The file a span belongs to, via its source snapshot.
fn span_file(map: &SourceMap, span: Span) -> Option<FileId> {
    map.get_source(span.source()).map(|text| text.file())
}

/// A diagnostic as a wire object: its code, severity, message, and range.
fn diagnostic_json(map: &SourceMap, diagnostic: &Diagnostic) -> Value {
    json!({
        "code": diagnostic.code.to_string(),
        "severity": severity_name(diagnostic.severity),
        "message": diagnostic.message,
        "range": range_json(map, diagnostic.primary_span),
        "related": diagnostic
            .secondary_labels
            .iter()
            .filter_map(|label| {
                Some(json!({
                    "message": label.message,
                    "location": location_json(map, label.span)?,
                }))
            })
            .collect::<Vec<_>>(),
    })
}

/// The stable severity name.
fn severity_name(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Note => "note",
    }
}

/// The wire range object for `span`, or `null` if it cannot be located.
fn range_json(map: &SourceMap, span: Span) -> Value {
    match map.get_source(span.source()) {
        Some(text) => serde_json::to_value(Range::of_span(text, span)).unwrap_or(Value::Null),
        None => Value::Null,
    }
}

/// A `{uri, range}` location object for `span`, or `None` if unlocatable.
fn location_json(map: &SourceMap, span: Span) -> Option<Value> {
    let text = map.get_source(span.source())?;
    Some(json!({
        "uri": map.file_name(text.file()),
        "range": serde_json::to_value(Range::of_span(text, span)).ok()?,
    }))
}

/// Resolve a wire range to `(start, end)` byte offsets in `file`.
fn range_offsets(map: &SourceMap, file: FileId, range: Range) -> (u32, u32) {
    let text = map.latest(file).and_then(|id| map.get_source(id));
    match text {
        Some(text) => (
            range.start.to_offset(text).as_u32(),
            range.end.to_offset(text).as_u32(),
        ),
        None => (0, u32::MAX),
    }
}

/// Whether two byte ranges overlap (touching endpoints count).
fn overlaps(a: (u32, u32), b: (u32, u32)) -> bool {
    a.0 <= b.1 && b.0 <= a.1
}

/// A single safe fix: its title, the diagnostic it addresses, and its edits.
fn fix_json(map: &SourceMap, diagnostic: &Diagnostic, suggestion: &Suggestion) -> Option<Value> {
    let mut edits = Vec::new();
    for edit in &suggestion.edits {
        let text = map.get_source(edit.span.source())?;
        edits.push(json!({
            "uri": map.file_name(text.file()),
            "range": serde_json::to_value(Range::of_span(text, edit.span)).ok()?,
            "new_text": edit.replacement,
        }));
    }
    Some(json!({
        "title": suggestion.message,
        "diagnostic": diagnostic.code.to_string(),
        "edits": edits,
    }))
}

/// A spec's identity and location, for `specs_for`.
fn spec_json(sema: &Semantics<'_>, spec: SymbolId) -> Option<Value> {
    let span = sema
        .resolution
        .spec_symbols()
        .find(|(_, id)| *id == spec)
        .map(|(span, _)| span)
        .or_else(|| sema.resolution.symbol(spec).declaration)?;
    let location = location_json(sema.map, span)?;
    Some(json!({
        "name": sema.resolution.symbol(spec).name,
        "id": spec.as_u32(),
        "location": location,
    }))
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

/// Whether a symbol kind is an importable (module-level) entity.
fn is_importable_kind(kind: SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Function
            | SymbolKind::Struct
            | SymbolKind::Enum
            | SymbolKind::Interface
            | SymbolKind::Const
    )
}

/// The `::`-joined path of a module symbol (empty for the root module).
fn module_path(resolution: &Resolution, module: tuo_resolve::ModuleId) -> String {
    let symbol = resolution.module(module);
    symbol.path.join("::")
}

// ----------------------------------------------------------------------
// Spec report → JSON.
// ----------------------------------------------------------------------

/// The full spec report as a wire object: per-spec results and a summary.
fn spec_report_json(report: &tuo_spec::SpecReport, map: &SourceMap) -> Value {
    let runs: Vec<Value> = report
        .runs
        .iter()
        .map(|run| spec_run_json(run, map))
        .collect();
    let skipped: Vec<Value> = report
        .skipped
        .iter()
        .map(|s| {
            json!({
                "name": s.name,
                "id": s.symbol.as_u32(),
                "reason": s.reason,
                "range": range_json(map, s.span),
            })
        })
        .collect();
    let ran = report.ran();
    let failed = report.failures();
    json!({
        "passed": report.passed(),
        "summary": {
            "passed": ran - failed,
            "failed": failed,
            "ran": ran,
            "skipped": report.skipped.len(),
            "duration_micros": report.total_duration().as_micros(),
        },
        "runs": runs,
        "skipped_specs": skipped,
    })
}

/// One executed spec's result.
fn spec_run_json(run: &tuo_spec::SpecRun, map: &SourceMap) -> Value {
    json!({
        "name": run.name,
        "id": run.symbol.as_u32(),
        "target": run.target.map(|s| s.as_u32()),
        "passed": run.passed(),
        "range": range_json(map, run.span),
        "duration_micros": run.duration.as_micros(),
        "assertions": run
            .assertions
            .iter()
            .map(|a| assertion_json(a, map))
            .collect::<Vec<_>>(),
    })
}

/// One assertion's outcome.
fn assertion_json(assertion: &tuo_spec::AssertionResult, map: &SourceMap) -> Value {
    let (outcome, detail) = match &assertion.outcome {
        tuo_spec::Outcome::Passed => ("passed", Value::Null),
        tuo_spec::Outcome::Failed(failure) => (
            "failed",
            json!({ "expected": failure.expected, "actual": failure.actual }),
        ),
        tuo_spec::Outcome::Errored(trap) => (
            "errored",
            json!({
                "trap": trap.label,
                "message": trap.message,
                "range": range_json(map, trap.span),
            }),
        ),
    };
    json!({
        "assertion": assertion.kind.keyword(),
        "source": assertion.source,
        "range": range_json(map, assertion.span),
        "outcome": outcome,
        "detail": detail,
    })
}
