//! ABI layout tests: the computed layouts obey the `#[repr(C)]` rule.
//!
//! The unit tests in `src/abi.rs` pin specific expected numbers. This suite
//! pins the *rule*: for the shapes the ABI describes, the layout
//! `tuo_runtime::abi` computes must equal what a real `#[repr(C)]` Rust type of
//! the same shape has — same size, same alignment, same field packing. Rust's
//! `#[repr(C)]` is the C layout the spec references, so agreeing with it is the
//! ground-truth check that `layout_of` implements the documented rule rather
//! than a look-alike.

use tuo_runtime::abi::{Layout, POINTER_SIZE, layout_of};
use tuo_types::{IntKind, Ty, TypeckResult};

/// The layout the ABI computes for `ty` in an empty type environment.
fn abi(ty: &Ty) -> Layout {
    layout_of(ty, &TypeckResult::default()).expect("a v0 value type has a layout")
}

/// The `#[repr(C)]` ground truth: `(size, align)` of a Rust type `T`.
fn repr_c<T>() -> Layout {
    Layout {
        size: size_of::<T>() as u64,
        align: align_of::<T>() as u64,
    }
}

#[test]
fn scalars_match_their_rust_repr_c_counterparts() {
    assert_eq!(abi(&Ty::Bool), repr_c::<bool>());
    assert_eq!(abi(&Ty::Char), repr_c::<char>()); // char is 4-byte aligned in Rust
    assert_eq!(abi(&Ty::Int(IntKind::I8)), repr_c::<i8>());
    assert_eq!(abi(&Ty::Int(IntKind::I16)), repr_c::<i16>());
    assert_eq!(abi(&Ty::Int(IntKind::I32)), repr_c::<i32>());
    assert_eq!(abi(&Ty::Int(IntKind::I64)), repr_c::<i64>());
    assert_eq!(abi(&Ty::Int(IntKind::U64)), repr_c::<u64>());
}

#[test]
fn the_string_and_array_headers_match_a_three_word_repr_c_struct() {
    // (ptr, len, cap) — three pointer-words. Model it as a repr(C) struct.
    #[repr(C)]
    struct ThreeWords {
        ptr: *const u8,
        len: usize,
        cap: usize,
    }
    assert_eq!(abi(&Ty::String), repr_c::<ThreeWords>());
    assert_eq!(abi(&Ty::Array(Box::new(Ty::int()))), repr_c::<ThreeWords>());
}

#[test]
fn the_str_slice_header_matches_a_two_word_repr_c_struct() {
    #[repr(C)]
    struct TwoWords {
        ptr: *const u8,
        len: usize,
    }
    assert_eq!(abi(&Ty::Str), repr_c::<TwoWords>());
}

#[test]
fn fixed_arrays_match_rust_repr_c_arrays() {
    // ADR-0004 Stage 2: `[T; N]` is inline — size = N × stride(T),
    // align = align(T), element i at i × stride(T) — exactly a Rust `[T; N]`.
    let fixed = |elem: Ty, n: u64| Ty::FixedArray(Box::new(elem), n);
    // [I8; 3] = size 3 / align 1.
    assert_eq!(abi(&fixed(Ty::Int(IntKind::I8), 3)), repr_c::<[i8; 3]>());
    // [I32; 0] = size 0 / align 4 — a ZST that still aligns like I32.
    assert_eq!(abi(&fixed(Ty::Int(IntKind::I32), 0)), repr_c::<[i32; 0]>());
    // [I64; 4] = size 32 / align 8.
    assert_eq!(abi(&fixed(Ty::Int(IntKind::I64), 4)), repr_c::<[i64; 4]>());
    // [(I8, I32); 2]: the element strides at 8 (tail padding included), so
    // size 16 / align 4.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Pair {
        a: i8,
        b: i32,
    }
    let pair = Ty::Tuple(vec![Ty::Int(IntKind::I8), Ty::Int(IntKind::I32)]);
    assert_eq!(abi(&fixed(pair, 2)), repr_c::<[Pair; 2]>());
    // Nesting: [[I8; 3]; 2] = size 6 / align 1.
    let inner = fixed(Ty::Int(IntKind::I8), 3);
    assert_eq!(abi(&fixed(inner, 2)), repr_c::<[[i8; 3]; 2]>());
}

#[test]
fn a_mixed_tuple_packs_exactly_like_repr_c() {
    // (I8, I32, I8): i8@0, pad, i32@4, i8@8, pad to align 4 → size 12, align 4.
    #[repr(C)]
    struct Mixed {
        a: i8,
        b: i32,
        c: i8,
    }
    let ty = Ty::Tuple(vec![
        Ty::Int(IntKind::I8),
        Ty::Int(IntKind::I32),
        Ty::Int(IntKind::I8),
    ]);
    assert_eq!(abi(&ty), repr_c::<Mixed>());
}

#[test]
fn a_nested_tuple_packs_like_a_nested_repr_c_struct() {
    #[repr(C)]
    struct Inner {
        a: i16,
        b: i64,
    }
    #[repr(C)]
    struct Outer {
        head: i8,
        inner: Inner,
    }
    let inner = Ty::Tuple(vec![Ty::Int(IntKind::I16), Ty::Int(IntKind::I64)]);
    let outer = Ty::Tuple(vec![Ty::Int(IntKind::I8), inner]);
    assert_eq!(abi(&outer), repr_c::<Outer>());
}

#[test]
fn every_wrapper_is_exactly_one_pointer() {
    use tuo_types::WrapperKind;
    for kind in [WrapperKind::Box, WrapperKind::Shared, WrapperKind::Weak] {
        let ty = Ty::Wrapper(kind, Box::new(Ty::int()));
        assert_eq!(abi(&ty).size, POINTER_SIZE);
        assert_eq!(abi(&ty).align, POINTER_SIZE);
    }
}
