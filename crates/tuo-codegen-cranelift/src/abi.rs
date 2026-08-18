//! The scalar ABI: how v0's register-held types map to Cranelift types.
//!
//! This is the whole of the type universe the backend lowers today — the
//! scalar core. Every supported value lives in a single machine register, so
//! the mapping is total on that subset and returns `None` for everything else
//! (aggregates, arrays, strings), which the caller turns into a
//! [`CodegenError::unsupported`](tuo_codegen::CodegenError). The interpreter is
//! the reference for the rejected cases.

use cranelift_codegen::ir::{Type, types};
use tuo_types::{FloatKind, IntKind, Ty};

/// The Cranelift register type a supported scalar `ty` lowers to, or `None` if
/// `ty` is outside the backend's v0 subset.
///
/// `Bool` is a byte (`i8`, 0 or 1), `Char` a 32-bit scalar value, each
/// integer kind its exact width (`Isize`/`Usize` are 64-bit, matching the
/// interpreter's target-independent choice), and each float kind its IEEE-754
/// register type (`F32`/`F64`). A function value (ADR-0008 Tier 1) is a single
/// pointer-width code pointer (`I64` on the 64-bit targets, matching
/// `Isize`/`Usize` and `layout_of(Ty::Fn) = pointer()`). `Unit` has no
/// register representation — a function returning `()` is handled specially by
/// the lowering, not here.
#[must_use]
pub(crate) fn scalar_type(ty: &Ty) -> Option<Type> {
    match ty {
        Ty::Bool => Some(types::I8),
        Ty::Char => Some(types::I32),
        Ty::Int(kind) => Some(int_type(*kind)),
        Ty::Float(kind) => Some(float_type(*kind)),
        // A function value is a pointer-width code pointer. The scalar core is
        // 64-bit (like `Isize`/`Usize`), so it maps to `I64` — the target's
        // pointer type on every supported platform.
        Ty::Fn(_) => Some(types::I64),
        _ => None,
    }
}

/// The Cranelift floating-point type of a [`FloatKind`].
#[must_use]
pub(crate) fn float_type(kind: FloatKind) -> Type {
    match kind {
        FloatKind::F32 => types::F32,
        FloatKind::F64 => types::F64,
    }
}

/// The Cranelift integer type of an [`IntKind`].
#[must_use]
pub(crate) fn int_type(kind: IntKind) -> Type {
    match int_width_bits(kind) {
        8 => types::I8,
        16 => types::I16,
        32 => types::I32,
        _ => types::I64,
    }
}

/// The bit width of an integer kind. `Isize`/`Usize` are 64-bit — the same
/// width the interpreter models them at, so results agree across the two.
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
/// Cranelift opcodes (division, comparison, extension).
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
    use cranelift_codegen::ir::types;
    use tuo_types::{FloatKind, IntKind, Ty};

    #[test]
    fn scalar_types_map_to_their_register_widths() {
        assert_eq!(scalar_type(&Ty::Bool), Some(types::I8));
        assert_eq!(scalar_type(&Ty::Char), Some(types::I32));
        assert_eq!(scalar_type(&Ty::Int(IntKind::I32)), Some(types::I32));
        assert_eq!(scalar_type(&Ty::Int(IntKind::Usize)), Some(types::I64));
        assert_eq!(scalar_type(&Ty::Float(FloatKind::F32)), Some(types::F32));
        assert_eq!(scalar_type(&Ty::Float(FloatKind::F64)), Some(types::F64));
        // Outside the subset → no register mapping (caller reports unsupported).
        assert_eq!(scalar_type(&Ty::String), None);
        assert_eq!(scalar_type(&Ty::Array(Box::new(Ty::int()))), None);
    }

    #[test]
    fn integer_widths_and_signedness_match_the_kinds() {
        assert_eq!(int_width_bits(IntKind::I8), 8);
        assert_eq!(int_width_bits(IntKind::Usize), 64);
        assert!(is_signed(IntKind::I64));
        assert!(!is_signed(IntKind::U8));
    }
}
