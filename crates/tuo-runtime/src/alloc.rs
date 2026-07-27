//! The memory allocation boundary.
//!
//! Every heap allocation a tuonelang program makes — for `Box`, `Shared`,
//! `String`, and `Array` — passes through two C-ABI runtime symbols, so the
//! allocator is a single swappable seam and no backend embeds `malloc` calls
//! directly. See [`specification/abi.md`](../../../specification/abi.md).
//!
//! ```text
//! void *tuo_rt_alloc(size_t size, size_t align);            // never null; traps on OOM
//! void  tuo_rt_dealloc(void *ptr, size_t size, size_t align);
//! ```
//!
//! The size/align are always the *layout* values from [`crate::abi`]. Carrying
//! them back into `dealloc` keeps the runtime free of allocation metadata.
//!
//! The **operative allocator is C** ([`alloc_runtime_c_source`]) — the build
//! driver links it into every built binary so a generated executable needs no
//! Rust runtime, and because this crate forbids `unsafe`, a real aligned heap
//! allocator cannot live here anyway. What *does* live here in Rust is the
//! allocator's **policy**, the part that is pure and testable independent of
//! any real allocation: the zero-size sentinel, the minimum alignment, and the
//! decision to trap (never return null) on failure. The Rust policy and the C
//! source are pinned to agree by this module's tests, so the boundary's
//! observable contract is fixed without invoking a C compiler.

use crate::TrapCode;

/// The name of the C-ABI symbol generated code calls to allocate.
pub const ALLOC_SYMBOL: &str = "tuo_rt_alloc";

/// The name of the C-ABI symbol generated code calls to free.
pub const DEALLOC_SYMBOL: &str = "tuo_rt_dealloc";

/// The non-null pointer value returned for a zero-size allocation.
///
/// A zero-size request must still return a non-null pointer that is never
/// dereferenced and is handed back to `dealloc` unchanged. The value is a fixed
/// sentinel equal to the largest alignment v0 requests (a pointer-word times
/// eight), so it satisfies any `align` a zero-size layout could carry.
pub const ZERO_SIZE_SENTINEL: usize = 64;

/// The trap code the allocator raises when it cannot satisfy a request.
///
/// Out-of-memory is a deterministic abort, not a value a program can observe;
/// the closest stable [`TrapCode`] for "the requested size is unsatisfiable" is
/// [`TrapCode::IntegerOverflow`] (an allocation that overflows the address
/// space is the same failure class). Both the Rust policy and the C source use
/// this exact code so a native OOM aborts identically to the reference.
pub const ALLOC_FAILURE_TRAP: TrapCode = TrapCode::IntegerOverflow;

/// What `tuo_rt_alloc` does with a request, before touching any real memory —
/// the pure policy the C allocator implements.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AllocDecision {
    /// A zero-size request: return the [`ZERO_SIZE_SENTINEL`] without
    /// allocating. `align` never matters — the sentinel satisfies every v0
    /// alignment.
    Sentinel,
    /// A real allocation of `bytes` at `align`, where `align` has been raised
    /// to at least a pointer word and `bytes` rounded up to a multiple of
    /// `align` (the shape C's `aligned_alloc` requires). If the underlying
    /// allocator returns null, the runtime raises [`ALLOC_FAILURE_TRAP`].
    Allocate {
        /// The allocation size, rounded up to a multiple of `align`.
        bytes: usize,
        /// The alignment, at least a pointer word, a power of two.
        align: usize,
    },
}

/// The pure allocation policy: given a layout's `size`/`align`, decide what the
/// allocator does. This is exactly the branch structure of `tuo_rt_alloc` in
/// [`alloc_runtime_c_source`], factored out so it is testable without a real
/// heap.
#[must_use]
pub fn alloc_decision(size: usize, align: usize) -> AllocDecision {
    if size == 0 {
        return AllocDecision::Sentinel;
    }
    let align = align.max(size_of::<usize>());
    // Round `size` up to a multiple of `align`, saturating so an overflowing
    // request degrades to the max — which the C allocator will fail and trap on
    // rather than wrapping to a small allocation.
    let bytes = size
        .checked_add(align - 1)
        .map_or(usize::MAX, |sum| sum & !(align - 1));
    AllocDecision::Allocate { bytes, align }
}

