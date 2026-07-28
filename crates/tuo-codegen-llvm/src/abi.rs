//! The scalar ABI: how v0's register-held types map to LLVM integer types.
//!
//! This mirrors the Cranelift backend's `abi` module exactly, so the two
//! backends and the interpreter share one memory model for the scalar core.
//! Every supported value is an LLVM integer of the type's exact width, held in
//! a virtual register; the mapping is total on that subset and returns `None`
//! for everything else (aggregates, arrays, strings, floats), which the caller
//! turns into a [`CodegenError::unsupported`](tuo_codegen::CodegenError). The
//! interpreter is the reference for the rejected cases.

use inkwell::context::Context;
use inkwell::types::IntType;
use tuo_types::{IntKind, Ty};

/// The LLVM integer type a supported scalar `ty` lowers to, or `None` if `ty`
/// is outside the backend's v0 subset.
///
/// `Bool` is an `i8` (0 or 1, matching the Cranelift backend and the runtime
/// ABI), `Char` a 32-bit integer (a Unicode scalar value), and each integer
/// kind its exact width (`Isize`/`Usize` are 64-bit, matching the interpreter's
/// target-independent choice). `Unit` has no register representation — a
/// function returning `()` is handled specially by the lowering, not here.
#[must_use]
pub(crate) fn scalar_type<'ctx>(ctx: &'ctx Context, ty: &Ty) -> Option<IntType<'ctx>> {
    match ty {
        Ty::Bool => Some(ctx.i8_type()),
        Ty::Char => Some(ctx.i32_type()),
        Ty::Int(kind) => Some(int_type(ctx, *kind)),
        _ => None,
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
    use tuo_types::{IntKind, Ty};

    #[test]
    fn scalar_types_map_to_their_register_widths() {
        let ctx = Context::create();
        assert_eq!(scalar_type(&ctx, &Ty::Bool), Some(ctx.i8_type()));
        assert_eq!(scalar_type(&ctx, &Ty::Char), Some(ctx.i32_type()));
        assert_eq!(
            scalar_type(&ctx, &Ty::Int(IntKind::I32)),
            Some(ctx.i32_type())
        );
        assert_eq!(
            scalar_type(&ctx, &Ty::Int(IntKind::Usize)),
            Some(ctx.i64_type())
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
