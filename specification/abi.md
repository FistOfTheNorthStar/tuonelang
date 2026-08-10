# The tuonelang runtime ABI (v0)

- **Status:** accepted (unstable — versioned, not yet frozen)
- **ABI version:** `2` (see `tuo_runtime::abi::ABI_VERSION`)
- **Companion crate:** [`tuo-runtime`](../crates/tuo-runtime), which is the
  single normative *implementation* of this document. Where prose and crate
  disagree, the crate's `abi` module — and the tests that pin it — win, and
  this file is corrected to match.

This document specifies how a running tuonelang program lays its values out in
memory, how it starts and stops, how it acquires and releases memory, how it
destroys values, and the internal calling conventions the compiler and runtime
rely on. It is the contract every backend must satisfy and the interpreter must
stay semantically equivalent to.

## Why an ABI now, and why version it

The Cranelift backend today lowers only the **scalar, control-flow core**;
aggregates, strings, arrays, and the memory wrappers are still refused rather
than mis-compiled (see `tuo-codegen-cranelift`). This document therefore
specifies more than any backend currently *emits*: it is the target the
backends grow into, fixed up front so two backends (Cranelift and LLVM) and the
reference interpreter cannot drift into two incompatible memory models.

Because it is not yet frozen, the ABI carries an explicit integer
**version** (`ABI_VERSION`), bumped on any layout-affecting change, exactly as
the machine-output protocol and the diagnostics schema are versioned. Versioning
*before* stability is deliberate: it lets tests assert "this is ABI v0" and lets
a future artifact detect a mismatch instead of silently reinterpreting bytes.

## Design rules

1. **Backend-independent.** Nothing here mentions Cranelift or LLVM. Layouts are
   expressed in bytes and alignment, computed by `tuo_runtime::abi`, which
   depends only on `tuo-types`/`tuo-mir` and the standard library. A backend
   *consults* these layouts; it never defines its own.
2. **Interpreter is the reference for values.** The abstract value the
   interpreter (`tuo-mir-interp`) computes for a program is the meaning; the
   in-memory representation here must encode exactly that value with no
   observable difference. The scalar widths, the enum discriminant numbering,
   and the field order below are chosen to match the interpreter's `Value`.
3. **Host-first, but host-agnostic in shape.** v0 targets the development host.
   Sizes that are "pointer-width" resolve to the host pointer width (8 bytes on
   the supported 64-bit hosts). `Isize`/`Usize` are pointer-width by definition;
   the interpreter models them as 64-bit, which agrees on those hosts.
4. **`#[repr(C)]`, no niche tricks in v0.** Aggregates are laid out in
   declaration order with natural alignment and tail padding — the C rule — so
   the layout is obvious, inspectable, and identical across backends. Optional
   values do **not** use pointer niches in v0; `Option[T]` is an ordinary
   two-variant enum with an explicit discriminant. Niche optimization is a later,
   version-bumping change.

## Primitive ABI

Every primitive is a single value in a machine register or a fixed-width slot in
memory. Widths are exact and match `tuo_types::IntKind`/`FloatKind`.

| Type | Size (bytes) | Align | Encoding |
|------|-------------:|------:|----------|
| `Unit` (`()`) | 0 | 1 | zero-sized; never occupies a register |
| `Bool` | 1 | 1 | `0` = false, `1` = true; no other bit pattern is valid |
| `Char` | 4 | 4 | a Unicode scalar value as `u32` (`0..=0x10FFFF`, excluding surrogates) |
| `I8`/`U8` | 1 | 1 | two's-complement / unsigned |
| `I16`/`U16` | 2 | 2 | two's-complement / unsigned |
| `I32`/`U32` | 4 | 4 | two's-complement / unsigned |
| `I64`/`U64` | 8 | 8 | two's-complement / unsigned |
| `Isize`/`Usize` | 8 (pointer-width) | 8 | index/length type |
| `F32` | 4 | 4 | IEEE-754 binary32 |
| `F64` (`Float`) | 8 | 8 | IEEE-754 binary64 |

Integer arithmetic **traps** on overflow, on division/remainder by zero, and on
`MIN / -1` (Constitution §24); it never wraps. This is a semantic rule, not a
layout rule, but it is part of the ABI because the trap path (below) is how the
running program reports it.

## String representation

`String` is an **owned, heap-allocated, growable** UTF-8 buffer. Its value is a
three-word header, by value:

