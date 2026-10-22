# ADR-0031: Wrapping arithmetic and rotation — named non-trapping integer operations (Q-0006)

- **Status:** accepted (2026-10-22 — Stages A–D landed; see Resolution)
- **Date:** 2026-10-14

## Context

Four documents defer the same question to "its own ADR", and none of them is
it. ADR-0030 leaves "32-bit arithmetic" as the other half of `sha256-hash`'s gap.
ADR-0020 records "the trap tension" — a trap is a data-dependent branch, so
constant-time code cannot use `+` — as "recorded but not resolved". ADR-0022's
amendment makes it requirement (5), "limb arithmetic that cannot trap", and
names two ways to discharge it without choosing between them. Open question
**Q-0006** (`specification/open-questions.md`) asks for the names and forms of
opt-in wrapping, saturating, and checked arithmetic. Constitution §24 promises
these operations and gives `a.wrapping_add(b)` only as an example spelling.

The findings below come from probing the compiler (a copy of the worktree's
`target/debug/tuo`) on an Apple M4, not from reading its source. Native times use
`tuo build --release` (LLVM) and the lab's C flags (`-O2 -ffp-contract=off`),
fastest of 11 interleaved runs. The probe programs are scratch files. The
benchmark plan commits the ones that matter.

### Finding 1 — the integer types exist; non-trapping arithmetic on them does not

`IntKind` (`crates/tuo-types/src/ty.rs`) has all ten widths, including `U32`
and `U64`, and all of them lower natively. But `+` traps at every width:

```tuo
fn add(take a: U32, take b: U32) -> U32 {
    a + b
}
// add(4294967295, 1): `tuo check` is clean; `tuo run` prints
// "tuo: trap: integer overflow" and exits 134.
```

So the catalog does its 32-bit work in `Int` with masks
(`std::bits::add32`/`mul32`/`rotr32`/`rotl32`). Its 64-bit work goes through
32-bit halves (`std::sha512::add64`), because "two words with their high bits
set would abort the process". These helpers appear on 77 non-comment, non-spec
lines across `bits.tuo`, `crypto.tuo`, `chacha.tuo`, and `sha512.tuo`.

### Finding 2 — a wrapping cast exists, and most of SHA-256's gap needs nothing more

MIR §5.4 (`specification/mir.md`) defines an integer `as` cast as "wrap to the
target width". The probe confirms it: `4294967296 as U32` is `0` in the
interpreter and on both backends. That makes a wrapping 32-bit add writable
today without masks:

```tuo
fn add32(take a: U32, take b: U32) -> U32 {
    ((a as U64) + (b as U64)) as U32
}
```

A rotation by a constant is also writable on `U32`, as `(x >> 7) | (x << 25)`:
a shift drops the bits it pushes out without trapping. The committed
`benchmarks/runtime/programs/tuo/sha256-hash.tuo` was retyped in exactly this
spelling, with no other change, and its round count scaled from 200 to 200,000:

| SHA-256, two blocks per round | time | vs C |
|---|---|---|
| C peer, `uint32_t` | 54.3 ms | 1.00x |
| committed program, `Int` + masks | 85.0 ms | 1.57x |
| retyped `U32`, the spelling above | 63.4 ms | 1.17x |

In the retyped program, LLVM proves the `U64` sum cannot carry, deletes the
check, and emits all ten rotations as `ror`. No branch to the trap remains in
the inlined digest loop. The masked `Int` form gets no `ror` at all. **So most
of the arithmetic gap ADR-0030 deferred to this ADR closes with no language
change.** The remaining 1.17x is not arithmetic: the program allocates
`padded()` and reads its constants through `std::array::get` on every round. In
the spec interpreter, one digest takes 7.9 ms in the `Int` form and 4.9 ms in
the `U32` form (on a debug-built compiler, so only the ratio is meaningful).

The cast trick has two limits. In the Cranelift debug build each `add32` still
compiles to `adds`, `cset hs`, `cbnz` → trap. And it **stops at 64 bits**,
because there is no wider type to widen into.

### Finding 3 — at 64 bits the whole gap is the add

