//! The tuonelang runtime ABI: how values are laid out in memory.
//!
//! This module is the single normative implementation of
//! [`specification/abi.md`](../../../specification/abi.md). It computes the
//! **backend-independent** in-memory layout of every tuonelang type — its size,
//! alignment, and (for aggregates) field offsets and enum discriminant
//! numbering — from `tuo-types` alone. No Cranelift or LLVM type
//! appears here; a backend *consults* these layouts, it never defines its own,
//! so the two backends and the interpreter cannot drift into incompatible
//! memory models.
//!
//! # Version
//!
//! The ABI is not yet frozen, so it carries an explicit integer
//! [`ABI_VERSION`], bumped on any layout-affecting change — exactly as the
//! machine protocol and diagnostics schema are versioned. Versioning before
//! stability lets tests assert "this is ABI v0" and lets a future artifact
//! detect a mismatch rather than silently reinterpret bytes.
//!
//! # Scope (v0)
//!
//! Layouts are computed for the concrete, non-generic value types. Types that
//! carry no runtime value (`Never`, `Fn`, `Range`), that still need generic
//! substitution, or that are compiler-internal placeholders (`Var`, `Param`,
//! `Error`) yield a [`LayoutError`] rather than a guessed layout — the caller
//! reports "unsupported", mirroring the backend's own refusal to lower them.

use tuo_types::{FloatKind, IntKind, Ty, TypeckResult, WrapperKind};

/// The version of the runtime ABI these layouts implement.
///
/// `0` — unstable. Any change to a layout, offset, discriminant numbering,
/// calling-convention rule, or runtime-symbol meaning **must** increment this
/// in the same commit that updates the tests pinning the affected layout.
/// Additive, non-layout-affecting clarifications do not bump it.
pub const ABI_VERSION: u32 = 0;

/// The pointer width, in bytes, of the ABI's supported hosts.
///
/// v0 targets 64-bit development hosts, so a pointer — and `Isize`/`Usize` — is
/// 8 bytes. This is the width the interpreter models `Isize`/`Usize` at, so
/// results agree. A future 32-bit target would bump [`ABI_VERSION`].
pub const POINTER_SIZE: u64 = 8;

/// The alignment, in bytes, of a pointer — equal to [`POINTER_SIZE`].
pub const POINTER_ALIGN: u64 = 8;

/// The size and alignment of a value in memory, in bytes.
///
/// `align` is always a power of two; `size` is always a multiple of `align`
/// (values are padded to their alignment, the `#[repr(C)]` rule), so an array
/// of the value tiles with no gaps.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Layout {
    /// The size in bytes, including tail padding to a multiple of `align`.
    pub size: u64,
    /// The alignment in bytes; a power of two, at least 1.
    pub align: u64,
}

impl Layout {
    /// A zero-sized value with alignment 1 (the layout of `Unit`).
    pub const ZERO: Self = Self { size: 0, align: 1 };

    /// A scalar layout of `size` bytes, self-aligned (`align == size`), for a
    /// power-of-two `size`. This is the shape of every primitive.
    #[must_use]
    pub const fn scalar(size: u64) -> Self {
        Self { size, align: size }
    }

    /// A single machine pointer.
    #[must_use]
    pub const fn pointer() -> Self {
        Self {
            size: POINTER_SIZE,
            align: POINTER_ALIGN,
        }
    }

    /// `n` pointer-width words, pointer-aligned — the shape of the `String`,
    /// `Array`, `Str`, and slice headers.
    #[must_use]
    pub const fn words(n: u64) -> Self {
        Self {
            size: POINTER_SIZE * n,
            align: POINTER_ALIGN,
        }
    }

    /// The stride of this layout in an array: `size` already includes tail
    /// padding, so element *i* sits at `i * stride`.
    #[must_use]
    pub const fn stride(self) -> u64 {
        self.size
    }
}

/// Why a type has no v0 runtime layout.
///
/// These are the exact cases the backend also refuses: types that carry no
/// value, that still need generic substitution, or that are compiler-internal
/// placeholders. The caller turns this into an "unsupported" codegen error.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LayoutError {
    /// A human-readable reason, naming the offending type.
    pub message: String,
}