```
struct tuo_string {
    u8   *ptr;   // pointer to UTF-8 bytes (never null; a dangling non-null for len 0)
    usize len;   // number of valid UTF-8 bytes
    usize cap;   // allocated capacity in bytes, cap >= len
}
```

- Size = 3 × pointer-width (24 bytes on 64-bit), align = pointer-width.
- The bytes are always valid UTF-8; `len` counts bytes, not characters.
- An empty `String` has `len == 0` and a non-null, suitably-aligned `ptr` (no
  allocation is required for the empty string; `ptr` may be a fixed non-null
  sentinel). Backends must not assume `ptr` is dereferenceable when `len == 0`.
- Dropping a `String` frees `ptr`'s allocation (see Destruction).

## Slices

`Str` is a **borrowed** UTF-8 slice: a fat pointer, by value:

```
struct tuo_str {
    u8   *ptr;   // pointer to UTF-8 bytes
    usize len;   // number of bytes
}
```

- Size = 2 × pointer-width (16 bytes), align = pointer-width. Owns nothing;
  dropping it is a no-op.
- In v0 a `Str` only ever points at a string **literal** with static storage
  duration (ADR-0003 ruling 8), so its bytes outlive any borrow. `String`→`Str`
  borrowing is deferred (Q-0012); the representation is already the general fat
  pointer so that later work needs no layout change.

The same fat-pointer shape is the general **slice** layout `[T]` for a future
array-slice type: `(T *ptr, usize len)`. It is specified now so the shape is
fixed; only `Str` is inhabited in v0.

## Arrays

`Array[T]` is the builtin **owned, growable** homogeneous sequence, indexed by
`Usize`. Like `String`, its value is a three-word header:

```
struct tuo_array {
    T    *ptr;   // pointer to `cap` contiguous elements (aligned to align_of(T))
    usize len;   // number of initialized elements
    usize cap;   // capacity in elements, cap >= len
}
```

- Size = 3 × pointer-width, align = pointer-width.
- Elements are stored contiguously with `T`'s natural stride (`size_of(T)`
  rounded up to `align_of(T)`); element *i* is at `ptr + i*stride`.
- An out-of-bounds index (`i >= len`) **traps** (`IndexOutOfBounds`); it never
  reads past `len`.
- Dropping an `Array[T]` drops elements front-to-back, then frees the buffer.

## Fixed-size arrays

`[T; N]` is the builtin **inline, fixed-length** homogeneous sequence
(ADR-0004 Stage 2). Unlike `Array[T]` — the owned, growable sequence whose
value is a three-word `(ptr, len, cap)` heap header — a `[T; N]` value **is**
its `N` elements, stored inline wherever the value lives (a local's stack
slot, a struct field), with no header, no indirection, and no allocation.

- `size = N × stride(T)`, `align = align(T)`; element *i* is at offset
  `i × stride(T)`. For `N = 0` the value is zero-sized but still aligns like
  `T`.
- `N` is part of the type: `[I64; 3]` and `[I64; 4]` are distinct types with
  distinct sizes.
- Dropping a `[T; N]` drops elements front-to-back and frees nothing (the
  storage is inline); a `Copy` element type makes the whole array `Copy` and
  its drop a no-op.
- The `Array[T]` header layout above is unchanged by this section.

## Box

`Box[T]` is a **unique, owning** heap pointer — one word:

```
struct tuo_box { T *ptr; }   // == a single non-null pointer
```

- Size = pointer-width, align = pointer-width. Never null.
- `ptr` addresses a single heap `T`, allocated with `T`'s size and alignment
  through the allocation boundary.
- Move semantics: moving a `Box` copies the pointer and leaves the source
  uninitialized (the checker guarantees no double-drop). There is no reference
  count.
- Dropping a `Box[T]` drops the pointee `T`, then frees the allocation.

## Shared

`Shared[T]` is **shared ownership** with reference counting. The value is a
single pointer to a heap **control block** that co-locates the counts with the
payload:

```
struct tuo_shared { tuo_shared_block[T] *ptr; }   // one word, never null

struct tuo_shared_block[T] {
    usize strong;   // number of live Shared handles
    usize weak;     // number of live Weak handles (+1 while strong > 0)
    T     value;    // the shared payload, at the block's natural alignment
}
```

- `Shared` handle size = pointer-width. The block is a single allocation sized
  and aligned for `{ usize, usize, T }` in that order.
