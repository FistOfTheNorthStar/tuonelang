# ADR-0016: std::json and the data increment — Float elements, indexed writes, and the recursion boundary

- **Status:** accepted (2026-08-24 — all stages landed; see Resolution)
- **Date:** 2026-08-24
- **Context:** the Go-parity review's last data-shaped gap is `encoding/json`
  — "no reflection" was the recorded blocker, but the real one is
  **recursive data**: a JSON tree is a recursive type, and v0 has no way to
  run one natively. Investigating what that would take surfaced three facts:

  1. **Recursive nominal types are a backend-surgery project, not a checker
     tweak.** Both native backends emit deep-copy and drop glue by
     *compile-time inline recursion over the type structure*
     (`emit_heap_glue` descending through `struct_shape`/`enum_shape`); a
     type that reaches itself through an `Array` field would recurse the
     **compiler** forever. Supporting it means per-type native glue
     functions that call themselves at runtime — real work in both
     backends, deserving its own ADR.
  2. **Worse, the front end does not reject recursive types at all today.**
     `struct S { next: S }` (infinite size) and
     `enum Json { Arr(Array[Json]) }` (finite via indirection) both
     type-check — the element-support comment in the checker claims an
     infinite-size error is "caught elsewhere", and it is not. The first
     hangs `layout_of` at codegen; the second hangs the glue emitter. A
     compiler hang on an accepted program is a standing soundness hole.
  3. **JSON does not need recursive types.** A document is a tree, and a
     tree flattens into an **index arena** — parallel arrays in DFS
     pre-order (`kinds`, `nums`, `texts`, `keys`, `firsts`, `nexts`) inside
     one plain struct, which v0 aggregates already run natively. What the
     arena *does* need and v0 lacks: `Float` as an array element (numbers),
     and an **indexed write** on growable arrays (linking a parent to a
     child appended later) — `std::array` has push/pop/len/get and no
     `set`, itself a plain Go-parity gap (`xs[i] = v`).

  The project rule applies: each is a language change, so each lands here as
  a deliberate increment, never ad hoc.

- **Decision:** land the **data increment** — three compiler changes plus
  the module they pay for:

  1. **The recursion boundary (new front-end check).** A struct or enum
     that reaches itself through its own fields/payloads — through *any*
     chain of by-value fields, `Array`/`Map` elements, `Option`/`Result`
     payloads, or tuple/fixed-array components — is a **type error at the
     declaration** (`T0016`), *unless* every cycle passes through a heap
     wrapper (`Box`/`Shared`/`Weak`), whose pointer indirection keeps the
     size finite and which the backends already refuse cleanly at
     value-construction time. This is the check the element rules' comment
     already assumed: it closes the compiler-hang hole for both the
     by-value and the through-`Array` cycle, and it is the honest boundary
     until a successor ADR gives the backends runtime-recursive glue. The
     diagnostic names the cycle and points at the wrapper escape hatch.
  2. **`Float` joins the array element set.** `Array[Float]` was excluded
     from ADR-0012's element set for no deep reason — a `Float` is a
     `Copy` scalar with a fixed 8-byte layout, exactly like `Int`. The
     checker admits it; both backends store/load `F64` elements; the
     interpreter needs nothing.
  3. **`std::array::set(mut xs: Array[T], take i: Int, take v: T)`** — the
     indexed write, over the same checker-accepted element set as
     `push`/`get`. Traps `IndexOutOfBounds` exactly as `get` does; for a
     heap-owning element the old value is dropped in place before the new
     one moves in (the existing per-site glue — no new recursion shape).
     One obvious API: `set` is the only in-place element write, mirroring
     `get` as the only read.
  4. **`std::json` — the payoff module.** JSON parsing and rendering,
     written in tuonelang, **entirely in the executable tier** (parsing is
     pure computation): a `Json` arena struct over the increment above, a
     recursive-descent parser (`parse(in s: Str) -> Result[Json, Str]`
     with positioned error messages), navigation accessors
     (`kind_of`/`num_of`/`text_of`/`key_of`/`first_child`/`next_sibling`/
     `member`), and a canonical `render`. Number parsing/formatting
     duplicate the module's own private float helpers (modules stay
     independent, the `std::time` precedent). Honest limits are documented
     in-module: numbers are IEEE `Float`, `\uXXXX` escapes are a parse
     error naming the gap, and objects preserve member order (the arena is
     insertion-ordered; there is no `Map[Str, Json]` in v0). Every public
     function carries specs — the whole module runs in the spec sandbox.

