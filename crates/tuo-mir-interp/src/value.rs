//! The interpreter's runtime value model.
//!
//! A [`Value`] is the interpreter's in-memory representation of a MIR value.
//! It mirrors the MIR/type universe closely enough to execute every
//! instruction's documented semantics deterministically, and no more: there
//! are no host pointers, no addresses, and no undefined bit patterns. Every
//! value is an owned tree; MIR's aliasing (borrow-mode call arguments) is
//! modelled by copy-in/copy-back at the call boundary, which is observably
//! identical because a borrow lives only for the duration of one call.
//!
//! Aggregates store their fields positionally, in declaration order — exactly
//! the order MIR projections and `Aggregate` rvalues use. `Option` and
//! `Result` are ordinary enums here (`Some`/`Ok` = variant 0, `None`/`Err` =
//! variant 1), matching [`tuo_mir`]'s discriminant convention.

use tuo_resolve::SymbolId;
use tuo_types::{FloatKind, IntKind};

/// A runtime value: the interpreter's representation of one MIR value.
///
/// Values are compared for equality *structurally* by the `Eq` binary
/// operators the interpreter implements, never by this type's derived
/// equality — floats need IEEE semantics (`NaN != NaN`), which a derived
/// `PartialEq` would get wrong. The derived comparison here exists only for
/// test assertions on non-float values.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// The unit value `()`.
    Unit,
    /// A boolean.
    Bool(bool),
    /// An integer, carrying its kind so arithmetic can trap and wrap at the
    /// right width. The stored `i128` is always the mathematically correct
    /// value within the kind's range.
    Int(i128, IntKind),
    /// A floating-point value at `f64` precision (an `F32` is kept rounded to
    /// `f32` and re-widened, per [`tuo_mir`]'s `Const::Float` convention).
    Float(f64, FloatKind),
    /// A Unicode scalar value.
    Char(char),
    /// An owned or borrowed string as its **byte buffer**. `Str` and
    /// `String` share this representation at runtime; the type distinction
    /// is a compile-time ownership concern the interpreter has already had
    /// checked for it. The bytes of a literal are UTF-8, but a value need
    /// not stay valid UTF-8: `std::str::slice` is a byte-range operation
    /// and may split a multi-byte code point (the documented ADR-0006 v0
    /// contract), so the representation is bytes, with equality,
    /// comparison, and `len` all byte-wise.
    Str(Vec<u8>),
    /// A tuple, struct, or range: positional fields in declaration order
    /// (a range is `[start, end]`).
    Aggregate(Vec<Value>),
    /// An enum, `Option`, or `Result`: the active variant's declaration-order
    /// index and its payload fields in declaration order.
    Variant {
        /// The active variant's declaration-order index.
        variant: u32,
        /// The variant's payload fields, in declaration order.
        fields: Vec<Value>,
    },
    /// An array of elements.
    Array(Vec<Value>),
    /// A **function value** (ADR-0008 Tier 1): a callable naming a top-level
    /// function by its `SymbolId`. `Copy` (cloning copies the id), rendered by
    /// the function's symbol. An indirect call resolves this to the function
    /// body and dispatches exactly like a direct call.
    Fn(SymbolId),
}

impl Value {
    /// Interpret this value as a boolean, or `None` if it is not a `Bool`.
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Interpret this value as an integer `(value, kind)`, if it is one.
    #[must_use]
    pub fn as_int(&self) -> Option<(i128, IntKind)> {
        match self {
            Self::Int(value, kind) => Some((*value, *kind)),
            _ => None,
        }
    }

    /// The declaration-order index of the active variant of an enum-like
    /// value (a [`Value::Variant`]), if this is one.
    #[must_use]
    pub fn as_variant(&self) -> Option<u32> {
        match self {
            Self::Variant { variant, .. } => Some(*variant),
            _ => None,
        }
    }

    /// A short, deterministic rendering for traces and test diagnostics.
    #[must_use]
    pub fn render(&self) -> String {
        match self {
            Self::Unit => "()".to_owned(),
            Self::Bool(b) => b.to_string(),
            Self::Int(value, kind) => format!("{value}{}", kind.name()),
            Self::Float(value, kind) => format!("{value}{}", kind.name()),
            Self::Char(c) => format!("'{}'", c.escape_default()),
            // Render through a lossy UTF-8 view: identical to the literal
            // for valid UTF-8, and a replacement-character marker for bytes
            // a code-point-splitting slice produced.
            Self::Str(s) => format!("{:?}", String::from_utf8_lossy(s)),
            Self::Aggregate(fields) => {
                let inner: Vec<String> = fields.iter().map(Self::render).collect();
                format!("({})", inner.join(", "))
            }
            Self::Variant { variant, fields } => {
                if fields.is_empty() {
                    format!("#{variant}")
                } else {
                    let inner: Vec<String> = fields.iter().map(Self::render).collect();
                    format!("#{variant}({})", inner.join(", "))
                }
            }
            Self::Array(elements) => {
                let inner: Vec<String> = elements.iter().map(Self::render).collect();
                format!("[{}]", inner.join(", "))
            }
            // A function value is rendered by its symbol id (the interpreter
            // has no name table here); it is `Copy` and names a top-level fn.
            Self::Fn(symbol) => format!("fn {symbol}"),
        }
    }
}
