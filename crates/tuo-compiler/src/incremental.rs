//! Fine-grained incremental compilation over the real pipeline stages.
//!
//! [`IncrementalSession`] is the shared driver an interactive host (the CLI's
//! watch mode, the LSP, the agent protocol) uses to recompute compiler results
//! after an edit without redoing unaffected work. It owns a [`tuo_db`]
//! [`QueryEngine`](tuo_db::QueryEngine) and registers the seven tracked queries
//! the incremental model is built around:
//!
//! ```text
//! source(file)            input   — the current text of a file
//! parse(file)             derived — the file's parse result (diagnostics digest)
//! interface(file)         derived — the file's signature-level summary
//! resolution()            derived — whole-program name resolution digest
//! type_of(function)       derived — one function's checked signature type
//! mir_of(function)        derived — one function's lowered MIR (rendered)
//! spec_deps(spec)         derived — one spec's semantic dependency closure
//! ```
//!
//! # Why the engine lives in `tuo-db` but the wiring lives here
//!
//! `tuo-db` sits low in the dependency layering — it may see only `tuo-source`
//! and `tuo-diagnostics`, never a stage crate. So `tuo-db` provides the
//! *stage-agnostic* red-green engine, and this crate — which depends on every
//! stage — provides the stage *wiring* by implementing
//! [`QueryHost`](tuo_db::QueryHost). No layering rule is bent: the engine speaks
//! only opaque keys and values.
//!
//! # Whole-program passes, fine-grained invalidation
//!
//! The stage passes (`tuo_resolve::resolve`, `tuo_types::check`,
//! `tuo_mir::lower`) are whole-program today: they are not yet internally
//! per-item incremental. The session therefore recomputes each **whole-program**
//! result at most once per revision (memoized in [`Snapshot`], keyed on the
//! engine revision) and exposes genuinely fine-grained queries by **slicing**
//! per-symbol results out of that snapshot. The per-symbol queries store
//! comparable values (a signature [`Ty`](tuo_types::Ty), a rendered MIR string,
//! a dependency list) so the engine's **early cutoff** fires: an edit that
//! changes one function's body re-runs that function's slice queries, but the
//! *sliced* results of every other function compare equal and so do not
//! invalidate their dependents (caller typing, MIR, specs).
//!
//! This is honest fine-grained invalidation at the query-graph level. When a
//! stage later becomes internally incremental it slots in behind these same
//! query keys with no change to callers. The execution log
//! ([`IncrementalSession::executed_queries`]) records exactly which per-item
//! queries re-executed, which is what the measurement suite reports.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use tuo_ast::Ast;
use tuo_db::{Computed, QueryCtx, QueryEngine, QueryHost, QueryKey, downcast, stored};
use tuo_resolve::{Resolution, SymbolId, SymbolKind};
use tuo_source::{FileId, SourceId, SourceMap};
use tuo_types::{Ty, TypeckResult};

/// Query-kind tags (the `kind` field of a [`QueryKey`]).
mod kind {
    /// Input: the current text of a file (arg = `FileId` raw).
    pub(super) const SOURCE: u32 = 0;
    /// Input: the set of files in the program (arg = 0). Bumped when a file is
    /// added, so whole-program queries depend on program membership.
    pub(super) const FILE_SET: u32 = 1;
    /// Derived: a file's parse digest (arg = `FileId` raw).
    pub(super) const PARSE: u32 = 2;
    /// Derived: a file's signature-level interface (arg = `FileId` raw).
    pub(super) const INTERFACE: u32 = 3;
    /// Derived: whole-program resolution digest (arg = 0).
    pub(super) const RESOLUTION: u32 = 4;
    /// Derived: a function's checked signature type (arg = `SymbolId` raw).
    pub(super) const TYPE_OF: u32 = 5;
    /// Derived: a function's lowered MIR (arg = `SymbolId` raw).
    pub(super) const MIR_OF: u32 = 6;
    /// Derived: a spec's semantic dependency closure (arg = spec `SymbolId`).
    pub(super) const SPEC_DEPS: u32 = 7;
    /// Derived: a fingerprint of one function's body text (arg = `SymbolId`).
    /// Isolated from the file's parse digest so an edit to *one* function's body
    /// re-lowers only that function's MIR, not every function in the file.
    pub(super) const FN_BODY: u32 = 8;
    /// Derived: the whole-program spec dependency graph digest (arg = 0).
    /// Separate from resolution so a spec's dependency change re-runs only spec
    /// queries, never function typing.
    pub(super) const SPEC_GRAPH: u32 = 9;
}