This microbenchmark runs `x = add64(x, y); y = rotr(y ^ x, 29)` for 50,000,000
iterations:

| 64-bit add + rotate | time | vs C |
|---|---|---|
| C, `uint64_t` | 35.4 ms | 1.00x |
| `Int`, `sha512.tuo`'s `add64`/`rotr64` verbatim | 59.7 ms | 1.69x |
| `U64`, shift-or rotation, add through halves | 59.6 ms | 1.69x |

C's loop body is `add; eor; ror`. Both tuonelang loops already emit the single
`ror` and differ from C only in the halves' masks and adds. Retyping cannot help
here. This is the strongest performance case for a builtin, and it covers
`std::sha512`, the hash that `std::ed25519` is defined over.

### Finding 4 — the two hand-rolled rotations disagree at the edge

`std::bits::rotr32(x, -1)` returns `x`, as documented: a negative amount counts
as `0`. `std::sha512::rotr64(x, -1)` **traps** with `invalid_shift: shift amount
65 is out of range for I64`. Every caller today passes a constant, so neither
path is reached. Still, two helpers for one operation follow two rules, and one
of them can abort. Rotating by `n` and by `n + width` is the same rotation, so
that is a definition rather than a policy choice, and it should be written once.

### Finding 5 — the constant-time gate admits a trapping shift

`#[constant_time]` (ADR-0020 Stage C, `crates/tuo-types/src/check.rs`) refuses
`+ - * / %` (`T0019`) and calls to unmarked functions (`T0020`). Both wrapping
spellings above are refused with `T0019`. A call to the builtin
`std::float::sqrt` is refused with `T0020`, since a builtin is never marked: any
new builtin is refused unless the gate lists it. `(x >> 7) | (x << 25)` is
accepted, correctly. But a shift by a variable amount is accepted too, and it is
not branchless:

