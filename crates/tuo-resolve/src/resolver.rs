//! The resolution pass: two collection phases (declarations, then imports)
//! followed by a scoped walk of every body.
//!
//! Phase order is what makes **forward references** work: every module-level
//! declaration in every file is collected before any path is resolved, so a
//! function may freely call one declared later (or in another file of the
//! same module).
//!
//! Names whose meaning depends on *types* — field accesses, method calls,
//! struct-literal field names, `Self`, and path tails hanging off a value —
//! are deliberately **skipped, not errored**: resolving them is the type
//! checker's job, and guessing here would produce false diagnostics.

use std::collections::HashMap;

use tuo_ast::{
    Ast, BindingStmt, Block, ElseBranch, Expr, FnDecl, GenericParams, ImportDecl, Item, Name,
    Pattern, SpecDecl, SpecStatement, Statement, TypeArgs, TypePath, TypeRef,
};
use tuo_diagnostics::{Diagnostic, DiagnosticCode, Namespace, StructuredValue};
use tuo_source::Span;

use crate::ids::{ModuleId, SymbolId};
use crate::resolution::{ModuleInfo, Resolution};
use crate::symbol::{Reference, SpecTarget, Symbol, SymbolKind};

/// Type names the language provides without a declaration: the fixed
/// primitive set of Constitution §10 (with the `Int`/`Float` aliases of §11)
/// plus the builtin `Array` type constructor.
const BUILTIN_TYPES: &[&str] = &[
    "I8", "I16", "I32", "I64", "Isize", "U8", "U16", "U32", "U64", "Usize", "F32", "F64", "Int",
    "Float", "Bool", "Char", "String", "Str", "Array",
];

/// Is `name` a builtin type name (resolved silently, without a symbol)?
///
/// Later stages (HIR lowering, type checking) use this to give the same
/// meaning to builtin names that resolution did.
#[must_use]
pub fn is_builtin_type(name: &str) -> bool {
    BUILTIN_TYPES.contains(&name)
}

/// The resolver's diagnostic codes (the reserved `Rxxxx` namespace).
///
/// - `R0001` — duplicate definition.
/// - `R0002` — undefined name.
/// - `R0003` — unresolved import.
/// - `R0004` — ambiguous name.
/// - `R0005` — private item accessed from outside its module.
/// - `R0006` — spec target is not a function.
fn code(number: u16) -> DiagnosticCode {
    DiagnosticCode::new(Namespace::Resolution, number)
}

/// Resolve every name in `files` — one parsed program snapshot — into a
/// [`Resolution`] of stable semantic IDs.
///
/// Files of the same declared `module` path share one module scope; files
/// without a `module` declaration share the root module. The result is
/// deterministic: equal inputs produce equal symbols, IDs, references, and
/// diagnostics.
#[must_use]
pub fn resolve(files: &[Ast<'_>]) -> Resolution {
    let mut resolver = Resolver::default();
    resolver.install_prelude();
    let modules: Vec<ModuleId> = files
        .iter()
        .map(|ast| resolver.collect_file(*ast))
        .collect();
    for (ast, module) in files.iter().zip(&modules) {
        resolver.resolve_imports(*ast, *module);
    }
    for (ast, module) in files.iter().zip(&modules) {
        resolver.resolve_file(*ast, *module);
    }
    resolver.out
}

/// What a module-scope name is bound to.
enum Binding {
    /// A declaration in the module itself.
    Decl(SymbolId),
    /// An `import` binding: the targets it may refer to, each with the span
    /// of the import that bound it. More than one distinct target makes the
    /// name ambiguous at every use.
    Import(Vec<(Target, Span)>),
}

/// What a path segment resolves to while navigating.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    Symbol(SymbolId),
    Module(ModuleId),
}

/// The outcome of looking a name up in one module scope.
enum Hit {
    Symbol(SymbolId),
    Module(ModuleId),
    /// Ambiguous import — the diagnostic has already been emitted.
    Ambiguous,
    Missing,
}

/// Whether a path is being resolved where a value or a type is expected
/// (only builtin type names behave differently).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pos {
    Value,
    Type,
}

/// The lexical scope stack for one body walk, rooted at a module.
struct Scopes {
    module: ModuleId,
    stack: Vec<HashMap<String, SymbolId>>,
}

impl Scopes {
    fn new(module: ModuleId) -> Self {
        Self {
            module,
            stack: Vec::new(),
        }
    }

    fn push(&mut self) {
        self.stack.push(HashMap::new());
    }

    fn pop(&mut self) {
        self.stack.pop();
    }

    fn lookup(&self, name: &str) -> Option<SymbolId> {
        self.stack
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
    }

    fn top(&mut self) -> &mut HashMap<String, SymbolId> {
        self.stack.last_mut().expect("a scope has been pushed")
    }
}

#[derive(Default)]
struct Resolver {
    out: Resolution,
    /// Per-module name bindings, parallel to `out.modules`.
    module_scopes: Vec<HashMap<String, Binding>>,
    /// Full module path → module, including implicit prefix modules.
    by_path: HashMap<Vec<String>, ModuleId>,
    /// Enum symbol → its variants by name.
    enum_variants: HashMap<SymbolId, HashMap<String, SymbolId>>,
    /// Spec block span → the spec's own symbol (phase 1 → phase 3).
    spec_symbols: HashMap<Span, SymbolId>,
}

impl Resolver {
    // ------------------------------------------------------------------
    // Arena plumbing
    // ------------------------------------------------------------------

    fn new_symbol(
        &mut self,
        name: &str,
        kind: SymbolKind,
        module: ModuleId,
        is_pub: bool,
        declaration: Option<Span>,
    ) -> SymbolId {
        let id = SymbolId::from_raw(
            u32::try_from(self.out.symbols.len()).expect("symbol count fits in u32"),
        );
        self.out.symbols.push(Symbol {
            name: name.to_owned(),
            kind,
            module,
            is_pub,
            declaration,
        });
        id
    }

    fn sym(&self, id: SymbolId) -> &Symbol {
        &self.out.symbols[id.as_u32() as usize]
    }

    fn scope(&self, module: ModuleId) -> &HashMap<String, Binding> {
        &self.module_scopes[module.as_u32() as usize]
    }

    fn scope_mut(&mut self, module: ModuleId) -> &mut HashMap<String, Binding> {
        &mut self.module_scopes[module.as_u32() as usize]
    }