- **Clone is explicit** (ADR-0003 ruling 7): cloning a `Shared` increments
  `strong`. Dropping a `Shared` decrements `strong`; at `strong == 0` the
  payload `T` is dropped in place and the implicit weak count is released.
- The block's backing allocation is freed only when **both** `strong == 0` and
  `weak == 0` — so a live `Weak` keeps the block (but not the payload) alive.
- Counts are plain (non-atomic) in v0: tuonelang v0 is single-threaded, so a
  `Shared` is never shared across threads. Making the counts atomic for a
  future threaded target is a version-bumping change.

## Weak

`Weak[T]` is a **non-owning** handle to a `Shared[T]`'s block — one word:

```
struct tuo_weak { tuo_shared_block[T] *ptr; }   // one word, never null
```

- Size = pointer-width. Points at the same control block as the `Shared` it was
  derived from; holds a count in `weak`, not in `strong`.
- A `Weak` reaches the payload **only via upgrade** (ADR-0003 ruling 7), which
  yields `Option[Shared[T]]`: `Some` (incrementing `strong`) iff `strong > 0` at
  the moment of upgrade, else `None`.
- Dropping a `Weak` decrements `weak`; if that leaves both counts zero, the
  block allocation is freed.
- `Shared` cycles leak safely and are broken with `Weak` — the counts make a
  cycle's payloads never reach `strong == 0`, which is a leak, not a
  use-after-free (Constitution §24 safety over liveness).

## Aggregates: tuples, structs, enums

- **Tuple / struct**: fields in declaration order, each at its natural
  alignment, with the aggregate aligned to the maximum field alignment and
  padded to a multiple of that alignment (the `#[repr(C)]` rule). A field's
  offset is the running size rounded up to that field's alignment. This matches
  the interpreter's positional `Aggregate(Vec<Value>)` field order.
- **Enum / `Option` / `Result`**: a `u32` **discriminant** followed by the
  active variant's payload, the whole sized/aligned to hold the largest variant.
  The discriminant is the variant's **declaration-order index** — byte-identical
  to the interpreter's `Value::Variant { variant, .. }`. `Option[T]` is
  `{ 0 => Some(T), 1 => None }`; `Result[T,E]` is `{ 0 => Ok(T), 1 => Err(E) }`,
  matching declaration order and the interpreter/MIR numbering (`Some`/`Ok` = 0).
  No niche/pointer packing in v0.

## Panic / trap behavior

A **trap** is the native counterpart of the interpreter's structured abort
(Constitution §24). At each trap site — integer overflow, division by zero, an
out-of-bounds index, or a proved-unreachable point — generated code calls the
runtime symbol `tuo_rt_trap(i32 code)` with a stable `TrapCode`. The runtime:

1. writes one stable line to **stderr** (never stdout — stdout is the program's
   own output), keyed by the code;
2. calls `abort()`, terminating the process with `TRAP_EXIT_STATUS` (134 =
   128 + SIGABRT on the supported Unix hosts);
3. **does not unwind** — no destructors run (ADR-0003 ruling 6). A trap is
   final; cleanup is abandoned on purpose.

`tuo_rt_trap` never returns. The codes and messages are the shared `TrapCode`
taxonomy (`tuo_runtime::TrapCode`), stable and append-only.

## Process startup

The runtime does **not** own `main`. The backend synthesizes a C `main` shim
(see `tuo-codegen-cranelift`) that:

1. calls the program's nullary entry function, and
2. returns the entry's integer result as the process exit status.

v0 has no command-line arguments, no environment capture, and no global
constructors: startup is exactly "call the entry." A future runtime-owned
startup (argument marshalling, stdlib init) is an additive, version-bumping
change; the shim seam is where it will attach.

## Process exit