// ----------------------------------------------------------------------
// Stored value shapes (all `PartialEq`, so early cutoff compares them).
// ----------------------------------------------------------------------

/// The parse digest of one file: a content fingerprint of the file plus its
/// parse diagnostics' codes. It is **body-sensitive** on purpose — any edit to
/// the file changes it — so a downstream body consumer (`mir_of`) re-runs. The
/// signature-level cutoff that shields callers from a body-only edit happens one
/// layer up, at `interface` / `resolution`, whose digests ignore bodies.
#[derive(Clone, PartialEq, Eq, Debug)]
struct ParseDigest {
    /// A fingerprint of the file's exact text.
    content: u64,
    /// The file's parse diagnostics' codes.
    diagnostic_codes: Vec<String>,
}

/// One item's signature entry in a file's interface: its symbol, name, kind,
/// visibility, and signature type. Deliberately **excludes** function bodies,
/// so a body-only edit leaves the interface equal.
#[derive(Clone, PartialEq, Eq, Debug)]
struct InterfaceItem {
    symbol: u32,
    name: String,
    kind_tag: u8,
    is_pub: bool,
    signature: Option<Ty>,
}

/// A file's signature-level interface: its items' signatures, in symbol order.
#[derive(Clone, PartialEq, Eq, Debug)]
struct Interface {
    items: Vec<InterfaceItem>,
}

/// The whole-program resolution digest: the program's symbol structure —
/// `(symbol raw, name, kind tag)` for every symbol, in id order. It changes iff
/// a symbol is added/removed/renamed or changes kind. It deliberately excludes
/// the spec dependency graph (which lives in [`SpecGraphDigest`]), so a change
/// to *which* symbols a spec references does not invalidate function typing.
/// The coarse dependency of the per-symbol `type_of` / `interface` queries.
#[derive(Clone, PartialEq, Eq, Debug)]
struct ResolutionDigest {
    symbols: Vec<(u32, String, u8)>,
}

/// The whole-program spec dependency graph: `(spec raw, target raw or
/// u32::MAX, dependency raws)` for every spec. The coarse dependency of the
/// `spec_deps` queries — separate from [`ResolutionDigest`] so a spec's
/// dependency change re-runs only spec queries, never function typing.
#[derive(Clone, PartialEq, Eq, Debug)]
struct SpecGraphDigest {
    graph: Vec<(u32, u32, Vec<u32>)>,
}

/// A function's checked signature type, stored for the `type_of` query.
#[derive(Clone, PartialEq, Eq, Debug)]
struct SignatureType(Option<Ty>);

/// A function's lowered MIR, stored as its canonical rendering.
#[derive(Clone, PartialEq, Eq, Debug)]
struct MirText(String);

/// A spec's semantic dependency closure: the module-level symbols its body
/// (transitively) depends on, sorted and deduplicated.
#[derive(Clone, PartialEq, Eq, Debug)]
struct SpecDeps(Vec<u32>);

/// A function's body fingerprint, stored for the `fn_body` query.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct BodyHash(u64);

// ----------------------------------------------------------------------
// The whole-program snapshot (recomputed at most once per revision).
// ----------------------------------------------------------------------

/// A whole-program compilation of the current sources, memoized by revision.
/// The stage passes are whole-program, so this is computed lazily and reused
/// by every per-symbol slice query within one revision.
struct Snapshot {
    /// The revision this snapshot reflects.
    revision: u64,
    /// The source map holding every file's current text.
    map: SourceMap,
    /// Name resolution over the whole program.
    resolution: Resolution,
    /// Type checking over the whole program.
    types: TypeckResult,
    /// Per-function lowered MIR, rendered, keyed by function symbol raw.
    mir: BTreeMap<u32, String>,
    /// Per-function body fingerprint (of the whole declaration's source text),
    /// keyed by function symbol raw. Isolates one function's body edits from
    /// the rest of its file.
    fn_body: BTreeMap<u32, u64>,
    /// Whether the front end accepted the program (no error diagnostics).
    accepted: bool,
    /// Every diagnostic the front end produced for this snapshot, in stage
    /// order (parse, resolve, type, ownership). Retained — not just reduced to
    /// the `accepted` boolean — so interactive hosts (the LSP, the agent
    /// protocol) can surface diagnostics with their spans.
    diagnostics: Vec<tuo_diagnostics::Diagnostic>,
}

