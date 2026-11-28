# ADR-0023: Widening the map surface — values beyond `Int`, and what still needs traits

- **Status:** proposed (Stages A, B, C **landed** 2026-09-06; Stage **B2.1** — the borrowed `Str` value — **landed** 2026-09-08; Stage **B2.2** — owned values — **partly landed** 2026-11-28: the owned `String` value; struct/enum values, the `py2tuo` `dict[str, str]` translation with the survey re-run, and the `Map[Str, String]` `map-lookup` variant remain open)
- **Date:** 2026-09-06

## Context

ADR-0011 shipped the hash map with a deliberately narrow *operation* surface:
`Map[Int, Int]` and `Map[Str, Int]`. It was explicit that the narrowness was a
staging decision rather than a design one, listing under "Deliberately out of
scope":

> **`Map[K, V]` for value types beyond `Int`** in the *operation* surface — the
> type exists for any `V`; the v0 ops are `V = Int`. `Map[Str, String]` and
> friends are additive once the drop path (already specified) is exercised.

Three things have happened since that make the widening worth taking up now.

**1. ADR-0012 built the machinery.** The element-generic array surface needed
exactly the same capability: a container holding an element that may itself own
heap, requiring a deep copy on read-out and recursive drop glue on destruction.
That landed as `HeapGlue::DeepFixup` / `HeapGlue::DropInPlace`, driven by
`ty_owns_heap` — which *already* matches `Ty::Map(..)`. The map widening is
therefore mostly the reuse of a mechanism that exists and is three-way pinned,
not the invention of a new one.

**2. Dogfooding produced a concrete demand.** The `tools/py2tuo` Python
transcoder (a compiler from a typed Python subset to tuonelang) can translate
dicts only where they land on the two v0 shapes. A survey of 1,870 CPython
standard-library files found 58 dict-typed annotations, of which **7%** are
`dict[str, int]` or `dict[int, int]`. The single most common unsupported shape
is `dict[str, str]`. This is a measurement, not an intuition, and it is the
first evidence-backed demand for the widening ADR-0011 anticipated.

**3. The narrowness is currently enforced *unsoundly*.** See "The soundness
defect" below: unsupported pairs can pass `tuo check` and fail in codegen. That
is a violation of the project's central invariant — *the compiler refuses what
it cannot compile; it never mis-compiles* — and it lives in the very code this
ADR would change. Widening the surface without fixing the guard would widen a
hole.

### The soundness defect

`reject_unsupported_map_pair` (`crates/tuo-types/src/check.rs`) is *itself*
correct: it permits only `V = Int` with `K ∈ {Int, Str}`. But it runs eagerly at
each call site and returns early when either type is still an inference
variable:

```rust
let undetermined = |ty: &Ty| matches!(ty, Ty::Var(_) | Ty::Error | Ty::Never);
if undetermined(&key) || undetermined(&value) {
    return;
}
```

With `std::map::empty()` the key and value are fresh vars, unsolved at that
moment. Inference solves them later and **nothing re-checks**, so the malformed
map reaches the backend. Observed today:

| program | `tuo check` | `tuo run` |
|---|---|---|
| `insert(m, 1, true)` — `Map[Int, Bool]` | **passes** | `codegen: Compilation error: Verifier errors` |
| `insert(m, 1, 2.5)` — `Map[Int, Float]` | **passes** | `codegen: Compilation error: Verifier errors` |
| `insert(m, 1, "s")` — `Map[Int, Str]` | **passes** | `codegen: a Str constant reached the scalar constant path` |
| `insert(m, "k", "v")` — `Map[Str, Str]` | correctly `T0001` | — |
| annotated `var m: Map[Int, Bool]` | correctly `T0001` | — |

`Map[Str, Str]` is caught only *incidentally*: a string-literal key resolves
immediately, whereas an integer literal stays an unsolved numeric variable. The
guard works exactly when the program did not need it.

## Decision

Widen the map **value** surface to the same element set ADR-0012 defined for
arrays, and fix the guard that enforces the boundary. **User key types stay
deferred to the trait system**, unchanged from ADR-0011.

Staged, each stage independently shippable and independently pinned:

### Stage A — make the boundary sound (no surface change) — **LANDED**

Move the pair check from an eager per-call-site test to a **post-inference
pass** over recorded map-builtin sites, so a pair solved after the fact is still
refused. The surface does not change in this stage; only its enforcement does.
It ships first, alone, because it is a correctness fix that the rest of the ADR
would otherwise widen.

