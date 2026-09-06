# ADR-0011: The hash map — a keyed associative container

- **Status:** accepted (2026-08-22 — all three stages landed; see Resolution)
- **Date:** 2026-08-19
- **Context:** tuonelang has **no associative container**. The runnable core
  grew a growable `Array[Int]` (ADR-0009) and generic higher-order combinators
  over it (ADR-0008), but there is no `map[K]V` — no way to look a value up by
  key in better than O(n), and no way to model the countless real programs whose
  natural shape is a dictionary (counting occurrences, deduplicating by key,
  indexing records, memoizing). This is the single largest capability gap
  between tuonelang's runnable core and a mainstream systems language: Go's
  built-in `map[K]V`, Rust's `HashMap<K, V>`, and their standard-library
  algorithms all assume it. Dogfooding felt it directly — `examples/data-pipeline`
  and `examples/http-service` both reach for "group by key" / "route table"
  shapes they must currently fake with linear scans over parallel arrays.

  The reason it is not already here is that a hash map needs three things v0
  lacks, and each is a real decision:

  1. **A container generic over its element types.** `Array` is generic in the
     *type* (`Ty::Array` exists for any `T`) but its **operation surface is
     monomorphic `Int`** (ADR-0009): `push`/`get` only type-check for
     `Array[Int]`, because v0 has no method dispatch or generic instantiation for
     builtins. A map is generic in **two** parameters (`K` and `V`), so the same
     wall stands twice as tall.
  2. **A key-equality and hashing contract.** A hash map must (a) hash a key to a
     bucket and (b) compare keys for equality on collision. v0 defines `==` only
     on the **scalars** `Int`/`Bool`/`Str`/`String` (and `Str`/`String` by byte
     content); there is no user-extensible equality (no traits — Q for the trait
     system), and there is **no hashing primitive** anywhere in the ABI or
     runtime. Both are new surface.
  3. **A collision and growth strategy in the ABI.** `abi.md` specifies the
     `String`/`Array` `{ptr, len, cap}` header and the `tuo_rt_alloc`/`dealloc`
     boundary, but says nothing about a hash table's layout, load factor,
     probing/chaining, or its interaction with drop glue (a map owning non-`Copy`
     values must drop each on removal and on the map's own drop).

  This ADR does **not** try to land all three at once. Per the project's
  staged, honest-tier discipline, it lands the **narrowest map that is genuinely
  useful and genuinely sound**, and names precisely what stays deferred so
  nothing advertises a capability the compiler cannot perform.

- **Decision (proposed):** land a **v0 map core** — a monomorphic
  `Map[Int, Int]` (and, in the same shape, `Map[Str, Int]`) hash map as a
  language-provided builtin, exactly as ADR-0009 landed the monomorphic
  `Array[Int]` surface over the generic `Ty::Array` type. The generic type
  `Map[K, V]` exists; the v0 **operation surface is monomorphic** over the two
  key types the scalar equality/hash contract already supports.

  1. **The type.** A new generic `Ty::Map` (`Map[K, V]`), **non-`Copy`**
     (it owns a heap table), parallel to `Ty::Array`. The v0 builtins operate on
     `Map[Int, Int]` and `Map[Str, Int]` only; a call with any other key or value
     type is an ordinary type error (`T0001`), never a silent half-feature.

  2. **The builtin module `std::map`:**

     | Signature | Meaning |
     |-----------|---------|
     | `fn empty() -> Map[Int, Int]` | A new empty map. Never traps. |
     | `fn insert(mut m: Map[Int, Int], take k: Int, take v: Int) -> Option[Int]` | Insert/overwrite `k → v`; return the **previous** value for `k` (`Some`) or `None` if `k` was absent. |
     | `fn get(in m: Map[Int, Int], take k: Int) -> Option[Int]` | The value for `k`, or `None`. Never traps. |
     | `fn contains_key(in m: Map[Int, Int], take k: Int) -> Bool` | Is `k` present? Never traps. |
     | `fn remove(mut m: Map[Int, Int], take k: Int) -> Option[Int]` | Remove `k`, returning its value (`Some`) or `None`. |
     | `fn len(in m: Map[Int, Int]) -> Int` | The entry count. Never traps. |
     | `fn keys(in m: Map[Int, Int]) -> Array[Int]` | A **new** array of the map's keys, in **unspecified but deterministic** order (see below). |

     The `Map[Str, Int]` surface is the identical set with `K = Str`. There is
     **one** obvious API per operation — no `get_or_default`/`entry` variants in
     v0 (a caller writes `match get(m, k) { … }`, the established `Option`
     idiom).

  3. **The hashing contract — a language-owned primitive, not user-extensible.**
     v0 adds an internal, ABI-owned hash over the two supported key types:
     `Int` hashes by a fixed integer mix; `Str`/`String` hashes over its bytes
     (a fixed, documented function — e.g. FNV-1a, matching the "hand-rolled,
     vector-pinned, no new dependency" precedent `tuo-package`'s `sha256` set).
     The hash is **not** exposed as a user builtin and **not** user-overridable
     (no traits in v0); it exists only to place keys in buckets. Equality on
     collision reuses the existing scalar `==` (byte content for `Str`/`String`).
     When the trait system lands, a user `Hash`/`Eq` contract supersedes this
     internal one for user key types — an additive change, since v0 keys are
     exactly the scalars whose equality is already fixed.

  4. **Determinism — the load-bearing rule.** Every observable result must be
     **deterministic**, because specs run in the interpreter and the differential
     suites demand interpreter == Cranelift == LLVM. Therefore:
     - the hash function is **fixed** (no per-run seed, no ASLR-derived
       randomization — unlike Go, whose map iteration is intentionally
       randomized);
     - `keys` (and any future iteration) returns entries in a **fixed order**
       derived from the table's deterministic layout (e.g. bucket-index then
       insertion order within a bucket), documented as "unspecified content
       order but identical across runs and backends." A program that needs sorted
       keys calls `std::collections::sorted_ascending(keys(m))`.
     This is stricter than mainstream maps and is deliberate: reproducibility is a
     tuonelang invariant (the spec sandbox, the differential gate), and a
     nondeterministic container would break it.

  5. **Ownership and drop.** `Map[K, V]` is non-`Copy`; passing to `take` moves
     it, `mut` requires a mutable place (`O0004` otherwise). Drop glue frees the
     table **and** drops each contained value (for a `V` that is non-`Copy` — not
     in the v0 `Int` value surface, but the drop machinery is specified now so
     `Map[Str, String]` is additive later). `remove`/`insert`-overwrite drop the
     displaced value. Exactly the drop-placement discipline ADR-0009 pinned for
     `String`/`Array`, extended to key/value pairs.

  6. **ABI and MIR.** `abi.md` gains the map header (a `{ptr, len, cap}`-style
     table descriptor + the chosen probing/chaining strategy, e.g. open
     addressing with Robin Hood or separate chaining — decided in Stage A and
     pinned against a `#[repr(C)]` reference), and `abi::ABI_VERSION` **bumps**
     (this is a new heap layout). MIR extends the ADR-0009 shapes rather than
     inventing new ones: value-producing pure ops (`get`/`contains_key`/`len`/
     `keys`) are `Rvalue::HeapOp` variants borrowing the subject place; the
     mutators (`insert`/`remove`) are `Statement::HeapMutate` through a
     `mut`-borrowed place. The verifier gains the map well-formedness invariants
     (`M00xx`); the optimizer treats map `HeapOp` as non-foldable (allocates) and
     `HeapMutate` as never-eliminable, as it already does for arrays. The
     allocator seam (`tuo_rt_alloc`/`dealloc`) is reused unchanged.

  The load-bearing rules, restated:

  - **Heap map ops are pure** (allocation is deterministic computation, ADR-0009
    §"Heap operations are pure"), so **specs may build and query maps freely**,
    bounded by the interpreter's `MemoryBudget`. No map op is effectful.
  - **`Map == Map` is deferred** (a type error in v0, like `String` ordering
    was): structural map equality needs a canonical comparison over an
    unordered container and is not needed to ship the core. Specs compare
    *observations* (`get`, `len`, `keys` after sorting), the scalar-extraction
    idiom the other heap specs already use.
  - **A wrong key/value type is `T0001`**, today — the narrow surface is honest,
    and the generic `Ty::Map` underneath means widening (more key/value types,
    then user types under the trait system) is purely additive.

  **Deliberately out of scope** (each stays refused or a plain type error):

  - **User key/value types** (structs, enums as keys) — needs the trait system's
    `Hash`/`Eq`; v0 keys are the scalars `Int`/`Str` whose equality is fixed.
  - **`Map[K, V]` for value types beyond `Int`** in the *operation* surface — the
    type exists for any `V`; the v0 ops are `V = Int`. `Map[Str, String]` and
    friends are additive once the drop path (already specified) is exercised.
    *(Taken up by [ADR-0023](ADR-0023-map-value-widening.md), which also fixes a
    soundness defect in how this narrowness is enforced.)*
  - **Ordered maps / sorted iteration as a built-in** — `keys` is deterministic
    but content-unordered; sorted views compose via `std::collections::sorted_*`.
  - **`Map == Map`, map literals, and an `entry`-style API** — later increments.
  - **A `Set[K]`** — a set is a `Map[K, Unit]`; ship the map first, add the set
    as a thin layer (or a stdlib wrapper) once it lands.

- **Consequences:**
  - *Easier:* the dictionary-shaped programs that currently fake it with parallel
    arrays get their natural O(1)-amortized shape — counting, dedup-by-key,
    record indexing, memoization. `std::collections` gains a `group_by`/`counts`
    executable tier over the map; `examples/data-pipeline` can index by key and
    `examples/http-service` can hold a real route table. The stdlib's Go-parity
    gap closes on its single biggest item.
  - *Harder:* this is the **first two-parameter generic container** and the
    **first hashing primitive** — real new surface in the type system (a second
    `Ty::Map` monomorphic-builtin family), the ABI (a new heap layout, an ABI
    version bump), the runtime (a documented, vector-pinned hash), and the drop
    machinery (key/value pairs). It is materially larger than ADR-0009, which is
    why it is staged and why the surface is kept monomorphic.
  - *Trade-off:* the fixed, unseeded hash and deterministic iteration are
    **stricter than mainstream maps** (Go randomizes iteration; Rust seeds its
    hasher). tuonelang chooses reproducibility over hash-flooding resistance in
    v0 — a defensible v0 stance given that programs run through a deterministic
    interpreter and a three-way differential gate, and additive to revisit
    (a seeded hasher behind a non-spec build flag is a later option). The narrow
    monomorphic surface is the same honest-narrowness trade ADR-0009 made for
    `Array[Int]`.

- **Staging (spec-before-code, per the ADR-0004/0006/0009 precedent):**
  - **Stage A** — this normative text; `abi.md`'s map layout + hash function
    (vector-pinned) + ABI version bump; the front end (`Ty::Map`, the
    `Map[Int,Int]`/`Map[Str,Int]` builtin surface, purity registration, the
    `T0001` for other type args); MIR (`HeapOp`/`HeapMutate` map variants,
    verifier `M00xx`); and the reference interpreter (a deterministic table).
    Pinned by `tests/types/fixtures/{ok,err}/map.tuo`,
    `crates/tuo-mir-interp/tests/conformance.rs`, and drop-placement fixtures
    (insert-overwrite, remove, map drop each free exactly once).
  - **Stage B** — native lowering on both backends (the table, probing/growth,
    and per-entry drop matching the interpreter instruction for instruction),
    linked through the existing `tuo_rt_alloc`/`dealloc`; pinned by `map_*`
    fixtures agreeing interpreter == Cranelift == LLVM in
    `crates/tuo-cli/tests/codegen_three_way.rs`, and a native leak-proxy loop
    (insert/remove churn completing in bounded memory) proving every entry frees
    exactly once.
  - **Stage C** — the stdlib payoff (`std::map` module honest-tiered; a
    `std::collections` `counts`/`group_by` executable tier over it), a dogfood
    oracle (`data-pipeline` or `http-service` answers a query through a real
    map, spec-pinned equal to its array-scan predecessor), and the
    performance-lab workload below. At that point this ADR can be accepted.

- **Benchmark consideration:** this ADR should add a new performance-lab
  workload — call it **`map-lookup`** — an insert-N / lookup-N / churn loop over
  `Map[Int, Int]`, with an equivalent-semantics C peer (a straightforward
  `#[repr(C)]`-compatible open-addressing table, same operations, same exit).
  Following the lab's rule, its `Verdict` is `Measured` only when both sides
  compile and run and agree on the observable exit; it publishes no number until
  Stage B lands the native table. Adding the entry follows the exact mechanics by
  which ADR-0004/0006/0009 added `collections`/`string-processing`/`allocation`:
  a new `Support::Supported` entry with a committed `.tuo` program and its C peer,
  no other change to the lab. This ADR is not "accepted" until `map-lookup` is
  committed and measured.

- **Dependencies and sequencing:** the map is **independent** of ADR-0010
  (`String`→`Str` view) and of the trait system, and can land on today's
  monomorphic-builtin machinery exactly as `Array[Int]` did. It becomes
  *materially better* once traits land (user key types, real `Hash`/`Eq`), but it
  does **not** wait for them — v0 keys are the fixed-equality scalars. If ADR-0010
  lands first, `Map[Str, Int]` composes cleanly with `String`-derived `Str` keys;
  if not, keys come from `Str` literals or copied `String`s, which is enough to
  ship.

- **Resolution (2026-08-22) — all three stages landed; the ADR is accepted:**
  - **Stage A.** `Ty::Map(K, V)` (non-`Copy`) joined the type core with the
    monomorphic `std::map` builtin surface exactly as decided — `empty`,
    `insert`, `get`, `contains_key`, `remove`, `len`, `keys` over
    `Map[Int, Int]` and `Map[Str, Int]`, receiver-witnessed `K`/`V`, `T0001`
    for any other determined pair, `T0011` for an undetermined `empty()`, and
    `Map == Map` refused (`T0006`) rather than given an order-sensitive
    stand-in. MIR gained `HeapOp::{MapEmpty, MapGet, MapContainsKey, MapLen,
    MapKeys}` and `HeapMutOp::{MapInsert, MapRemove}` (verified under
    `M0012`/`M0013`, `mir.md` §5.7/§4.3); the optimizer's existing
    never-fold/never-eliminate rules cover them via the shared `HeapOp`/
    `HeapMutate` arms. The **reference interpreter models the map as its
    observable contract** — an insertion-ordered association list — which
    resolved the determinism question *more strongly* than the proposal's
    bucket-order sketch: `keys` is **insertion order**, `remove` preserves
    the relative order of the rest, an overwrite keeps its position, so no
    observable depends on the hash at all. Pinned by
    `tests/types/fixtures/{ok,err}/map.tuo` (+ snapshot),
    `crates/tuo-mir-interp/tests/conformance.rs`
    (`map_reference_semantics_are_insertion_ordered`).
  - **Stage B.** `abi.md` §Maps specifies the layout: the same three-word
    `{ptr, len, cap}` header as `String`/`Array` over **dense
    insertion-ordered entries** (`{k, v}` stride 16 for `Int` keys;
    `{ptr, len, v}` stride 24 for `Str` keys), with the open-addressing hash
    index hidden *inside* the same allocation and owned entirely by the
    `tuo_rt_map_*` C runtime shim (`tuo_runtime::map`) — so both backends
    lower every map operation to the one shim (mirroring the
    `tuo_rt_alloc` seam) and cannot drift from each other, while `len`/
    `empty` lower inline as header reads/stores. The hash is the fixed,
    unseeded pair the decision named — splitmix64-finalizer for `Int`,
    FNV-1a for `Str` bytes — vector-pinned in Rust and constant-pinned in
    the C source; `abi::ABI_VERSION` bumped to **6**. Pinned by the
    three-way fixtures `map_int_ops`/`map_str_ops`/`map_churn`
    (interpreter == Cranelift == LLVM,
    `map_operations_agree_across_all_three_engines`) — the churn fixture
    crosses the growth threshold repeatedly with a sliding remove window —
    and the churn binary measured at **0 leaked bytes** under macOS `leaks`
    (a measurement, not a CI promise).
  - **Stage C.** The stdlib payoff: `std::collections::counts` (the
    frequency table — the `group_by` shapes holding an `Array` per key await
    non-`Int` values, as scoped) with specs over the map's own observables;
    the dogfood oracle in `examples/data-pipeline` — `totals_by_category`
    folds the whole batch into a `Map[Int, Int]` in ONE scan, spec-pinned
    equal to the streaming per-category re-scans and the record-struct path,
    and `main` **cross-checks the map path against the record path at
    runtime** (exit 144 only when they agree, natively); and the
    **`map-lookup`** performance-lab workload (insert-1000 / lookup-1000 /
    remove-500 churn, exit byte 232) with committed C *and Go* peers,
    measured through the same lab machinery as every other workload
    (`Support::Supported`, the tenth catalog entry, ninth supported).
    Pinned by `crates/tuo-cli/tests/stdlib.rs`, the data-pipeline specs in
    `dogfood_examples.rs`, and the lab suites (`tuo-bench/tests/lab.rs`,
    `tuo-cli/tests/lab_command.rs`).

  The acceptance condition — `map-lookup` committed and measured — is met.