impl LayoutError {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            message: reason.into(),
        }
    }
}

impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for LayoutError {}

/// Round `offset` up to the next multiple of `align` (a power of two).
#[must_use]
pub const fn align_up(offset: u64, align: u64) -> u64 {
    // `align` is a power of two, so this is the standard bit-twiddle.
    offset.wrapping_add(align - 1) & !(align - 1)
}

/// The exact byte width of an integer kind. `Isize`/`Usize` are pointer-width
/// ([`POINTER_SIZE`]), matching the interpreter's 64-bit model on these hosts.
#[must_use]
pub const fn int_size(kind: IntKind) -> u64 {
    match kind {
        IntKind::I8 | IntKind::U8 => 1,
        IntKind::I16 | IntKind::U16 => 2,
        IntKind::I32 | IntKind::U32 => 4,
        IntKind::I64 | IntKind::U64 => 8,
        IntKind::Isize | IntKind::Usize => POINTER_SIZE,
    }
}

/// The byte width of a floating-point kind.
#[must_use]
pub const fn float_size(kind: FloatKind) -> u64 {
    match kind {
        FloatKind::F32 => 4,
        FloatKind::F64 => 8,
    }
}

/// The discriminant type of every enum-like type: an unsigned 32-bit tag whose
/// value is the variant's declaration-order index — byte-identical to the
/// interpreter's `Value::Variant { variant, .. }`.
pub const DISCRIMINANT_SIZE: u64 = 4;
/// The alignment of the [`DISCRIMINANT_SIZE`] tag.
pub const DISCRIMINANT_ALIGN: u64 = 4;

/// The layout of a tuonelang type, per [`specification/abi.md`].
///
/// `types` resolves user structs and enums to their fields; it may be a
/// [`TypeckResult::default`] for a program that uses no user aggregates. A type
/// with no v0 layout (see [module docs](self)) yields [`LayoutError`].
///
/// # Errors
///
/// Returns [`LayoutError`] for a type that carries no runtime value, still
/// needs generic substitution, or is a compiler-internal placeholder.
pub fn layout_of(ty: &Ty, types: &TypeckResult) -> Result<Layout, LayoutError> {
    match ty {
        Ty::Unit => Ok(Layout::ZERO),
        Ty::Bool => Ok(Layout::scalar(1)),
        Ty::Char => Ok(Layout::scalar(4)),
        Ty::Int(kind) => Ok(Layout::scalar(int_size(*kind))),
        Ty::Float(kind) => Ok(Layout::scalar(float_size(*kind))),

        // Owned/borrowed headers, all pointer-word tuples.
        Ty::String => Ok(Layout::words(3)),   // (ptr, len, cap)
        Ty::Str => Ok(Layout::words(2)),      // (ptr, len)
        Ty::Array(_) => Ok(Layout::words(3)), // (ptr, len, cap)

        // Memory wrappers: a single (non-null) pointer each.
        Ty::Wrapper(WrapperKind::Box | WrapperKind::Shared | WrapperKind::Weak, _) => {
            Ok(Layout::pointer())
        }

        Ty::Tuple(fields) => aggregate_layout(fields.iter().cloned(), types),

        Ty::Struct(symbol, args) => {
            let shape = types
                .struct_shape(*symbol)
                .ok_or_else(|| LayoutError::new("layout of an unknown struct"))?;
            require_monomorphic(&shape.type_params, args, "struct")?;
            aggregate_layout(shape.fields.iter().map(|(_, ty)| ty.clone()), types)
        }

        Ty::Enum(symbol, args) => {
            let shape = types
                .enum_shape(*symbol)
                .ok_or_else(|| LayoutError::new("layout of an unknown enum"))?;
            require_monomorphic(&shape.type_params, args, "enum")?;
            let variants = shape
                .variants
                .iter()
                .map(|(_, fields)| fields.iter().map(|(_, ty)| ty.clone()).collect::<Vec<_>>());
            enum_layout(variants, types)
        }

        // `Option`/`Result` are the two canonical two-variant enums; they lay
        // out exactly as user enums do, in declaration order.
        Ty::Option(inner) => enum_layout([Vec::new(), vec![(**inner).clone()]], types),
        Ty::Result(ok, err) => enum_layout([vec![(**ok).clone()], vec![(**err).clone()]], types),

        // No runtime value / needs substitution / internal placeholder.
        Ty::Never => Err(LayoutError::new("`Never` has no runtime layout")),
        Ty::Range(_) => Err(LayoutError::new(
            "`Range` is iteration-internal and has no v0 value layout",
        )),
        Ty::Fn(_) => Err(LayoutError::new(
            "function types are not first-class values in v0",
        )),
        Ty::Param(_) => Err(LayoutError::new(
            "a generic parameter has no layout until monomorphized",
        )),
        Ty::Var(_) => Err(LayoutError::new("an inference variable has no layout")),
        Ty::Error => Err(LayoutError::new("the error type has no layout")),
    }
}