/// A read-only view of one program snapshot's semantic results, handed to a
/// closure by [`IncrementalSession::with_semantics`].
///
/// It bundles the compiler's answers an interactive host reads — resolution,
/// types, the source map, and diagnostics — all anchored to the *same*
/// revision, so spans, symbols, and line/column conversion are mutually
/// consistent. The borrows are scoped to the closure: the view never outlives
/// the snapshot it describes.
pub struct Semantics<'a> {
    /// The source map the snapshot's spans are anchored to. Line/column
    /// conversion for any span in `resolution`/`types`/`diagnostics` must go
    /// through this map.
    pub map: &'a SourceMap,
    /// Whole-program name resolution: symbols, references, spec attachments and
    /// dependencies. The seam for go-to-definition, find-references, rename, and
    /// spec navigation.
    pub resolution: &'a Resolution,
    /// Whole-program type checking: symbol and expression types. The seam for
    /// hover.
    pub types: &'a TypeckResult,
    /// Every diagnostic the front end produced, in stage order (parse, resolve,
    /// type, ownership), each carrying its span(s).
    pub diagnostics: &'a [tuo_diagnostics::Diagnostic],
    /// Whether the front end accepted the program (no error diagnostics).
    pub accepted: bool,
}

/// A fine-grained incremental compiler session over the real pipeline stages.
pub struct IncrementalSession {
    /// The generic red-green engine tracking per-item query dependencies.
    engine: QueryEngine,
    /// File interning and current text, mirrored into each snapshot's map.
    files: Vec<(FileId, String)>,
    /// The source map used to intern file names.
    map: SourceMap,
    /// The memoized whole-program snapshot for the current revision.
    snapshot: RefCell<Option<Snapshot>>,
}

impl Default for IncrementalSession {
    fn default() -> Self {
        Self::new()
    }
}

impl IncrementalSession {
    /// A new, empty session.
    #[must_use]
    pub fn new() -> Self {
        Self {
            engine: QueryEngine::new(),
            files: Vec::new(),
            map: SourceMap::new(),
            snapshot: RefCell::new(None),
        }
    }

    /// Set (or replace) a file's text, creating the file on first use. Bumps the
    /// engine's revision, invalidating the queries that read this file and the
    /// whole-program snapshot.
    pub fn set_file(&mut self, name: &str, text: &str) -> FileId {
        let file = self.map.intern_file(name);
        if let Some(slot) = self.files.iter_mut().find(|(id, _)| *id == file) {
            slot.1 = text.to_owned();
        } else {
            self.files.push((file, text.to_owned()));
            self.engine.bump_input(QueryKey::new(kind::FILE_SET, 0));
        }
        self.engine.bump_input(source_key(file));
        // A new revision invalidates the whole-program snapshot.
        *self.snapshot.borrow_mut() = None;
        file
    }

    /// The files currently in the program, in insertion order.
    #[must_use]
    pub fn files(&self) -> Vec<FileId> {
        self.files.iter().map(|(id, _)| *id).collect()
    }

    // ------------------------------------------------------------------
    // Queries.
    // ------------------------------------------------------------------

    /// Query: the parse digest of `file`.
    ///
    /// # Errors
    ///
    /// [`tuo_db::QueryError::Cancelled`] if cancellation was requested.
    pub fn parse(&self, file: FileId) -> Result<Vec<String>, tuo_db::QueryError> {
        let value = self.engine.query(parse_key(file), self)?;
        Ok(downcast::<ParseDigest>(&value)
            .expect("parse stores a digest")
            .diagnostic_codes
            .clone())
    }

    /// Query: whether the whole program currently resolves and type-checks
    /// without error (drives the `resolution()` digest query).
    ///
    /// # Errors
    ///
    /// [`tuo_db::QueryError::Cancelled`] if cancellation was requested.
    pub fn resolution(&self) -> Result<(), tuo_db::QueryError> {
        let _ = self.engine.query(resolution_key(), self)?;
        Ok(())
    }

    /// Query: the signature type of function `symbol`, or `None` if it has no
    /// checked type (not a function, or the program did not check).
    ///
    /// # Errors
    ///
    /// [`tuo_db::QueryError::Cancelled`] if cancellation was requested.
    pub fn type_of_function(&self, symbol: SymbolId) -> Result<Option<Ty>, tuo_db::QueryError> {
        let value = self.engine.query(type_of_key(symbol), self)?;
        Ok(downcast::<SignatureType>(&value)
            .expect("type_of stores a signature")
            .0
            .clone())
    }

