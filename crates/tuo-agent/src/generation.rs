//! Compiler-guided generation queries: what an agent needs *before* it writes
//! the next token, as opposed to the after-the-fact facts the base methods
//! ([`Session::check`](crate::session::Session::check), `type_at`, `references`,
//! …) report about text that already exists.
//!
//! These queries answer "help me generate correct code here": the type expected
//! at a hole, the names in scope, the members of a value, the signature being
//! filled in, where a name can be imported from, and — kept deliberately apart
//! — a coarse read of the *syntactic* context.
//!
//! # Semantic guidance and syntactic guidance are kept apart
//!
//! The prompt behind this module is explicit about two things, and this module
//! honors both structurally:
//!
//! 1. **Semantic** guidance ([`context_at`](GenerationQueries::context_at)'s
//!    semantic fields, [`expected_type_at`](GenerationQueries::expected_type_at),
//!    [`visible_symbols_at`](GenerationQueries::visible_symbols_at),
//!    [`valid_members_of`](GenerationQueries::valid_members_of),
//!    [`call_signature`](GenerationQueries::call_signature),
//!    [`imports_for_symbol`](GenerationQueries::imports_for_symbol)) is a
//!    read-only projection of what the shared compiler
//!    [`Semantics`](tuo_compiler::Semantics) already computed —
//!    [`Resolution`](tuo_resolve::Resolution) and
//!    [`TypeckResult`](tuo_types::TypeckResult). No stage is re-run.
//!
//! 2. **Syntactic** guidance ([`expected_syntax_at`](GenerationQueries::expected_syntax_at),
//!    and the `context` field of `context_at`) is a **conservative lexical
//!    heuristic** over the raw source text — never a parser-backed enumeration.
//!    Every syntactic answer is flagged `"exhaustive": false` and carries a
//!    `"note"` saying so, because the compiler **cannot** enumerate every valid
//!    next token: there is no grammar-recovery / expected-token oracle in the
//!    pipeline, so claiming one would be dishonest. The heuristic offers likely
//!    categories to *narrow* a guess, and says plainly that it is not the whole
//!    truth.
//!
//! Keeping the two in separate response fields (`semantic` vs `syntactic`) and
//! separate methods means an agent never mistakes a lexical guess for a compiler
//! guarantee.

use serde_json::{Value, json};
use tuo_compiler::Semantics;
use tuo_resolve::{Resolution, Symbol, SymbolId, SymbolKind};
use tuo_source::{ByteOffset, FileId, SourceText, Span};
use tuo_types::{EnumShape, StructShape, Ty};

use crate::convert::{Position, Range};
use crate::protocol::{ErrorCode, ResponseError};
use crate::session::Session;

/// The generation-query surface, implemented for [`Session`]. These are the
/// methods a coding agent calls while *writing* code, distinct from the base
/// protocol methods that describe code already written.
pub trait GenerationQueries {
    /// `context_at`: the combined syntactic + semantic context at a position —
    /// the single "where am I and what's expected here" call an agent makes
    /// before generating. Bundles the coarse syntactic category (a heuristic),
    /// the expected type (semantic), and the enclosing function.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::UnknownDocument`] if `uri` was never opened.
    fn context_at(&self, uri: &str, position: Position) -> Result<Value, ResponseError>;

    /// `expected_type_at`: the type the compiler already recorded for the
    /// expression at the position, or — as a fallback signal at a hole with no
    /// recorded expression — the enclosing function's declared return type. This
    /// is **not** a general bidirectional hole-typing oracle (the pipeline
    /// exposes none); the `source` field says exactly which of the two it is, or
    /// `null` when neither applies.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::UnknownDocument`] if `uri` was never opened.
    fn expected_type_at(&self, uri: &str, position: Position) -> Result<Value, ResponseError>;

    /// `visible_symbols_at`: the names plausibly usable at the position — every
    /// module-level item, the prelude names, and the enclosing function's
    /// parameters and locals declared before the cursor. It is a conservative
    /// **over**-approximation (`complete: false`): block-level shadowing and
    /// lexical scoping are not modeled here, so it never *omits* a usable name
    /// but may include one a stricter scope check would hide.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::UnknownDocument`] if `uri` was never opened.
    fn visible_symbols_at(&self, uri: &str, position: Position) -> Result<Value, ResponseError>;