Audit `reject_unsupported_array_element` for the same eager shape in the same
commit. It appears to behave correctly today (`Array[Float]` compiles and runs),
but the code shape is identical and the difference may be accidental.

Pinned by: a checker test per unsupported pair in both the *inferred* and
*annotated* spellings — the inferred spelling is the one that regressed, so it
is the one that must be tested. A test that only writes the annotation would
have passed throughout the defect's life.

**Landed 2026-09-06.** `Checker` gains a `deferred_map_pairs` buffer: a
map-builtin site whose pair is still an inference variable is *recorded* rather
than dropped, and `finish_body` — which already runs after `icx.finalize()` —
re-applies the substitution and runs the identical rule. The rule itself is
factored into `check_map_pair` so the eager and deferred paths cannot drift.
A pair still undetermined after inference is left alone, so a genuinely
unconstrained `empty()` remains `T0011` ("type annotation needed") rather than
gaining a second, spurious complaint.

Seven tests in `crates/tuo-types/tests/typeck.rs` pin it, all written in the
*inferred* spelling. Their value was verified by reverting the fix: **five fail
without it** and all pass with it. The two that pass either way are the
guard tests — that supported pairs are not over-refused, and that an
unconstrained `empty()` does not report a pair error.

The audit of `reject_unsupported_array_element` found it **not** vulnerable,
and for a specific reason worth recording: it treats an unsolved element as
*supported* and rejects on structure, so `Array[Array[_]]` is caught even while
the inner element is a variable. The map guard's defect was that it treated an
unsolved pair as a reason to skip the check entirely. Same shape, opposite
default — and the map's default was the unsound one.

### Stage B — widen `V` to the `Copy` scalars — **LANDED**

**Implementation note (2026-09-06): three of this stage's premises were wrong,
and the measured shape is simpler than what follows.** Before writing code the
guard was temporarily relaxed and all three engines were run against `Bool`,
`Float`, and `Str` values. What that found:

1. **The interpreter needed no change at all.** It stores a `Value` and never
   inspects the value's type, so it was already value-generic. The plan below
   implies a three-engine change; it was a two-engine change.
2. **The runtime C shim needed no change either.** It is *already*
   stride-parametric internally (`tuo_map_grow`, `tuo_map_block_size`, and
   `tuo_rt_map_drop` all take a stride), and — more to the point — the value
   slot is one machine word. The compiler widens a `Copy` scalar into that word
   on the way in and narrows it back on the way out, so the shim never learns
   which scalar it carries. **No `tuo_rt_map_*` symbol changed.**
3. **The ABI did not bump.** Because the value is word-widened rather than
   stored at its natural width, `INT_ENTRY_STRIDE` (16) and `STR_ENTRY_STRIDE`
   (24) are unchanged. The plan below asserted a bump was required; it was not.

The actual blocker was in neither the checker nor the runtime: **both native
backends declare every map-shim parameter as pointer-width** (`call_map_shim`),
and materialize the result through a hardcoded `Option[Int]` writer. Passing a
`Bool` (i1/I8) or `Float` (double/F64) raw is a call-signature mismatch — the
"Verifier errors" and "Call parameter type mismatch" that made non-`Int` values
unsound. The fix is a matched pair per backend: `value_as_word` on the way in
and a payload-typed narrowing on the way out, with `Float` crossing by
**bitcast, never numeric conversion** (a conversion would turn 2.5 into 2 —
compiling, running, and silently wrong; the `map_float_values` fixture is built
to catch exactly that).

**Landed scope:** `V ∈ {Int, Bool, Float}` — the `Copy` scalars — on both key
kinds. Pinned by two new three-way differential fixtures
(`tests/codegen/fixtures/map_bool_values.tuo`, `map_float_values.tuo`) proving
interpreter == Cranelift == LLVM, plus checker tests for the new boundary. The
`Bool` fixture stores *both* truth values and reads back a displaced one, so a
wrong narrowing cannot pass by luck; the `Float` fixture uses fractional and
negative values, so a numeric conversion or a lost sign bit changes the result.

**Deferred to Stage B2:** `Str`, `String`, and struct/enum values — everything
that owns or borrows heap. Those genuinely need the deep-copy-on-read and
recursive-drop path (and there the original plan's analysis holds, including
`tuo_rt_map_drop` not currently running per-value glue). They remain refused by
the type checker, pinned by a test.

