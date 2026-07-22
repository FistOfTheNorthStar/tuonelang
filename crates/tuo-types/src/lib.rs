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
use tuo_source::Span;

pub use ty::{FloatKind, FnTy, InferVar, IntKind, Ty, WrapperKind};

/// The declaration-context shape of one user struct: its type parameters
/// (in declaration order) and its fields with their declared types.
///
/// Field types are expressed in terms of the struct's own type parameters
/// ([`Ty::Param`]); consumers instantiate them by substituting the type
/// arguments of a concrete [`Ty::Struct`].
#[derive(Clone, Debug)]
pub struct StructShape {
    /// The struct's generic parameters, in declaration order.
    pub type_params: Vec<SymbolId>,
    /// The fields, in declaration order, with their declared types.
    pub fields: Vec<(String, Ty)>,
}

/// The declaration-context shape of one user enum: its type parameters and
/// each variant's payload fields (see [`StructShape`] for instantiation).
#[derive(Clone, Debug)]
pub struct EnumShape {
    /// The enum's generic parameters, in declaration order.
    pub type_params: Vec<SymbolId>,
    /// Each variant (by symbol) with its payload fields, in declaration
    /// order.
    pub variants: Vec<(SymbolId, Vec<(String, Ty)>)>,
}

/// Everything type checking produced for one program snapshot.
#[derive(Debug, Default)]
pub struct TypeckResult {
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) symbol_types: HashMap<SymbolId, Ty>,
    pub(crate) expr_types: HashMap<Span, Ty>,
    pub(crate) struct_shapes: HashMap<SymbolId, StructShape>,
    pub(crate) enum_shapes: HashMap<SymbolId, EnumShape>,
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

    /// The checked type of the expression at `span` (the exact span the
    /// expression covers in source), with the body's inference solution
    /// applied. Every expression the checker visited has an entry; MIR
    /// lowering reads operand and result types from this table instead of
    /// re-running inference.
    #[must_use]
    pub fn expr_ty(&self, span: Span) -> Option<&Ty> {
        self.expr_types.get(&span)
    }

    /// The declared shape of a user struct, for downstream stages that walk
    /// field types (the ownership checker derives `Copy`-ness and field-path
    /// types from this).
    #[must_use]
    pub fn struct_shape(&self, symbol: SymbolId) -> Option<&StructShape> {
        self.struct_shapes.get(&symbol)
    }

    /// The declared shape of a user enum (see [`Self::struct_shape`]).
    #[must_use]
    pub fn enum_shape(&self, symbol: SymbolId) -> Option<&EnumShape> {
        self.enum_shapes.get(&symbol)
    }
}

/// Type-check `files` against `resolution` (produced by
/// [`tuo_resolve::resolve`] over the same parse).
#[must_use]
pub fn check(files: &[Ast<'_>], resolution: &Resolution) -> TypeckResult {
    check::run(files, resolution)
}