    fn record(&mut self, name: Name<'_>, symbol: SymbolId) {
        self.out.references.push(Reference {
            span: name.span,
            symbol,
        });
    }

    /// The module's displayable name for diagnostics.
    fn module_name(&self, module: ModuleId) -> String {
        let path = &self.out.modules[module.as_u32() as usize].path;
        if path.is_empty() {
            "the root module".to_owned()
        } else {
            format!("`{}`", path.join("::"))
        }
    }

    // ------------------------------------------------------------------
    // The prelude
    // ------------------------------------------------------------------

    /// Install the language prelude: the canonical `Option`/`Result` enums
    /// (Constitution §14, §15) with their variants `Some`/`None`/`Ok`/`Err`
    /// bound directly, so patterns like `Some { value }` resolve without a
    /// qualifying path. Prelude names live in a synthetic `$prelude` module
    /// (unreachable from source, since `$` cannot start an identifier) and
    /// are consulted *after* every user scope, so declarations shadow them.
    fn install_prelude(&mut self) {
        let prelude_path = vec!["$prelude".to_owned()];
        let module = self.ensure_module(&prelude_path, None);
        for (enum_name, variant_names) in [("Option", ["Some", "None"]), ("Result", ["Ok", "Err"])]
        {
            let id = self.new_symbol(enum_name, SymbolKind::Enum, module, true, None);
            self.out.prelude.push((enum_name.to_owned(), id));
            let mut by_name: HashMap<String, SymbolId> = HashMap::new();
            let mut ordered = Vec::new();
            for variant_name in variant_names {
                let variant =
                    self.new_symbol(variant_name, SymbolKind::Variant, module, true, None);
                by_name.insert(variant_name.to_owned(), variant);
                ordered.push(variant);
                self.out.prelude.push((variant_name.to_owned(), variant));
            }
            self.enum_variants.insert(id, by_name);
            self.out.variants.insert(id, ordered);
        }
    }

    /// The prelude symbol of `name`, if any.
    fn prelude_lookup(&self, name: &str) -> Option<SymbolId> {
        self.out
            .prelude
            .iter()
            .find(|(entry, _)| entry == name)
            .map(|(_, id)| *id)
    }

    // ------------------------------------------------------------------
    // Phase 1: modules and module-level declarations
    // ------------------------------------------------------------------

    fn collect_file(&mut self, ast: Ast<'_>) -> ModuleId {
        let file = ast.file();
        let (path, decl): (Vec<String>, Option<Span>) = match file.module().and_then(|m| m.path()) {
            Some(p) => {
                let names: Vec<Name<'_>> = p.segment_names().collect();
                (
                    names.iter().map(|n| n.text.to_owned()).collect(),
                    names.last().map(|n| n.span),
                )
            }
            None => (Vec::new(), None),
        };
        let module = self.ensure_module(&path, decl);
        for item in file.items() {
            self.collect_item(module, item);
        }
        module
    }

    /// Get or create the module at `path`, creating every implicit prefix
    /// module along the way.
    fn ensure_module(&mut self, path: &[String], decl: Option<Span>) -> ModuleId {
        for end in 1..path.len() {
            self.ensure_one(&path[..end], None);
        }
        self.ensure_one(path, decl)
    }

    fn ensure_one(&mut self, path: &[String], decl: Option<Span>) -> ModuleId {
        if let Some(&id) = self.by_path.get(path) {
            if decl.is_some() {
                let symbol = self.out.modules[id.as_u32() as usize].symbol;
                let data = &mut self.out.symbols[symbol.as_u32() as usize];
                if data.declaration.is_none() {
                    data.declaration = decl;
                }
            }
            return id;
        }
        let id = ModuleId::from_raw(
            u32::try_from(self.out.modules.len()).expect("module count fits in u32"),
        );
        let name = path.last().map_or("", String::as_str);
        let symbol = self.new_symbol(name, SymbolKind::Module, id, true, decl);
        self.out.modules.push(ModuleInfo {
            path: path.to_vec(),
            symbol,
        });
        self.module_scopes.push(HashMap::new());
        self.by_path.insert(path.to_vec(), id);
        id
    }

    fn collect_item(&mut self, module: ModuleId, item: Item<'_>) {
        match item {
            Item::Import(_) | Item::Impl(_) | Item::Error(_) => {}
            Item::Fn(decl) => {
                self.declare(module, decl.name_ref(), SymbolKind::Function, decl.is_pub());
            }
            Item::Struct(decl) => {
                self.declare(module, decl.name_ref(), SymbolKind::Struct, decl.is_pub());
            }
            Item::Enum(decl) => self.collect_enum(module, decl),
            Item::Interface(decl) => {
                self.declare(
                    module,
                    decl.name_ref(),
                    SymbolKind::Interface,
                    decl.is_pub(),
                );
            }
            Item::Const(decl) => {
                self.declare(module, decl.name_ref(), SymbolKind::Const, decl.is_pub());
            }
            Item::Spec(decl) => self.collect_spec(module, decl),
        }
    }

    fn collect_enum(&mut self, module: ModuleId, decl: tuo_ast::EnumDecl<'_>) {
        let Some(id) = self.declare(module, decl.name_ref(), SymbolKind::Enum, decl.is_pub())
        else {
            return;
        };
        let mut variants: HashMap<String, SymbolId> = HashMap::new();
        let mut ordered: Vec<SymbolId> = Vec::new();
        for variant in decl.variants() {
            let Some(name) = variant.name_ref() else {
                continue;
            };
            if let Some(&previous) = variants.get(name.text) {
                let previous = self.sym(previous).declaration;
                self.duplicate_definition(name, previous);
                continue;
            }
            let symbol = self.new_symbol(
                name.text,
                SymbolKind::Variant,
                module,
                decl.is_pub(),
                Some(name.span),
            );
            variants.insert(name.text.to_owned(), symbol);
            ordered.push(symbol);
        }
        self.enum_variants.insert(id, variants);
        self.out.variants.insert(id, ordered);
    }

