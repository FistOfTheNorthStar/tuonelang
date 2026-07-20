//! The output of resolution: symbols, references, spec attachments, and
//! diagnostics, with the queries tooling asks of them.

use tuo_diagnostics::Diagnostic;
use tuo_source::Span;

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
    pub(crate) diagnostics: Vec<Diagnostic>,
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

    /// Resolution diagnostics (`Rxxxx` codes), in discovery order.
    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
}
