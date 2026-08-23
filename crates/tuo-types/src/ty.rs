//! The type representation: tuonelang's v0 type universe.
//!
//! Widths and semantics follow the Constitution: `Int` is an alias for
//! `I64` (two's-complement, trapping overflow, §11/§24), `Float` for `F64`
//! (IEEE-754 binary64), and there are **no implicit numeric conversions**
//! between any two types (§10) — type equality is exact.

use tuo_resolve::{Resolution, SymbolId};

/// The largest length a fixed-size array type `[T; N]` may declare
/// (ADR-0004 Stage 2).
///
/// The repeat literal `[x; N]` lowers to `N` explicit MIR operands, so an
/// unbounded `N` would be a compile-time blowup; the cap keeps that
/// expansion small until a dedicated `Rvalue::Repeat` lifts it (never
/// silently raised — see the ADR).
pub const MAX_FIXED_ARRAY_LEN: u64 = 65_536;

/// A signed or unsigned integer type of exactly specified width
/// (Constitution §10). `Isize`/`Usize` are pointer-width; `Usize` is the
/// index/length type.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum IntKind {
    /// 8-bit signed.
    I8,
    /// 16-bit signed.
    I16,
    /// 32-bit signed.
    I32,
    /// 64-bit signed — the meaning of the `Int` alias. Two's-complement;
    /// overflow is a defined, deterministic trap (§24), never wraparound.
    I64,
    /// Pointer-width signed.
    Isize,
    /// 8-bit unsigned.
    U8,
    /// 16-bit unsigned.
    U16,
    /// 32-bit unsigned.
    U32,
    /// 64-bit unsigned.
    U64,
    /// Pointer-width unsigned — the index/length type.
    Usize,
}

impl IntKind {
    /// The surface-syntax name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::I8 => "I8",
            Self::I16 => "I16",
            Self::I32 => "I32",
            Self::I64 => "I64",
            Self::Isize => "Isize",
            Self::U8 => "U8",
            Self::U16 => "U16",
            Self::U32 => "U32",
            Self::U64 => "U64",
            Self::Usize => "Usize",
        }
    }

    /// Is this a signed kind (unary negation is only defined for these)?
    #[must_use]
    pub const fn is_signed(self) -> bool {
        matches!(
            self,
            Self::I8 | Self::I16 | Self::I32 | Self::I64 | Self::Isize
        )
    }
}

/// A floating-point type (IEEE-754, bit-reproducible across backends, §11).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum FloatKind {
    /// IEEE-754 binary32.
    F32,
    /// IEEE-754 binary64 — the meaning of the `Float` alias.
    F64,
}

impl FloatKind {
    /// The surface-syntax name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::F32 => "F32",
            Self::F64 => "F64",
        }
    }
}

/// A memory wrapper type (Constitution §25).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum WrapperKind {
    /// `Box[T]` — unique owned heap allocation.
    Box,
    /// `Shared[T]` — shared ownership.
    Shared,
    /// `Weak[T]` — non-owning handle to a `Shared`.
    Weak,
}

impl WrapperKind {
    /// The surface-syntax name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Box => "Box",
            Self::Shared => "Shared",
            Self::Weak => "Weak",
        }
    }
}

/// A unification variable created during local type inference.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct InferVar(pub(crate) u32);

/// The passing mode of a function-type parameter (ADR-0008 Tier 1).
///
/// Modes are part of a function *type* — the ownership vocabulary is part of a
/// function's calling contract, and a function value is a code pointer whose
/// indirect-call site drives its per-argument borrows from these modes. Type
/// equality is exact, modes included: `fn(take Int) -> Int` is a different type
/// from `fn(in Int) -> Int`.
///
/// This is the type-layer twin of `tuo_hir::ParamMode`; the two cannot be
/// unified because `tuo-types` sits below `tuo-hir` in the pipeline.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ParamMode {
    /// `in` — read-only borrow for the call (the declaration default).
    In,
    /// `mut` — exclusive mutable borrow for the call.
    Mut,
    /// `take` — ownership transfer.
    Take,
}

impl ParamMode {
    /// The surface-syntax keyword.
    #[must_use]
    pub const fn keyword(self) -> &'static str {
        match self {
            Self::In => "in",
            Self::Mut => "mut",
            Self::Take => "take",
        }
    }
}

/// One parameter of a function type: its passing mode and its type
/// (ADR-0008 Tier 1). Modes are mandatory in the function-type syntax and
/// participate in exact type equality.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FnParam {
    /// The parameter's passing mode.
    pub mode: ParamMode,
    /// The parameter's type.
    pub ty: Ty,
}

/// A function signature as a type: parameter (mode + type) list and the return
/// type.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FnTy {
    /// Parameters (mode + type), in declaration order.
    pub params: Vec<FnParam>,
    /// The return type.
    pub ret: Ty,
}