```tuo
#[constant_time]
fn shl_var(take x: U32, take n: U32) -> U32 {
    x << n
}
// `tuo check`: clean. Release ARM64: `cmp w1, #0x20; b.hs <trap>`.
```

ADR-0019 makes an out-of-range shift amount trap, and a trap check is exactly
the data-dependent branch `T0019` exists to refuse. No marked catalog function
is affected, because every shift in `std::ct`'s marked functions has a literal
amount. But `std::ct`'s header says its mask "cannot trap because no shift
can", which is true for `63` and false in general.

### Finding 6 — §24's spelling fails confusingly; checked arithmetic has a caller

`a.wrapping_add(b)` on two `U32`s passes `tuo check` with no diagnostics, then
`tuo run` fails with "codegen: the program has no function named `main` to
compile". A model with Rust priors will write exactly this call.

`std::str::parse_int("99999999999999999999")` traps `integer_overflow` rather
than returning `Err`. This is documented ("bound the input length first"), so
it is not a bug, but it is the workaround a checked multiply removes, in a
function whose doc says failure "is data, not a trap". `std::der`'s
`max_length` ceiling is the same workaround.

## Decision

### The form: free builtin functions in `std::int`, named `<family>_<op>`

This ADR resolves Q-0006 as follows. Non-trapping integer operations are
**builtin free functions in a new builtin module `std::int`**, the integer
counterpart of `std::float` (ADR-0029). Each is named after Rust's spelling
without the method syntax: `wrapping_add`, `checked_mul`, and so on. Each
operation takes operands of **one integer type `T`** (any of the ten) and returns
`T`, or `Option[T]` for `checked_*`. This mirrors `+`, which is already
polymorphic over its operands' integer type. The checker uses the same mechanism
as the parametric `std::map` builtins (`check_map_builtin_call`).

Rejected:

- **Methods** (§24's example). They are not lowered, and Finding 6 shows the
  failure misleads.
- **Operators with modifiers** (Zig's `+%`, Swift's `&+`). Symbol queries, the
  generated brief, and `grep` cannot find them by name. They also need a grammar
  change, and `&` already means bitwise and. Q-0006 asks that these operations
  be "discoverable and hard to use by accident", and a long name at every call
  site is both.
- **Per-type names** (`wrapping_add_u32`): fifty names, for a type already
  written at the operands. **A wrapping type** (Rust's `Wrapping<T>`): `+` would
  then depend on a declaration far from the call site — the accident Q-0006
  warns about.
- **`std::bits`** (ADR-0022's suggestion). `std::bits` is a catalog module with
  declared dependencies, while these are bodiless compiler builtins. A separate
  `std::int` keeps the two mechanisms apart, and its names collide with nothing.
  `add` and `mul` are already owned by `std::collections` and `std::bignum`.

The behaviour is identical in the interpreter, the debug build, and the release
build. §24's phrase "in release builds" implies no build-dependent arithmetic.

### What lands: five operations

The *proposed* signatures follow. `T` and `N` are any integer types, and `w` is
the bit width of `T`.

```
std::int::wrapping_add(take a: T, take b: T) -> T    // (a + b) mod 2^w
std::int::wrapping_sub(take a: T, take b: T) -> T    // (a - b) mod 2^w
std::int::wrapping_mul(take a: T, take b: T) -> T    // (a * b) mod 2^w
std::int::rotate_left(take x: T, take n: N) -> T     // by n mod w (Euclidean)
std::int::rotate_right(take x: T, take n: N) -> T
```

None of the five traps, for any input. Results are two's-complement bit
patterns, so signed and unsigned operands agree bit for bit. There is no
`wrapping_neg`, because `wrapping_sub(0, x)` is the one spelling. The rotation
amount has its own type, independent of the value's, just as a shift amount does
(`mir.md` §5.3). The amount is reduced Euclidean mod `w`, so
`rotate_left(x, -1) == rotate_right(x, 1)` and `rotate_left(x, w) == x`. Shifts
work differently on purpose. A shift by `w` or more means different things on
different hardware, which is why ADR-0019 makes it trap. A rotation by `n + w`
is the same rotation as by `n`.

**`checked_add`/`checked_sub`/`checked_mul` → `Option[T]`** follow the same
naming rule and land in Stage D. **`saturating_*`** is reserved by the rule but
not landed, because nothing in the catalog, the examples, or the lab needs it.

### Semantics on each engine

- **MIR**: five new integer-only `BinOp` variants: `WrappingAdd`,
  `WrappingSub`, `WrappingMul`, `RotateLeft`, `RotateRight`. They are new
  variants rather than a mode flag on `Add`. That forces every exhaustive
  `match` — the verifier, the four optimization passes, both backends, and the
  interpreter — to handle them explicitly, so no existing `Add` arm silently
  starts wrapping. Constant folding (`crates/tuo-mir/src/opt/const_fold.rs`) can
  always fold them, because none of them traps.
- **Interpreter** (`crates/tuo-mir-interp/src/interp.rs`): applies the existing
  cast helper `wrap_int` to the exact `i128` result, with one exception.
  `u64::MAX * u64::MAX` exceeds `i128`, so 64-bit products are computed in
  wrapping `u128`. Otherwise the reference engine itself would panic, which
  `tuo-fuzz` counts as an internal error. Rotation reduces the amount with
  `n.rem_euclid(w)`, then shifts and ORs within the width.
- **Cranelift** (`crates/tuo-codegen-cranelift/src/lower.rs`): `iadd`, `isub`,
  and `imul` replace the `sadd_overflow`/`uadd_overflow` path that `Add`
  uses today, and rotation is `rotl`/`rotr`.
- **LLVM** (`crates/tuo-codegen-llvm/src/lower.rs`): `add`/`sub`/`mul` without
  `nsw`/`nuw`, and `llvm.fshl`/`llvm.fshr` with the value as both inputs. LLVM
  defines the funnel-shift amount as unsigned modulo the width. Truncating or
  zero-extending the amount to the value's type keeps its low bits, and since
  widths are powers of two, those bits are all a Euclidean reduction uses.
  Stage B's fixtures pin both backends' amount reduction rather than assuming it.

### Constant time

- `T0020` gains an **explicit allowlist** of builtin callees. It holds exactly
  these five. `checked_*` never joins it, since its `Option` must be branched on,
  and `std::float::sqrt` stays refused.
- `T0019` is extended to **refuse any shift whose amount is not an integer
  literal** (Finding 5). Its help text points at `rotate_left`/`rotate_right`.
- ADR-0020's three links still hold, and the third is **hardware**, not the
  compiler. `wrapping_mul` is the operation with known exceptions: some embedded
  cores use early-terminating multipliers. `std::int`'s documentation states
  this the way `std::ct`'s header states its own limits.
- This settles the **timing** half of ADR-0022's requirement (5) by
  construction, since an operation that cannot trap has no trap branch to leak
  through. It does **not** settle the correctness half. A limb sum that exceeds
  its intended range now wraps silently instead of aborting, so limb code still
  needs the vector-pinned specs it has today. ADR-0022's other option, range
  reasoning in the checker, would cover both halves and stays open.

## Staging

**Stage A — the gate fix, landable on its own.** Extend `T0019` to non-literal
shift amounts, add a `tests/types` fixture for `shl_var`, and correct `std::ct`'s
"no shift can". This is a soundness fix that needs no builtin, like ADR-0023's
Stage A. *Landed 2026-10-16:* `T0019` refuses a shift whose amount is not an
integer literal (through any parentheses) inside `#[constant_time]`; pinned by
`a_shift_by_a_variable_amount_is_rejected` in
`crates/tuo-types/tests/constant_time.rs`, which also pins that literal
amounts stay accepted; every marked `std::ct` function still passes, and its
header now says why a literal shift cannot trap.

