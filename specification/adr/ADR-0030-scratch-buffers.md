# ADR-0030: Scratch buffers — indexed assignment, and arrays made at their size

- **Status:** accepted (2026-10-03 — landed; see Resolution)
- **Date:** 2026-10-03

## Context

The Computer Language Benchmarks Game suite in the performance lab now puts
tuonelang at or near C on every supported program (CI's x86-64 runner:
`nbody` 1.06x, `spectral-norm` 0.99x, `fannkuch-redux` 0.92x, `mandelbrot`
0.99x, `fasta` 0.96x). The gaps that remain sit in the language-feature
workloads, and the largest one is not a compiler problem: `sha256-hash` runs
at about **2.3x C**, and the reason is that tuonelang cannot write the
program the way C does.

SHA-256 needs a 64-word message schedule per block. C declares it on the
stack and fills it by index:

```c
uint32_t w[64];
for (int t = 0; t < 16; t++) w[t] = ...;
```

tuonelang has the stack array — the repeat literal `var w = [0; 64];` is a
zero-filled `[Int; 64]` with no heap allocation (ADR-0004) — but it cannot
write one element of it: `w[t] = x` is refused with `O0004` ("index
expressions are not places in v0"). The only writable indexed storage is a
growable `Array[Int]` through `std::array::set` (ADR-0016), so a program must:

1. allocate the buffer on the heap, and
2. bring it to size one `push` at a time, since there is no way to make an
   array of a given length — 64 pushes, through capacities 8, 16, 32, 64:
   four allocations and three copies before the first real write.

Measured, not assumed:

- **`std::crypto::sha256_bytes`**, after the 2026-10-02 rewrite that already
  reuses one schedule buffer per call: about **16 heap allocations per
  digest**, and `malloc`/`free` are **~27%** of its profile. A one-shot digest
  barely benefited from that rewrite (1.05x) because these costs are per call.
- **A C program written with tuonelang's constraints** (heap schedule grown by
  push, everything else identical) runs **26% slower** than the same program
  with a stack array (33.9 ms vs 26.9 ms over the scaled workload). That is
  the part of the gap this ADR can close; the rest is SHA-256's 32-bit
  arithmetic done in 64-bit `Int` (see Out of scope).

The pattern is not specific to hashing. Any fixed-size working buffer — a
checksum table, a ring buffer, a matrix row, a decoder's window — has the
same two choices today: an immutable stack array, or a heap array grown from
empty.

## Decision (proposed)

### 1. Indexed assignment

`place[index] = value` becomes an assignment, for:

- a **fixed array** `[T; N]` held in a mutable place — a `var` binding, a
  `mut` parameter, or a field of one; and
- a **growable `Array[T]`** in the same positions, as the surface spelling of
  `std::array::set` (the Go-parity gap ADR-0016 recorded, `xs[i] = v`).

Semantics are exactly those of the existing operations:

- the index is a `Usize`, as for indexed reads;
- the write is **bounds-checked** and an out-of-range index **traps**
  `IndexOutOfBounds`, as a read does;
- the element type must be **`Copy`** in this ADR. A non-`Copy` element needs
  the old value dropped and the place re-initialized, which is
  `std::array::set`'s job for `Array[T]` and has no fixed-array counterpart
  yet; it is a separable follow-up, not a reason to hold the `Copy` case.

Only the write is new. The index expression becomes a *place for assignment
only*: it still cannot be moved out of, borrowed `mut`, or partially moved
(`O0007` and the ownership model are unchanged), so no new aliasing or
initialization state enters the checker.

### 2. `std::array::filled(take n: Int, take value: T) -> Array[T]`

A new pure builtin: an array of length `n`, every element `value`, made with
**one** allocation of exactly `n` elements. `T` must be `Copy`, for the same
reason the repeat literal requires it (`O0010`). A negative `n` traps — a
length is never negative, and silently returning an empty array would hide
the bug.

### What already exists underneath

The change is mostly in the front end:

- MIR already has assignment to a place with an `Index` projection, the
  interpreter already writes through one (`write_projected`), and both
  backends already compute an indexed element's address for any place.
- MIR lowering already emits a bounds `Assert` before every indexed use;
  a write needs the same assert in front of an assignment.
- The checker and the ownership pass are where `O0004` refuses the
  assignment today; they gain the `Copy`-element index place as an
  assignment target.

`filled` is a builtin like `std::array::empty`, lowering to one
`tuo_rt_alloc` and a fill loop.