/// A tuonelang type.
///
/// Nominal types (`Struct`, `Enum`, `Param`) are identified by their stable
/// [`SymbolId`] from name resolution — never by name or AST position. Type
/// equality is derived structural equality over this representation, which
/// makes it *exact*: `I32` never equals `I64`, `String` never equals `Str`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Ty {
    /// `()` — one value, also the implicit return type.
    Unit,
    /// The type of expressions that never yield a value (`return`, `break`,
    /// `continue`, an endless `loop`). Unifies with every type.
    Never,
    /// `Bool`.
    Bool,
    /// `Char` — a Unicode scalar value.
    Char,
    /// `String` — owned UTF-8 text.
    String,
    /// `Str` — a borrowed UTF-8 slice.
    Str,
    /// An integer type of exact width.
    Int(IntKind),
    /// A floating-point type.
    Float(FloatKind),
    /// A tuple. (No surface syntax constructs one in v0 — tuple structs are
    /// deliberately excluded by Constitution §12 — but the type core
    /// supports them for `.N` access and future syntax.)
    Tuple(Vec<Ty>),
    /// `Array[T]` — the builtin homogeneous array, indexed by `Usize`.
    Array(Box<Ty>),
    /// `Map[K, V]` — the builtin hash map (ADR-0011). The type is generic;
    /// the v0 *operation* surface is monomorphic over `(Int, Int)` and
    /// `(Str, Int)`, exactly as `Array`'s ops were `Int`-monomorphic under
    /// ADR-0009. Non-`Copy` (it owns a heap table).
    Map(Box<Ty>, Box<Ty>),
    /// `[T; N]` — the inline fixed-length array; `N` elements of `T`,
    /// length part of the type. Distinct from `Array[T]`, the growable
    /// heap sequence. (ADR-0004 Stage 2.)
    FixedArray(Box<Ty>, u64),
    /// The type of `a .. b` ranges (internal; iterable by `for`).
    Range(Box<Ty>),
    /// A function type.
    Fn(Box<FnTy>),
    /// `Option[T]` — the canonical maybe-absent value (Constitution §14).
    Option(Box<Ty>),
    /// `Result[T, E]` — the canonical fallible outcome (Constitution §15).
    Result(Box<Ty>, Box<Ty>),
    /// A user-declared struct, with its type arguments.
    Struct(SymbolId, Vec<Ty>),
    /// A user-declared enum, with its type arguments.
    Enum(SymbolId, Vec<Ty>),
    /// A memory wrapper: `Box[T]`, `Shared[T]`, `Weak[T]`.
    Wrapper(WrapperKind, Box<Ty>),
    /// A generic type parameter, by its declaration symbol.
    Param(SymbolId),
    /// An unsolved inference variable.
    Var(InferVar),
    /// The poison type: produced after an error and unifying with
    /// everything, so one mistake is reported once.
    Error,
}

impl Ty {
    /// Shorthand for the `Int` alias target (`I64`).
    #[must_use]
    pub const fn int() -> Self {
        Self::Int(IntKind::I64)
    }

    /// Shorthand for the `Float` alias target (`F64`).
    #[must_use]
    pub const fn float() -> Self {
        Self::Float(FloatKind::F64)
    }

    /// Is this an integer type?
    #[must_use]
    pub const fn is_int(&self) -> bool {
        matches!(self, Self::Int(_))
    }

    /// Is this a numeric (integer or floating-point) type?
    #[must_use]
    pub const fn is_numeric(&self) -> bool {
        matches!(self, Self::Int(_) | Self::Float(_))
    }

    /// Render the type in tuonelang surface syntax, resolving nominal
    /// symbols through `resolution`. Unsolved inference variables render as
    /// `{integer}`, `{float}`, or `_` by class — callers should
    /// [`apply`](crate::infer::InferCtx::apply) first.
    #[must_use]
    pub fn render(&self, resolution: &Resolution) -> String {
        match self {
            Self::Unit => "()".to_owned(),
            Self::Never => "Never".to_owned(),
            Self::Bool => "Bool".to_owned(),
            Self::Char => "Char".to_owned(),
            Self::String => "String".to_owned(),
            Self::Str => "Str".to_owned(),
            Self::Int(kind) => kind.name().to_owned(),
            Self::Float(kind) => kind.name().to_owned(),
            Self::Tuple(items) => {
                let inner: Vec<String> = items.iter().map(|ty| ty.render(resolution)).collect();
                format!("({})", inner.join(", "))
            }
            Self::Array(item) => format!("Array[{}]", item.render(resolution)),
            Self::Map(key, value) => format!(
                "Map[{}, {}]",
                key.render(resolution),
                value.render(resolution)
            ),
            Self::FixedArray(elem, n) => format!("[{}; {n}]", elem.render(resolution)),
            Self::Range(item) => format!("Range[{}]", item.render(resolution)),
            Self::Fn(fn_ty) => {
                let params: Vec<String> = fn_ty
                    .params
                    .iter()
                    .map(|param| {
                        format!("{} {}", param.mode.keyword(), param.ty.render(resolution))
                    })
                    .collect();
                format!(
                    "fn({}) -> {}",
                    params.join(", "),
                    fn_ty.ret.render(resolution)
                )
            }
            Self::Option(item) => format!("Option[{}]", item.render(resolution)),
            Self::Result(ok, err) => format!(
                "Result[{}, {}]",
                ok.render(resolution),
                err.render(resolution)
            ),
            Self::Struct(symbol, args) | Self::Enum(symbol, args) => {
                let name = &resolution.symbol(*symbol).name;
                if args.is_empty() {
                    name.clone()
                } else {
                    let inner: Vec<String> = args.iter().map(|ty| ty.render(resolution)).collect();
                    format!("{name}[{}]", inner.join(", "))
                }
            }
            Self::Wrapper(kind, item) => format!("{}[{}]", kind.name(), item.render(resolution)),
            Self::Param(symbol) => resolution.symbol(*symbol).name.clone(),
            Self::Var(_) => "_".to_owned(),
            Self::Error => "{error}".to_owned(),
        }
    }
}