    /// Query: the rendered MIR of function `symbol` (empty if the program did
    /// not lower it).
    ///
    /// # Errors
    ///
    /// [`tuo_db::QueryError::Cancelled`] if cancellation was requested.
    pub fn mir_of(&self, symbol: SymbolId) -> Result<String, tuo_db::QueryError> {
        let value = self.engine.query(mir_of_key(symbol), self)?;
        Ok(downcast::<MirText>(&value)
            .expect("mir_of stores text")
            .0
            .clone())
    }

    /// Query: the semantic dependency closure of spec `symbol` — the
    /// module-level symbols its body (transitively) depends on.
    ///
    /// # Errors
    ///
    /// [`tuo_db::QueryError::Cancelled`] if cancellation was requested.
    pub fn spec_dependencies(&self, symbol: SymbolId) -> Result<Vec<SymbolId>, tuo_db::QueryError> {
        let value = self.engine.query(spec_deps_key(symbol), self)?;
        Ok(downcast::<SpecDeps>(&value)
            .expect("spec_deps stores a list")
            .0
            .iter()
            .map(|&raw| SymbolId::from_raw(raw))
            .collect())
    }

    /// Whether the current program is accepted by the front end (parses,
    /// resolves, type-checks, and ownership-checks without error). MIR is only
    /// available for an accepted program.
    #[must_use]
    pub fn is_accepted(&self) -> bool {
        self.with_snapshot(|snap| snap.accepted)
    }

