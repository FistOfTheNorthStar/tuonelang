//! The output of resolution: symbols, references, spec attachments, and
//! diagnostics, with the queries tooling asks of them.

use std::collections::HashMap;

use tuo_diagnostics::Diagnostic;
use tuo_source::Span;

use crate::builtin::Builtin;
use crate::ids::{ModuleId, SymbolId};
use crate::symbol::{Reference, SpecTarget, Symbol};

/// One module of the resolved program.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ModuleInfo {
    /// The full module path (`["std", "io"]`); empty for the root module.
    pub path: Vec<String>,
    /// The module's own symbol.
    pub symbol: SymbolId,
}

/// Everything name resolution produced for one program snapshot.
///
/// All data is owned — no AST borrows — and keyed by the stable IDs in
/// [`crate::ids`].
#[derive(Clone, Debug, Default)]
pub struct Resolution {
    pub(crate) symbols: Vec<Symbol>,
    pub(crate) modules: Vec<ModuleInfo>,
    pub(crate) references: Vec<Reference>,
    pub(crate) spec_targets: Vec<SpecTarget>,
    pub(crate) spec_symbols: HashMap<Span, SymbolId>,
    pub(crate) spec_deps: HashMap<SymbolId, Vec<SymbolId>>,
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) variants: HashMap<SymbolId, Vec<SymbolId>>,
    pub(crate) prelude: Vec<(String, SymbolId)>,
    pub(crate) builtins: Vec<(Builtin, SymbolId)>,
}

impl Resolution {
    /// The symbol behind `id`.
    ///
    /// # Panics
    ///
    /// Panics if `id` did not come from this resolution.
    #[must_use]
    pub fn symbol(&self, id: SymbolId) -> &Symbol {
        &self.symbols[id.as_u32() as usize]
    }

    /// Every symbol with its ID, in declaration order.
    pub fn symbols(&self) -> impl Iterator<Item = (SymbolId, &Symbol)> {
        self.symbols
            .iter()
            .enumerate()
            .map(|(index, symbol)| (SymbolId::from_raw(index as u32), symbol))
    }

    /// The program's modules, in first-seen order.
    #[must_use]
    pub fn modules(&self) -> &[ModuleInfo] {
        &self.modules
    }

    /// The module behind `id`.
    ///
    /// # Panics
    ///
    /// Panics if `id` did not come from this resolution.
    #[must_use]
    pub fn module(&self, id: ModuleId) -> &ModuleInfo {
        &self.modules[id.as_u32() as usize]
    }

    /// Every resolved name use, in resolution order.
    #[must_use]
    pub fn references(&self) -> &[Reference] {
        &self.references
    }