- **Consequences:**
  - *Easier:* real JSON decode/encode end to end, natively; `Array[Float]`
    numeric work; in-place array algorithms (`set` unlocks sort-in-place
    and friends later); the compiler can no longer be hung by a recursive
    declaration.
  - *Harder:* recursive nominal types are now *explicitly* rejected where
    before they were silently accepted-then-hung — a program that wants a
    recursive enum must wait for the successor ADR (or use the arena
    pattern this ADR demonstrates). The wrapper escape hatch keeps the
    already-shipped cyclic-shape declarations (`Weak` back-edges) legal.
  - *Trade-off:* an arena API is less ergonomic than a recursive enum with
    pattern matching; it is the honest v0 shape, and the successor ADR can
    layer the enum on later without breaking `std::json`'s surface.

- **Benchmark consideration (the gating workload):** a new `json-parse`
  runtime workload — per round, recursive-descent parse of a fixed 149-byte
  document into a kind/number arena over this ADR's increment, folding a
  structural checksum (the exit byte). The lab's workloads are single-file
  by design, so the workload carries its own committed parser rather than
  importing `std::json` (which is pinned natively by the stdlib suite
  instead); the peers are **C** (a `strtod`-based recursive-descent parser
  making the identical walk) and **Go**'s standard `encoding/json` — the
  parity target this ADR answers — unmarshalling into `any` and walking to
  the same checksum.

- **Deliberately out of scope:** recursive nominal types (the successor
  ADR: per-type, runtime-recursive clone/drop glue functions in both
  backends — this ADR's `T0016` is its enabling boundary), `Map[Str, Json]`
  (map values are `Int`-only per ADR-0011), `\uXXXX` escape decoding,
  reflection/derive-style serialization of user structs (needs traits),
  building a document from scratch (v0 renders what it parsed), and a
  streaming parser. Each is additive when dogfooding demands it.

- **Resolution (2026-08-24):** all stages landed in one increment and every
  acceptance condition is met by a committed, test-pinned artifact:
  - *The recursion boundary (`T0016`):* `check_recursion_boundary` in the
    type checker rejects every struct/enum that reaches itself through
    fields/payloads without a heap-wrapper indirection — by-value cycles,
    `Array`/`Map`/`Option`/`Result`/tuple/fixed-array cycles, and generic
    self-application alike — while `Weak`/`Shared`/`Box` back-edges (the
    shipped cyclic-shape ownership fixtures) stay legal. Pinned by
    `tests/types/fixtures/err/recursion.tuo` (snapshot: one `T0016` per
    participating declaration) and `ok/recursion_wrappers.tuo`. The
    compiler-hang hole is closed: an accepted program can no longer recurse
    `layout_of` or the backends' glue emitters.
  - *`Float` elements:* one checker arm (`is_supported_array_element`) —
    both backends already load/store elements at their own scalar width, so
    `Array[Float]` runs natively with **no backend change**; pinned by the
    `array_float_elements` codegen fixture through the interpreter ==
    Cranelift == LLVM differential suites.
  - *`std::array::set`:* the new builtin lowers to `HeapMutOp::Set`
    (verified by `M0012`'s shape table; `IndexOutOfBounds` on the `get`
    bounds), implemented in the interpreter (`elements[i] = v`, the old
    element dropped by replacement) and both backends (bounds guard, then
    an in-place drop of the old element's owned buffers before the new
    value's store/memcpy); pinned by the `array_set_in_place` codegen
    fixture (scalar and owned-`String` elements) through all three engines.
  - *`std::json`:* the twelfth stdlib module — kinds, the arena accessors
    (`root`/`node_count`/`kind_of`/`num_of`/`text_of`/`key_of`/
    `first_child`/`next_sibling`/`child_count`/`member`), positioned-error
    `parse`, and canonical `render` — entirely executable tier, ten spec
    blocks green in the sandbox, and pinned **natively** on both backends
    by `crates/tuo-cli/tests/stdlib.rs`
    (`stdlib_json_parses_navigates_and_renders_natively`), which exercises
    the whole increment for real.
  - *Benchmark condition:* the lab's thirteenth workload **`json-parse`**
    landed `Support::Supported` (exit byte 54) with its C and Go peers,
    measured live by `crates/tuo-cli/tests/lab_command.rs`.
