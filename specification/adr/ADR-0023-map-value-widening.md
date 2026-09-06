# ADR-0023: Widening the map surface — values beyond `Int`, and what still needs traits

- **Status:** proposed
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

### Stage A — make the boundary sound (no surface change)

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

### Stage B — widen `V` to the ADR-0012 element set

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

### Stage C — the stdlib and dogfooding payoff

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