    /// The spans of every use of `id` (excluding its declaration).
    pub fn references_to(&self, id: SymbolId) -> impl Iterator<Item = Span> + '_ {
        self.references
            .iter()
            .filter(move |reference| reference.symbol == id)
            .map(|reference| reference.span)
    }

    /// Every span a rename of `id` must rewrite: the declaring name token
    /// (when the symbol has one) followed by all uses, in resolution order.
    #[must_use]
    pub fn rename_spans(&self, id: SymbolId) -> Vec<Span> {
        let mut spans: Vec<Span> = self.symbol(id).declaration.into_iter().collect();
        spans.extend(self.references_to(id));
        spans
    }

    /// The symbol whose declaration or use occupies exactly `span`, if any —
    /// the entry point for rename and go-to-definition.
    #[must_use]
    pub fn resolved_at(&self, span: Span) -> Option<SymbolId> {
        if let Some(hit) = self
            .references
            .iter()
            .find(|reference| reference.span == span)
        {
            return Some(hit.symbol);
        }
        self.symbols()
            .find(|(_, symbol)| symbol.declaration == Some(span))
            .map(|(id, _)| id)
    }

    /// Every resolved spec attachment, in source order.
    #[must_use]
    pub fn spec_targets(&self) -> &[SpecTarget] {
        &self.spec_targets
    }

    /// The **spec's own** symbol (a [`SymbolKind::Spec`](crate::SymbolKind))
    /// for the `spec { … }` block occupying exactly `span`, if any. A spec's
    /// name is a target reference, never a binding, so this is the only way
    /// to name a spec's stable identity — [`spec_targets`](Self::spec_targets)
    /// exposes it for attached specs, and this query covers every spec,
    /// including free-standing (string-named) ones. The `span` is the whole
    /// block's span, as carried by the AST/HIR spec declaration.
    #[must_use]
    pub fn spec_at(&self, span: Span) -> Option<SymbolId> {
        self.spec_symbols.get(&span).copied()
    }

    /// Every spec block's `(span, own symbol)` pair, in unspecified order —
    /// the enumerable form of [`spec_at`](Self::spec_at) for callers that
    /// need to index specs by their block span.
    pub fn spec_symbols(&self) -> impl Iterator<Item = (Span, SymbolId)> + '_ {
        self.spec_symbols
            .iter()
            .map(|(&span, &symbol)| (span, symbol))
    }

    /// The specs attached to `function`, in source order (file order, then
    /// declaration order). Multiple specs may attach to one function; empty
    /// for symbols no spec targets. See ADR-0002.
    #[must_use]
    pub fn specs_for(&self, function: SymbolId) -> Vec<SymbolId> {
        self.spec_targets
            .iter()
            .filter(|attachment| attachment.target == function)
            .map(|attachment| attachment.spec)
            .collect()
    }

    /// The function an identifier-named spec is attached to, or `None` for
    /// string-named (free-standing) specs and specs whose target did not
    /// resolve. See ADR-0002.
    #[must_use]
    pub fn target_of(&self, spec: SymbolId) -> Option<SymbolId> {
        self.spec_targets
            .iter()
            .find(|attachment| attachment.spec == spec)
            .map(|attachment| attachment.target)
    }

    /// The module-level items `spec`'s body depends on — its target plus
    /// every function, type, enum variant, interface, and constant it
    /// references — deduplicated, in first-use order. Items declared inside
    /// the spec block itself are not dependencies. This is the seed of the
    /// compilation closure a spec runner must lower before executing the
    /// spec. See ADR-0002 "Dependency discovery".
    #[must_use]
    pub fn dependencies_of(&self, spec: SymbolId) -> &[SymbolId] {
        self.spec_deps.get(&spec).map_or(&[], Vec::as_slice)
    }

    /// Resolution diagnostics (`Rxxxx` codes), in discovery order.
    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// The variants of an enum symbol, in declaration order (empty for
    /// non-enum symbols).
    #[must_use]
    pub fn variants_of(&self, id: SymbolId) -> &[SymbolId] {
        self.variants.get(&id).map_or(&[], Vec::as_slice)
    }

    /// The prelude symbol of `name`, if the language provides one
    /// (`Option`, `Result`, and their variants `Some`, `None`, `Ok`,
    /// `Err`). Prelude names are in scope everywhere but may be shadowed
    /// by module declarations.
    #[must_use]
    pub fn prelude_symbol(&self, name: &str) -> Option<SymbolId> {
        self.prelude
            .iter()
            .find(|(entry, _)| entry == name)
            .map(|(_, id)| *id)
    }

    /// The [`Builtin`] behind `id`, if `id` is one of the builtin
    /// functions the language installs in `std::rt` / `std::str` /
    /// `std::string` / `std::array`
    /// (ADR-0006, ADR-0009). Downstream stages use this to give calls to a builtin
    /// their fixed signature (type checking) and their dedicated MIR form
    /// (lowering).
    #[must_use]
    pub fn builtin(&self, id: SymbolId) -> Option<Builtin> {
        self.builtins
            .iter()
            .find(|(_, symbol)| *symbol == id)
            .map(|(builtin, _)| *builtin)
    }

    /// The symbol of a builtin function.
    #[must_use]
    pub fn builtin_symbol(&self, builtin: Builtin) -> Option<SymbolId> {
        self.builtins
            .iter()
            .find(|(entry, _)| *entry == builtin)
            .map(|(_, id)| *id)
    }

    /// Every builtin function with its symbol, in installation order.
    pub fn builtins(&self) -> impl Iterator<Item = (Builtin, SymbolId)> + '_ {
        self.builtins.iter().copied()
    }
}