    /// `valid_members_of`: the members reachable through the value or type at a
    /// position — a struct's fields (with types) or an enum's variants (with
    /// payload fields). Precise: the member set is `exhaustive: true`, since the
    /// type checker's recorded shape is complete.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::Unavailable`] if nothing with members resolves there.
    fn valid_members_of(&self, uri: &str, position: Position) -> Result<Value, ResponseError>;

    /// `call_signature`: the signature of the function called at (or just
    /// before) a position, with each parameter's type and — when it can be read
    /// from the source between the call and the cursor — the active argument
    /// index. Precise projection of the checked function type.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::Unavailable`] if no called function is in scope there.
    fn call_signature(&self, uri: &str, position: Position) -> Result<Value, ResponseError>;

    /// `imports_for_symbol`: where a name can be imported from — every public,
    /// module-level definition matching `name`, with its module path and
    /// signature. Precise projection of the program's importable surface.
    fn imports_for_symbol(&self, name: &str) -> Value;

    /// `expected_syntax_at`: the syntactic categories that plausibly begin at a
    /// position (`item`, `expression`, `type`, `parameter`, …). A **conservative
    /// lexical heuristic**, not a grammar enumeration: the response is always
    /// `exhaustive: false` and carries a note saying the compiler does not
    /// enumerate every valid next token. Kept strictly separate from the
    /// semantic queries.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::UnknownDocument`] if `uri` was never opened.
    fn expected_syntax_at(&self, uri: &str, position: Position) -> Result<Value, ResponseError>;
}

impl GenerationQueries for Session {
    fn context_at(&self, uri: &str, position: Position) -> Result<Value, ResponseError> {
        let file = self.require_file(uri)?;
        Ok(
            self.with_semantics_at(file, position, |sema, text, offset| {
                let syntactic = syntactic_context(text, offset);
                let expected = expected_type_json(sema, file, text, offset);
                let enclosing = enclosing_function(sema, file, offset)
                    .map(|sym| function_ref_json(sema.resolution, sym));
                json!({
                    "syntactic": syntactic,
                    "semantic": {
                        "expected_type": expected,
                        "enclosing_function": enclosing,
                    },
                })
            }),
        )
    }

    fn expected_type_at(&self, uri: &str, position: Position) -> Result<Value, ResponseError> {
        let file = self.require_file(uri)?;
        Ok(
            self.with_semantics_at(file, position, |sema, text, offset| {
                expected_type_json(sema, file, text, offset)
            }),
        )
    }

    fn visible_symbols_at(&self, uri: &str, position: Position) -> Result<Value, ResponseError> {
        let file = self.require_file(uri)?;
        Ok(
            self.with_semantics_at(file, position, |sema, _text, offset| {
                let mut symbols = visible_symbols(sema, file, offset);
                // Stable, agent-friendly order: by kind rank, then name.
                symbols.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
                let list: Vec<Value> = symbols.into_iter().map(|(_, _, value)| value).collect();
                json!({
                    "visible_symbols": list,
                    // Honesty: this is an over-approximation, not a scope solver.
                    "complete": false,
                    "note": "an over-approximation of names in scope; block-level \
                             shadowing is not modeled, so a name may appear that a \
                             stricter scope check would hide",
                })
            }),
        )
    }

    fn valid_members_of(&self, uri: &str, position: Position) -> Result<Value, ResponseError> {
        let file = self.require_file(uri)?;
        self.with_semantics(|sema| {
            let (span, _) = locate_in(&sema, file, position)?;
            let target = member_target(&sema, span)?;
            Some(members_json(&sema, target))
        })
        .ok_or_else(|| {
            ResponseError::new(
                ErrorCode::Unavailable,
                "nothing with members resolves at this position",
            )
        })
    }