/// The C source of the runtime's allocation boundary.
///
/// The build driver writes this to a `.c` file, compiles it, and links it into
/// the final executable so [`ALLOC_SYMBOL`]/[`DEALLOC_SYMBOL`] resolve. It is
/// freestanding C over `<stdlib.h>` and `<stdint.h>`, so it compiles anywhere
/// the platform `cc` runs and drags no Rust runtime into the target binary.
///
/// Behavior mirrors [`alloc_decision`] and
/// [`specification/abi.md`](../../../specification/abi.md):
///
/// - `tuo_rt_alloc(size, align)` returns an `align`-aligned block of at least
///   `size` bytes, never null; a zero `size` returns the
///   [`ZERO_SIZE_SENTINEL`]; a failed allocation calls `tuo_rt_trap` with
///   [`ALLOC_FAILURE_TRAP`] and never returns.
/// - `tuo_rt_dealloc(ptr, size, align)` releases a block via `free`; a
///   zero-size block (the sentinel) is a no-op. `size`/`align` are
///   informational — `free` needs only the pointer.
#[must_use]
pub fn alloc_runtime_c_source() -> String {
    // `aligned_alloc` (C11) requires `size` be a multiple of `align`; the
    // rounding here matches `alloc_decision`. `tuo_rt_trap` is declared by
    // `trap_runtime_c_source`; both C units link together, so the extern
    // resolves against it.
    let sentinel = ZERO_SIZE_SENTINEL;
    let failure = ALLOC_FAILURE_TRAP.as_i32();
    format!(
        "#include <stdlib.h>\n\
         #include <stdint.h>\n\
         \n\
         extern void tuo_rt_trap(int code);\n\
         \n\
         void *tuo_rt_alloc(size_t size, size_t align) {{\n\
         \x20   if (size == 0) return (void *){sentinel};\n\
         \x20   if (align < sizeof(void *)) align = sizeof(void *);\n\
         \x20   size_t rounded = (size + (align - 1)) & ~(align - 1);\n\
         \x20   void *p = aligned_alloc(align, rounded);\n\
         \x20   if (p == NULL) tuo_rt_trap({failure});\n\
         \x20   return p;\n\
         }}\n\
         \n\
         void tuo_rt_dealloc(void *ptr, size_t size, size_t align) {{\n\
         \x20   (void)size;\n\
         \x20   (void)align;\n\
         \x20   if (ptr == NULL || ptr == (void *){sentinel}) return;\n\
         \x20   free(ptr);\n\
         }}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::{
        ALLOC_FAILURE_TRAP, ALLOC_SYMBOL, AllocDecision, DEALLOC_SYMBOL, ZERO_SIZE_SENTINEL,
        alloc_decision, alloc_runtime_c_source,
    };

    #[test]
    fn a_zero_size_request_returns_the_sentinel() {
        assert_eq!(alloc_decision(0, 8), AllocDecision::Sentinel);
        assert_eq!(alloc_decision(0, 1), AllocDecision::Sentinel);
        assert_eq!(ZERO_SIZE_SENTINEL % 8, 0, "the sentinel is 8-aligned");
    }

    #[test]
    fn a_real_request_rounds_size_up_and_raises_small_alignments() {
        // align raised to a pointer word (8), size rounded up to a multiple of it.
        assert_eq!(
            alloc_decision(1, 1),
            AllocDecision::Allocate { bytes: 8, align: 8 }
        );
        assert_eq!(
            alloc_decision(8, 8),
            AllocDecision::Allocate { bytes: 8, align: 8 }
        );
        assert_eq!(
            alloc_decision(9, 8),
            AllocDecision::Allocate {
                bytes: 16,
                align: 8
            }
        );
        // A larger alignment is honored as-is.
        assert_eq!(
            alloc_decision(1, 16),
            AllocDecision::Allocate {
                bytes: 16,
                align: 16
            }
        );
    }

    #[test]
    fn the_c_source_defines_both_boundary_symbols_and_matches_the_policy() {
        let source = alloc_runtime_c_source();
        assert!(source.contains(&format!("void *{ALLOC_SYMBOL}(")));
        assert!(source.contains(&format!("void {DEALLOC_SYMBOL}(")));
        // OOM routes to the trap with the policy's code; never returns null.
        assert!(source.contains(&format!("tuo_rt_trap({});", ALLOC_FAILURE_TRAP.as_i32())));
        assert!(source.contains("aligned_alloc(align, rounded)"));
        // The zero-size sentinel value matches the Rust policy.
        assert!(source.contains(&format!("return (void *){ZERO_SIZE_SENTINEL};")));
        assert!(source.contains(&format!("ptr == (void *){ZERO_SIZE_SENTINEL}")));
    }
}