    /// Run `f` with a read-only view of the current program's *semantic*
    /// results — name resolution, type checking, the source map, and every
    /// diagnostic — computed once per revision and shared by every interactive
    /// query.
    ///
    /// This is the seam through which a language server or agent protocol reads
    /// the compiler's answers: go-to-definition and rename off
    /// [`Resolution`](tuo_resolve::Resolution), hover off
    /// [`TypeckResult`](tuo_types::TypeckResult), diagnostics with their spans,
    /// and span↔position conversion through the [`SourceMap`](tuo_source::SourceMap).
    /// It is deliberately a scoped-closure accessor: the semantics borrow the
    /// memoized snapshot, so they never outlive the revision they describe.
    ///
    /// The `SourceMap` handed to `f` is the snapshot's own map — spans produced
    /// by resolution and typing are anchored to *its* [`SourceId`]s, so line/
    /// column conversion must go through this same map.
    pub fn with_semantics<R>(&self, f: impl FnOnce(Semantics<'_>) -> R) -> R {
        self.with_snapshot(|snap| {
            f(Semantics {
                map: &snap.map,
                resolution: &snap.resolution,
                types: &snap.types,
                diagnostics: &snap.diagnostics,
                accepted: snap.accepted,
            })
        })
    }

    /// Every spec symbol in the current program, in declaration order.
    #[must_use]
    pub fn spec_symbols(&self) -> Vec<SymbolId> {
        self.with_snapshot(|snap| {
            snap.resolution
                .symbols()
                .filter(|(_, symbol)| symbol.kind == SymbolKind::Spec)
                .map(|(id, _)| id)
                .collect()
        })
    }

    /// Every function symbol in the current program, in declaration order.
    #[must_use]
    pub fn function_symbols(&self) -> Vec<SymbolId> {
        self.with_snapshot(|snap| {
            snap.resolution
                .symbols()
                .filter(|(_, symbol)| symbol.kind == SymbolKind::Function)
                .map(|(id, _)| id)
                .collect()
        })
    }

    /// The symbols **defined** in `file` (declared there), for computing which
    /// specs an edit to that file may have affected.
    #[must_use]
    pub fn symbols_in_file(&self, file: FileId) -> Vec<SymbolId> {
        self.with_snapshot(|snap| symbols_defined_in(&snap.map, &snap.resolution, file))
    }

    /// Affected-spec selection: every spec whose semantic dependency closure
    /// intersects a symbol defined in one of `changed_files`, plus every spec
    /// defined in a changed file itself. Conservative — a spec is included
    /// whenever its dependencies *may* have changed.
    ///
    /// This drives `tuo verify`'s ability to run only the specs an edit could
    /// have affected. Correctness is prioritized: when the closure cannot be
    /// determined the spec is included.
    ///
    /// # Errors
    ///
    /// [`tuo_db::QueryError::Cancelled`] if cancellation was requested.
    pub fn affected_specs(
        &self,
        changed_files: &[FileId],
    ) -> Result<Vec<SymbolId>, tuo_db::QueryError> {
        // Symbols touched by the edit: everything declared in a changed file.
        let mut changed_symbols: BTreeSet<u32> = BTreeSet::new();
        for &file in changed_files {
            for symbol in self.symbols_in_file(file) {
                changed_symbols.insert(symbol.as_u32());
            }
        }

        let mut affected = Vec::new();
        for spec in self.spec_symbols() {
            let deps = self.spec_dependencies(spec)?;
            let spec_defined_in_changed = changed_symbols.contains(&spec.as_u32());
            let touches_changed = deps
                .iter()
                .any(|dep| changed_symbols.contains(&dep.as_u32()));
            if spec_defined_in_changed || touches_changed {
                affected.push(spec);
            }
        }
        Ok(affected)
    }

    // ------------------------------------------------------------------
    // Cancellation & instrumentation.
    // ------------------------------------------------------------------

    /// Request cooperative cancellation (see [`QueryEngine::request_cancellation`]).
    pub fn request_cancellation(&self) {
        self.engine.request_cancellation();
    }

    /// Clear a previously requested cancellation.
    pub fn clear_cancellation(&self) {
        self.engine.clear_cancellation();
    }

    /// The labels of every per-item query that **executed** (not a cache hit or
    /// revalidation) since the last [`IncrementalSession::clear_executions`].
    #[must_use]
    pub fn executed_queries(&self) -> Vec<String> {
        self.engine.executions()
    }

    /// Forget the recorded executions.
    pub fn clear_executions(&self) {
        self.engine.clear_executions();
    }

    // ------------------------------------------------------------------
    // Whole-program snapshot.
    // ------------------------------------------------------------------

    /// Run `f` with the current whole-program snapshot, computing it if the
    /// revision advanced since the last one.
    fn with_snapshot<R>(&self, f: impl FnOnce(&Snapshot) -> R) -> R {
        self.ensure_snapshot();
        let borrow = self.snapshot.borrow();
        f(borrow.as_ref().expect("snapshot ensured"))
    }

    /// Ensure the memoized snapshot reflects the current revision.
    fn ensure_snapshot(&self) {
        let revision = self.engine.revision();
        if self
            .snapshot
            .borrow()
            .as_ref()
            .is_some_and(|snap| snap.revision == revision)
        {
            return;
        }
        let snapshot = self.compile_whole_program(revision);
        *self.snapshot.borrow_mut() = Some(snapshot);
    }

    /// Compile the whole program from the current file texts. This is the
    /// coarse pass the per-item queries slice; it runs at most once per
    /// revision.
    fn compile_whole_program(&self, revision: u64) -> Snapshot {
        let mut map = SourceMap::new();
        let mut sources: Vec<SourceId> = Vec::new();
        for (id, text) in &self.files {
            let name = self.map.file_name(*id).to_owned();
            let file = map.intern_file(&name);
            let source = map.add_source(file, text.as_str()).expect("text fits");
            sources.push(source);
        }

        let parses: Vec<tuo_parser::ParseResult> = sources
            .iter()
            .map(|&id| tuo_parser::parse(map.source(id)))
            .collect();
        let asts: Vec<Ast<'_>> = parses
            .iter()
            .zip(&sources)
            .map(|(parse, &id)| Ast::new(&parse.tree, map.source(id).text()))
            .collect();
        let resolution = tuo_resolve::resolve(&asts);
        let types = tuo_types::check(&asts, &resolution);
        let ownership = tuo_ownership::check(&asts, &resolution, &types);

        // Collect every diagnostic, in stage order — the same sequence the
        // `check_sources` front end reports, retained for interactive hosts.
        let mut diagnostics: Vec<tuo_diagnostics::Diagnostic> = parses
            .iter()
            .flat_map(tuo_parser::ParseResult::all_diagnostics)
            .collect();
        diagnostics.extend_from_slice(resolution.diagnostics());
        diagnostics.extend_from_slice(types.diagnostics());
        diagnostics.extend_from_slice(ownership.diagnostics());
        let accepted = !has_errors(&diagnostics);

        // Lower MIR only for an accepted program (MIR is defined only then).
        let mut mir = BTreeMap::new();
        let mut fn_body = BTreeMap::new();
        if accepted {
            let hir = tuo_hir::lower(&asts, &resolution);
            let program = tuo_mir::lower(&hir, &resolution, &types);
            for function in &program.functions {
                let single = tuo_mir::Program {
                    functions: vec![function.clone()],
                    skipped: Vec::new(),
                };
                mir.insert(
                    function.symbol.as_u32(),
                    tuo_mir::render(&single, &resolution),
                );
                // Fingerprint the function's whole declaration text, so a body
                // edit to one function invalidates only its MIR.
                if let Some(text) = span_text(&map, function.span) {
                    fn_body.insert(function.symbol.as_u32(), fingerprint(text));
                }
            }
        }

        Snapshot {
            revision,
            map,
            resolution,
            types,
            mir,
            fn_body,
            accepted,
            diagnostics,
        }
    }
}

impl QueryHost for IncrementalSession {
    fn compute(&self, key: QueryKey, ctx: &QueryCtx<'_>) -> Result<Computed, tuo_db::QueryError> {
        match key.kind() {
            kind::PARSE => {
                let file = file_of(key);
                let _ = ctx.depend_on_input(source_key(file))?;
                let digest = self.with_snapshot(|snap| parse_digest(&snap.map, file));
                Ok(Computed::new(
                    stored(digest),
                    format!("parse(file#{})", file.as_raw()),
                ))
            }
            kind::INTERFACE => {
                let file = file_of(key);
                // An interface depends on the file's own parse and on resolution
                // (which assigns symbols and signatures across files).
                let _ = ctx.query(parse_key(file))?;
                let _ = ctx.query(resolution_key())?;
                let interface = self.with_snapshot(|snap| build_interface(snap, file));
                Ok(Computed::new(
                    stored(interface),
                    format!("interface(file#{})", file.as_raw()),
                ))
            }
            kind::RESOLUTION => {
                // Resolution reads every file's parse.
                let _ = ctx.depend_on_input(QueryKey::new(kind::FILE_SET, 0))?;
                for file in self.files() {
                    let _ = ctx.query(parse_key(file))?;
                }
                let digest = self.with_snapshot(build_resolution_digest);
                Ok(Computed::new(stored(digest), "resolution()".to_owned()))
            }
            kind::SPEC_GRAPH => {
                // The spec graph reads every file's parse (spec attachments and
                // dependencies are resolved across the whole program).
                let _ = ctx.depend_on_input(QueryKey::new(kind::FILE_SET, 0))?;
                for file in self.files() {
                    let _ = ctx.query(parse_key(file))?;
                }
                let digest = self.with_snapshot(build_spec_graph_digest);
                Ok(Computed::new(stored(digest), "spec_graph()".to_owned()))
            }
            kind::TYPE_OF => {
                let symbol = symbol_of(key);
                // A function's signature depends on interfaces (its own and
                // those it may reference), captured coarsely as resolution.
                let _ = ctx.query(resolution_key())?;
                for file in self.files() {
                    let _ = ctx.query(interface_key(file))?;
                }
                let ty = self.with_snapshot(|snap| snap.types.type_of(symbol).cloned());
                Ok(Computed::new(
                    stored(SignatureType(ty)),
                    format!("type_of(sym{})", symbol.as_u32()),
                ))
            }
            kind::FN_BODY => {
                let symbol = symbol_of(key);
                // The body fingerprint depends on the parse of the file the
                // function lives in (any edit to that file re-parses; early
                // cutoff on this fingerprint then limits the blast radius to
                // functions whose own text actually changed).
                if let Some(file) = self.with_snapshot(|snap| defining_file(snap, symbol)) {
                    let _ = ctx.query(parse_key(file))?;
                }
                let hash = self
                    .with_snapshot(|snap| snap.fn_body.get(&symbol.as_u32()).copied().unwrap_or(0));
                Ok(Computed::new(
                    stored(BodyHash(hash)),
                    format!("fn_body(sym{})", symbol.as_u32()),
                ))
            }
            kind::MIR_OF => {
                let symbol = symbol_of(key);
                // MIR is lowered from the function's *body*, so it depends both
                // on the function's signature (`type_of`, which folds in the
                // signatures of callees it may reference) and on the fingerprint
                // of the function's own body text — a body-only edit changes the
                // body fingerprint but not the signature, and must still re-lower
                // this MIR while leaving sibling functions' MIR untouched.
                let _ = ctx.query(type_of_key(symbol))?;
                let _ = ctx.query(body_key(symbol))?;
                let text = self.with_snapshot(|snap| {
                    snap.mir.get(&symbol.as_u32()).cloned().unwrap_or_default()
                });
                Ok(Computed::new(
                    stored(MirText(text)),
                    format!("mir_of(sym{})", symbol.as_u32()),
                ))
            }
            kind::SPEC_DEPS => {
                let symbol = symbol_of(key);
                // A spec's dependency graph depends on the whole-program spec
                // graph (attachment and dependency discovery) and on the
                // type-check of each function in its closure. It depends on the
                // spec-graph digest, *not* the resolution digest, so a change to
                // which symbols a spec references does not touch function typing.
                let _ = ctx.query(spec_graph_key())?;
                let closure = self.with_snapshot(|snap| spec_closure(&snap.resolution, symbol));
                for &dep in &closure {
                    let dep_sym = SymbolId::from_raw(dep);
                    if self.with_snapshot(|snap| {
                        snap.resolution.symbol(dep_sym).kind == SymbolKind::Function
                    }) {
                        let _ = ctx.query(type_of_key(dep_sym))?;
                    }
                }
                Ok(Computed::new(
                    stored(SpecDeps(closure)),
                    format!("spec_deps(sym{})", symbol.as_u32()),
                ))
            }
            other => unreachable!("IncrementalSession has no query of kind {other}"),
        }
    }
}

// ----------------------------------------------------------------------
// Slicing helpers.
// ----------------------------------------------------------------------

/// Whether any diagnostic is an error.
fn has_errors(diagnostics: &[tuo_diagnostics::Diagnostic]) -> bool {
    diagnostics
        .iter()
        .any(|d| d.severity == tuo_diagnostics::Severity::Error)
}

/// The parse digest of one file: a content fingerprint plus its parse
/// diagnostics' codes.
fn parse_digest(map: &SourceMap, file: FileId) -> ParseDigest {
    let Some(source) = map.latest(file) else {
        return ParseDigest {
            content: 0,
            diagnostic_codes: Vec::new(),
        };
    };
    let text = map.source(source);
    let parse = tuo_parser::parse(text);
    ParseDigest {
        content: fingerprint(text.text()),
        diagnostic_codes: parse
            .all_diagnostics()
            .iter()
            .map(|d| d.code.to_string())
            .collect(),
    }
}

/// A stable content fingerprint of a string (deterministic across runs — the
/// default hasher is not, so we fix the algorithm here).
fn fingerprint(text: &str) -> u64 {
    // FNV-1a: deterministic, dependency-free, and fine for change detection.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// A stable numeric tag for a symbol kind (for storing in comparable values).
fn kind_tag(kind: SymbolKind) -> u8 {
    match kind {
        SymbolKind::Module => 0,
        SymbolKind::Function => 1,
        SymbolKind::Struct => 2,
        SymbolKind::Enum => 3,
        SymbolKind::Variant => 4,
        SymbolKind::Interface => 5,
        SymbolKind::Const => 6,
        SymbolKind::Spec => 7,
        SymbolKind::TypeParam => 8,
        SymbolKind::Param => 9,
        SymbolKind::Local => 10,
    }
}

/// The source text a span covers, or `None` if it cannot be located.
fn span_text(map: &SourceMap, span: tuo_source::Span) -> Option<&str> {
    let text = map.get_source(span.source())?;
    let range = span.range();
    text.text()
        .get(range.start().as_usize()..range.end().as_usize())
}

/// The file a symbol is declared in, if it has a written declaration.
fn defining_file(snap: &Snapshot, symbol: SymbolId) -> Option<FileId> {
    snap.resolution
        .symbol(symbol)
        .declaration
        .and_then(|span| snap.map.get_source(span.source()))
        .map(|text| text.file())
}

/// The symbols declared in `file` — those whose declaration span is in `file`.
fn symbols_defined_in(map: &SourceMap, resolution: &Resolution, file: FileId) -> Vec<SymbolId> {
    resolution
        .symbols()
        .filter(|(_, symbol)| {
            symbol
                .declaration
                .and_then(|span| map.get_source(span.source()))
                .is_some_and(|text| text.file() == file)
        })
        .map(|(id, _)| id)
        .collect()
}

/// A file's signature-level interface: the module-level items declared in it,
/// with their signature types but **not** their bodies.
fn build_interface(snap: &Snapshot, file: FileId) -> Interface {
    let mut items: Vec<InterfaceItem> = symbols_defined_in(&snap.map, &snap.resolution, file)
        .into_iter()
        .filter(|&id| {
            matches!(
                snap.resolution.symbol(id).kind,
                SymbolKind::Function
                    | SymbolKind::Struct
                    | SymbolKind::Enum
                    | SymbolKind::Variant
                    | SymbolKind::Interface
                    | SymbolKind::Const
            )
        })
        .map(|id| {
            let symbol = snap.resolution.symbol(id);
            InterfaceItem {
                symbol: id.as_u32(),
                name: symbol.name.clone(),
                kind_tag: kind_tag(symbol.kind),
                is_pub: symbol.is_pub,
                signature: snap.types.type_of(id).cloned(),
            }
        })
        .collect();
    items.sort_by_key(|item| item.symbol);
    Interface { items }
}

/// The whole-program resolution digest (symbol structure only).
fn build_resolution_digest(snap: &Snapshot) -> ResolutionDigest {
    let symbols = snap
        .resolution
        .symbols()
        .map(|(id, symbol)| (id.as_u32(), symbol.name.clone(), kind_tag(symbol.kind)))
        .collect();
    ResolutionDigest { symbols }
}

/// The whole-program spec dependency graph digest.
fn build_spec_graph_digest(snap: &Snapshot) -> SpecGraphDigest {
    let mut graph: Vec<(u32, u32, Vec<u32>)> = snap
        .resolution
        .symbols()
        .filter(|(_, symbol)| symbol.kind == SymbolKind::Spec)
        .map(|(spec, _)| {
            let target = snap
                .resolution
                .target_of(spec)
                .map_or(u32::MAX, |t| t.as_u32());
            let deps = snap
                .resolution
                .dependencies_of(spec)
                .iter()
                .map(|d| d.as_u32())
                .collect();
            (spec.as_u32(), target, deps)
        })
        .collect();
    graph.sort_by_key(|(spec, _, _)| *spec);
    SpecGraphDigest { graph }
}

/// A spec's transitive semantic dependency closure: seed with
/// [`Resolution::dependencies_of`], then follow each dependency's own
/// dependencies (a function may call another). Sorted, deduplicated raws.
fn spec_closure(resolution: &Resolution, spec: SymbolId) -> Vec<u32> {
    let mut seen: BTreeSet<u32> = BTreeSet::new();
    let mut stack: Vec<SymbolId> = resolution.dependencies_of(spec).to_vec();
    if let Some(target) = resolution.target_of(spec) {
        stack.push(target);
    }
    while let Some(symbol) = stack.pop() {
        if !seen.insert(symbol.as_u32()) {
            continue;
        }
        // Follow the callee's own dependencies. Only functions carry a body
        // that references further items; other kinds are leaves here.
        for &next in resolution.dependencies_of(symbol) {
            if !seen.contains(&next.as_u32()) {
                stack.push(next);
            }
        }
    }
    seen.into_iter().collect()
}

// ----------------------------------------------------------------------
// Key constructors and decoders.
// ----------------------------------------------------------------------

fn source_key(file: FileId) -> QueryKey {
    QueryKey::new(kind::SOURCE, u64::from(file.as_raw()))
}

fn parse_key(file: FileId) -> QueryKey {
    QueryKey::new(kind::PARSE, u64::from(file.as_raw()))
}

fn interface_key(file: FileId) -> QueryKey {
    QueryKey::new(kind::INTERFACE, u64::from(file.as_raw()))
}

fn resolution_key() -> QueryKey {
    QueryKey::new(kind::RESOLUTION, 0)
}

fn spec_graph_key() -> QueryKey {
    QueryKey::new(kind::SPEC_GRAPH, 0)
}

fn type_of_key(symbol: SymbolId) -> QueryKey {
    QueryKey::new(kind::TYPE_OF, u64::from(symbol.as_u32()))
}

fn mir_of_key(symbol: SymbolId) -> QueryKey {
    QueryKey::new(kind::MIR_OF, u64::from(symbol.as_u32()))
}

fn spec_deps_key(symbol: SymbolId) -> QueryKey {
    QueryKey::new(kind::SPEC_DEPS, u64::from(symbol.as_u32()))
}

fn body_key(symbol: SymbolId) -> QueryKey {
    QueryKey::new(kind::FN_BODY, u64::from(symbol.as_u32()))
}

fn file_of(key: QueryKey) -> FileId {
    FileId::from_raw(u32::try_from(key.arg()).expect("file id fits"))
}

fn symbol_of(key: QueryKey) -> SymbolId {
    SymbolId::from_raw(u32::try_from(key.arg()).expect("symbol id fits"))
}