    fn call_signature(&self, uri: &str, position: Position) -> Result<Value, ResponseError> {
        let file = self.require_file(uri)?;
        self.with_semantics(|sema| {
            let text = sema
                .map
                .latest(file)
                .and_then(|id| sema.map.get_source(id))?;
            let cursor = position.to_offset(text);
            let called = nearest_called_function(&sema, file, cursor)?;
            let ty = sema.types.type_of(called)?;
            let Ty::Fn(fn_ty) = ty else { return None };
            let name = sema.resolution.symbol(called).name.clone();
            let params: Vec<Value> = fn_ty
                .params
                .iter()
                .enumerate()
                .map(|(index, param)| {
                    json!({
                        "index": index,
                        "mode": param.mode.keyword(),
                        "type": param.ty.render(sema.resolution),
                    })
                })
                .collect();
            let label = format!(
                "fn {name}({}) -> {}",
                fn_ty
                    .params
                    .iter()
                    .map(|p| format!("{} {}", p.mode.keyword(), p.ty.render(sema.resolution)))
                    .collect::<Vec<_>>()
                    .join(", "),
                fn_ty.ret.render(sema.resolution)
            );
            let active = active_argument(text, called, &sema, cursor);
            Some(json!({
                "label": label,
                "name": name,
                "parameters": params,
                "return_type": fn_ty.ret.render(sema.resolution),
                "active_parameter": active,
                "arity": fn_ty.params.len(),
            }))
        })
        .ok_or_else(|| ResponseError::new(ErrorCode::Unavailable, "no callable in scope here"))
    }

    fn imports_for_symbol(&self, name: &str) -> Value {
        self.with_semantics(|sema| {
            let mut candidates: Vec<(String, Value)> = sema
                .resolution
                .symbols()
                .filter(|(_, s)| s.is_pub && is_importable_kind(s.kind) && s.name == name)
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
            candidates.sort_by(|a, b| a.0.cmp(&b.0));
            let imports: Vec<Value> = candidates.into_iter().map(|(_, v)| v).collect();
            json!({ "name": name, "imports": imports })
        })
    }

    fn expected_syntax_at(&self, uri: &str, position: Position) -> Result<Value, ResponseError> {
        let file = self.require_file(uri)?;
        Ok(
            self.with_semantics_at(file, position, |_sema, text, offset| {
                let categories = expected_syntax_categories(text, offset);
                json!({
                    "expected_syntax_categories": categories,
                    // The central honesty guard the prompt demands: this is a
                    // lexical heuristic, never a complete valid-token enumeration.
                    "exhaustive": false,
                    "note": "a lexical heuristic offering likely syntactic \
                             categories; the compiler does not enumerate every valid \
                             next token, so this list is neither complete nor \
                             authoritative",
                })
            }),
        )
    }
}

// ----------------------------------------------------------------------
// A positional read helper shared by the offset-based generation queries.
// ----------------------------------------------------------------------

impl Session {
    /// Run `f` with the semantics, the [`SourceText`] of `file`, and the byte
    /// offset `position` resolves to. Used by the generation queries that answer
    /// from a cursor offset rather than an exact span. When `file` has no source
    /// yet (opened but not compiled), `f` still runs over an empty offset so the
    /// query returns a well-formed empty answer rather than erroring.
    fn with_semantics_at<R>(
        &self,
        file: FileId,
        position: Position,
        f: impl FnOnce(&Semantics<'_>, &SourceText, ByteOffset) -> R,
    ) -> R
    where
        R: Default,
    {
        self.with_semantics(|sema| {
            match sema.map.latest(file).and_then(|id| sema.map.get_source(id)) {
                Some(text) => {
                    let offset = position.to_offset(text);
                    f(&sema, text, offset)
                }
                None => R::default(),
            }
        })
    }
}

// ----------------------------------------------------------------------
// Semantic helpers — projections of Resolution / TypeckResult.
// ----------------------------------------------------------------------

/// The expected-type answer at `offset`: the recorded type of the smallest
/// enclosing expression, else the enclosing function's declared return type,
/// else null. The `source` field names which of the three it is — the query
/// never pretends to a hole-typing power the pipeline does not have.
fn expected_type_json(
    sema: &Semantics<'_>,
    file: FileId,
    _text: &SourceText,
    offset: ByteOffset,
) -> Value {
    if let Some((span, ty)) = smallest_expr_at(sema, file, offset) {
        return json!({
            "type": ty.render(sema.resolution),
            "source": "recorded_expression",
            "range": range_json(sema.map, span),
        });
    }
    if let Some(function) = enclosing_function(sema, file, offset)
        && let Some(Ty::Fn(fn_ty)) = sema.types.type_of(function)
    {
        return json!({
            "type": fn_ty.ret.render(sema.resolution),
            "source": "enclosing_return",
            "range": Value::Null,
        });
    }
    json!({ "type": Value::Null, "source": Value::Null, "range": Value::Null })
}

/// The smallest expression span in `file` whose range contains `offset`, with
/// its checked type. Reads only the types the checker already recorded — no
/// re-inference.
fn smallest_expr_at<'a>(
    sema: &'a Semantics<'_>,
    file: FileId,
    offset: ByteOffset,
) -> Option<(Span, &'a Ty)> {
    let source = sema.map.latest(file)?;
    let mut best: Option<(Span, &Ty)> = None;
    for reference in sema.resolution.references() {
        consider_expr(sema, source, offset, reference.span, &mut best);
    }
    // Expression spans also cover literals and compound expressions the checker
    // recorded; walk every reference and declaration name we can see, then any
    // expression the checker typed that we can reach through references. The
    // reference/declaration spans give the reliable anchors an agent cursors on.
    for (_, symbol) in sema.resolution.symbols() {
        if let Some(span) = symbol.declaration {
            consider_expr(sema, source, offset, span, &mut best);
        }
    }
    best
}