### Stage B2.1 — the borrowed `Str` value — **LANDED (2026-09-08)**

Stage B2 splits, because `Str` and `String` need different things and only one
of them needs the ownership machinery:

- a **borrowed `Str`** is a fixed-size two-word view. It needs the *value slot*
  to stop being one machine word, but it **borrows** — so no deep copy on
  read-out, no drop glue, no per-entry ownership at all;
- an **owned `String`** (and structs containing one) additionally needs
  `HeapGlue::DeepFixup` on `get` and a per-entry drop loop.

Splitting there isolates the risky part. This stage does the slot; B2.2 does
the ownership.

**What changed.** The premise in the Stage B note — "the runtime C shim needed
no change, it is *already* stride-parametric" — is true of the **entry** stride
(`tuo_map_grow`, `tuo_map_block_size`, `tuo_rt_map_drop` all took one) but was
**not** true of the value slot: `INT_ENTRY_STRIDE`/`STR_ENTRY_STRIDE` were
compile-time constants with the value's `+ 8`/`+ 16` offset baked into ~20
memcpy sites. So:

- the entry stride is now **computed** (`entry_stride(key_size, value_stride)`),
  with `INT_KEY_SIZE`/`STR_KEY_SIZE` and `WORD_VALUE_STRIDE`/`STR_VALUE_STRIDE`
  as the primitives; the old `INT_ENTRY_STRIDE`/`STR_ENTRY_STRIDE` survive as
  derived constants;
- every `tuo_rt_map_*` entry point takes an `unsigned long long vs`, and the
  value crosses **by pointer** (`const void *v`) rather than by value. The shim
  memcpy's `vs` opaque bytes and never learns the value's type — the
  stride-parametric option (2) the original Stage B plan picked, now actually
  built;
- both backends gained `map_value_stride` and pass the stride at every call
  site; the `{found, previous}` out buffer widened from two words to three, so
  the same buffer serves a word value and a two-word view;
- `is_supported_map_value` gains `Ty::Str`.

**ABI version 12 → 13.** Both the entry layout *and* every shim signature
changed. Stage B did not bump (it word-widened into the existing slot); this
stage must, and the pinning test moved in the same commit.

**Pinned by** `tests/codegen/fixtures/map_str_values.tuo`, a three-way
differential fixture (interpreter == Cranelift == LLVM, exit 87) built
adversarially: every stored string has a **distinct length**, so a wrong stride
reads a wrong answer rather than coinciding; the `Str`-keyed map's value lengths
differ from its key lengths, so reading the key where the value belongs is
visible; an overwrite and a removal happen before the final reads, exercising
the memmove + index-rebuild at the new 24/32-byte strides; and one case compares
the returned bytes with `==` rather than only its length, so a surviving length
with a wrong pointer is caught.

**Found while landing this:** the Stage B fixtures `map_bool_values.tuo` and
`map_float_values.tuo` were committed but **never registered in any test**, so
they had never run. They are now in
`codegen_three_way.rs::map_operations_agree_across_all_three_engines` with the
new one, and all six map fixtures pass three-way.

**Still refused, deliberately:** owned `String` and struct/enum values
(Stage B2.2 — they need the drop loop, and admitting them now would leak one
allocation per entry), and nested `Map` values (out of scope; now pinned by
`nested_maps_stay_refused_as_map_values` rather than resting on an implicit
catch-all arm).

### Stage B2.2 (first slice) — the owned `String` value — **LANDED (2026-11-28)**

The first slice of B2.2 admits the one owned value whose glue both backends
already emit for array elements: `String`, behind both key kinds. Struct and
enum values are the rest of B2.2.

**What changed.**

- `is_supported_map_value` gains `Ty::String`, and nothing else; the `T0001`
  label and help now list `Int`/`Bool`/`Float`/`Str`/`String` and say
  struct/enum and nested `Map` values are not yet supported.
- `tuo_runtime::map` gains `STRING_VALUE_STRIDE = 24` (the `{ptr, len, cap}`
  header). The shim stays type-blind: insert/get/remove still memcpy `vs`
  bytes. Both backends' `map_value_stride` return it for `String`, so
  `Map[Int, String]` strides 32 and `Map[Str, String]` 40, and the
  `{found, value}` out buffer widens from three words to four.