- **Normal exit:** the entry's `Int` return value **is** the process exit status
  (truncated to the platform's status width — the low 8 bits on Unix). This is
  the same value the interpreter reports for the same entry, which is what the
  differential suite checks.
- **Trap exit:** `TRAP_EXIT_STATUS` (134), as above — distinguishable from any
  small integer a program deliberately returns.
- There is no other exit path in v0: no `exit()` builtin, no unwinding to top of
  stack. A function either returns (propagating to the shim) or traps.

## Memory allocation boundary

All heap memory (`Box`, `Shared`, `String`, `Array`) is acquired and released
through **two C-ABI runtime symbols**, so the allocator is a single, swappable
seam and no backend embeds `malloc` calls directly. (A `[T; N]` fixed-size
array never participates: its storage is inline and it never touches
`tuo_rt_alloc`.)

```
void *tuo_rt_alloc(usize size, usize align);          // never returns null; traps on OOM
void  tuo_rt_dealloc(void *ptr, usize size, usize align);
```

- `tuo_rt_alloc` returns a block of at least `size` bytes aligned to `align`
  (`align` a power of two). A zero `size` returns a non-null, `align`-aligned
  sentinel that must not be dereferenced and must be passed back to
  `tuo_rt_dealloc` with the same `size`/`align`. Allocation failure **traps**
  (it does not return null): out-of-memory is a deterministic abort, not a value
  a program can observe.
- `tuo_rt_dealloc` releases a block previously returned by `tuo_rt_alloc` for
  the *same* `size`/`align`. Passing a mismatched size/align, or a pointer not
  from `tuo_rt_alloc`, is undefined — but generated code always pairs them from
  the layout, so it never does.
- The size/align are always the *layout* values from this document (`Box`:
  `size_of(T)`/`align_of(T)`; `Shared`: the control block's;
  `String`/`Array`: `cap × stride`). Carrying size/align back into `dealloc`
  keeps the runtime free of allocation metadata.

Like the trap, the boundary is provided as portable **C source**
(`tuo_runtime::alloc_runtime_c_source`) linked into every built binary, so a
generated executable needs no Rust runtime.

## Destruction

Destruction is **compiler-generated drop glue** only — no user destructors in
v0 (ADR-0003 ruling 9). A `Drop` on a place runs the glue for its static type:

- **`Copy` scalars** (`Bool`, `Char`, integers, floats, `Unit`, `Str`): a no-op.
- **`String`**: free the buffer via `tuo_rt_dealloc(ptr, cap, 1)`.
- **`Array[T]`**: drop elements front-to-back, then
  `tuo_rt_dealloc(ptr, cap*stride, align_of(T))`.
- **`Box[T]`**: drop the pointee, then free the single-`T` allocation.
- **`Shared[T]`**: decrement `strong`; at zero, drop the payload; free the block
  when both counts hit zero.
- **`Weak[T]`**: decrement `weak`; free the block if both counts are zero.
- **struct / enum**: drop the (active variant's) fields in declaration order,
  then the aggregate storage (which is inline — no separate free).

Drops run in **reverse declaration order** on every normal exit path
(ADR-0003 ruling 6); assignment drops the old value first; a **trap runs no
drops at all** (no unwinding). Initialization state is statically known at every
drop point — there are no runtime drop flags.

## Internal calling conventions

- v0 uses the **platform C calling convention** for every tuonelang function and
  for the runtime symbols, so the C `main` shim, the trap, and the allocator
  interoperate without a bespoke convention and a future FFI has a natural seam.
- **Scalars** (this section's primitives) are passed and returned **by value**
  in registers per the platform ABI.
- **Passing modes** (`take`/`in`/`mut`, `tuo_mir::PassMode`): a `take` (by-value)
  parameter passes the value — a scalar in a register, an aggregate by the
  platform's aggregate rule (small aggregates in registers, larger ones by
  hidden pointer, per the C ABI). An `in`/`mut` borrow passes a **pointer** to
  the caller's place; the borrow lasts only for the call (ADR-0003), and `mut`
  permits writes through the pointer. No tuonelang reference *types* exist — a
  borrow is purely a calling-convention pointer, never a first-class value.
- **Returns**: a scalar returns in a register; an aggregate return follows the
  platform's sret rule (hidden out-pointer) when it is too large for registers.
- Calls are always **direct** (`tuo_mir` has no function-typed values); there is
  no vtable or closure-environment convention in v0.
- **No unwinding**: the convention has no exception/unwind path. The only
  non-returning transfer is `tuo_rt_trap`, which aborts.

## Versioning

`ABI_VERSION` is `2`. Any change that alters a layout, an offset, a
discriminant numbering, a calling-convention rule, or the meaning of a runtime
symbol **must** increment it, in the same commit that changes the tests pinning
the affected layout. Additive, non-layout-affecting clarifications do not bump
it. Version `1` corrected `Option`'s variant numbering (`Some` = 0, `None` = 1)
to match the interpreter and MIR. Version `2` added the inline `[T; N]`
fixed-size array layout (`size = N × stride(T)`, `align = align(T)`, element
`i` at `i × stride(T)`); no existing layout changed. The version is asserted by the crate's tests so a silent reinterpretation of
bytes is impossible.