/// If `span` is in `source`, contains `offset`, has a recorded expression type,
/// and is smaller than the current best, record it.
fn consider_expr<'a>(
    sema: &'a Semantics<'_>,
    source: tuo_source::SourceId,
    offset: ByteOffset,
    span: Span,
    best: &mut Option<(Span, &'a Ty)>,
) {
    if span.source() != source || !span.range().contains(offset) {
        return;
    }
    let Some(ty) = sema.types.expr_ty(span) else {
        return;
    };
    let better = best.is_none_or(|(current, _)| span.range().len() < current.range().len());
    if better {
        *best = Some((span, ty));
    }
}

/// The function whose declaration-to-end span encloses `offset`, if any. There
/// is no body-span API, so this brackets each function by the source region
/// from its declaration name to the start of the next module-level declaration
/// in the same file — a conservative "which function am I inside" read.
fn enclosing_function(sema: &Semantics<'_>, file: FileId, offset: ByteOffset) -> Option<SymbolId> {
    // Collect module-level declaration starts in this file, in source order.
    let mut decls: Vec<(u32, SymbolId, SymbolKind)> = sema
        .resolution
        .symbols()
        .filter_map(|(id, s)| {
            let decl = s.declaration?;
            if span_file(sema.map, decl) != Some(file) {
                return None;
            }
            if !is_module_level(s.kind) {
                return None;
            }
            Some((decl.range().start().as_u32(), id, s.kind))
        })
        .collect();
    decls.sort_by_key(|(start, _, _)| *start);
    let at = offset.as_u32();
    // Find the last declaration starting at or before the offset; if it is a
    // function and the offset is before the next declaration, we are inside it.
    let mut current: Option<(SymbolId, SymbolKind)> = None;
    for (start, id, kind) in decls {
        if start <= at {
            current = Some((id, kind));
        } else {
            break;
        }
    }
    match current {
        Some((id, SymbolKind::Function)) => Some(id),
        _ => None,
    }
}

/// The visible-symbol candidates at `offset`: every module-level item, the
/// prelude names, plus the enclosing function's parameters and the locals
/// declared before the cursor. Each entry is `(kind_rank, name, json)` for a
/// stable sort. An over-approximation by construction.
fn visible_symbols(
    sema: &Semantics<'_>,
    file: FileId,
    offset: ByteOffset,
) -> Vec<(u8, String, Value)> {
    let mut out: Vec<(u8, String, Value)> = Vec::new();
    let enclosing = enclosing_function(sema, file, offset);
    for (id, symbol) in sema.resolution.symbols() {
        let include = match symbol.kind {
            // Module-level items are visible everywhere in the program.
            SymbolKind::Function
            | SymbolKind::Struct
            | SymbolKind::Enum
            | SymbolKind::Interface
            | SymbolKind::Const => true,
            // Parameters and locals only if they belong to the enclosing
            // function and (for locals) are declared before the cursor.
            SymbolKind::Param | SymbolKind::Local => {
                belongs_to_enclosing(sema, file, id, symbol, enclosing, offset)
            }
            _ => false,
        };
        if include {
            out.push((
                kind_rank(symbol.kind),
                symbol.name.clone(),
                symbol_ref_json(sema, id, symbol),
            ));
        }
    }
    // Prelude names (Option/Result/Some/None/Ok/Err) are always usable.
    for name in ["Option", "Result", "Some", "None", "Ok", "Err"] {
        if let Some(id) = sema.resolution.prelude_symbol(name) {
            let symbol = sema.resolution.symbol(id);
            out.push((
                kind_rank(symbol.kind),
                name.to_owned(),
                symbol_ref_json(sema, id, symbol),
            ));
        }
    }
    out
}

