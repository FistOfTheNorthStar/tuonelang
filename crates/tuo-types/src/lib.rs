//! Type checking and inference for tuonelang.
//!
//! [`check`] takes one resolved program snapshot — the parsed files plus
//! the [`tuo_resolve::Resolution`] built from them — and verifies the
//! Constitution's core type system:
//!
//! - the fixed primitive universe (§10/§11): `()`, `Never`, `Bool`,
//!   `Char`, exact-width integers (`Int` = `I64`, two's-complement with
//!   trapping overflow per §24), IEEE-754 floats (`Float` = `F64`),
//!   `String`/`Str`, tuples, `Array[T]`, function types, user structs and
//!   enums, and the canonical `Option[T]`/`Result[T, E]`;
//! - **explicit signatures** (§8): parameter and return types are never
//!   inferred (an absent `->` means `()`);
//! - **local inference** by unification where it is unambiguous: `let`
//!   bindings, numeric literals (defaulting to `I64`/`F64`), and generic
//!   instantiations — anything still unknown afterwards is "type
//!   annotation needed", never a guess;
//! - **no implicit conversions**, numeric or otherwise (§10): `I32` and
//!   `I64` only meet through an explicit `as` cast;
//! - call checking (arity + argument types), return checking, operator
//!   checking, field access (structs and tuple indices), struct-literal
//!   and pattern field checking, and the exhaustiveness foundations for
//!   `match` over enums, `Option`/`Result`, and `Bool` (§16).
//!
//! Every mismatch diagnostic carries **structured expected and actual
//! types** ([`tuo_diagnostics::StructuredValue::Type`]) alongside the
//! rendered message, in the reserved `Txxxx` namespace.
//!
//! Method calls, interface bounds, `Self`, and interface-member bodies are
//! deferred to the trait system: they poison to an error type that unifies
//! with everything rather than producing speculative diagnostics.

mod check;
mod infer;
mod ty;

use std::collections::HashMap;

use tuo_ast::Ast;
use tuo_diagnostics::Diagnostic;
use tuo_resolve::{Resolution, SymbolId};

pub use ty::{FloatKind, FnTy, InferVar, IntKind, Ty, WrapperKind};

/// Everything type checking produced for one program snapshot.
#[derive(Debug, Default)]
pub struct TypeckResult {
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) symbol_types: HashMap<SymbolId, Ty>,
}

impl TypeckResult {
    /// Type diagnostics (`Txxxx` codes), in discovery order per body.
    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// The checked type of a symbol: functions get their signature type,
    /// constants their declared type, and parameters/locals their
    /// (inferred) binding type with literal defaults applied.
    #[must_use]
    pub fn type_of(&self, symbol: SymbolId) -> Option<&Ty> {
        self.symbol_types.get(&symbol)
    }
}

/// Type-check `files` against `resolution` (produced by
/// [`tuo_resolve::resolve`] over the same parse).
#[must_use]
pub fn check(files: &[Ast<'_>], resolution: &Resolution) -> TypeckResult {
    check::run(files, resolution)
}
