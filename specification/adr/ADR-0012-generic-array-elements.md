# ADR-0012: Generic `Array[T]` element types — widening the monomorphic builtin surface

- **Status:** proposed (Stages A/B/C landed, including the owned-element native
  increment — deep-copy `get` + recursive drop glue on both backends; the
  ADR-0008 combinator instantiations and the dogfood oracle remain before
  acceptance)
- **Date:** 2026-08-19
- **Context:** tuonelang's growable `Array` is **generic in its type but
  monomorphic in its operations**. ADR-0009 landed `Ty::Array(Box<Ty>)` — the
  type `Array[T]` exists in the checker for *any* element `T`
  ([`tuo-types` `ty.rs`], `Ty::Array`) — and `abi.md` §Arrays already specifies
  the `{ptr, len, cap}` header for a *generic* `T` (contiguous storage at `T`'s
  natural stride, per-element drop front-to-back, the `tuo_rt_alloc`/`dealloc`
  boundary). But the **operation surface is hardcoded to `Int`**: the array
  builtins in [`tuo-types` `check.rs` `builtin_signature`] read

  ```rust
  let int_array = || Ty::Array(Box::new(Ty::int()));
  Builtin::ArrayEmpty => (Vec::new(), int_array()),
  Builtin::ArrayPush  => (vec![int_array(), Ty::int()], Ty::Unit),
  Builtin::ArrayGet   => (vec![int_array(), Ty::int()], Ty::int()),
  // …ArrayPop, ArrayLen likewise fixed to Int
  ```

  so `std::array::push(xs, some_string)` is a plain `T0001` ("expected `I64`,
  found `String`"). This is the same honest-narrowness ADR-0009 chose for its
  first increment and ADR-0011 (the hash map) inherits — it names this exact wall
  as its first blocker ("`Array` is generic in the *type* … but its operation
  surface is monomorphic `Int` … because v0 has no method dispatch or generic
  instantiation for builtins").

  This monomorphic surface is now the single most-cited prerequisite in the
  backlog. It blocks:

  - **`std::str::split` / `join`** (this ADR's motivating case, ADR-0012's
    sibling to the P1 string work): both inherently produce or consume a
    **sequence of strings**, i.e. `Array[Str]` / `Array[String]`. The rest of the
    Go-parity string layer (`replace`, `replace_first`, `builder`) already
    shipped over owned `String`; split/join are the only ones that cannot be
    written today, and *only* because the element type is not `Int`.
  - **`Array[T]` for aggregate elements** — arrays of user structs/enums, of
    `Str`, of `String` — which real programs and LLM-generated code reach for by
    reflex (an array of records, an array of parsed tokens).
  - The **value side of ADR-0011's hash map** in part: `keys(m) -> Array[Int]`
    is fine today, but `keys` over a `Map[Str, _]` wants `Array[Str]`, and a
    `group_by` returning grouped values wants `Array[V]`.

  The reason it is not already here is a real decision about **how far to
  generalize**. There are two very different things one could mean by "generics":

  1. **Widen the builtin element surface** — keep the operations
    *language-provided builtins* (no user type parameters, no dispatch), but make
    their signatures **element-parametric**: `push` accepts `(Array[T], T)` and
    `get` returns `T` for a *defined set* of element types `T`, the checker
    unifying `T` from the receiver exactly as it already does for the fixed
    `[T; N]` and `Option[T]`. The ABI is already generic; only the checker's
    signature machinery, the MIR ops' element-size awareness, and the backends'
    per-element lowering need to catch up.
  2. **User-written generics** — `fn map[T, U](xs: Array[T], f: fn(T) -> U) ->
    Array[U]`, generic structs, monomorphization or dictionary-passing, bounds/
    traits. This is a whole type-system feature (Q-0010 territory: it interacts
    with the still-absent trait system, inference over user type parameters, and a
    monomorphization or erasure strategy in MIR/codegen).

  These are **not the same size**, and conflating them would either overpromise
  (claiming user generics the compiler cannot do) or underdeliver (leaving
  split/join blocked on a feature years away). This ADR takes option 1 and
  **explicitly defers option 2**, exactly as ADR-0008 shipped Tier 1 function
  values and deferred Tier 2 closures, and ADR-0011 ships a monomorphic map and
  defers user key types to the trait system.

- **Decision (proposed):** land **generic `Array[T]` element types over a defined
  set of `T`** — widen the ADR-0009 array builtin surface from `Int` to the
  element types the runtime can already lay out and drop, keeping the operations
  as language-provided builtins. **User-written generics stay deferred** (a
  future ADR, tracked under Q-0010).

  1. **The type is unchanged.** `Ty::Array(Box<Ty>)` already admits any `T`; no
     new `Ty` variant. `Array[T]` is `Copy` **iff `T` is `Copy`** — wait: no. An
     `Array[T]` owns a heap buffer, so it is **never `Copy`** regardless of `T`
     (exactly as today for `Array[Int]`, and as ADR-0009 fixed for `String`).
     `T`'s own `Copy`-ness governs only whether *elements* need drop glue (see
     §5), not the array's move semantics.

  2. **The supported element set, staged.** The v0 increment supports element
     types the ABI already lays out and the drop path already handles:

     | Element `T` | `Copy`? | Notes |
     |-------------|---------|-------|
     | `Int` (`I64`) | yes | today's surface, unchanged |
     | `Bool` | yes | scalar, trivial stride/align |
     | `Str` | yes (borrowed fat pointer) | element is `{ptr,len}`; **borrow provenance** rules apply (see §6) |
     | `String` | **no** | owned; array drop must drop each element (ADR-0009 drop glue) |
     | user `struct` / `enum` whose fields are supported | per fields | recursively laid out (ADR-0004); drop if any field is non-`Copy` |

     Deliberately **not** in this increment: `Array[Array[T]]` and
     `Array[Map[…]]` (nested owned containers — sound in principle via the same
     drop recursion, but staged to keep the first increment's drop-placement test
     matrix bounded), and any element type behind a memory wrapper
     (`Box`/`Shared`/`Weak` values, themselves deferred). A call with an
     unsupported element type is an ordinary `T0001`, never a silent half-feature
     — the same honesty rule ADR-0009/0011 hold.

  3. **The builtins become element-parametric.** `builtin_signature` for the
     `Array*` builtins stops returning a fixed `int_array()` and instead
     describes a signature *schema* unified against the call: `ArrayPush` is
     `(Array[T], T) -> Unit`, `ArrayGet` is `(Array[T], Int) -> T`, `ArrayPop` is
     `(Array[T]) -> Option[T]`, `ArrayLen` is `(Array[T]) -> Int`, `ArrayEmpty` is
     `() -> Array[T]` (the element inferred from context/unification, as an empty
     `[T; N]` literal's element already is). The checker unifies `T` from the
     receiver's element type using the **existing** `Ty::Array` unification
     ([`infer.rs`] already recurses `(Ty::Array(a), Ty::Array(b)) => unify(a,b)`),
     then checks the element argument/result against it. **`ArrayEmpty` with no
     inferable element is a type error** naming that the element type is
     undetermined (a fresh `Rxxxx`/`Txxxx` "cannot infer array element type"),
     never a silent default to `Int` — the same way an undetermined numeric
     literal is not silently sized. This is the whole type-checker change: no new
     inference machinery, no user type parameters — the receiver *is* the witness
     for `T`.

  4. **MIR carries the element size.** The ADR-0009 array `HeapOp`/`HeapMutate`
     MIR variants become **element-size aware**: `push`/`get`/`pop` compute the
     element offset as `i × stride(T)` from `abi::layout_of(T)` (already the ABI
     rule, `abi.md` §Arrays), rather than assuming an 8-byte `Int`. The MIR
     verifier's array invariants extend to check the element type is one of the
     supported set and that a `get`/`push` element type matches the array's
     (the `Array[I64]`-specific check at [`verify.rs`] generalizes to
     `Array[T]`). No new MIR instruction — the existing shapes gain a type
     operand, exactly as ADR-0011 extends rather than invents.

  5. **Drop glue follows the element type.** Dropping an `Array[T]` where `T` is
     non-`Copy` (`String`, a struct with an owned field) drops **each element
     front-to-back**, then frees the buffer — the rule `abi.md` §Arrays already
     states generically and ADR-0009 pinned for the `String` scalar. `pop`
     transfers ownership of the popped element out (it becomes the caller's to
     drop); overwriting is not an array op (no `set`; a v0 array grows and is
     read, matching today's surface). This is the load-bearing soundness point
     and gets the most test weight (see Staging): every element allocated is
     freed exactly once, on the interpreter and both backends, under
     insert/drop churn.

  6. **`Array[Str]` and borrow provenance.** An element of type `Str` is a
     borrowed fat pointer, so an `Array[Str]` holds borrows, not owned bytes.
     ADR-0010 established that a `Str` derived from a `String` is **borrow-scoped**
     (`O0011`: a view may not outlive its `String`). The same rule extends to
     array elements: an `Array[Str]` built from `String`-derived views keeps
     those `String`s borrowed for the array's lifetime, and returning such an
     array where the source `String`s do not outlive the frame is refused
     (`O0011`). An `Array[Str]` of **literal** `Str`s (which borrow nothing) is
     unrestricted. This means **`split` returning `Array[Str]` over a borrowed
     input** is sound within the input's borrow, and **`split` returning
     `Array[String]`** (owned copies) escapes freely — the ADR ships **both** are
     not needed; §Consequences picks one.

  7. **The ABI does not change layout, but the version may still bump.** The
     `Array[T]` header is `{ptr, len, cap}` for any `T` already (`abi.md`
     §Arrays), so no *layout* changes. However, because native code for
     `Array[String]`/`Array[Str]`/`Array[struct]` is **new emitted behavior**
     (element-size-aware indexing and per-element drop the backends did not
     previously generate), the increment is pinned against `#[repr(C)]` reference
     types for each new element category and `abi::ABI_VERSION` **bumps** in the
     same commit as those pinning tests, per the "bump on any layout-affecting
     change in the same commit that moves the pinning tests" rule — here the
     conservative reading (new heap-touching codegen) takes the bump even though
     the header bytes are unchanged.

  The load-bearing rules, restated:

  - **This is builtin widening, not user generics.** No `fn f[T]`, no generic
    structs, no traits/bounds, no monomorphization pass. The array operations
    stay language-provided builtins whose signatures are element-parametric and
    resolved by unifying against the receiver. A user still cannot write their own
    function generic over `T` — that is Q-0010 and a separate future ADR.
  - **Heap array ops stay pure** (ADR-0009 §"Heap operations are pure"), for any
    element type — allocation is deterministic computation — so **specs may build
    and query `Array[String]`/`Array[Str]`/`Array[struct]` freely**, bounded by
    the interpreter's `MemoryBudget`. Widening the element type does not make an
    op effectful.
  - **An unsupported element type is `T0001`** today; the generic `Ty::Array`
    underneath means each later element category (nested arrays, wrapped values,
    and — under the future trait system — arbitrary user types with the trait
    bounds their ops require) is **purely additive**.

  **Deliberately out of scope** (each stays refused or a plain type error):

  - **User-written generic functions, structs, and impls** (`fn map[T,U]`,
    `struct Pair[A,B]`) — the whole option-2 feature; deferred to a future ADR
    under Q-0010. The stdlib's higher-order combinators stay the ADR-0008 shape
    (concrete element types), gaining `String`/`Str`/struct instantiations as this
    ADR widens the array they fold over, not user type parameters.
  - **Nested owned containers** (`Array[Array[T]]`, `Array[Map[K,V]]`) — sound by
    the same drop recursion, staged to a later increment.
  - **Native lowering of a heap-owning element** (`Array[String]`,
    `Array[struct-with-owned-field]`) — deferred at first (Stage B initially
    lowered no-heap elements only), **since landed** as the owned-element
    increment: a native `get` deep-copy and array drop's per-element recursive
    free on both backends (see the Stage B follow-up below). `std::str::split`
    returning `Array[String]` (Stage C) therefore runs natively too.
  - **An array `set`/`insert-at`/`remove-at`** — v0 arrays grow (`push`) and are
    read (`get`/`pop`/`len`); mutating an interior element is a later increment
    (and interacts with drop of the displaced element).
  - **`Array == Array`** for non-scalar elements — element-wise structural
    equality over owned elements is deferred, as `Map == Map` and `String`
    ordering were; specs compare *observations* (`len`, `get(i)` extracted to a
    scalar), the established heap-spec idiom.

- **Consequences:**
  - *Easier:* the string layer completes — `std::str::split(s, sep) ->
    Array[String]` and `std::str::join(parts, sep) -> String` become writable and
    spec-checkable, closing the last Go-parity string gap this project's
    dogfooding named. Arrays of records, tokens, and rows become expressible, so
    `examples/data-pipeline` can hold parsed records in an `Array[struct]` instead
    of packed parallel `Array[Int]`s, and the ADR-0008 combinators (`fold`,
    `map_into`, `filter_into`) gain `String`/struct element instantiations. It is
    also the enabling increment under ADR-0011's richer views (`keys` over
    `Map[Str,_]`, `group_by`).
  - *Harder:* this is the **first per-element-type drop and codegen matrix** for
    a growable container — the backends must emit element-size-aware indexing and,
    for non-`Copy` elements, a drop loop that matches the interpreter instruction
    for instruction. It is smaller than ADR-0011 (no new type, no hashing, no new
    ABI header) but touches the same three layers (checker signatures, MIR
    element-size ops, backend lowering) and carries real drop-soundness risk that
    the test matrix must cover per element category.
  - *Trade-off — one `split` return type, chosen for honesty:* `split` returns
    **`Array[String]`** (owned copies), not `Array[Str]` (borrowed views). The
    owned form escapes its function freely (no `O0011` lifetime tangle at every
    call site), matches how a caller expects to *keep* split pieces, and composes
    with the whole `String` surface. The borrowed `Array[Str]` form is more
    efficient but forces every caller into the ADR-0010 borrow scope; it is
    deferred as an optimization, not shipped as a second spelling (the "one
    obvious API" rule). `join` accepts `Array[Str]` **or** `Array[String]`? No —
    it accepts **`Array[Str]`** (each piece viewed via `as_str` at the call site
    if owned), the single input form. This keeps split/join a clean pair without a
    combinatorial API.
  - *Trade-off — receiver-witnessed inference:* because `T` is inferred from the
    receiver, `std::array::empty()` in a context with no element evidence is a
    type error, slightly less convenient than a defaulting rule. This is
    deliberate and matches the numeric-literal-must-be-determined stance: a silent
    default to `Int` would resurrect exactly the monomorphic assumption this ADR
    removes.

- **Staging (spec-before-code, per the ADR-0004/0006/0009/0011 precedent):**
  - **Stage A — landed.** The checker widening: the `Array*` builtins are
    checked by `Checker::check_array_builtin_call` (in `tuo-types`), which
    resolves the element type `T` from the receiver argument (a fresh var for
    `empty()`) and validates `push`/`get`/`pop` against it; an undetermined
    `empty()` element is `T0011` ("type annotation needed"), and an element
    outside the supported set (the scalars `Int`/`Bool`/`Str`/`String` and
    user structs/enums whose fields are supported) is `T0001` naming the type.
    The MIR verifier (`tuo-mir::verify`) reads the array element from the
    subject/target place's `Ty::Array(elem)` instead of a hardcoded `Int`, so
    `push`/`get`/`pop` verify against the real element. The reference interpreter
    needed **no execution change** — `Value::Array(Vec<Value>)` already holds any
    element and `push`/`get`/`pop` move `Value`s abstractly; drop is
    de-initialization, and Rust's own `Vec<Value>` drop recurses through owned
    elements (`String`), so `Array[String]` frees correctly with no new glue.
    The **native backends refuse a non-`Int` element** with an honest
    `CodegenError::unsupported` pointing back to the interpreter
    (`require_native_array_element` in both `tuo-codegen-cranelift` and
    `tuo-codegen-llvm`), so nothing mis-compiles ahead of Stage B. Pinned by
    `tests/types/fixtures/{ok,err}/array_elements.tuo` (+ blessed snapshot),
    `crates/tuo-mir-interp/tests/conformance.rs::array_generic_elements_round_trip`
    (`Array[Str]`/`Array[String]`/`Array[Bool]` build→get→pop), and the existing
    native + three-way differential suites (unchanged: `Array[Int]` still lowers
    and agrees interpreter == Cranelift == LLVM).
  - **Stage B — landed for no-heap elements.** Both backends
    (`tuo-codegen-cranelift`, `tuo-codegen-llvm`) lower `push`/`get`/`pop` for
    every element type that **owns no heap**: a scalar (`Int`/`Bool`) is a
    single load/store of the element's own register width (`I8` for `Bool`,
    not a hardcoded `I64`), and an aggregate that owns no heap (`Str` — a
    two-word fat pointer — and a `Copy` struct/enum) is moved by a memcpy of
    `stride` bytes to/from the buffer, indexing at `i × stride(T)` from the one
    runtime ABI both backends already consult. `array::get` of an aggregate
    element routes through the aggregate rvalue path (into a dest slot) rather
    than a scalar register value. The `{ptr,len,cap}` header layout is unchanged
    across element types, so **no `abi::ABI_VERSION` bump** was needed. Pinned by
    `array_str_elements`/`array_bool_elements`/`array_struct_elements` fixtures
    agreeing interpreter == Cranelift == LLVM in
    `crates/tuo-cli/tests/codegen_three_way.rs`
    (`generic_array_elements_agree_across_all_three_engines`), plus a native
    build/drop churn loop completing in bounded memory (each array's buffer freed
    once; no-heap elements need no per-element drop).

    **Heap-owning elements were at first interpreter-only**, a refinement
    discovered during implementation: the reference interpreter's `array::get`
    returns a **deep clone** of the element (`Value::clone`), so a native `get`
    of an owned element (`String`, a struct with an owned field) needs a
    recursive deep copy — not just a header memcpy, which would create two
    owners of one buffer (double-free) — and array drop needs per-element
    recursive free. Both backends initially **refused** a heap-owning element
    (`ty_owns_heap`) with an honest `unsupported` diagnostic. `Str` (a borrowed
    pointer that owns nothing) is *not* heap-owning, so `Array[Str]` lowered
    from the start.

    **Stage B follow-up — the owned-element increment landed.** Both backends
    now lower the whole checker-accepted element set through one recursive
    walker (`emit_heap_glue`, mirrored Cranelift/LLVM) with two modes over the
    same traversal, so copy and drop can never disagree about what owns a
    buffer: **deep-copy fixup** — after `get`'s shallow stride memcpy, every
    heap-owning part of the copy (a `String`'s bytes, a struct/tuple field at
    its ABI offset, the live enum/`Option`/`Result` variant's payload found by a
    discriminant-compare chain, each element of a nested buffer via a genuine
    counted loop — codegen's first back-edge) is re-pointed at a freshly
    allocated copy, exactly the interpreter's `elements[index].clone()`; and
    **drop-in-place** — the same walk frees element buffers front to back (the
    interpreter's `Vec` drop order) before the containing buffer, each exactly
    once. `push`/`pop` stay shallow moves (MIR de-initializes the source). The
    refusal seam narrows from `ty_owns_heap` to `ty_contains_wrapper` — only an
    element carrying a `Box`/`Shared`/`Weak` (wrapper values are not lowered
    anywhere) is still refused. Two side effects worth naming: recursive drop
    also **fixes a pre-existing silent leak** (dropping a `String`-carrying
    struct/enum/`Option` local natively was a no-op before — invisible to
    exit-code differentials), and the increment forced `std::string::as_str`'s
    native lowering (ADR-0010 Stage B, a two-word zero-copy view), which the
    `str.tuo` module's own spec helpers needed to compile natively. No header
    or layout change — **no `abi::ABI_VERSION` bump**. Pinned by the
    `array_owned_string_elements` (deep-copy independence: mutate the copy,
    re-read the element; a whole un-matched `Some { String }` dropped through
    the variant glue) and `array_owned_struct_elements` (per-field copy/drop)
    fixtures agreeing interpreter == Cranelift == LLVM
    (`owned_array_elements_agree_across_all_three_engines`), the `str_as_str_view`
    three-way fixture, and `stdlib_split_and_join_run_natively` (the Stage C
    payoff, both backends); both owned-element fixtures were additionally
    leak-checked with macOS `leaks` at 0 bytes leaked — a measurement, not a CI
    promise.
  - **Stage C — split/join landed.** `std::str::split(in s: Str, in sep: Str)
    -> Array[String]` and `std::str::join(in parts: Array[Str], in sep: Str)
    -> String` ship in the pure executable tier with specs, following Go's
    `strings.Split`/`Join` semantics: `"a.b.c"` on `"."` yields
    `["a", "b", "c"]`, adjacent/leading/trailing separators yield empty pieces,
    the empty input yields one empty piece, separators do not overlap, and the
    **round-trip law** `join(split(s, sep), sep) == s` is spec-pinned (via
    `rejoins_3`, which also exercises §6's borrow-provenance case for real: an
    `Array[Str]` of `String`-derived `as_str` views with every source piece
    live, accepted by the ownership checker). An empty separator yields the
    whole input as one piece — Go's rune-splitting is a Unicode algorithm the
    byte-level core does not implement, and the doc says so. Because
    `Array == Array` over non-scalar elements is deferred, the specs compare
    observations (`len` + a per-index `nth_is` text check), the established
    heap-spec idiom. `split`'s result is an owned-element array; since the
    owned-element increment landed it **runs natively on both backends**
    (`stdlib_split_and_join_run_natively`), and `join` consumes the
    natively-lowered `Array[Str]`. **Still open before this ADR can be
    accepted:** the ADR-0008
    combinators' `String`/struct instantiations, and the dogfood oracle
    (`data-pipeline` holding parsed records in an `Array[struct]`, spec-pinned
    equal to its packed-`Int` predecessor).

- **Benchmark consideration:** this ADR adds no *new* performance-lab workload of
  its own — it widens an existing capability rather than adding a new runtime
  shape. Its measurable payoff rides on the workloads its consumers add:
  ADR-0011's `map-lookup` (whose `keys`/`group_by` use `Array[Str]`), and a
  **`string-processing` variant** that tokenizes via `split` into an
  `Array[String]` and rejoins — added as a comparable syntax/semantics variant of
  the existing workload (with C and Go peers building an array of owned strings
  the same way), following the exact mechanics by which the runtime lab flips a
  workload from scan-only to build-and-scan. Per the lab's rule, any such
  variant's `Verdict` is `Measured` only when both sides compile, run, and agree
  on the observable exit; the native per-element lowering that gates it has
  since landed (the owned-element increment), so the variant is now addable.
  This ADR is not "accepted" until its Stage C consumers (split/join with
  specs, and the dogfood oracle) are committed and green.

- **Dependencies and sequencing:** this ADR is **independent of the trait
  system** and lands on today's monomorphic-builtin machinery — it widens that
  machinery's element surface, it does not replace it. It **composes with
  ADR-0010** (`String`→`Str` view): `Array[Str]` borrow provenance is exactly the
  ADR-0010 rule extended to array elements, and `split -> Array[String]` +
  `join(Array[Str])` rely on `as_str` to bridge owned pieces to borrowed views at
  call sites. It is **the enabling increment for the richer half of ADR-0011**
  (`Array[Str]` keys/values) but does **not** block ADR-0011's core, which ships
  `Array[Int]` keys today. It **supersedes nothing**; it is the natural successor
  increment to ADR-0009 (which shipped `Array[Int]`) and the sibling of the P1
  string work (which shipped `replace`/`builder` and left `split`/`join` to this
  ADR). When user-written generics eventually land (Q-0010, a future ADR), this
  builtin surface remains — user generics add a *second* way to be polymorphic
  (over user type parameters), not a replacement for the language-provided array
  ops, exactly as user `Hash`/`Eq` will supersede ADR-0011's internal hash for
  user key types without removing the built-in scalar-key maps.