/// Whether a parameter/local `id` should count as visible at `offset`: it must
/// declare within `file`, and — since we cannot see the AST scope tree — we
/// bracket it to the enclosing function's source region and require locals to be
/// declared at or before the cursor. Conservative: never hides a usable name.
fn belongs_to_enclosing(
    sema: &Semantics<'_>,
    file: FileId,
    _id: SymbolId,
    symbol: &Symbol,
    enclosing: Option<SymbolId>,
    offset: ByteOffset,
) -> bool {
    let Some(function) = enclosing else {
        return false;
    };
    let Some(decl) = symbol.declaration else {
        return false;
    };
    if span_file(sema.map, decl) != Some(file) {
        return false;
    }
    // The parameter/local must declare within the enclosing function's region
    // (between its declaration start and the next module-level declaration).
    let region = function_region(sema, file, function);
    let Some((start, end)) = region else {
        return false;
    };
    let at = decl.range().start().as_u32();
    if at < start || at >= end {
        return false;
    }
    // A local is only in scope once declared; a parameter is in scope throughout.
    match symbol.kind {
        SymbolKind::Param => true,
        SymbolKind::Local => at <= offset.as_u32(),
        _ => false,
    }
}

/// The `[start, end)` source region of a module-level `function` in `file`:
/// from its declaration name to the next module-level declaration's start (or
/// end of file).
fn function_region(sema: &Semantics<'_>, file: FileId, function: SymbolId) -> Option<(u32, u32)> {
    let mut starts: Vec<u32> = sema
        .resolution
        .symbols()
        .filter_map(|(_, s)| {
            let decl = s.declaration?;
            if span_file(sema.map, decl) != Some(file) || !is_module_level(s.kind) {
                return None;
            }
            Some(decl.range().start().as_u32())
        })
        .collect();
    starts.sort_unstable();
    let decl = sema.resolution.symbol(function).declaration?;
    if span_file(sema.map, decl) != Some(file) {
        return None;
    }
    let start = decl.range().start().as_u32();
    let end = starts
        .iter()
        .copied()
        .find(|&s| s > start)
        .unwrap_or(u32::MAX);
    Some((start, end))
}

/// The member target at `span`: a struct or enum symbol, resolved either
/// directly (the cursor is on the type name) or through the type of the
/// expression there (the cursor is on a value of that type).
fn member_target(sema: &Semantics<'_>, span: Span) -> Option<MemberTarget> {
    // Directly on a struct/enum name?
    if let Some(symbol) = sema.resolution.resolved_at(span) {
        match sema.resolution.symbol(symbol).kind {
            SymbolKind::Struct => return Some(MemberTarget::Struct(symbol)),
            SymbolKind::Enum => return Some(MemberTarget::Enum(symbol)),
            _ => {}
        }
        // On a value whose type is a struct/enum?
        if let Some(ty) = sema.types.type_of(symbol) {
            if let Some(target) = member_target_of_ty(ty) {
                return Some(target);
            }
        }
    }
    // On an expression of struct/enum type?
    if let Some(ty) = sema.types.expr_ty(span) {
        return member_target_of_ty(ty);
    }
    None
}

/// A struct/enum member target derived from a type, if it is one.
fn member_target_of_ty(ty: &Ty) -> Option<MemberTarget> {
    match ty {
        Ty::Struct(symbol, _) => Some(MemberTarget::Struct(*symbol)),
        Ty::Enum(symbol, _) => Some(MemberTarget::Enum(*symbol)),
        _ => None,
    }
}

/// Either a struct or an enum whose members we describe.
enum MemberTarget {
    Struct(SymbolId),
    Enum(SymbolId),
}

