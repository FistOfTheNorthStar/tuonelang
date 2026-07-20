//! The symbol data model: what a name *is* once resolution has run.

use tuo_source::Span;

use crate::ids::{ModuleId, SymbolId};

/// What kind of entity a symbol names.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SymbolKind {
    /// A module (declared with `module a::b;`, or an implicit path prefix).
    Module,
    /// A module-level function.
    Function,
    /// A `struct` declaration.
    Struct,
    /// An `enum` declaration.
    Enum,
    /// One variant of an enum.
    Variant,
    /// An `interface` declaration.
    Interface,
    /// A `const` item (module-level or block-local).
    Const,
    /// A `spec` block.
    Spec,
    /// A generic type parameter.
    TypeParam,
    /// A function parameter (including the `self` receiver).
    Param,
    /// A local binding (`let`, `var`, pattern bindings, `given` bindings).
    Local,
}

impl SymbolKind {
    /// The lowercase noun used in diagnostics ("function", "struct", …).
    #[must_use]
    pub const fn noun(self) -> &'static str {
        match self {
            Self::Module => "module",
            Self::Function => "function",
            Self::Struct => "struct",
            Self::Enum => "enum",
            Self::Variant => "enum variant",
            Self::Interface => "interface",
            Self::Const => "constant",
            Self::Spec => "spec",
            Self::TypeParam => "type parameter",
            Self::Param => "parameter",
            Self::Local => "local binding",
        }
    }
}

/// One resolved entity: its name, what it is, where it lives, and where it
/// was declared.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Symbol {
    /// The name as written at the declaration.
    pub name: String,
    /// What the symbol is.
    pub kind: SymbolKind,
    /// The module the symbol belongs to (a module symbol belongs to
    /// itself).
    pub module: ModuleId,
    /// Is the declaration marked `pub`? (Variants inherit their enum's
    /// visibility; scoped symbols — locals, parameters — are never `pub`.)
    pub is_pub: bool,
    /// The span of the declaring name token, or `None` for entities with no
    /// written declaration (implicit prefix modules, the root module).
    pub declaration: Option<Span>,
}

/// One resolved use of a symbol: the exact source span of the name token
/// and the symbol it refers to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Reference {
    /// Where the name is written.
    pub span: Span,
    /// What it resolves to.
    pub symbol: SymbolId,
}

/// One resolved spec attachment: an identifier-named `spec` block and the
/// function symbol it targets.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SpecTarget {
    /// The spec's own symbol.
    pub spec: SymbolId,
    /// The targeted function — the *same* symbol calls to that function
    /// resolve to.
    pub target: SymbolId,
}
