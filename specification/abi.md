# The tuonelang runtime ABI (v0)

- **Status:** accepted (unstable — versioned, not yet frozen)
- **ABI version:** `10` (see `tuo_runtime::abi::ABI_VERSION`)
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

The backends today (Cranelift and LLVM alike) lower the **v0 runnable core**:
the scalar control-flow core plus floats, borrow-mode calls, the ADR-0004
aggregates — structs, enums, `Option`/`Result`, and fixed `[T; N]` arrays —
and, since ADR-0006 Stage B, the `Str` fat pointer, the `std::str` byte
operations, and the `std::rt` effect statements (through the effect runtime
symbols below), all laid out solely by this document's rules. Heap-*owning*
values — `String`, the growable `Array[T]`, and the memory wrappers — are
still **refused** rather than mis-compiled (see `tuo-codegen-cranelift`; they
await the allocator ADR). This document therefore specifies more than any
backend currently *emits*: it is the target the backends grow into, fixed up
front so two backends and the reference interpreter cannot drift into two
incompatible memory models.

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
- The bytes are UTF-8 **by convention**, not by invariant: `len` counts bytes,
  not characters, and the byte-level operations (ADR-0006's contract, extended
  to `String` by ADR-0009 — `std::string::slice` copies an arbitrary byte
  range) may produce a buffer that is not valid UTF-8 on its own. Nothing in
  the ABI depends on the bytes' validity.
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
- A `Str` points either at a string **literal** with static storage duration
  (ADR-0003 ruling 8), or — since ADR-0010 (resolving Q-0012) — at a live
  `String`'s buffer, obtained by `std::string::as_str` as the `{ptr, len}`
  prefix of the `String` header (no copy, no layout change: the representation
  was already this general fat pointer). A literal-derived `Str` outlives every
  borrow; a `String`-derived `Str` is a **shared borrow** of that `String`, and
  the ownership checker (`ownership.md` §13, `O0011`) forbids moving, mutating,
  dropping, or overwriting the `String` while the view is live and forbids the
  view escaping its frame — so the viewed bytes are always valid for the view's
  lifetime and the backend needs no defensive copy. Dropping a `Str` is still a
  no-op (it owns nothing).

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

This header layout is **generic in `T`** and unchanged across element types. The
*type* `Array[T]` admits any `T`; the **v0 element set** the builtins operate
over is staged (ADR-0012). Both the **reference interpreter** and the **native
backends** run every element type in the supported set: the scalars `Int`/`Bool`
(a single load/store of the element's own width), the borrowed `Str` (a two-word
fat pointer), the owned `String`, and user structs/enums whose fields are
themselves supported — indexing at `ptr + i×stride(T)`. A **heap-owning**
element (`String`, or a struct/enum carrying one) gets real per-element glue
(the ADR-0012 owned-element increment): a native `get` is a shallow stride copy
followed by a recursive **deep-copy fixup** — every owned buffer in the copy is
replaced with a fresh allocation, matching the interpreter's element clone — and
array drop walks elements front-to-back, freeing each element's buffers before
the array's own, each exactly once; `push`/`pop` move shallowly (the source is
de-initialized). `Str` owns nothing, so `Array[Str]` needs no glue. Only an
element containing a `Box`/`Shared`/`Weak` wrapper is refused natively (wrapper
values are not lowered anywhere); nested owned containers stay out of the
checker's element set entirely — a plain type error, not a silent half-feature.

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

## Maps (ADR-0011)

`Map[K, V]` is the builtin hash map. Its program-visible value is the **same
three-word header** as `String`/`Array` — `(ptr, len, cap)`, `Layout::words(3)`
— where `ptr` points at the **dense entries** buffer, `len` is the live entry
count, and `cap` the entry capacity. An empty map is the sentinel header
`(ZERO_SIZE_SENTINEL, 0, 0)` with no allocation. The v0 operation surface is
`Map[Int, Int]` and `Map[Str, Int]`; entries are stored **in insertion
order**:

- `Map[Int, Int]` entry: `{ i64 key, i64 value }`, stride 16.
- `Map[Str, Int]` entry: `{ const u8 *key_ptr, u64 key_len, i64 value }`,
  stride 24 — the borrowed `Str` key stored as its two-word view, never
  copied.