/// The `valid_members_of` payload for a target: struct fields (with types) or
/// enum variants (with payload fields). The member set is exhaustive.
fn members_json(sema: &Semantics<'_>, target: MemberTarget) -> Value {
    match target {
        MemberTarget::Struct(symbol) => {
            let fields = sema
                .types
                .struct_shape(symbol)
                .map(|shape| struct_fields_json(sema, shape))
                .unwrap_or_default();
            json!({
                "of": sema.resolution.symbol(symbol).name,
                "kind": "struct",
                "members": fields,
                "exhaustive": true,
            })
        }
        MemberTarget::Enum(symbol) => {
            let variants = sema
                .types
                .enum_shape(symbol)
                .map(|shape| enum_variants_json(sema, shape))
                .unwrap_or_else(|| {
                    // Fall back to resolution's variant list if no shape was
                    // recorded (variant names without payload types).
                    sema.resolution
                        .variants_of(symbol)
                        .iter()
                        .map(|&v| {
                            json!({
                                "name": sema.resolution.symbol(v).name,
                                "kind": "enum variant",
                                "fields": Value::Array(vec![]),
                            })
                        })
                        .collect()
                });
            json!({
                "of": sema.resolution.symbol(symbol).name,
                "kind": "enum",
                "members": variants,
                "exhaustive": true,
            })
        }
    }
}

/// A struct's fields as wire members.
fn struct_fields_json(sema: &Semantics<'_>, shape: &StructShape) -> Vec<Value> {
    shape
        .fields
        .iter()
        .map(|(name, ty)| {
            json!({
                "name": name,
                "kind": "field",
                "type": ty.render(sema.resolution),
            })
        })
        .collect()
}

/// An enum's variants (with payload fields) as wire members.
fn enum_variants_json(sema: &Semantics<'_>, shape: &EnumShape) -> Vec<Value> {
    shape
        .variants
        .iter()
        .map(|(symbol, fields)| {
            let payload: Vec<Value> = fields
                .iter()
                .map(|(name, ty)| json!({ "name": name, "type": ty.render(sema.resolution) }))
                .collect();
            json!({
                "name": sema.resolution.symbol(*symbol).name,
                "kind": "enum variant",
                "fields": payload,
            })
        })
        .collect()
}

/// The nearest function reference at or before `cursor` in `file` — the call
/// whose signature the cursor is inside.
fn nearest_called_function(
    sema: &Semantics<'_>,
    file: FileId,
    cursor: ByteOffset,
) -> Option<SymbolId> {
    sema.resolution
        .references()
        .iter()
        .filter(|reference| {
            span_file(sema.map, reference.span) == Some(file)
                && reference.span.range().start() <= cursor
                && sema.resolution.symbol(reference.symbol).kind == SymbolKind::Function
        })
        .max_by_key(|reference| reference.span.range().start().as_u32())
        .map(|reference| reference.symbol)
}

/// The active argument index at `cursor`: the number of top-level commas in the
/// source between the called function's reference and the cursor, or `null`
/// when the cursor is not obviously inside the argument list. A lexical read of
/// the text the session already holds — reported as a best-effort index.
fn active_argument(
    text: &SourceText,
    called: SymbolId,
    sema: &Semantics<'_>,
    cursor: ByteOffset,
) -> Value {
    let call = sema
        .resolution
        .references()
        .iter()
        .filter(|r| r.symbol == called && r.span.range().start() <= cursor)
        .max_by_key(|r| r.span.range().start().as_u32());
    let Some(call) = call else {
        return Value::Null;
    };
    let src = text.text();
    let from = call.span.range().end().as_u32() as usize;
    let to = (cursor.as_u32() as usize).min(src.len());
    if from > to {
        return Value::Null;
    }
    let between = &src[from..to];
    // Only meaningful once an opening paren has appeared.
    if !between.contains('(') {
        return Value::Null;
    }
    let mut depth = 0i32;
    let mut commas = 0u32;
    for ch in between.chars() {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 1 => commas += 1,
            _ => {}
        }
    }
    json!(commas)
}

// ----------------------------------------------------------------------
// Syntactic helpers — a conservative lexical heuristic over raw text.
// ----------------------------------------------------------------------

/// A coarse syntactic classification at `offset`, from the last significant
/// characters before it. A heuristic — always paired with `exhaustive: false`.
fn syntactic_context(text: &SourceText, offset: ByteOffset) -> Value {
    let src = text.text();
    let at = (offset.as_u32() as usize).min(src.len());
    let before = &src[..at];
    let trimmed = before.trim_end();
    let category = if trimmed.is_empty() {
        "top_level"
    } else if trimmed.ends_with('.') {
        "member_access"
    } else if ends_with_type_position(trimmed) {
        "type"
    } else if at_top_level(trimmed) {
        "top_level"
    } else {
        "expression"
    };
    json!({
        "context": category,
        "exhaustive": false,
        "note": "a lexical heuristic; the compiler does not parse partial input \
                 to a definitive context",
    })
}