    fn collect_spec(&mut self, module: ModuleId, decl: SpecDecl<'_>) {
        // Spec names are *targets*, not bindings: a `spec classify { … }`
        // never collides with `fn classify` and cannot be referenced.
        let target = decl.target_name();
        let name = decl.name().unwrap_or_default();
        let symbol = self.new_symbol(
            name,
            SymbolKind::Spec,
            module,
            false,
            target.map(|n| n.span),
        );
        if let Some(span) = decl.span() {
            self.spec_symbols.insert(span, symbol);
            // Expose the spec's own symbol on the resolution so tooling (the
            // spec runner) can name a spec's identity by its block span —
            // free-standing string-named specs have no `SpecTarget` entry.
            self.out.spec_symbols.insert(span, symbol);
        }
    }

    /// Bind a declaration in the module scope, reporting `R0001` (and
    /// creating no symbol) on collision.
    fn declare(
        &mut self,
        module: ModuleId,
        name: Option<Name<'_>>,
        kind: SymbolKind,
        is_pub: bool,
    ) -> Option<SymbolId> {
        let name = name?;
        if let Some(existing) = self.scope(module).get(name.text) {
            let previous = match existing {
                Binding::Decl(id) => self.sym(*id).declaration,
                Binding::Import(targets) => targets.first().map(|(_, span)| *span),
            };
            self.duplicate_definition(name, previous);
            return None;
        }
        let id = self.new_symbol(name.text, kind, module, is_pub, Some(name.span));
        self.scope_mut(module)
            .insert(name.text.to_owned(), Binding::Decl(id));
        Some(id)
    }

    // ------------------------------------------------------------------
    // Phase 2: imports
    // ------------------------------------------------------------------

    fn resolve_imports(&mut self, ast: Ast<'_>, module: ModuleId) {
        for item in ast.file().items() {
            if let Item::Import(import) = item {
                self.resolve_import(module, import);
            }
        }
    }

    fn resolve_import(&mut self, module: ModuleId, import: ImportDecl<'_>) {
        let Some(path) = import.path() else {
            return;
        };
        let segments: Vec<Name<'_>> = path.segment_names().collect();
        let Some((&last, prefix)) = segments.split_last() else {
            return;
        };
        let leaves: Vec<_> = import.leaves().collect();
        if leaves.is_empty() {
            let Some(target) = self.resolve_import_target(module, prefix, last) else {
                return;
            };
            let bind = import.alias_name().unwrap_or(last);
            self.bind_import(module, bind, target);
        } else {
            let Some(source) = self.resolve_module_prefix(module, &segments) else {
                return;
            };
            for leaf in leaves {
                let Some(name) = leaf.name_ref() else {
                    continue;
                };
                let Some(target) = self.import_lookup(module, source, name) else {
                    continue;
                };
                let bind = leaf.alias_name().unwrap_or(name);
                self.bind_import(module, bind, target);
            }
        }
    }