## Benchmark plan

The ADR is accepted only if the numbers move; the plan is fixed before
implementation so the result cannot be chosen after the fact.

1. **`sha256-hash`** (lab workload): rewrite its schedule as
   `var w = [0; 64];` with indexed writes, and build the padded message with
   `filled`. Expected: the heap-schedule share of the gap closes — the C-side
   experiment predicts ~20% off the tuonelang time. The C, Go, and Rust peers
   are unchanged.
2. **`std::crypto::sha256_bytes`**: move the schedule to a stack `[Int; 64]`
   and create the message copy with `filled`. Measured on the one-shot digest
   benchmark used for the 2026-10-02 rewrite (20,000 digests of 64 bytes) and
   on PBKDF2 at 40,960 iterations. Target: at least 1.2x on the one-shot
   digest; no regression on PBKDF2.
3. **No regression elsewhere**: the Benchmarks Game speed table and the full
   lab, before and after.
4. **Three-way agreement**: new `tests/codegen/fixtures/` programs for indexed
   writes (fixed and growable, in range, out of range trapping, `mut`
   parameter, struct field) and for `filled` (including `n = 0` and a
   negative `n` trapping), pinned interpreter = Cranelift = LLVM.

## Out of scope

- **32-bit arithmetic.** The other half of `sha256-hash`'s gap is
  `std::bits::add32`/`rotr32` doing 32-bit work in 64-bit `Int` with masks.
  `U32` exists, but its `+` traps on the carry a hash requires, and there is
  no rotate. Wrapping arithmetic and rotation as builtins is its own question
  (ADR-0026 deliberately made `+` trap) and gets its own ADR.
- **Non-`Copy` elements** for fixed-array writes, as above.
- **`Usize` friction.** Indexing takes a `Usize`, so a loop counted in `Int`
  needs `i as Usize` at each index. That is a usability question about
  indexing in general, not about writes.

## Resolution (2026-10-03)

Landed as proposed, and the benchmark plan was run as written.

- **Indexed assignment**: the ownership pass checks `xs[i] = v` as a mutation
  of the array place beneath it (`O0001`/`O0004` as for a `mut` argument) and
  refuses a non-`Copy` element with the new **`O0012`**; MIR lowers the target
  through the same bounds-checked `Index` projection a read uses, so the
  interpreter and both backends needed no new write path.
- **`std::array::filled`**: a new `HeapOp::ArrayFilled` — one
  `ensure_capacity` for all `n` elements and a fill loop in each backend, the
  interpreter checking its live-value budget before it builds the array. A
  negative `n` (or one whose bytes overflow) traps `IntegerOverflow`; a
  non-`Copy` value is `O0010`.
- **Three-way pinned**: `arr_index_write`, `arr_filled`,
  `arr_index_write_trap_oob`, and `arr_filled_trap_negative` agree on the
  interpreter, Cranelift, and LLVM; the ownership fixtures cover every refusal.

Measured (release, fastest of 9–11 interleaved runs):

| | before | after | |
|---|---|---|---|
| `sha256-hash` lab workload (scaled) | 39.3 ms | 25.6 ms | 1.53x — the C-side experiment predicted ~1.25x |
| `std::crypto::sha256_bytes`, 200,000 one-shot digests | 171.5 ms | 112.5 ms | 1.52x (target was 1.2x) |
| PBKDF2-HMAC-SHA-256, 40,960 iterations | 44.6 ms | 25.7 ms | 1.74x (target was no regression) |

`std::crypto` now keeps the hash state in a stack `[Int; 8]` (a `Copy` value,
so resuming HMAC from a saved key-block state is a copy) and the schedule in a
stack `[Int; 64]`; its padded tail, digest, and `bytes_of_str` are made with
`filled`. The Benchmarks Game speed table did not move. One cost is visible:
the interpreter copies a fixed array in and out of a `mut` parameter, so the
crypto specs run slower in the spec sandbox (about 1.3 s, from 0.15 s), still
well within its fuel.

The brief (`tuo cheatsheet`) dropped its anti-pattern row that called
`xs[i] = v;` wrong and now says when it applies.

## Consequences

- Fixed-size buffers stop costing heap allocations, in user code and in the
  standard library.
- `xs[i] = v` becomes the obvious spelling for both array kinds; `std::array::set`
  remains, as `std::array::get` remains beside `xs[i]`.
- No ABI change: fixed arrays are already laid out inline, and `filled`
  produces the existing `Array` header.
