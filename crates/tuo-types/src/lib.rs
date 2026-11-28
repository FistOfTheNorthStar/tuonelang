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
//! Interfaces and impls are checked (ADR-0032 Stage B): conformance,
//! coherence, placement, and the non-generic qualified call `I::m(x)`, whose
//! selected impl member [`TypeckResult::impl_call`] publishes. Interface
//! bounds are resolved but not enforced, and an interface call in a generic
//! body is refused (`T0025`), until Stage C.

mod check;
mod infer;
mod ty;

use std::collections::{BTreeSet, HashMap};

use tuo_ast::Ast;
use tuo_diagnostics::Diagnostic;
use tuo_resolve::{Resolution, SymbolId};
use tuo_source::Span;

pub use ty::{
    FloatKind, FnParam, FnTy, InferVar, IntKind, MAX_FIXED_ARRAY_LEN, ParamMode, Ty, WrapperKind,
    parse_int_literal,
};

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

/// Where one impl member belongs (ADR-0032 Stage B): the interface its block
/// implements and the type it implements it for.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ImplMember {
    /// The implemented interface.
    pub interface: SymbolId,
    /// The impl's target type (`Point` in `impl Area for Point`).
    pub target: Ty,
}

/// Everything type checking produced for one program snapshot.
#[derive(Debug, Default)]
pub struct TypeckResult {
    pub(crate) impl_calls: HashMap<Span, SymbolId>,
    pub(crate) impl_members: HashMap<SymbolId, ImplMember>,
    pub(crate) interface_members: HashMap<SymbolId, Vec<(String, FnTy)>>,
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) symbol_types: HashMap<SymbolId, Ty>,
    pub(crate) expr_types: HashMap<Span, Ty>,
    pub(crate) struct_shapes: HashMap<SymbolId, StructShape>,
    pub(crate) enum_shapes: HashMap<SymbolId, EnumShape>,
    pub(crate) effectful: BTreeSet<SymbolId>,
    pub(crate) constant_time: BTreeSet<SymbolId>,
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

    /// The impl member a qualified interface call selected (ADR-0032 Stage
    /// B), keyed by the call expression's exact span: `Area::area(p)` with
    /// `p: Point` names the `area` of `impl Area for Point`. The ownership
    /// checker and MIR lowering read the selection here rather than
    /// re-deriving it.
    #[must_use]
    pub fn impl_call(&self, span: Span) -> Option<SymbolId> {
        self.impl_calls.get(&span).copied()
    }

    /// The interface and target of an impl member, if `symbol` is a member
    /// of an accepted `impl` block.
    #[must_use]
    pub fn impl_member(&self, symbol: SymbolId) -> Option<&ImplMember> {
        self.impl_members.get(&symbol)
    }

    /// The members of an interface, in declaration order, each with its
    /// signature. `Self` is spelled as [`Ty::Param`] of the interface's own
    /// symbol. Empty for a symbol that is not an interface.
    #[must_use]
    pub fn interface_members(&self, interface: SymbolId) -> &[(String, FnTy)] {
        self.interface_members
            .get(&interface)
            .map_or(&[], Vec::as_slice)
    }

    /// Is `symbol` an **effectful** function (ADR-0006 Stage A)?
    ///
    /// A function is effectful iff its body calls (or references) one of the
    /// `std::rt` effect builtins, or transitively calls an effectful
    /// function — a fixed point over the program's call graph, computed
    /// conservatively (naming an effectful function as a value taints the
    /// referrer). The effect builtins themselves are in the set; the
    /// `std::str` builtins and every function whose closure never reaches an
    /// effect are not. `main` and ordinary functions *may* be effectful;
    /// only specs are restricted (the `R0007` gate), so this query is how
    /// the spec runner, CLI, and agent read one purity computation.
    #[must_use]
    pub fn is_effectful(&self, symbol: SymbolId) -> bool {
        self.effectful.contains(&symbol)
    }

    /// Whether `symbol` is a function marked `#[constant_time]` (ADR-0020
    /// Stage C) — one whose body the checker verified free of data-dependent
    /// control flow. A backend keeps such a function out of line, so the
    /// code that runs is the body that was checked rather than a copy the
    /// optimizer merged into a caller.
    #[must_use]
    pub fn is_constant_time(&self, symbol: SymbolId) -> bool {
        self.constant_time.contains(&symbol)
    }

    /// Every effectful function symbol (see [`Self::is_effectful`]), in
    /// symbol order.
    pub fn effectful_functions(&self) -> impl Iterator<Item = SymbolId> + '_ {
        self.effectful.iter().copied()
    }
}

/// Type-check `files` against `resolution` (produced by
/// [`tuo_resolve::resolve`] over the same parse).
#[must_use]
pub fn check(files: &[Ast<'_>], resolution: &Resolution) -> TypeckResult {
    check::run(files, resolution)
}