Every observable is defined by the insertion-ordered dense region: `keys`
lists keys in insertion order, `remove` shifts the tail down one slot
(preserving the relative order of the rest), and an overwrite keeps the key's
position — exactly the reference interpreter's association-list semantics.

The **hash index is not part of the ABI a backend consults**: it lives inside
the same allocation, *before* the entries
(`[ u64 index[index_cap] ][ u64 index_cap ][ entries ]`, `index_cap = 2 ×
cap`, slots holding `dense_index + 1` or `0`), and is owned entirely by the
`tuo_rt_map_*` runtime shim (`tuo_runtime::map::map_runtime_c_source`,
linked into every built binary). Backends lower the map operations to shim
calls and never touch the internals; only `len` (a header word read) and
`empty` (a sentinel header store) lower inline. The hash functions are fixed
and unseeded — reproducibility over hash-flooding resistance, the documented
ADR-0011 trade — and **unobservable** (no observable depends on bucket
placement): `Int` keys mix through the splitmix64 finalizer, `Str` keys hash
with 64-bit FNV-1a over their bytes, both vector-pinned in
`tuo_runtime::map`.

```c
void tuo_rt_map_int_insert(long long *hdr, long long k, long long v, long long *out);
void tuo_rt_map_int_get(const long long *hdr, long long k, long long *out);
void tuo_rt_map_int_remove(long long *hdr, long long k, long long *out);
void tuo_rt_map_int_keys(const long long *hdr, long long *out_hdr);
void tuo_rt_map_str_insert(long long *hdr, const unsigned char *kp,
                           unsigned long long kn, long long v, long long *out);
void tuo_rt_map_str_get(const long long *hdr, const unsigned char *kp,
                        unsigned long long kn, long long *out);
void tuo_rt_map_str_remove(long long *hdr, const unsigned char *kp,
                           unsigned long long kn, long long *out);
void tuo_rt_map_str_keys(const long long *hdr, long long *out_hdr);
void tuo_rt_map_drop(long long *hdr, long long stride);
```

`out` is a two-word `{found, value}` buffer (`found` ∈ {0, 1}; `value` is 0
when absent) from which the backend materializes the `Option[Int]` result;
`keys` writes a fresh `Array[K]` header through `out_hdr` (allocated via
`tuo_rt_alloc`; the sentinel header for an empty map); `drop` frees the whole
block (index + entries) via `tuo_rt_dealloc`, taking the entry stride only to
compute the block size. Growth doubles the entry capacity from 8, allocating
a new block, copying the dense entries, and rebuilding the index. `Map[K, V]`
is non-`Copy`; a map moves as a 24-byte header memcpy like `String`/`Array`.

## Function values (Tier 1)

A **function value** (ADR-0008 Tier 1) — a value of a function type
`fn(mode T, …) -> R` — is a single **code pointer**:

```c
void (*fp)(...);   // one non-null pointer to the callee's entry
```

- Size = pointer-width (8 bytes on 64-bit), align = pointer-width. `layout_of`
  returns `Layout::pointer()`.
- **`Copy`** (copying the value copies the pointer) and non-heap; its drop is a
  no-op and it never traps. In Tier 1 a function value always points at a
  compile-time-known top-level function, so the pointer is a link-time constant,
  never null.
