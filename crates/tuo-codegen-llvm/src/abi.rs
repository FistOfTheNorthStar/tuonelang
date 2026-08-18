//! The scalar ABI: how v0's register-held types map to LLVM scalar types.
//!
//! This mirrors the Cranelift backend's `abi` module exactly, so the two
//! backends and the interpreter share one memory model for the scalar core.
//! Every supported value is an LLVM integer of the type's exact width — or an
//! IEEE-754 `float`/`double` — held in a virtual register; the mapping is
//! total on that subset and returns `None` for everything else (aggregates,
//! arrays, strings), which the caller turns into a
//! [`CodegenError::unsupported`](tuo_codegen::CodegenError). The interpreter
//! is the reference for the rejected cases.

use inkwell::AddressSpace;
use inkwell::context::Context;
use inkwell::types::{BasicTypeEnum, FloatType, IntType};
use tuo_types::{FloatKind, IntKind, Ty};

/// The LLVM scalar type a supported scalar `ty` lowers to, or `None` if `ty`
/// is outside the backend's v0 subset.
///
/// `Bool` is an `i8` (0 or 1, matching the Cranelift backend and the runtime
/// ABI), `Char` a 32-bit integer (a Unicode scalar value), each integer
/// kind its exact width (`Isize`/`Usize` are 64-bit, matching the interpreter's
/// target-independent choice), and each float kind its IEEE-754 type
/// (`float`/`double`). A function value (ADR-0008 Tier 1) is a single
/// pointer-width code pointer, mapped to the opaque `ptr` type
/// (`layout_of(Ty::Fn) = pointer()`). `Unit` has no register representation —
/// a function returning `()` is handled specially by the lowering, not here.
#[must_use]
pub(crate) fn scalar_type<'ctx>(ctx: &'ctx Context, ty: &Ty) -> Option<BasicTypeEnum<'ctx>> {
    match ty {
        Ty::Bool => Some(ctx.i8_type().into()),
        Ty::Char => Some(ctx.i32_type().into()),
        Ty::Int(kind) => Some(int_type(ctx, *kind).into()),
        Ty::Float(kind) => Some(float_type(ctx, *kind).into()),
        // A function value is a code pointer — the opaque pointer scalar.
        Ty::Fn(_) => Some(ctx.ptr_type(AddressSpace::default()).into()),
        _ => None,
    }
}

/// The LLVM floating-point type of a [`FloatKind`].
#[must_use]
pub(crate) fn float_type(ctx: &Context, kind: FloatKind) -> FloatType<'_> {
    match kind {
        FloatKind::F32 => ctx.f32_type(),
        FloatKind::F64 => ctx.f64_type(),
    }
}

/// The LLVM integer type of an [`IntKind`].
#[must_use]
pub(crate) fn int_type(ctx: &Context, kind: IntKind) -> IntType<'_> {
    match int_width_bits(kind) {
        8 => ctx.i8_type(),
        16 => ctx.i16_type(),
        32 => ctx.i32_type(),
        _ => ctx.i64_type(),
    }
}

/// The bit width of an integer kind. `Isize`/`Usize` are 64-bit — the same
/// width the interpreter and the Cranelift backend model them at, so results
/// agree across all three.
#[must_use]
pub(crate) fn int_width_bits(kind: IntKind) -> u32 {
    match kind {
        IntKind::I8 | IntKind::U8 => 8,
        IntKind::I16 | IntKind::U16 => 16,
        IntKind::I32 | IntKind::U32 => 32,
        IntKind::I64 | IntKind::U64 | IntKind::Isize | IntKind::Usize => 64,
    }
}

/// Whether an integer kind is signed. Signedness selects signed vs unsigned
/// LLVM instructions (division, comparison, extension) and the sign of a
/// constant.
#[must_use]
pub(crate) fn is_signed(kind: IntKind) -> bool {
    match kind {
        IntKind::I8 | IntKind::I16 | IntKind::I32 | IntKind::I64 | IntKind::Isize => true,
        IntKind::U8 | IntKind::U16 | IntKind::U32 | IntKind::U64 | IntKind::Usize => false,
    }
}

/// The integer kind `ty` returns, if it is an integer type — the shape the v0
/// entry ABI requires of a buildable `main`, since the entry's return value
/// becomes the exit status. `None` for any non-integer return.
#[must_use]
pub(crate) fn entry_returns_int(ty: &Ty) -> Option<IntKind> {
    match ty {
        Ty::Int(kind) => Some(*kind),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{int_width_bits, is_signed, scalar_type};
    use inkwell::context::Context;
    use tuo_types::{FloatKind, IntKind, Ty};

    #[test]
    fn scalar_types_map_to_their_register_widths() {
        let ctx = Context::create();
        assert_eq!(scalar_type(&ctx, &Ty::Bool), Some(ctx.i8_type().into()));
        assert_eq!(scalar_type(&ctx, &Ty::Char), Some(ctx.i32_type().into()));
        assert_eq!(
            scalar_type(&ctx, &Ty::Int(IntKind::I32)),
            Some(ctx.i32_type().into())
        );
        assert_eq!(
            scalar_type(&ctx, &Ty::Int(IntKind::Usize)),
            Some(ctx.i64_type().into())
        );
        assert_eq!(
            scalar_type(&ctx, &Ty::Float(FloatKind::F32)),
            Some(ctx.f32_type().into())
        );
        assert_eq!(
            scalar_type(&ctx, &Ty::Float(FloatKind::F64)),
            Some(ctx.f64_type().into())
        );
        // Outside the subset → no register mapping (caller reports unsupported).
        assert_eq!(scalar_type(&ctx, &Ty::String), None);
        assert_eq!(scalar_type(&ctx, &Ty::Array(Box::new(Ty::int()))), None);
    }

    #[test]
    fn integer_widths_and_signedness_match_the_kinds() {
        assert_eq!(int_width_bits(IntKind::I8), 8);
        assert_eq!(int_width_bits(IntKind::Usize), 64);
        assert!(is_signed(IntKind::I64));
        assert!(!is_signed(IntKind::U8));
    }
}