    /// Resolve `prefix::last` for a plain import: the prefix must be a chain
    /// of modules; the final segment may be a module or an item.
    fn resolve_import_target(
        &mut self,
        importing: ModuleId,
        prefix: &[Name<'_>],
        last: Name<'_>,
    ) -> Option<Target> {
        let Some((&first, rest)) = prefix.split_first() else {
            // `import name;` — a top-level module.
            let module = self.top_level_module(last)?;
            return Some(Target::Module(module));
        };
        let mut current = self.top_level_module(first)?;
        for &segment in rest {
            current = self.submodule(current, segment)?;
        }
        self.import_lookup(importing, current, last)
    }

    /// Resolve a whole import path as a chain of modules (the group form's
    /// prefix).
    fn resolve_module_prefix(
        &mut self,
        _importing: ModuleId,
        segments: &[Name<'_>],
    ) -> Option<ModuleId> {
        let (&first, rest) = segments.split_first()?;
        let mut current = self.top_level_module(first)?;
        for &segment in rest {
            current = self.submodule(current, segment)?;
        }
        Some(current)
    }

    /// A top-level module by name, or `R0003`.
    fn top_level_module(&mut self, name: Name<'_>) -> Option<ModuleId> {
        if let Some(&module) = self
            .by_path
            .get(std::slice::from_ref(&name.text.to_owned()))
        {
            let symbol = self.out.modules[module.as_u32() as usize].symbol;
            self.record(name, symbol);
            return Some(module);
        }
        self.out.diagnostics.push(
            Diagnostic::error(
                code(3),
                format!("unresolved import: module `{}` does not exist", name.text),
                name.span,
            )
            .with_primary_label("no such module")
            .with_actual(StructuredValue::Name(name.text.to_owned())),
        );
        None
    }

    /// The direct submodule `parent::name`, or `R0003`.
    fn submodule(&mut self, parent: ModuleId, name: Name<'_>) -> Option<ModuleId> {
        if let Some(module) = self.submodule_of(parent, name.text) {
            let symbol = self.out.modules[module.as_u32() as usize].symbol;
            self.record(name, symbol);
            return Some(module);
        }
        let parent_name = self.module_name(parent);
        self.out.diagnostics.push(
            Diagnostic::error(
                code(3),
                format!(
                    "unresolved import: no module `{}` in {parent_name}",
                    name.text
                ),
                name.span,
            )
            .with_primary_label("no such module")
            .with_actual(StructuredValue::Name(name.text.to_owned())),
        );
        None
    }

    fn submodule_of(&self, parent: ModuleId, name: &str) -> Option<ModuleId> {
        let mut path = self.out.modules[parent.as_u32() as usize].path.clone();
        path.push(name.to_owned());
        self.by_path.get(&path).copied()
    }

    /// Look `name` up inside `source` for an import: a declared item
    /// (visibility-checked) or a submodule. Imports inside `source` are not
    /// re-exported.
    fn import_lookup(
        &mut self,
        importing: ModuleId,
        source: ModuleId,
        name: Name<'_>,
    ) -> Option<Target> {
        match self.scope(source).get(name.text) {
            Some(Binding::Decl(symbol)) => {
                let symbol = *symbol;
                self.check_visible(importing, symbol, name);
                self.record(name, symbol);
                Some(Target::Symbol(symbol))
            }
            Some(Binding::Import(_)) => {
                let source_name = self.module_name(source);
                self.out.diagnostics.push(
                    Diagnostic::error(
                        code(3),
                        format!("unresolved import: no `{}` in {source_name}", name.text),
                        name.span,
                    )
                    .with_primary_label("only an import, not a declaration")
                    .with_note("imports are not re-exported; import from the declaring module")
                    .with_actual(StructuredValue::Name(name.text.to_owned())),
                );
                None
            }
            None => {
                if let Some(module) = self.submodule_of(source, name.text) {
                    let symbol = self.out.modules[module.as_u32() as usize].symbol;
                    self.record(name, symbol);
                    return Some(Target::Module(module));
                }
                let source_name = self.module_name(source);
                self.out.diagnostics.push(
                    Diagnostic::error(
                        code(3),
                        format!("unresolved import: no `{}` in {source_name}", name.text),
                        name.span,
                    )
                    .with_primary_label("not found in the imported module")
                    .with_actual(StructuredValue::Name(name.text.to_owned())),
                );
                None
            }
        }
    }

    /// Bind an import in the module scope. Colliding with a declaration is
    /// `R0001`; a second import of the same name records an extra target,
    /// making every *use* ambiguous (`R0004`).
    fn bind_import(&mut self, module: ModuleId, bind: Name<'_>, target: Target) {
        match self.scope(module).get(bind.text) {
            Some(Binding::Decl(previous)) => {
                let previous = self.sym(*previous).declaration;
                self.duplicate_definition(bind, previous);
            }
            Some(Binding::Import(_)) => {
                if let Some(Binding::Import(targets)) = self.scope_mut(module).get_mut(bind.text)
                    && !targets.iter().any(|(existing, _)| *existing == target)
                {
                    targets.push((target, bind.span));
                }
            }
            None => {
                self.scope_mut(module).insert(
                    bind.text.to_owned(),
                    Binding::Import(vec![(target, bind.span)]),
                );
            }
        }
    }

    // ------------------------------------------------------------------
    // Phase 3: bodies
    // ------------------------------------------------------------------

    fn resolve_file(&mut self, ast: Ast<'_>, module: ModuleId) {
        for item in ast.file().items() {
            match item {
                Item::Import(_) | Item::Error(_) => {}
                Item::Fn(decl) => {
                    let mut scopes = Scopes::new(module);
                    self.walk_fn(&mut scopes, decl);
                }
                Item::Struct(decl) => {
                    let mut scopes = Scopes::new(module);
                    scopes.push();
                    self.declare_generics(&mut scopes, decl.generics());
                    for field in decl.fields() {
                        if let Some(ty) = field.ty() {
                            self.walk_type(&mut scopes, ty);
                        }
                    }
                    self.walk_where(&mut scopes, decl.where_clause());
                    scopes.pop();
                }
                Item::Enum(decl) => {
                    let mut scopes = Scopes::new(module);
                    scopes.push();
                    self.declare_generics(&mut scopes, decl.generics());
                    for variant in decl.variants() {
                        for field in variant.fields() {
                            if let Some(ty) = field.ty() {
                                self.walk_type(&mut scopes, ty);
                            }
                        }
                    }
                    scopes.pop();
                }
                Item::Interface(decl) => {
                    let mut scopes = Scopes::new(module);
                    scopes.push();
                    self.declare_generics(&mut scopes, decl.generics());
                    for member in decl.members() {
                        self.walk_fn(&mut scopes, member);
                    }
                    scopes.pop();
                }
                Item::Impl(decl) => {
                    let mut scopes = Scopes::new(module);
                    scopes.push();
                    self.declare_generics(&mut scopes, decl.generics());
                    if let Some(interface) = decl.interface() {
                        self.resolve_type_path(&scopes, interface);
                    }
                    if let Some(args) = decl.interface_args() {
                        self.walk_type_args(&mut scopes, args);
                    }
                    if let Some(target) = decl.target() {
                        self.walk_type(&mut scopes, target);
                    }
                    self.walk_where(&mut scopes, decl.where_clause());
                    for function in decl.functions() {
                        self.walk_fn(&mut scopes, function);
                    }
                    scopes.pop();
                }
                Item::Const(decl) => {
                    let mut scopes = Scopes::new(module);
                    if let Some(ty) = decl.ty() {
                        self.walk_type(&mut scopes, ty);
                    }
                    if let Some(value) = decl.value() {
                        self.walk_expr(&mut scopes, value);
                    }
                }
                Item::Spec(decl) => self.walk_spec(module, decl),
            }
        }
    }

    fn walk_fn(&mut self, scopes: &mut Scopes, decl: FnDecl<'_>) {
        scopes.push(); // generic parameters
        self.declare_generics(scopes, decl.generics());
        scopes.push(); // parameters
        for param in decl.params() {
            if let Some(ty) = param.ty() {
                self.walk_type(scopes, ty);
            }
            if let Some(name) = param.name_ref() {
                self.declare_scoped_strict(scopes, name, SymbolKind::Param);
            }
        }
        if let Some(ret) = decl.return_type() {
            self.walk_type(scopes, ret);
        }
        self.walk_where(scopes, decl.where_clause());
        if let Some(body) = decl.body() {
            self.walk_block(scopes, body);
        }
        scopes.pop();
        scopes.pop();
    }

    fn declare_generics(&mut self, scopes: &mut Scopes, generics: Option<GenericParams<'_>>) {
        let Some(generics) = generics else {
            return;
        };
        for param in generics.params() {
            if let Some(name) = param.name_ref() {
                self.declare_scoped_strict(scopes, name, SymbolKind::TypeParam);
            }
            for bound in param.bounds() {
                self.resolve_type_path(scopes, bound);
            }
        }
    }

    fn walk_where(&mut self, scopes: &mut Scopes, clause: Option<tuo_ast::WhereClause<'_>>) {
        let Some(clause) = clause else {
            return;
        };
        for pred in clause.preds() {
            if let Some(ty) = pred.ty() {
                self.walk_type(scopes, ty);
            }
            for bound in pred.bounds() {
                self.resolve_type_path(scopes, bound);
            }
        }
    }

    fn walk_spec(&mut self, module: ModuleId, decl: SpecDecl<'_>) {
        let extent = decl.span();
        let spec = extent
            .and_then(|span| self.spec_symbols.get(&span))
            .copied();
        let first_reference = self.out.references.len();
        if let Some(name) = decl.target_name() {
            self.attach_spec(module, name, spec);
        }
        let mut scopes = Scopes::new(module);
        scopes.push();
        for statement in decl.statements() {
            match statement {
                SpecStatement::Given(clause) => {
                    for binding in clause.bindings() {
                        if let Some(ty) = binding.ty() {
                            self.walk_type(&mut scopes, ty);
                        }
                        if let Some(init) = binding.initializer() {
                            self.walk_expr(&mut scopes, init);
                        }
                        if let Some(name) = binding.name_ref() {
                            self.declare_scoped_strict(&mut scopes, name, SymbolKind::Local);
                        }
                    }
                }
                SpecStatement::When(clause) => {
                    if let Some(binding) = clause.binding() {
                        self.walk_binding_stmt(&mut scopes, binding);
                    } else if let Some(expr) = clause.expr() {
                        self.walk_expr(&mut scopes, expr);
                    }
                }
                SpecStatement::Then(clause) | SpecStatement::Assert(clause) => {
                    if let Some(expr) = clause.expr() {
                        self.walk_expr(&mut scopes, expr);
                    }
                }
                SpecStatement::Let(binding) | SpecStatement::Var(binding) => {
                    self.walk_binding_stmt(&mut scopes, binding);
                }
                SpecStatement::Expr(statement) => {
                    if let Some(expr) = statement.expr() {
                        self.walk_expr(&mut scopes, expr);
                    }
                }
                SpecStatement::Error(_) => {}
            }
        }
        scopes.pop();
        if let Some(spec) = spec {
            let deps = self.spec_dependencies(first_reference, extent);
            self.out.spec_deps.insert(spec, deps);
        }
    }

    /// The module-level items a spec block referenced: every reference
    /// recorded since `first_reference` (the target name included) whose
    /// symbol is an item declared outside the block, deduplicated in
    /// first-use order. See ADR-0002 "Dependency discovery".
    fn spec_dependencies(&self, first_reference: usize, extent: Option<Span>) -> Vec<SymbolId> {
        let mut deps: Vec<SymbolId> = Vec::new();
        for reference in &self.out.references[first_reference..] {
            let symbol = self.sym(reference.symbol);
            let is_item = matches!(
                symbol.kind,
                SymbolKind::Function
                    | SymbolKind::Struct
                    | SymbolKind::Enum
                    | SymbolKind::Variant
                    | SymbolKind::Interface
                    | SymbolKind::Const
            );
            let declared_inside = match (symbol.declaration, extent) {
                (Some(declaration), Some(extent)) => {
                    declaration.source() == extent.source()
                        && extent.range().contains_range(declaration.range())
                }
                _ => false,
            };
            if is_item && !declared_inside && !deps.contains(&reference.symbol) {
                deps.push(reference.symbol);
            }
        }
        deps
    }

    /// Resolve an identifier-named spec's target to the module-level
    /// function of that name — the same symbol calls resolve to.
    fn attach_spec(&mut self, module: ModuleId, name: Name<'_>, spec: Option<SymbolId>) {
        match self.lookup_module_scope(module, name) {
            // `lookup_module_scope` already recorded the reference.
            Hit::Symbol(target) if self.sym(target).kind == SymbolKind::Function => {
                if let Some(spec) = spec {
                    self.out.spec_targets.push(SpecTarget { spec, target });
                }
            }
            Hit::Symbol(target) => {
                let noun = self.sym(target).kind.noun();
                let mut diagnostic = Diagnostic::error(
                    code(6),
                    format!("spec target `{}` is a {noun}, not a function", name.text),
                    name.span,
                )
                .with_primary_label("must name a function")
                .with_actual(StructuredValue::Name(name.text.to_owned()));
                if let Some(declaration) = self.sym(target).declaration {
                    diagnostic = diagnostic
                        .with_secondary_label(declaration, format!("{noun} declared here"));
                }
                self.out.diagnostics.push(diagnostic);
            }
            Hit::Module(_) => {
                self.out.diagnostics.push(
                    Diagnostic::error(
                        code(6),
                        format!("spec target `{}` is a module, not a function", name.text),
                        name.span,
                    )
                    .with_primary_label("must name a function"),
                );
            }
            Hit::Ambiguous => {}
            Hit::Missing => {
                self.out.diagnostics.push(
                    Diagnostic::error(
                        code(2),
                        format!("cannot find `{}` in this scope", name.text),
                        name.span,
                    )
                    .with_primary_label("not found")
                    .with_help(
                        "an identifier-named spec targets a module-level function; \
                         use a string name for a free-standing spec",
                    )
                    .with_actual(StructuredValue::Name(name.text.to_owned())),
                );
            }
        }
    }

    // ------------------------------------------------------------------
    // Statements, patterns, expressions
    // ------------------------------------------------------------------

    fn walk_block(&mut self, scopes: &mut Scopes, block: Block<'_>) {
        scopes.push();
        for statement in block.statements() {
            match statement {
                Statement::Let(binding) | Statement::Var(binding) => {
                    self.walk_binding_stmt(scopes, binding);
                }
                Statement::Const(decl) => {
                    if let Some(ty) = decl.ty() {
                        self.walk_type(scopes, ty);
                    }
                    if let Some(value) = decl.value() {
                        self.walk_expr(scopes, value);
                    }
                    if let Some(name) = decl.name_ref() {
                        self.declare_scoped(scopes, name, SymbolKind::Const);
                    }
                }
                Statement::Expr(statement) => {
                    if let Some(expr) = statement.expr() {
                        self.walk_expr(scopes, expr);
                    }
                }
                Statement::Empty(_) | Statement::Error(_) => {}
            }
        }
        if let Some(tail) = block.tail() {
            self.walk_expr(scopes, tail);
        }
        scopes.pop();
    }

    /// A `let`/`var`: the initializer resolves *before* the pattern binds,
    /// so `let x = x;` refers to the outer `x` (shadowing).
    fn walk_binding_stmt(&mut self, scopes: &mut Scopes, binding: BindingStmt<'_>) {
        if let Some(ty) = binding.ty() {
            self.walk_type(scopes, ty);
        }
        if let Some(init) = binding.initializer() {
            self.walk_expr(scopes, init);
        }
        if let Some(pattern) = binding.pattern() {
            self.walk_pattern(scopes, pattern);
        }
    }

    /// Walk a binding pattern, declaring its names. Binding the same name
    /// twice in one pattern is `R0001`; alternatives of an or-pattern may
    /// each bind the same names.
    fn walk_pattern(&mut self, scopes: &mut Scopes, pattern: Pattern<'_>) {
        let mut seen: HashMap<String, Span> = HashMap::new();
        self.walk_pattern_inner(scopes, pattern, &mut seen);
    }

    fn walk_pattern_inner(
        &mut self,
        scopes: &mut Scopes,
        pattern: Pattern<'_>,
        seen: &mut HashMap<String, Span>,
    ) {
        match pattern {
            Pattern::Literal(_) | Pattern::Wildcard(_) => {}
            Pattern::Binding(binding) => {
                if let Some(name) = binding.name_ref() {
                    // A bare identifier pattern is ambiguous in the grammar: it
                    // is a fresh binding *unless* the name refers to a unit
                    // (payload-less) enum variant in scope, in which case it is
                    // that variant's pattern (`None`, `Empty`, …). Resolve it as
                    // a variant first — recording a reference, not a binding —
                    // so matching tests the discriminant instead of always
                    // succeeding. Anything else is a binding, as before.
                    if let Some(variant) = self.bare_unit_variant(scopes, name.text) {
                        self.record(name, variant);
                    } else {
                        self.declare_pattern_binding(scopes, name, seen);
                    }
                }
            }
            Pattern::Path(path) => {
                let segments: Vec<Name<'_>> = path.segment_names().collect();
                if !segments.is_empty() {
                    self.resolve_path(scopes, &segments, Pos::Value);
                }
                for field in path.fields() {
                    if let Some(sub) = field.pattern() {
                        self.walk_pattern_inner(scopes, sub, seen);
                    } else if let Some(name) = field.name_ref() {
                        // Shorthand `{ radius }` binds the field's name.
                        self.declare_pattern_binding(scopes, name, seen);
                    }
                }
            }
            Pattern::Or(or) => {
                for alternative in or.alternatives() {
                    let mut alternative_seen: HashMap<String, Span> = HashMap::new();
                    self.walk_pattern_inner(scopes, alternative, &mut alternative_seen);
                }
            }
            Pattern::Group(group) => {
                if let Some(inner) = group.inner() {
                    self.walk_pattern_inner(scopes, inner, seen);
                }
            }
        }
    }

    /// If a bare name refers to a unit (payload-less) enum variant that a
    /// pattern binding should resolve *to* rather than shadow, return it.
    ///
    /// Resolution mirrors a value-position bare path but is side-effect-free
    /// (no reference recorded, no diagnostic) and succeeds only for a variant:
    /// a user scope binding of the same name is a local that shadows any
    /// variant, so it suppresses this; otherwise the name is looked up in the
    /// enclosing module scope and then the prelude (`None`, `Ok`, …).
    fn bare_unit_variant(&self, scopes: &Scopes, name: &str) -> Option<SymbolId> {
        // A local/param binding in scope shadows a variant — then it is a
        // binding, not a variant pattern.
        if scopes.lookup(name).is_some() {
            return None;
        }
        let symbol = match self.scope(scopes.module).get(name) {
            Some(Binding::Decl(id)) => Some(*id),
            // Imports and ambiguities are not resolved here; a variant reaches
            // patterns as a module declaration or a prelude name.
            _ => self.prelude_lookup(name),
        }?;
        (self.sym(symbol).kind == SymbolKind::Variant).then_some(symbol)
    }

    fn declare_pattern_binding(
        &mut self,
        scopes: &mut Scopes,
        name: Name<'_>,
        seen: &mut HashMap<String, Span>,
    ) {
        if let Some(&previous) = seen.get(name.text) {
            self.out.diagnostics.push(
                Diagnostic::error(
                    code(1),
                    format!("`{}` is bound more than once in this pattern", name.text),
                    name.span,
                )
                .with_primary_label("bound again here")
                .with_secondary_label(previous, "first bound here")
                .with_actual(StructuredValue::Name(name.text.to_owned())),
            );
            return;
        }
        seen.insert(name.text.to_owned(), name.span);
        self.declare_scoped(scopes, name, SymbolKind::Local);
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one arm per expression form; splitting would only scatter the dispatch"
    )]
    fn walk_expr(&mut self, scopes: &mut Scopes, expr: Expr<'_>) {
        match expr {
            Expr::Literal(_) | Expr::Continue(_) => {}
            Expr::Path(path) => {
                if let Some(fish) = path.turbofish() {
                    self.walk_type_args(scopes, fish);
                }
                let segments: Vec<Name<'_>> = path.segment_names().collect();
                if !segments.is_empty() {
                    self.resolve_path(scopes, &segments, Pos::Value);
                }
            }
            Expr::StructLiteral(literal) => {
                if let Some(path) = literal.path() {
                    self.resolve_type_path(scopes, path);
                }
                if let Some(args) = literal.type_args() {
                    self.walk_type_args(scopes, args);
                }
                for init in literal.inits() {
                    if let Some(value) = init.value() {
                        self.walk_expr(scopes, value);
                    } else if let Some(name) = init.name_ref() {
                        // Shorthand `{ y }` *reads* `y` from the scope.
                        self.resolve_path(scopes, &[name], Pos::Value);
                    }
                }
            }
            Expr::Unary(unary) => {
                if let Some(operand) = unary.operand() {
                    self.walk_expr(scopes, operand);
                }
            }
            Expr::Binary(e) => self.walk_operands(scopes, e.lhs(), e.rhs()),
            Expr::Range(e) => self.walk_operands(scopes, e.lhs(), e.rhs()),
            Expr::Assign(e) => self.walk_operands(scopes, e.lhs(), e.rhs()),
            Expr::Call(call) => {
                if let Some(callee) = call.callee() {
                    self.walk_expr(scopes, callee);
                }
                for arg in call.args() {
                    self.walk_expr(scopes, arg);
                }
            }
            Expr::MethodCall(call) => {
                // The method *name* is type-dependent; only the receiver and
                // arguments resolve here.
                if let Some(receiver) = call.receiver() {
                    self.walk_expr(scopes, receiver);
                }
                for arg in call.args() {
                    self.walk_expr(scopes, arg);
                }
            }
            Expr::Field(field) => {
                // The field name is type-dependent.
                if let Some(receiver) = field.receiver() {
                    self.walk_expr(scopes, receiver);
                }
            }
            Expr::Index(index) => {
                self.walk_operands(scopes, index.base(), index.index());
            }
            Expr::Try(inner) => {
                if let Some(expr) = inner.inner() {
                    self.walk_expr(scopes, expr);
                }
            }
            Expr::Cast(cast) => {
                if let Some(inner) = cast.inner() {
                    self.walk_expr(scopes, inner);
                }
                if let Some(ty) = cast.ty() {
                    self.walk_type(scopes, ty);
                }
            }
            Expr::Group(group) => {
                if let Some(inner) = group.inner() {
                    self.walk_expr(scopes, inner);
                }
            }
            Expr::If(expr) => self.walk_if(scopes, expr),
            Expr::Match(expr) => {
                if let Some(scrutinee) = expr.scrutinee() {
                    self.walk_expr(scopes, scrutinee);
                }
                for arm in expr.arms() {
                    scopes.push();
                    if let Some(pattern) = arm.pattern() {
                        self.walk_pattern(scopes, pattern);
                    }
                    if let Some(guard) = arm.guard() {
                        self.walk_expr(scopes, guard);
                    }
                    if let Some(value) = arm.value() {
                        self.walk_expr(scopes, value);
                    }
                    scopes.pop();
                }
            }
            Expr::While(expr) => {
                if let Some(condition) = expr.condition() {
                    self.walk_expr(scopes, condition);
                }
                if let Some(body) = expr.body() {
                    self.walk_block(scopes, body);
                }
            }
            Expr::Loop(expr) => {
                if let Some(body) = expr.body() {
                    self.walk_block(scopes, body);
                }
            }
            Expr::For(expr) => {
                if let Some(iterable) = expr.iterable() {
                    self.walk_expr(scopes, iterable);
                }
                scopes.push();
                if let Some(pattern) = expr.pattern() {
                    self.walk_pattern(scopes, pattern);
                }
                if let Some(body) = expr.body() {
                    self.walk_block(scopes, body);
                }
                scopes.pop();
            }
            Expr::Unsafe(expr) => {
                if let Some(body) = expr.body() {
                    self.walk_block(scopes, body);
                }
            }
            Expr::Return(expr) => {
                if let Some(value) = expr.value() {
                    self.walk_expr(scopes, value);
                }
            }
            Expr::Break(expr) => {
                if let Some(value) = expr.value() {
                    self.walk_expr(scopes, value);
                }
            }
            Expr::Block(block) => self.walk_block(scopes, block),
        }
    }

    fn walk_operands(&mut self, scopes: &mut Scopes, lhs: Option<Expr<'_>>, rhs: Option<Expr<'_>>) {
        if let Some(lhs) = lhs {
            self.walk_expr(scopes, lhs);
        }
        if let Some(rhs) = rhs {
            self.walk_expr(scopes, rhs);
        }
    }

    fn walk_if(&mut self, scopes: &mut Scopes, expr: tuo_ast::IfExpr<'_>) {
        if let Some(condition) = expr.condition() {
            self.walk_expr(scopes, condition);
        }
        if let Some(then) = expr.then_block() {
            self.walk_block(scopes, then);
        }
        match expr.else_branch() {
            Some(ElseBranch::If(nested)) => self.walk_if(scopes, nested),
            Some(ElseBranch::Block(block)) => self.walk_block(scopes, block),
            None => {}
        }
    }

    // ------------------------------------------------------------------
    // Types
    // ------------------------------------------------------------------

    fn walk_type(&mut self, scopes: &mut Scopes, ty: TypeRef<'_>) {
        match ty {
            TypeRef::Unit(_) => {}
            TypeRef::Wrapper(wrapper) => {
                if let Some(args) = wrapper.args() {
                    self.walk_type_args(scopes, args);
                }
            }
            TypeRef::Path(path) => {
                if let Some(type_path) = path.path() {
                    self.resolve_type_path(scopes, type_path);
                }
                if let Some(args) = path.args() {
                    self.walk_type_args(scopes, args);
                }
            }
        }
    }

    fn walk_type_args(&mut self, scopes: &mut Scopes, args: TypeArgs<'_>) {
        for ty in args.types() {
            self.walk_type(scopes, ty);
        }
    }

    fn resolve_type_path(&mut self, scopes: &Scopes, path: TypePath<'_>) {
        let segments: Vec<Name<'_>> = path.segment_names().collect();
        if !segments.is_empty() {
            self.resolve_path(scopes, &segments, Pos::Type);
        }
    }

    // ------------------------------------------------------------------
    // Scoped declarations
    // ------------------------------------------------------------------

    /// Declare in the innermost scope, allowing shadowing.
    fn declare_scoped(&mut self, scopes: &mut Scopes, name: Name<'_>, kind: SymbolKind) {
        let id = self.new_symbol(name.text, kind, scopes.module, false, Some(name.span));
        scopes.top().insert(name.text.to_owned(), id);
    }

    /// Declare in the innermost scope, reporting `R0001` on a collision
    /// *within that scope* (parameters, generic parameters, `given`
    /// bindings). Outer names may still be shadowed.
    fn declare_scoped_strict(&mut self, scopes: &mut Scopes, name: Name<'_>, kind: SymbolKind) {
        if let Some(&previous) = scopes.top().get(name.text) {
            let previous = self.sym(previous).declaration;
            self.duplicate_definition(name, previous);
            return;
        }
        self.declare_scoped(scopes, name, kind);
    }

    fn duplicate_definition(&mut self, name: Name<'_>, previous: Option<Span>) {
        let mut diagnostic = Diagnostic::error(
            code(1),
            format!("the name `{}` is defined multiple times", name.text),
            name.span,
        )
        .with_primary_label("redefined here")
        .with_actual(StructuredValue::Name(name.text.to_owned()));
        if let Some(previous) = previous {
            diagnostic = diagnostic.with_secondary_label(previous, "first defined here");
        }
        self.out.diagnostics.push(diagnostic);
    }

    // ------------------------------------------------------------------
    // Path resolution
    // ------------------------------------------------------------------

    /// Look a first path segment up in the module scope: declarations, then
    /// imports (which may be ambiguous).
    fn lookup_module_scope(&mut self, module: ModuleId, name: Name<'_>) -> Hit {
        enum Found {
            Decl(SymbolId),
            One(Target),
            Many(Vec<Span>),
        }
        let found = match self.scope(module).get(name.text) {
            None => return Hit::Missing,
            Some(Binding::Decl(id)) => Found::Decl(*id),
            Some(Binding::Import(targets)) => match targets.as_slice() {
                [(target, _)] => Found::One(*target),
                many => Found::Many(many.iter().map(|(_, span)| *span).collect()),
            },
        };
        match found {
            Found::Decl(id) => {
                self.record(name, id);
                Hit::Symbol(id)
            }
            Found::One(Target::Symbol(id)) => {
                self.record(name, id);
                Hit::Symbol(id)
            }
            Found::One(Target::Module(id)) => {
                let symbol = self.out.modules[id.as_u32() as usize].symbol;
                self.record(name, symbol);
                Hit::Module(id)
            }
            Found::Many(spans) => {
                let mut diagnostic =
                    Diagnostic::error(code(4), format!("`{}` is ambiguous", name.text), name.span)
                        .with_primary_label("multiple imports bind this name")
                        .with_help("disambiguate with an `as` alias on one of the imports")
                        .with_actual(StructuredValue::Name(name.text.to_owned()));
                for span in spans {
                    diagnostic = diagnostic.with_secondary_label(span, "candidate imported here");
                }
                self.out.diagnostics.push(diagnostic);
                Hit::Ambiguous
            }
        }
    }

    /// Resolve a full path in scope, recording a reference for every
    /// resolved segment. Type-dependent tails (segments after a non-module,
    /// non-enum symbol, and everything involving `Self`) are skipped
    /// silently — they belong to the type checker.
    fn resolve_path(&mut self, scopes: &Scopes, segments: &[Name<'_>], pos: Pos) -> Option<Target> {
        let first = *segments.first()?;
        if first.text == "Self" {
            return None;
        }
        let mut current = if let Some(id) = scopes.lookup(first.text) {
            self.record(first, id);
            Target::Symbol(id)
        } else {
            match self.lookup_module_scope(scopes.module, first) {
                Hit::Symbol(id) => Target::Symbol(id),
                Hit::Module(id) => Target::Module(id),
                Hit::Ambiguous => return None,
                Hit::Missing => {
                    if let Some(&module) = self
                        .by_path
                        .get(std::slice::from_ref(&first.text.to_owned()))
                    {
                        let symbol = self.out.modules[module.as_u32() as usize].symbol;
                        self.record(first, symbol);
                        Target::Module(module)
                    } else if let Some(id) = self.prelude_lookup(first.text) {
                        self.record(first, id);
                        Target::Symbol(id)
                    } else if pos == Pos::Type
                        && segments.len() == 1
                        && BUILTIN_TYPES.contains(&first.text)
                    {
                        return None;
                    } else {
                        self.undefined_name(first);
                        return None;
                    }
                }
            }
        };
        for &segment in &segments[1..] {
            match current {
                Target::Module(module) => {
                    current = self.navigate_module(scopes.module, module, segment)?;
                }
                Target::Symbol(symbol) if self.sym(symbol).kind == SymbolKind::Enum => {
                    let variant = self
                        .enum_variants
                        .get(&symbol)
                        .and_then(|variants| variants.get(segment.text))
                        .copied();
                    if let Some(variant) = variant {
                        self.record(segment, variant);
                        current = Target::Symbol(variant);
                    } else {
                        let enum_name = self.sym(symbol).name.clone();
                        self.out.diagnostics.push(
                            Diagnostic::error(
                                code(2),
                                format!("no variant `{}` on enum `{enum_name}`", segment.text),
                                segment.span,
                            )
                            .with_primary_label("unknown variant")
                            .with_actual(StructuredValue::Name(segment.text.to_owned())),
                        );
                        return None;
                    }
                }
                Target::Symbol(_) => break,
            }
        }
        Some(current)
    }

    /// Step one path segment into a module: a declared item
    /// (visibility-checked) or a submodule.
    fn navigate_module(
        &mut self,
        using: ModuleId,
        module: ModuleId,
        name: Name<'_>,
    ) -> Option<Target> {
        match self.scope(module).get(name.text) {
            Some(Binding::Decl(symbol)) => {
                let symbol = *symbol;
                self.check_visible(using, symbol, name);
                self.record(name, symbol);
                Some(Target::Symbol(symbol))
            }
            Some(Binding::Import(_)) => {
                let module_name = self.module_name(module);
                self.out.diagnostics.push(
                    Diagnostic::error(
                        code(2),
                        format!("no `{}` in {module_name}", name.text),
                        name.span,
                    )
                    .with_primary_label("only an import, not a declaration")
                    .with_note("imports are not re-exported; refer to the declaring module")
                    .with_actual(StructuredValue::Name(name.text.to_owned())),
                );
                None
            }
            None => {
                if let Some(child) = self.submodule_of(module, name.text) {
                    let symbol = self.out.modules[child.as_u32() as usize].symbol;
                    self.record(name, symbol);
                    return Some(Target::Module(child));
                }
                let module_name = self.module_name(module);
                self.out.diagnostics.push(
                    Diagnostic::error(
                        code(2),
                        format!("no `{}` in {module_name}", name.text),
                        name.span,
                    )
                    .with_primary_label("not found in this module")
                    .with_actual(StructuredValue::Name(name.text.to_owned())),
                );
                None
            }
        }
    }

    /// Report `R0005` when `symbol` is used from outside its declaring
    /// module without being `pub`. The use still resolves — error recovery
    /// keeps later diagnostics meaningful.
    fn check_visible(&mut self, using: ModuleId, symbol: SymbolId, name: Name<'_>) {
        let data = self.sym(symbol);
        if data.module == using || data.is_pub {
            return;
        }
        let owner = self.module_name(data.module);
        let declaration = data.declaration;
        let mut diagnostic = Diagnostic::error(
            code(5),
            format!("`{}` is private to {owner}", name.text),
            name.span,
        )
        .with_primary_label("not visible outside its module")
        .with_help(format!("mark `{}` as `pub` to allow this", name.text))
        .with_actual(StructuredValue::Name(name.text.to_owned()));
        if let Some(declaration) = declaration {
            diagnostic =
                diagnostic.with_secondary_label(declaration, "declared here without `pub`");
        }
        self.out.diagnostics.push(diagnostic);
    }

    fn undefined_name(&mut self, name: Name<'_>) {
        self.out.diagnostics.push(
            Diagnostic::error(
                code(2),
                format!("cannot find `{}` in this scope", name.text),
                name.span,
            )
            .with_primary_label("not found")
            .with_actual(StructuredValue::Name(name.text.to_owned())),
        );
    }
}