**Stage B — the five builtins.** Add `std::int` in
`crates/tuo-resolve/src/builtin.rs`, then the checker rule, the MIR ops, the
interpreter, and both backends. Three-way fixtures in `tests/codegen/fixtures/`
cover a carry at every width, signed `MIN`/`MAX`, `U64::MAX * U64::MAX`, and
rotations by `0`, `w`, `-1`, and `w + 1`, including an `I8` amount on a `U64`.
Then add the `T0020` allowlist, and extend `tuo-cli/tests/constant_time.rs` to
check that each builtin, inside a marked function, emits no conditional branch
on either backend. Finally the brief: its operators paragraph, which today
recommends `std::bits::add32`, names `std::int::wrapping_add` instead, and an
anti-pattern row maps `a.wrapping_add(b)` to `std::int::wrapping_add(a, b)`.

**Stage C — the catalog.** `std::sha512::add64`/`rotr64` become
`wrapping_add`/`rotate_right` on `Int`. Wrapping on `Int` and on `U64` agree bit
for bit, so no caller changes, and the `rotr64(x, -1)` trap disappears.
`std::bits::add32` becomes `wrapping_add(a as U32, b as U32) as Int`, and its
existing specs prove the masked contract is unchanged. `rotr32`/`rotl32` keep
their documented rule that `n <= 0` returns `x`, and delegate otherwise; changing
that contract is not this ADR's to do silently. The lab's `sha256-hash` is
retyped `U32` on the builtins.

**Stage D — checked arithmetic.** Add `checked_add`/`checked_sub`/`checked_mul`
and rewrite `std::str::parse_int` on them. Its doc stops telling callers to bound
their input, and a spec pins `parse_int("99999999999999999999")` as an `Err`.

## Benchmark plan

The plan is fixed before implementation, so the results cannot be chosen
afterwards. Each "after" figure is compared with a "before" figure measured on
the same machine in the same session.

1. **The 64-bit micro from Finding 3**, committed as the lab workload
   `wrapping-add64` with a C peer. Predicted: **1.69x C → ≤ 1.05x**, because the
   loop body becomes C's `add; eor; ror`. This gates Stage B. A miss means the
   lowering is wrong.
2. **`std::sha512`**: native throughput for 10,000 digests of 64 bytes, plus the
   time of its spec run. Predicted: **≥ 1.3x natively**. Each round makes seven
   `add64` calls, and each call is several dependent instructions where one would
   do. Below 1.15x, the performance case for 64-bit wrapping is refuted. Then
   acceptance rests on constant-time code and Q-0006 alone, and the resolution
   must say so.