- `insert` passes a pointer to the `String`'s header, moved into the entry
  bitwise (the argument is a `take`). `insert`'s previous value and `remove`'s
  value are **moved out** into the `Option[String]` result with no copy, so
  freeing them is the caller's ordinary drop of that `Option`; a discarded
  result (`let _ = ...`) frees the `String` exactly once. This is what the
  interpreter already did, and the `-> Option[V]` signatures already said.
- `get` copies the stored header out and then runs
  `emit_heap_glue(.., HeapGlue::DeepFixup)` on the payload, on the found path
  only, so the caller's `String` and the map's never alias. A miss is `None`
  and allocates nothing.
- `tuo_rt_map_drop(hdr, stride)` becomes
  `tuo_rt_map_drop(hdr, key_size, vs, drop_value)`. When `drop_value` is
  non-null the shim calls it on the value slot of every **live** entry, never
  on a hole, before freeing the block. The compiler passes the address of the
  value type's existing out-of-line drop glue (the function `Box` glue already
  used, C calling convention) when the value owns heap, and null otherwise.
- The interpreter needed no change: it already cloned on `get`, moved out on
  `insert`/`remove`, and dropped a `Value::Map`'s values.

**The drop callback, not an emitted loop.** The original plan left both open:
a per-value callback, or a compiler-emitted drop loop before the plain
deallocating drop. The loop cannot be written correctly from the compiler.
`remove` leaves a hole whose bytes are the stale header of the value it moved
out, and only the shim's private `live` bytes tell a hole from an entry. A loop
over `ptr[0..len]` would read the wrong slots; a loop over the dense region
would free moved-out buffers a second time; a loop over `keys` would allocate a
key array and probe once per entry just to drop. The shim already walks the
live entries (`keys`, rehome), so it walks them for the drop too, and stays
type-blind by calling back into the compiler's glue.

**ABI version 15 → 16.** A new value stride, the wider out buffer, and the new
`tuo_rt_map_drop` signature are all layout-affecting. On the `Map[_, Int]`
path the only change is the null callback and the reshaped drop arguments;
`benchmarks/` is untouched.

**Pinned by**