- The parameter **modes** are part of the function *type* (used to drive the
  indirect-call site's borrows), not part of the value's runtime representation:
  two function values of different types have the same one-word layout.

**The indirect-call convention is identical to the direct one.** An indirect
call passes its arguments — by value, by `in` borrow, by `mut` borrow — and
returns its result (including via an `sret` out-pointer for a large aggregate
return) under **exactly** the same rules as a direct call to a function of the
same signature; the *only* difference is that the call target is loaded from the
function value rather than being a fixed symbol. A backend therefore reuses its
entire direct-call lowering and changes only the callee operand.

Native lowering of the function-value constant and the indirect call **lands
with ADR-0008 Stage B**; until then both backends refuse the MIR forms
(`Const::Fn`, an `Indirect` callee) cleanly rather than mis-compiling them, and
this section is the layout they will grow into. Tier 2 (capturing closures) will
need a different representation (a code pointer plus an environment) and is a
separate ADR increment — a Tier-1 function value carries no environment.

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
out-of-bounds index, an out-of-range byte value (ADR-0009's `InvalidByte`,
appended to the taxonomy with its interpreter counterpart), or a
proved-unreachable point — generated code calls the
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

The runtime does **not** own `main`. The backend synthesizes a `main` shim —
emitted natively into the object file (see `tuo-codegen-cranelift`'s
`emit_main_shim`; a C shim would be behaviorally identical) — that:

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
- There is no other exit path in the shipped v0 runtime: no unwinding to top of
  stack. A function either returns (propagating to the shim), traps, or asks
  for an exit: ADR-0006's `std::rt::exit` builtin terminates through the
  `tuo_rt_exit` effect symbol below.

## Effect symbols (ADR-0006 Stage B)

ADR-0006 Stage A gave the language its effect boundary in the front end, MIR
(`Statement::Effect`, `mir.md` §4.2), and the reference interpreter (which
never performs an effect). The **native** side of the boundary — landed with
the Stage B lowering — is three C-ABI runtime symbols, implemented by
`tuo_runtime::effect::effect_runtime_c_source` and linked into every built
binary alongside the trap shim. Both backends lower `Statement::Effect` to a
direct call:

```c
long long tuo_rt_write(long long fd, const unsigned char *ptr, unsigned long long len);
                                       /* total bytes written, or -1 on host error */
long long tuo_rt_read_byte(long long fd);
                                       /* 0..=255; -1 on EOF; -2 on host error */
_Noreturn void tuo_rt_exit(long long code);
                                       /* terminates with (code & 0xff); never returns */
void tuo_rt_par_map(long long f, const long long *tasks, long long n,
                    long long workers, long long *out_hdr);
                                       /* ADR-0007: structured fork-join */
```

- `tuo_rt_write` writes the `len` bytes at `ptr` to file descriptor `fd`,
  looping over partial writes and retrying `EINTR`, and returns the total
  bytes written, or `-1` on any other host error. It never traps; a `Str`
  argument is passed as its `{ptr, len}` pair.
- `tuo_rt_read_byte` reads one byte from `fd` (retrying `EINTR`), returning
  it (`0..=255`), `-1` on end of input, or `-2` on any other host error. It
  never traps.
- `tuo_rt_exit` terminates the process with `code & 0xff` as the exit status.
  Like `tuo_rt_trap` it **never returns** — but it is a *normal* exit path
  (the program asked for it), not a trap: it writes nothing to stderr and
  runs no destructors (none are pending by construction: a trap-free v0
  program's drops are statically placed, and an exit abandons them exactly as
  the documented trap rule abandons cleanup).
- `tuo_rt_par_map` (ADR-0007) applies the task function `f` — a tuonelang
  `fn(take Int) -> Int` code pointer, whose native calling convention is
  exactly C's `long long (*)(long long)` — to the `n` tasks at `tasks`,
  distributing them **round-robin** over `workers` POSIX threads (task `i`
  on thread `i % workers`; `workers < 1` behaves as 1, never more threads
  than tasks, capped at 64), joins every thread, and writes a fresh
  `Array[Int]` header of the results **in task order** through `out_hdr`
  (buffer via `tuo_rt_alloc`; the sentinel header for zero tasks). It is
  **structured fork-join**: nothing outlives the call; each thread reads the
  shared task buffer read-only and writes only its own disjoint result
  slots, so the shim contains no lock and needs none. A thread that fails to
  start runs its partition inline — the result is identical, only less
  parallel. It never traps.

**Static string data.** A `Str` *constant*'s bytes (a string literal) live in
**read-only static data** in the emitted object, deduplicated per module; its
runtime value is exactly the `{ptr, len}` fat pointer this document specifies
under Slices. No allocation and no copy is involved in materializing a `Str`
constant — both backends lower `Const::Str` to the address of that static
data plus a length. (An empty literal carries `len = 0` and a fixed non-null,
never-dereferenced pointer.) A `Str` produced by `std::str::slice` is a
derived fat pointer `{ptr + start, end - start}` into the same bytes.

Adding these symbols was an additive change (a new runtime surface, no layout
altered); the commit that landed them bumped `ABI_VERSION` to `3` per the
versioning rule, together with the tests that pin their behavior
(`tuo-runtime`'s `effect` module tests and `tuo-cli/tests/effects_native.rs`).

## OS-boundary effect symbols (ADR-0013)

ADR-0013 extends the same seam with six further C-ABI symbols — the clock,
argv, and file open/close/remove — implemented in the same effect shim and
linked into every built binary. Every one returns `long long`, never traps,
and reports errors as negative values:

```c
long long tuo_rt_now_nanos(void);      /* monotonic clock, ns since an arbitrary epoch */
long long tuo_rt_arg_count(void);      /* process argument count, argv[0] included */
long long tuo_rt_arg_byte(long long i, long long j);
                                       /* byte j of argument i; -1 out of range */
long long tuo_rt_open(const unsigned char *ptr, unsigned long long len, long long mode);
                                       /* fd >= 0; -2 not found; -1 other host error */
long long tuo_rt_close(long long fd);  /* 0 on success; -1 on host error */
long long tuo_rt_remove_file(const unsigned char *ptr, unsigned long long len);
                                       /* 0 on success; -2 not found; -1 other */
```

- `tuo_rt_now_nanos` reads `CLOCK_MONOTONIC` and returns whole nanoseconds
  since an arbitrary process-local epoch — only differences are meaningful.
  On the (practically unreachable) host failure it returns `0`.
- `tuo_rt_arg_count`/`tuo_rt_arg_byte` expose the process arguments the
  runtime **captures before `main` runs** via a platform initializer
  (`crt_externs.h` on macOS; an ELF constructor receiving `argc`/`argv` on
  glibc). `arg_byte` returns byte `j` (`0..=255`) of argument `i`, or `-1`
  when `i` is out of range or `j` is past that argument's end — the same
  "no more bytes" convention as `tuo_rt_read_byte`'s EOF.
- `tuo_rt_open` opens the file whose path is the `len` bytes at `ptr` (a
  `Str` passed as its `{ptr, len}` pair, exactly as `tuo_rt_write`'s text
  is). Modes: `0` read (`O_RDONLY`), `1` write (`O_WRONLY|O_CREAT|O_TRUNC`,
  mode `0644`), `2` append (`O_WRONLY|O_CREAT|O_APPEND`). It retries
  `EINTR` and returns the descriptor (`>= 0`), `-2` when the path does not
  exist, or `-1` on any other host error — an unknown mode and a path over
  the shim's bounded NUL-termination buffer (4096 bytes) included. The
  returned descriptor is exactly what `tuo_rt_write`/`tuo_rt_read_byte`
  accept, so file I/O is the composition of `open` with the ADR-0006
  descriptor seam; there is deliberately no separate whole-file primitive.
- `tuo_rt_close` closes the descriptor: `0` on success, `-1` on host error.
- `tuo_rt_remove_file` unlinks the path (same `{ptr, len}` convention and
  bounded buffer): `0` on success, `-2` when it does not exist, `-1`
  otherwise.

Adding these symbols (and the argv capture) was again additive — no layout
changed — and the commit that landed them bumped `ABI_VERSION` to `7`,
together with the tests that pin their behavior (`tuo-runtime`'s `effect`
module tests, `tuo-cli/tests/effects_native.rs`, and the `std::fs`/
`std::time`/`std::process` native pins in `tuo-cli/tests/stdlib.rs`).

## Socket effect symbols (ADR-0014)

ADR-0014 extends the seam with four further C-ABI symbols — descriptor
*producers*: a POSIX socket is a file descriptor, so the existing
`tuo_rt_write`/`tuo_rt_read_byte`/`tuo_rt_close` move and release the bytes;
these four only create connected or listening descriptors. Every one returns
`long long`, never traps, and reports every error as `-1` (deliberately no
finer taxonomy — a socket failure is environmental):

```c
long long tuo_rt_listen(long long port);     /* listening fd >= 0; -1 on error */
long long tuo_rt_bound_port(long long fd);   /* the bound local port; -1 on error */
long long tuo_rt_accept(long long fd);       /* connected fd >= 0; -1 on error */
long long tuo_rt_connect(const unsigned char *ptr, unsigned long long len,
                         long long port);    /* connected fd >= 0; -1 on error */
```

- `tuo_rt_listen` creates an IPv4 TCP socket bound to `127.0.0.1:port`
  (`SO_REUSEADDR`; loopback only, so no committed test or benchmark opens an
  externally reachable port) listening with backlog 16. Port `0` requests an
  ephemeral port — pair with `tuo_rt_bound_port` (`getsockname`) to learn it.
- `tuo_rt_accept` accepts one pending connection, retrying `EINTR`. Blocks.
- `tuo_rt_connect` connects to the numeric IPv4 address in the `{ptr, len}`
  bytes (`inet_pton`; no name resolution) at `port`. An `EINTR`'d connect
  that completes asynchronously (`EISCONN` on retry) is success.

Additive again — no layout changed; the landing commit bumped `ABI_VERSION`
to `8` together with the pins (`tuo-runtime`'s `effect` tests,
`tuo-cli/tests/effects_native.rs`'s single-process loopback roundtrip, and
the `std::net` native pin in `tuo-cli/tests/stdlib.rs`).

## Channel and mutex symbols (ADR-0015)

ADR-0015 adds runtime-owned synchronization objects behind opaque `long
long` handles — the same shape as a descriptor. Handles are process-lived
(no free; a bounded registry of 256 of each refuses exhaustion with `-1`,
never a trap). Every symbol returns `long long` and reports errors as `-1`:

```c
long long tuo_rt_chan_new(void);                     /* handle >= 0; -1 exhausted */
long long tuo_rt_chan_send(long long ch, long long v);
                          /* 0; -1 invalid/closed/negative v */
long long tuo_rt_chan_recv(long long ch);            /* blocks; value, or -1 closed+drained */
long long tuo_rt_chan_close(long long ch);           /* 0 (idempotent); -1 invalid */
long long tuo_rt_mutex_new(void);                    /* handle >= 0; -1 exhausted */
long long tuo_rt_mutex_lock(long long m);            /* blocks; 0, or -1 invalid/relock */
long long tuo_rt_mutex_unlock(long long m);          /* 0, or -1 invalid/not held */
```

- A channel is an unbounded FIFO of **non-negative** values: a `pthread`
  mutex + condition variable over a heap linked list whose nodes flow
  through the ADR-0009 allocation seam (`tuo_rt_alloc`/`tuo_rt_dealloc`).
  `send` refuses a negative `v` so `recv`'s `-1` closed/error signal stays
  unambiguous; `close` broadcasts, waking every blocked receiver once the
  queue drains. Values cross threads **by copy** — no tuonelang memory is
  ever shared, so ADR-0007's no-data-race property is preserved.
- A mutex is a `PTHREAD_MUTEX_ERRORCHECK` pthread mutex: a relock by the
  holding thread or an unlock by a non-holder is a `-1`, never undefined
  behavior. It guards critical sections over *external* resources (files,
  sockets); there is no shared tuonelang memory for it to guard.

Additive again — no layout changed; the landing commit bumped `ABI_VERSION`
to `9` together with the pins (`tuo-runtime`'s `effect` tests,
`tuo-cli/tests/effects_native.rs`'s policy and cross-thread drain
roundtrips, and the `std::sync` native pin in `tuo-cli/tests/stdlib.rs`).

## Bounded-wait symbols (ADR-0017)

ADR-0017 adds bounded-wait counterparts to the three seam operations that
otherwise block indefinitely. The blocking originals are **unchanged**; a
timeout is opt-in. Each takes a trailing `ms` deadline in milliseconds:

```c
long long tuo_rt_accept_timeout(long long fd, long long ms);
                          /* conn >= 0; -3 timed out; -1 host error */
long long tuo_rt_read_byte_timeout(long long fd, long long ms);
                          /* byte 0..=255; -1 EOF; -2 host error; -3 timed out */
long long tuo_rt_connect_timeout(const unsigned char *ptr,
                                 unsigned long long len,
                                 long long port, long long ms);
                          /* fd >= 0; -3 timed out; -1 host error */
```

- **A timeout is not an error**, and the ABI keeps them distinguishable: the
  timeout sentinel is `-3` (`tuo_runtime::effect::NET_TIMEOUT`). The seam
  already spends `-1` (`NET_ERROR`, and `read_byte`'s `READ_EOF`) and `-2`
  (`read_byte`'s `READ_ERROR`), and `read_byte_timeout` is the call where all
  four outcomes — a byte, end of input, a host error, and a timeout — are
  simultaneously possible, so `-3` is the first value that stays unambiguous
  everywhere. A program must be able to tell "the peer is slow" from "the peer
  is gone", so collapsing them would lose the only information a bounded wait
  exists to provide.
- **The deadline is honored across `EINTR`.** Each symbol computes a
  `CLOCK_MONOTONIC` deadline once and re-derives the remaining time on every
  retry (the shared `tuo_rt_poll_until` helper), so a signal storm cannot
  extend the wait past `ms`. A retry that finds no time left reports the
  timeout rather than polling again.
- **A negative `ms` is a host error** (`-1`), never an unbounded wait: a
  bounded primitive must not silently become a blocking one. An `ms` of `0`
  is a valid poll that returns immediately.
- `connect_timeout` performs the handshake on a temporarily non-blocking
  descriptor (`O_NONBLOCK`, restored before returning), polling `POLLOUT` and
  consulting `SO_ERROR` — the standard bounded-connect idiom. `EISCONN` on a
  retry is success, matching `tuo_rt_connect`'s existing policy.
- `read_byte_timeout` is a **descriptor** operation, not a socket-only one: it
  applies to any descriptor the seam produces, ADR-0013 files included.

## IPv6 symbols (ADR-0017)

ADR-0017 admits IPv6 on the same seam. The **client** side gains no new
symbol: `tuo_rt_connect` and `tuo_rt_connect_timeout` now parse their numeric
host into either family (`inet_pton` `AF_INET`, then `AF_INET6`) and open a
socket of the matching family. This is a strict widening — every host string
that parsed before parses identically. The **server** side cannot infer a
family from a port alone, so it gains two symbols:

```c
long long tuo_rt_listen6(long long port);     /* fd >= 0; -1 host error */
long long tuo_rt_peer_family(long long fd);   /* 4 or 6; -1 host error */
```

- `tuo_rt_listen6` binds `[::1]:port` — loopback only, for the same reason
  ADR-0014 gave — with `SO_REUSEADDR` and **`IPV6_V6ONLY`**. The v6-only
  setting is load-bearing: a dual-stack listener would also accept
  v4-mapped connections, making `tuo_rt_peer_family` ambiguous.
- `tuo_rt_peer_family` reports `4`/`6` (`FAMILY_IPV4`/`FAMILY_IPV6`) rather
  than the host's `AF_INET`/`AF_INET6`, whose numeric values are not
  portable.
- `tuo_rt_bound_port` reads the port through `sockaddr_storage`, switching on
  `ss_family`, so it serves both families and passes a correctly-sized
  address length.

## UDP symbols (ADR-0017)

ADR-0017 adds datagram sockets. A datagram is a **message**, not a stream, so
a receive reports the message boundary and stages the payload; a dedicated
indexer reads it. The stream-side `tuo_rt_read_byte` is deliberately
untouched — it calls `read(2)` directly and never consults the staging table,
so no file or TCP read pays for UDP's existence.

```c
long long tuo_rt_udp_bind(long long port);    /* fd >= 0; -1 host error */
long long tuo_rt_udp_send(long long fd, const unsigned char *hptr,
                          unsigned long long hlen, long long port,
                          const unsigned char *bptr,
                          unsigned long long blen);
                          /* bytes sent >= 0; -1 host error */
long long tuo_rt_udp_recv(long long fd, long long ms);
                          /* datagram length >= 0; -3 timed out; -1 error */
long long tuo_rt_udp_byte_at(long long fd, long long i);
                          /* byte 0..=255; -1 out of range / nothing staged */
long long tuo_rt_udp_peer_port(long long fd);
                          /* source port of the last recv; -1 if none */
```

- `tuo_rt_udp_send` is the seam's first **four-operand** effect and carries
  two `Str` values, so it takes six machine arguments (`{ptr, len}` each).
  The host address is parsed by the same `tuo_rt_addr_parse` helper
  `tuo_rt_connect` uses, so a datagram reaches either family.
- **Staging state.** `tuo_rt_udp_recv` `recvfrom`s into a fixed per-descriptor
  slot (a 16-entry table of `UDP_DATAGRAM_CAP` = 2048-byte buffers), recording
  the length and the sender's port. This is the socket seam's first
  per-descriptor state; it is process-lived like the ADR-0015 handle
  registries, so it adds no new lifetime concept. A datagram larger than the
  cap is **truncated while its true length is still reported**, exactly as
  `recvfrom` itself behaves — so `udp_byte_at` only serves indices actually
  captured.
- `tuo_rt_udp_peer_port` is what lets a datagram server reply to whoever wrote
  to it. The source *address* is not exposed as a `Str` in this version:
  returning host-allocated text needs a `String`-producing effect shape the
  seam has never had.

Additive — no layout changed; the landing commit bumped `ABI_VERSION` to `10`
together with the pins (`tuo-runtime`'s `effect` tests and
`tuo-cli/tests/effects_native.rs`'s bounded-wait, IPv6, and UDP roundtrips).

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
(`tuo_runtime::alloc_runtime_c_source`), so a generated executable needs no
Rust runtime. **Linking status:** since **ADR-0009 Stage B** the allocator C
source is **linked into every built binary** (unconditionally, like the trap
and effect shims and `-lm`), and both backends lower the `String`/`Array[Int]`
heap operations natively — every construction site flows through this seam. The
shim stays minimal: only `tuo_rt_alloc`/`tuo_rt_dealloc`. *Growth* (the
`push`/`append`/`push_byte` reallocation) is implemented in the **backend** as
alloc-new + copy + dealloc-old, never in the C shim, so the seam carries no
allocator policy beyond acquire/release. The reference interpreter models the
same observable behavior in its deterministic sandbox (growth counts against its
`MemoryBudget`); the native path and the interpreter agree on everything a
program can observe (length, contents, and `pop`'s `Option`), never on
buffer identity or capacity, which nothing observes.

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

`ABI_VERSION` is `10`. Any change that alters a layout, an offset, a
discriminant numbering, a calling-convention rule, or the meaning of a runtime
symbol **must** increment it, in the same commit that changes the tests pinning
the affected layout. Additive, non-layout-affecting clarifications do not bump
it. Version `1` corrected `Option`'s variant numbering (`Some` = 0, `None` = 1)
to match the interpreter and MIR. Version `2` added the inline `[T; N]`
fixed-size array layout (`size = N × stride(T)`, `align = align(T)`, element
`i` at `i × stride(T)`); no existing layout changed. Version `3` added the
ADR-0006 Stage B effect runtime symbols (`tuo_rt_write`, `tuo_rt_read_byte`,
`tuo_rt_exit`); no layout changed. Version `4` (ADR-0009 Stage B) made the
allocator seam (`tuo_rt_alloc`, `tuo_rt_dealloc`) load-bearing — it is now
linked into every built binary and used by the native `String`/`Array[Int]`
lowering — the runtime-symbol meaning went from unused to load-bearing; no
layout changed. Version `5` (ADR-0008 Tier 1) gave the **function type**
(`Ty::Fn`) a layout — a single code pointer (pointer-width, `Copy`) — where it
previously had none (`layout_of` returned a `LayoutError`); a previously
unlayoutable type gaining a layout is a layout-affecting change, so the version
bumps. Version `6` (ADR-0011) gave the **map type** (`Ty::Map`) its three-word
header layout and added the `tuo_rt_map_*` runtime symbols (and, with
ADR-0007, the `tuo_rt_par_map` fork-join symbol); a new heap layout is
layout-affecting, so the version bumps. Version `7` (ADR-0013) added the OS
effect symbols (`tuo_rt_now_nanos`, `tuo_rt_arg_count`, `tuo_rt_arg_byte`,
`tuo_rt_open`, `tuo_rt_close`, `tuo_rt_remove_file`); no layout changed.
Version `8` (ADR-0014) added the socket symbols (`tuo_rt_listen`,
`tuo_rt_bound_port`, `tuo_rt_accept`, `tuo_rt_connect`); no layout changed.
Version `9` (ADR-0015) added the channel and mutex symbols (`tuo_rt_chan_*`,
`tuo_rt_mutex_*`); no layout changed. Version `10` (ADR-0017) added the
bounded-wait symbols (`tuo_rt_accept_timeout`, `tuo_rt_connect_timeout`,
`tuo_rt_read_byte_timeout`), the distinct `-3` timeout sentinel, and the IPv6
symbols (`tuo_rt_listen6`, `tuo_rt_peer_family`, plus a family-inferring
`tuo_rt_connect`), and the UDP symbols (`tuo_rt_udp_bind`/`send`/`recv`/
`byte_at`/`peer_port`); no layout changed, but the new sentinel gives a
previously-unused return value a meaning, so the version bumps. The version is asserted by the crate's
tests so a silent reinterpretation of bytes is impossible.