/// v0 lays out only fully-concrete aggregates. A struct/enum that declares
/// generic parameters (whether or not `args` supplies them) needs
/// substitution, which is deferred; refuse it with a clear message.
fn require_monomorphic(
    type_params: &[impl Sized],
    args: &[Ty],
    what: &str,
) -> Result<(), LayoutError> {
    if type_params.is_empty() && args.is_empty() {
        Ok(())
    } else {
        Err(LayoutError::new(format!(
            "generic {what} layout needs monomorphization, deferred in v0"
        )))
    }
}

/// The `#[repr(C)]` layout of a sequence of fields in declaration order: each
/// field at its natural alignment, the whole aligned to the maximum field
/// alignment and padded to a multiple of it.
fn aggregate_layout(
    fields: impl IntoIterator<Item = Ty>,
    types: &TypeckResult,
) -> Result<Layout, LayoutError> {
    let mut offset = 0u64;
    let mut align = 1u64;
    for field in fields {
        let field_layout = layout_of(&field, types)?;
        offset = align_up(offset, field_layout.align);
        offset += field_layout.size;
        align = align.max(field_layout.align);
    }
    Ok(Layout {
        size: align_up(offset, align),
        align,
    })
}

/// The layout of an enum: a [`DISCRIMINANT_SIZE`] tag followed by the largest
/// variant's payload, the whole sized/aligned to hold any variant. The
/// discriminant is the variant's declaration-order index (see
/// [`DISCRIMINANT_SIZE`]); v0 uses no niche packing.
fn enum_layout(
    variants: impl IntoIterator<Item = Vec<Ty>>,
    types: &TypeckResult,
) -> Result<Layout, LayoutError> {
    let mut align = DISCRIMINANT_ALIGN;
    let mut payload_size = 0u64;
    for payload in variants {
        // Each variant's payload is itself an aggregate laid out after the tag.
        let body = aggregate_layout(payload, types)?;
        // The payload begins after the tag, rounded up to the payload's align.
        let start = align_up(DISCRIMINANT_SIZE, body.align.max(1));
        payload_size = payload_size.max(start + body.size);
        align = align.max(body.align);
    }
    // If there were no variants at all, the type is still tag-sized.
    let unpadded = payload_size.max(DISCRIMINANT_SIZE);
    Ok(Layout {
        size: align_up(unpadded, align),
        align,
    })
}

#[cfg(test)]
mod tests {
    use super::{ABI_VERSION, Layout, align_up, layout_of};
    use tuo_types::{IntKind, Ty, TypeckResult, WrapperKind};

    fn wrap(kind: WrapperKind, inner: Ty) -> Ty {
        Ty::Wrapper(kind, Box::new(inner))
    }

    #[test]
    fn the_abi_is_version_zero() {
        // A deliberate tripwire: bump this in the same commit that changes a
        // layout, never silently.
        assert_eq!(ABI_VERSION, 0);
    }