- `tests/codegen/fixtures/map_string_values.tuo`, a three-way differential
  fixture (interpreter == Cranelift == LLVM, exit 161) over `Map[Int, String]`
  and `Map[Str, String]`: distinct value lengths (and, for `Str` keys, lengths
  that differ from the keys'), a read overwrite result and a read removal
  result, content compared with `==`, a `get` copy grown with `push_byte`
  before the key is read again, a miss, and both maps dropped holding live
  entries after a removal left a hole;
- `crates/tuo-runtime/tests/map_model.c`, whose model checker now runs owned
  24-byte values: heap blocks the model frees when `insert`/`remove` hand them
  back and the `drop_value` callback frees at drop, asserting no allocation is
  outstanding after every run and the callback ran once per live entry (and
  never for an empty map), under ASan/UBSan. Its sanitized build had been
  failing to compile on GCC (a `-Wformat-truncation` warning under `-Werror`),
  so the suite had quietly been running unsanitized; that is fixed;
- `crates/tuo-cli/tests/map_string_balance.rs`, an exact allocation balance
  end to end: a program doing every map operation over many rounds, compiled
  to an object by each backend, linked with the real trap/effect/map runtime
  and a test-only counting allocator, must free exactly what it allocated
  (Cranelift, and LLVM unoptimized, where every allocation goes through the
  `tuo_rt_alloc` symbol);
- checker tests: `String` values check clean on both key kinds, struct and
  enum values (including a one-`Int`-field struct) stay `T0001` in the
  inferred and annotated spellings, and nested maps stay refused.

**Still open in B2.2:**

- struct and enum values, together with moving the value check to the single
  `is_supported_array_element` predicate;
- `tools/py2tuo` translating `dict[str, str]`, with the 1,870-file survey
  re-run to measure the coverage delta;
- the `Map[Str, String]` variant of the `map-lookup` workload the benchmark
  plan below calls for.

### Stage B (original plan) — widen `V` to the ADR-0012 element set

`V` becomes the set ADR-0012 already admits for array elements: `Int`, `Bool`,
`Float`, `Str`, `String`, and structs/enums whose fields are themselves
supported. `K` stays `Int`/`Str`.

The type checker's `value_ok` becomes the existing `is_supported_array_element`
predicate rather than a second, drifting list — one definition of "an element
tuonelang can hold", two containers consuming it.

The runtime shims are the real work. Today `tuo_rt_map_int_insert` and friends
take `long long` values by value; a `String` value is three words and a `Str`
two. Two options, and this ADR picks the second:

1. *A shim per value type.* Mirrors the existing `_int`/`_str` key split.
   Rejected: the surface is `|K| × |V|` and grows multiplicatively; the shim
   source is already the largest hand-written C in the tree.
2. **A value-stride-parametric shim.** The map stores opaque value bytes of a
   `stride` the caller supplies, exactly as `tuo_rt_map_drop` already takes a
   stride today. Insert/get/remove memcpy `stride` bytes; the *compiler* emits
   the deep-copy and drop glue around the call, which is precisely what
   `HeapGlue::DeepFixup`/`DropInPlace` already do for array elements. One shim
   family, any value type, and the ownership logic stays in the compiler where
   the type information is.

`tuo_rt_map_drop` gains a per-value drop callback (or, following the array
precedent, the compiler emits a drop loop over `keys` before calling the plain
deallocating drop). ADR-0011 specified that "drop glue frees the table **and**
drops each contained value"; the runtime today frees the table and does *not*
drop values, which is invisible while `V = Int` and a leak the moment `V` owns
heap. This stage makes the specified behavior real.

ABI version **bumps** (the map's value slot changes width and gains ownership).

### Stage C — the dogfooding payoff — **LANDED (with a null result)**

`tools/py2tuo` now translates `dict[K, V]` for the widened value set, and a
`dict[int, bool]` Python program compiles and agrees with CPython at runtime —
which was impossible before Stage B.

**The coverage delta on the motivating corpus is zero, and that is reported
rather than hidden.** Re-running the 1,870-file CPython survey: 4/58 dict-typed
annotations were translatable before Stage B, and 4/58 after. No
`dict[str, bool]` or `dict[int, float]` annotation appears in that corpus at
all. The widening's value is therefore **not** more Python coverage; it is that
`Bool` and `Float` map values now *work correctly on all three engines* instead
of passing `tuo check` and failing in the backend. The coverage argument for
this ADR rests entirely on Stage B2 (`dict[str, str]` is the most common
unsupported shape), which is exactly the stage still open.

### Stage C (original plan) — the stdlib and dogfooding payoff

`std::collections` gains the map combinators the widened surface makes
writable, and `tools/py2tuo` widens its `dict[K, V]` translation to the new
set — the demand that motivated the ADR, now measurable as a coverage delta
against the same 1,870-file corpus.

### Deliberately out of scope

- **User key types** (structs, enums as keys) — needs the trait system's
  `Hash`/`Eq`. Unchanged from ADR-0011: v0 keys are scalars whose equality is
  fixed by the language, not by a user contract. This is the item that genuinely
  needs traits, and conflating it with value widening is what made the whole
  area look blocked.
- **`Map == Map`, map literals, an `entry` API, `Set[K]`** — unchanged from
  ADR-0011.
- **`Map` as a `V`** (nested maps) — `ty_owns_heap` already reports `Ty::Map`
  as heap-owning, so the glue would work; but the recursion boundary (`T0016`)
  and the deep-copy cost of a nested container deserve their own decision.

## Benchmark plan

Per the project rule that a language change carries a benchmark plan:

- The existing **`map-lookup`** lab workload gains a `Map[Str, String]`
  variant, so the widened value's cost (a deep copy on `get`, drop glue on
  removal) is a *recorded measurement* against the `V = Int` baseline, not an
  assumption that it is free. The C peer stores `char*` values with the same
  ownership discipline; the Go peer uses `map[string]string`.
- The gate is the same as every prior container ADR: the workload must be
  committed and measuring before the ADR moves to accepted.

## Consequences

*Easier:* the dictionary-shaped programs that motivate a map at all — indexing
records by name, grouping strings, memoizing a computed string — stop needing
parallel arrays. `py2tuo`'s dict coverage rises from the two-shape floor.
`Map[Str, String]`, the single most common Python dict shape, becomes
expressible.

*Harder:* the value slot stops being a machine word, so map operations acquire
the deep-copy and drop obligations arrays already carry. A `get` on a
heap-owning value is no longer free, and the benchmark plan exists to keep that
honest rather than to hide it.

*Unchanged:* keys. The trait system remains the gate for user key types, and
this ADR deliberately does not smuggle in a partial `Hash`.

*Fixed:* a real soundness defect, in the code path this ADR touches anyway.
Stage A is worth shipping on its own even if Stages B and C are never taken up.