3. **`sha256-hash`**, measured in two steps: (a) retyped `U32` with today's
   spelling, then (b) moved onto the builtins. Predicted: (a) about 1.17x C, and
   **(b) within ±3% of (a)** on the release build. LLVM already folds the carry
   check and recognizes the rotations, so on SHA-256 the builtins buy readability
   and constant-time admissibility, not speed. The debug-build difference is
   recorded without a target.
4. **`std::chacha`** (`apply` over 64 KiB): predicted ≥ 1.1x, low confidence
   (the block function is also array-access heavy).
5. **Controls, predicted unchanged (±3%)**: `std::x25519::scalar_mult` and
   Ed25519 verification (dominated by `std::bignum`), the Benchmarks Game
   table, and the rest of the lab.
6. **Interpreter**: Finding 2's digest spec and the `std::crypto` spec suite's
   wall time, recorded with no figure promised.

## Resolution (2026-10-22)

All four stages landed. The benchmark plan was run as written, and two of its
predictions missed; both misses are recorded below with their causes rather
than re-scoped after the fact.

**Stage B.** `std::int` holds the five builtins, generic over the integer type
(a rotation's amount its own integer type), as MIR `BinOp::WrappingAdd`/
`WrappingSub`/`WrappingMul`/`RotateLeft`/`RotateRight`: Cranelift's
`iadd`/`isub`/`imul`/`rotl`/`rotr`, LLVM's flag-free `add`/`sub`/`mul` and
`llvm.fshl`/`llvm.fshr`, and the interpreter's width-wrapped `i128`
arithmetic. `int_wrapping` pins them three-way at every width, including
`U64::MAX * U64::MAX` and rotations by `0`, `w`, `-1`, and `w + 1`. They are
`T0020`'s allowlist, and `tuo-cli/tests/constant_time.rs` disassembles a marked
use of each on both backends and finds no conditional branch. The brief names
them and maps `a.wrapping_add(b)` to `std::int::wrapping_add(a, b)`.

**Stage C.** `std::sha512::add64`/`rotr64` and `std::bits::add32`/`mul32`/
`rotr32`/`rotl32` are adapters over the builtins; the `n <= 0` rule of the
32-bit rotations is kept. Two defects surfaced: `std::bits::mul32` *trapped*
whenever its masked product reached 2^63 (`mul32(4294967295, 4294967295)`),
contrary to its own doc, and is now spec-pinned; and `std::sha512::hash`
rebuilt its two 80-entry constant tables on **every round**, which is fixed
(`round_constants`, built once per digest) together with the message schedule
now made at its size (ADR-0030). The lab's `sha256-hash` is retyped `U32`.

**Stage D.** `checked_add`/`checked_sub`/`checked_mul` return `Option[T]`. MIR
gains the overflow predicates `BinOp::AddOverflows`/`SubOverflows`/
`MulOverflows`, each computed from the same flag the trapping operator guards
on, and the builtin lowers to a branch building `None` or `Some` of the
wrapping result. `int_checked` pins both sides of every boundary three-way.
`std::str::parse_int` accumulates toward the sign through them, so
`"99999999999999999999"` is an `Err` and `"-9223372036854775808"` parses. The
`checked_*` three are not on the constant-time allowlist: their `Option` is
there to be branched on.

Implementing the predicates exposed one more latent defect: the interpreter's
and the constant folder's `Mul` computed `a * b` in `i128`, which a `U64`
product can exceed — a host panic in a debug build. Both now use
`checked_mul`. And a dead out-of-range shift was deleted by `dead_locals` on
the native path while the interpreter trapped; shifts are now treated as
trapping there (`bitwise_dead_shift_trap`).

Measured on an Apple M4, release builds, fastest of 11 runs, every pair
interleaved in one session on a quiet machine:

| Plan item | Before | After | Prediction | |
|---|---|---|---|---|
| 1. `wrapping-add64` (lab workload) | 60.0 ms (1.68x C) | 36.0 ms (1.01x C) | ≤ 1.05x | met |
| 2. `std::sha512`, 10,000 64-byte digests, as specified | 118 ms | ~120 ms (builtins only) | ≥ 1.3x | **missed: ~1.0x** |
| 2. the same, constant table hoisted, 100,000 digests | 119.9 ms | 92.5 ms | | 1.30x |
| 2. all of Stage C together, 10,000 digests | 118.3 ms | 10.2 ms | | 11.6x |
| 3a. `sha256-hash` retyped `U32`, today's spelling (20,000 rounds) | 10.3 ms (1.43x C) | 8.5 ms (1.17x C) | ≈ 1.17x | met |
| 3b. … moved onto the builtins | 8.5 ms | 8.55 ms (1.18x C) | within ±3% of 3a | met (+0.6%) |
| 4. `std::chacha::apply`, 32 × 64 KiB | 13.1 ms | 12.9 ms | ≥ 1.1x (low confidence) | **missed: 1.02x** |
| 5. X25519 public key ×20 | 65.8 ms | 67.6 ms | ±3% | met |
| 5. Ed25519 verify ×5 | 99.9 ms | 98.4 ms | ±3% | met |

Item 2 missed because its premise was wrong: the prediction assumed the
digest's time was the round arithmetic, and the profile showed it was the
constant tables rebuilt each round (malloc and free outweighed the
compression function more than 8 to 1). With the tables out of the way the
builtins give exactly the predicted 1.30x, and the whole stage 11.6x — but the
plan's own figure, taken on the module as it stood, is ~1.0x, and that is the
one recorded as the outcome. Item 4 missed for the reason the plan flagged as
its low confidence: ChaCha's time is in array access and per-byte `push`, not
in the quarter round. The debug build of 3a/3b, recorded without a target:
57.1 ms (`Int`) → 41.7 ms (`U32`, today's spelling) → 34.5 ms (builtins).

Item 6, the interpreter (`tuo spec`, whole process, best of 5): `std::sha512`
470 → 94 ms, `std::chacha` 2.00 → 1.98 s, `std::crypto` 224 → 202 ms.

Q-0006 is marked resolved, and Constitution §24's example now reads
`std::int::wrapping_add(a, b)`.

## Consequences

- Modular arithmetic gets one name per operation at every width. The 64-bit
  case, which nothing can express efficiently today, becomes one instruction.
- `#[constant_time]` code can add, subtract, multiply, and rotate, and the gate
  stops admitting the one trapping construct it currently misses.
- For a while, a 32-bit wrapping add has two spellings. `std::bits::add32` works
  on the `Int`-holding-32-bits convention the crypto stack uses (bytes are
  `Array[Int]`), and `std::int::wrapping_add` works on `U32`. The first becomes an
  adapter over the second. Retiring it is a catalog decision for when the crypto
  modules move to `U32`.
- Builtins are installed before user symbols, so this ADR's eight builtins shift
  every user symbol id. Golden fixtures such as
  `tests/mir/opt/fixtures/fn_value.mir` and
  `benchmarks/runtime/results/example-report.json` must be re-blessed, after
  checking that the diff is pure renumbering. There is no ABI or grammar change.
- On acceptance, Q-0006 is marked resolved by this ADR, and §24's example becomes
  `std::int::wrapping_add(a, b)`. §24 already marks that spelling Experimental.

## What this ADR does not propose

- **Any change to `+`.** It still traps (ADR-0026). These functions are the
  named exception §24 always promised.
- **Saturating arithmetic.** Nothing calls it yet. The name is reserved.
- **A 128-bit type or a widening multiply** (`mul_high`). Field arithmetic over
  64-bit limbs (radix 2^51) needs one. A radix-2^25.5 representation fits its
  products in `I64` without one.
- **Making X25519, ChaCha20, or Poly1305 constant time.** Their loops and
  indexing are still refused with `T0017`/`T0018`, and `std::bignum`'s timing
  varies by algorithm (ADR-0022). This ADR removes one of three obstacles.
- **Fixing the misleading method-call error** (Finding 6), which belongs with
  method lowering, or **changing `as`**, whose wrapping is specified and, per
  Finding 2, earns its keep.