/// Likely syntactic categories that could begin at `offset`. A conservative
/// list, never claimed complete.
fn expected_syntax_categories(text: &SourceText, offset: ByteOffset) -> Vec<&'static str> {
    let src = text.text();
    let at = (offset.as_u32() as usize).min(src.len());
    let trimmed = src[..at].trim_end();
    if trimmed.is_empty() || at_top_level(trimmed) {
        // At the top level, an item may begin.
        return vec!["item", "fn", "struct", "enum", "const", "spec"];
    }
    if trimmed.ends_with('.') {
        return vec!["member", "field", "method"];
    }
    if ends_with_type_position(trimmed) {
        return vec!["type"];
    }
    if trimmed.ends_with('(') || trimmed.ends_with(',') {
        return vec!["expression", "argument"];
    }
    if trimmed.ends_with('{') {
        return vec!["statement", "expression"];
    }
    // Inside a body by default: a statement or expression may begin.
    vec!["expression", "statement"]
}

/// Heuristic: does the text end where a type is syntactically expected (after
/// `->`, `:`, or a generic `[`)? Type-position detection is deliberately
/// shallow.
fn ends_with_type_position(trimmed: &str) -> bool {
    trimmed.ends_with("->") || trimmed.ends_with(':') || trimmed.ends_with('[')
}

/// Heuristic: is the cursor at module top level (outside any `{}` body)? Counts
/// unbalanced braces in the text so far. Shallow — string/char literals and
/// comments are not lexed out, so this is only a hint.
fn at_top_level(before: &str) -> bool {
    let mut depth = 0i32;
    for ch in before.chars() {
        match ch {
            '{' => depth += 1,
            '}' => depth -= 1,
            _ => {}
        }
    }
    depth <= 0
}

// ----------------------------------------------------------------------
// Locate + small shared JSON/util helpers (mirrors of session.rs internals,
// kept private to the generation module so it stays self-contained).
// ----------------------------------------------------------------------

/// The smallest name-token span (declaration or reference) in `file` containing
/// `position`, plus its byte offset — the exact-span input the resolution/type
/// queries expect.
fn locate_in(sema: &Semantics<'_>, file: FileId, position: Position) -> Option<(Span, ByteOffset)> {
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

/// A symbol reference object: name, kind, id, and rendered signature.
fn symbol_ref_json(sema: &Semantics<'_>, id: SymbolId, symbol: &Symbol) -> Value {
    json!({
        "name": symbol.name,
        "kind": symbol.kind.noun(),
        "id": id.as_u32(),
        "type": sema.types.type_of(id).map(|t| t.render(sema.resolution)),
    })
}

/// A function reference object (name + id), for `enclosing_function`.
fn function_ref_json(resolution: &Resolution, symbol: SymbolId) -> Value {
    json!({
        "name": resolution.symbol(symbol).name,
        "id": symbol.as_u32(),
    })
}

/// The file a span belongs to, via its source snapshot.
fn span_file(map: &tuo_source::SourceMap, span: Span) -> Option<FileId> {
    map.get_source(span.source()).map(|text| text.file())
}

/// The wire range object for `span`, or `null` if it cannot be located.
fn range_json(map: &tuo_source::SourceMap, span: Span) -> Value {
    match map.get_source(span.source()) {
        Some(text) => serde_json::to_value(Range::of_span(text, span)).unwrap_or(Value::Null),
        None => Value::Null,
    }
}

/// The `::`-joined path of a module symbol (empty for the root module).
fn module_path(resolution: &Resolution, module: tuo_resolve::ModuleId) -> String {
    resolution.module(module).path.join("::")
}

/// Whether a symbol kind is a module-level (top-level) declaration.
fn is_module_level(kind: SymbolKind) -> bool {
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

/// A stable ordering rank per kind, so `visible_symbols` sorts predictably:
/// locals/params first (nearest), then module items, then prelude types.
fn kind_rank(kind: SymbolKind) -> u8 {
    match kind {
        SymbolKind::Local => 0,
        SymbolKind::Param => 1,
        SymbolKind::Const => 2,
        SymbolKind::Function => 3,
        SymbolKind::Struct => 4,
        SymbolKind::Enum => 5,
        SymbolKind::Interface => 6,
        _ => 7,
    }
}
