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
    aggregate_layout_and_offsets(fields, types).map(|(layout, _)| layout)
}

/// The `#[repr(C)]` layout of a sequence of fields, together with the byte
/// offset of each field within the aggregate (declaration order). The offsets
/// are what a backend adds to an aggregate's base address to reach a
/// [`Projection::Field`](tuo_mir::Projection) — the single source of truth for
/// where a field lives, shared by every backend so none invents its own.
fn aggregate_layout_and_offsets(
    fields: impl IntoIterator<Item = Ty>,
    types: &TypeckResult,
) -> Result<(Layout, Vec<u64>), LayoutError> {
    let mut offset = 0u64;
    let mut align = 1u64;
    let mut offsets = Vec::new();
    for field in fields {
        let field_layout = layout_of(&field, types)?;
        offset = align_up(offset, field_layout.align);
        offsets.push(offset);
        offset += field_layout.size;
        align = align.max(field_layout.align);
    }
    Ok((
        Layout {
            size: align_up(offset, align),
            align,
        },
        offsets,
    ))
}

/// The byte offsets of a **struct** or **tuple** value's fields, in declaration
/// order — the offset a backend adds to the aggregate's base address to reach
/// field *n*. For a `Struct` the field types come from the type's shape; for a
/// `Tuple` they are the element types directly.
///
/// # Errors
///
/// [`LayoutError`] if `ty` is not a struct/tuple, is generic, or any field has
/// no v0 layout — the same cases [`layout_of`] refuses.
pub fn struct_field_offsets(ty: &Ty, types: &TypeckResult) -> Result<Vec<u64>, LayoutError> {
    match ty {
        Ty::Tuple(fields) => {
            aggregate_layout_and_offsets(fields.iter().cloned(), types).map(|(_, offs)| offs)
        }
        Ty::Struct(symbol, args) => {
            let shape = types
                .struct_shape(*symbol)
                .ok_or_else(|| LayoutError::new("field offsets of an unknown struct"))?;
            require_monomorphic(&shape.type_params, args, "struct")?;
            aggregate_layout_and_offsets(shape.fields.iter().map(|(_, ty)| ty.clone()), types)
                .map(|(_, offs)| offs)
        }
        _ => Err(LayoutError::new(
            "field offsets requested for a non-struct, non-tuple type",
        )),
    }
}

/// The byte offsets of an enum **variant's payload fields**, measured from the
/// start of the enum value (i.e. already including the leading discriminant).
/// Variant `variant` is the declaration-order index; each payload field is laid
/// out `#[repr(C)]` starting after the [`DISCRIMINANT_SIZE`] tag, aligned to the
/// payload's alignment. Handles user enums and the canonical `Option`/`Result`.
///
/// # Errors
///
/// [`LayoutError`] if `ty` is not an enum-like type, `variant` is out of range,
/// it is generic, or a payload field has no v0 layout.
pub fn variant_field_offsets(
    ty: &Ty,
    variant: usize,
    types: &TypeckResult,
) -> Result<Vec<u64>, LayoutError> {
    let payload: Vec<Ty> = match ty {
        Ty::Enum(symbol, args) => {
            let shape = types
                .enum_shape(*symbol)
                .ok_or_else(|| LayoutError::new("field offsets of an unknown enum"))?;
            require_monomorphic(&shape.type_params, args, "enum")?;
            let (_, fields) = shape
                .variants
                .get(variant)
                .ok_or_else(|| LayoutError::new("enum variant index out of range"))?;
            fields.iter().map(|(_, ty)| ty.clone()).collect()
        }
        Ty::Option(inner) => match variant {
            0 => Vec::new(),
            1 => vec![(**inner).clone()],
            _ => return Err(LayoutError::new("Option variant index out of range")),
        },
        Ty::Result(ok, err) => match variant {
            0 => vec![(**ok).clone()],
            1 => vec![(**err).clone()],
            _ => return Err(LayoutError::new("Result variant index out of range")),
        },
        _ => {
            return Err(LayoutError::new(
                "variant field offsets requested for a non-enum type",
            ));
        }
    };

    // The payload begins after the tag, rounded up to the payload's alignment
    // (matching `enum_layout`), then fields are laid out `#[repr(C)]` from there.
    let (body, mut offsets) = aggregate_layout_and_offsets(payload, types)?;
    let start = align_up(DISCRIMINANT_SIZE, body.align.max(1));
    for off in &mut offsets {
        *off += start;
    }
    Ok(offsets)
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

    #[test]
    fn tuple_field_offsets_follow_c_padding() {
        use super::struct_field_offsets;
        let t = &TypeckResult::default();
        // (I8, I32): field 0 at 0, field 1 at 4 (i8 padded up to i32 align).
        let pair = Ty::Tuple(vec![Ty::Int(IntKind::I8), Ty::Int(IntKind::I32)]);
        assert_eq!(struct_field_offsets(&pair, t).unwrap(), vec![0, 4]);
        // (I8, I8): tightly packed, offsets 0 and 1.
        let bytes = Ty::Tuple(vec![Ty::Int(IntKind::I8), Ty::Int(IntKind::I8)]);
        assert_eq!(struct_field_offsets(&bytes, t).unwrap(), vec![0, 1]);
        // (I64, I8, I64): 0, 8, then pad to 16.
        let mixed = Ty::Tuple(vec![
            Ty::Int(IntKind::I64),
            Ty::Int(IntKind::I8),
            Ty::Int(IntKind::I64),
        ]);
        assert_eq!(struct_field_offsets(&mixed, t).unwrap(), vec![0, 8, 16]);
    }

    #[test]
    fn variant_field_offsets_sit_after_the_discriminant() {
        use super::variant_field_offsets;
        let t = &TypeckResult::default();
        // Option[I64]: Some's payload is after the tag, padded to 8 → offset 8.
        let opt = Ty::Option(Box::new(Ty::Int(IntKind::I64)));
        assert_eq!(variant_field_offsets(&opt, 1, t).unwrap(), vec![8]);
        // None has no payload.
        assert_eq!(variant_field_offsets(&opt, 0, t).unwrap(), Vec::<u64>::new());
        // Result[I64, I8]: Ok payload at 8 (pad to 8), Err payload at 4 (tag+align1).
        let res = Ty::Result(
            Box::new(Ty::Int(IntKind::I64)),
            Box::new(Ty::Int(IntKind::I8)),
        );
        assert_eq!(variant_field_offsets(&res, 0, t).unwrap(), vec![8]);
        assert_eq!(variant_field_offsets(&res, 1, t).unwrap(), vec![4]);
    }

    #[test]
    fn field_offsets_refuse_non_aggregates() {
        use super::{struct_field_offsets, variant_field_offsets};
        let t = &TypeckResult::default();
        assert!(struct_field_offsets(&Ty::int(), t).is_err());
        assert!(variant_field_offsets(&Ty::int(), 0, t).is_err());
        // An out-of-range variant is refused, not silently zero.
        let opt = Ty::Option(Box::new(Ty::Bool));
        assert!(variant_field_offsets(&opt, 2, t).is_err());
    }
}