    #[test]
    fn primitives_have_their_exact_widths() {
        let t = &TypeckResult::default();
        assert_eq!(layout_of(&Ty::Unit, t).unwrap(), Layout::ZERO);
        assert_eq!(layout_of(&Ty::Bool, t).unwrap(), Layout::scalar(1));
        assert_eq!(layout_of(&Ty::Char, t).unwrap(), Layout::scalar(4));
        assert_eq!(
            layout_of(&Ty::Int(IntKind::I8), t).unwrap(),
            Layout::scalar(1)
        );
        assert_eq!(
            layout_of(&Ty::Int(IntKind::I32), t).unwrap(),
            Layout::scalar(4)
        );
        assert_eq!(
            layout_of(&Ty::Int(IntKind::I64), t).unwrap(),
            Layout::scalar(8)
        );
        // Isize/Usize are pointer-width, matching the interpreter's 64-bit model.
        assert_eq!(
            layout_of(&Ty::Int(IntKind::Usize), t).unwrap(),
            Layout::scalar(8)
        );
    }

    #[test]
    fn headers_are_pointer_word_tuples() {
        let t = &TypeckResult::default();
        // String/Array are (ptr, len, cap) = 3 words; Str is (ptr, len) = 2.
        assert_eq!(layout_of(&Ty::String, t).unwrap(), Layout::words(3));
        assert_eq!(
            layout_of(&Ty::Array(Box::new(Ty::int())), t).unwrap(),
            Layout::words(3)
        );
        assert_eq!(layout_of(&Ty::Str, t).unwrap(), Layout::words(2));
    }

    #[test]
    fn wrappers_are_a_single_pointer() {
        let t = &TypeckResult::default();
        for kind in [WrapperKind::Box, WrapperKind::Shared, WrapperKind::Weak] {
            assert_eq!(
                layout_of(&wrap(kind, Ty::int()), t).unwrap(),
                Layout::pointer(),
                "{kind:?} is one word"
            );
        }
    }

    #[test]
    fn tuples_pack_in_declaration_order_with_c_padding() {
        let t = &TypeckResult::default();
        // (I8, I32): i8 at 0, pad to 4, i32 at 4..8 → size 8, align 4.
        let pair = Ty::Tuple(vec![Ty::Int(IntKind::I8), Ty::Int(IntKind::I32)]);
        assert_eq!(layout_of(&pair, t).unwrap(), Layout { size: 8, align: 4 });
        // (I8, I8): tightly packed, size 2, align 1.
        let bytes = Ty::Tuple(vec![Ty::Int(IntKind::I8), Ty::Int(IntKind::I8)]);
        assert_eq!(layout_of(&bytes, t).unwrap(), Layout { size: 2, align: 1 });
    }

    #[test]
    fn option_is_a_two_variant_enum_with_an_explicit_tag() {
        let t = &TypeckResult::default();
        // Option[I64]: u32 tag, pad to 8, i64 payload at 8..16 → size 16, align 8.
        let opt = Ty::Option(Box::new(Ty::Int(IntKind::I64)));
        assert_eq!(layout_of(&opt, t).unwrap(), Layout { size: 16, align: 8 });
        // Option[Bool]: tag(4) + bool(1) → 5, padded to align 4 → size 8.
        let opt_bool = Ty::Option(Box::new(Ty::Bool));
        assert_eq!(
            layout_of(&opt_bool, t).unwrap(),
            Layout { size: 8, align: 4 }
        );
    }

    #[test]
    fn result_lays_out_to_the_larger_variant() {
        let t = &TypeckResult::default();
        // Result[I64, I8]: tag + max(payload) = tag(4)→pad8 + i64(8) = 16.
        let res = Ty::Result(
            Box::new(Ty::Int(IntKind::I64)),
            Box::new(Ty::Int(IntKind::I8)),
        );
        assert_eq!(layout_of(&res, t).unwrap(), Layout { size: 16, align: 8 });
    }

    #[test]
    fn types_without_a_v0_layout_are_refused() {
        let t = &TypeckResult::default();
        assert!(layout_of(&Ty::Never, t).is_err());
        assert!(layout_of(&Ty::Error, t).is_err());
        assert!(layout_of(&Ty::Range(Box::new(Ty::int())), t).is_err());
    }

    #[test]
    fn align_up_rounds_to_powers_of_two() {
        assert_eq!(align_up(0, 8), 0);
        assert_eq!(align_up(1, 8), 8);
        assert_eq!(align_up(8, 8), 8);
        assert_eq!(align_up(9, 8), 16);
        assert_eq!(align_up(5, 1), 5);
    }
}
