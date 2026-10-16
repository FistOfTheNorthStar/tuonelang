# ADR-0033: Function-valued array elements — admitting `fn` values to the growable `Array`

- **Status:** proposed
- **Date:** 2026-10-14

## Context

`examples/router/src/main.tuo` is a dispatch table, and its header (lines
23–56) records why it is split in two. The shape every web framework uses is
one table of route records:

```tuo
struct Route { path: String, handler: fn(in Request) -> Response }
```

held in a growable `Array[Route]`. v0 refuses that, so the router keeps a
`Map[Str, Int]` from path to *slot* and a fixed `[fn(in Request) -> Response; 4]`
of handlers (`handler_table`), and the two halves are joined by integers written
by hand in `routes()`. Nothing in the type system relates them; the specs pin
the correspondence instead. The header calls itself "the evidence a future ADR
on function-valued `Array` elements would be argued from". This is that ADR.

### What the compiler does today

Probed with the current `tuo` binary, on every engine (`tuo spec` for the
interpreter, `tuo run` for Cranelift, `tuo run --release` for LLVM):

| Program | check | interp | Cranelift | LLVM |
|---|---|---|---|---|
| `[fn(take Int) -> Int; 3]` literal, indexed, called, `for`-iterated | ok | ok | 40 | 40 |
| `[inc; 4]` repeat literal; `t[i] = dbl` indexed write (ADR-0030) | ok | ok | 42 | 42 |
| struct field `handler: fn(take Int) -> Int`, read and called | ok | ok | 42 | 42 |
| `[Route; 2]` where `Route` has a `fn` field | ok | ok | 32 | 32 |
| `Option[fn(…)]` payload; enum variant field of `fn` type | ok | ok | 42 | 42 |
| struct whose `fn` field takes the struct itself (`fn(in Node) -> Int`) | ok | ok | 42 | 42 |
| `std::array::push(hs, inc)`; `std::array::filled(3, inc)` | **T0001** | — | — | — |
| `std::array::push(routes, Route { … })` | **T0001** | — | — | — |
| `std::map::insert(m, "/", inc)` | **T0001** | — | — | — |

So a function value is already a first-class *field* and a first-class *fixed
array element* on all three engines. The only refusals are the growable
`Array` and the `Map`, and both are the checker's:

```
error[T0001]: `Array[Route]` is not supported in v0
 --> growable_struct.tuo:16:5
   = help: nested owned containers and wrapped values are outside the v0 element set (ADR-0012)
```

The refusal comes from `is_supported_array_element` in
`crates/tuo-types/src/check.rs`, whose catch-all arm lists `Fn` among the types
"out of the v0 element set". A struct with a `fn` field is refused only because
`aggregate_fields_supported` recurses into that field. The map refusal is
`is_supported_map_value` in the same file.

Two further findings:

1. **`Array[fn(…)]` is already an inhabited type, with exactly one value.**
   The guard runs on the builtins that store elements (`push`, `pop`, `get`,
   `set`, `filled`), not on the type, as `specification/abi.md` intends ("the
   *type* `Array[T]` admits any `T`"). A parameter or `let` annotated
   `Array[fn(take Int) -> Int]`, filled from `std::array::empty()`, passes
   `check`. Indexing it (`hs[i]`) and writing it (`hs[i] = inc`, ADR-0030)
   pass too, compile on **both** backends, and trap `IndexOutOfBounds` on all
   three engines (exit 134), as they should on an empty array. The MIR is the
   ordinary `len` → bounds `assert` → `copy _0[_2]` → `call copy _5(…)`. So the
   native read path for a function-valued growable element is already emitted
   and linked today. Only a non-empty array has never been built.
2. **Nothing pins the parts that work.** No fixture under `tests/codegen/fixtures/`
   puts a function value in an array, a struct field, or a payload. The six
   `fnval_*` fixtures cover locals, parameters, and returns. The only
   regression guard for `[fn; N]` is the router's dogfood test
   (`crates/tuo-cli/tests/dogfood_examples.rs`).

### Why the backends should need no new code

ADR-0008 Tier 1 made `Ty::Fn` a single pointer-width `Copy` code pointer:
`layout_of(Ty::Fn)` is `Layout::pointer()` (`crates/tuo-runtime/src/abi.rs`,
ABI v5), and both backends' `scalar_type` (`crates/tuo-codegen-*/src/abi.rs`)
map it to a register type (`I64` in Cranelift, the opaque `ptr` in LLVM).
Every growable-array operation in both `lower.rs` files dispatches on
`scalar_type(element)`: push and set are a single store, `get` is a single
load (`lower_heap_op_scalar`), and `filled` and `pop` work by stride and
layout. `ty_owns_heap` is false for `Ty::Fn`, so no deep-copy or drop glue
applies. `require_native_array_element` refuses only wrapper-containing
elements, and `ty_contains_wrapper` does not descend into a function's
signature. The interpreter stores `Value::Fn(SymbolId)` in the same
`Value::Array(Vec<Value>)` as every other element.

This is a reading of the code, not a measurement. The only measured part is
finding 1, which exercises the read path on an empty array. Stage A below
starts by measuring the rest, as ADR-0023 Stage B did. That stage found three
of its own premises wrong.

## Decision

**Admit non-capturing function values (`Ty::Fn`) to the growable-`Array`
element set,** and with them every struct or enum whose fields are otherwise
supported. That includes the router's `Route { path: String, handler: fn(…) }`.

- **Checker:** one arm in `is_supported_array_element`,
  `Ty::Fn(_) => true`, treated as a **leaf**. The rule does not recurse into the
  parameter or return types, because a function value's representation does not
  depend on them. It is one word whatever it points to, and the self-referential
  `fn(in Node) -> Int` field above already lays out and runs. The
  `reject_unsupported_array_element` label and help text gain "a function value"
  in the list of supported elements.
- **Ownership:** no change. `Ty::Fn` is already `Copy`, which the ADR-0030
  indexed write (`O0012`) and the repeat literal / `filled` (`O0010`) require,
  and which fixed arrays already satisfy (probed above).
- **MIR and the verifier:** no change. Array element operations are already
  typed by the element. Indirect calls on the loaded value are ADR-0008's
  `Callee::Indirect`.
- **Purity (`R0007`):** no change, and the reason is ADR-0008's own. A
  function value can still only be made by naming a top-level `fn`, and naming
  one records a call-graph edge. Probed: a spec reaching
  `pick` is refused with `R0007` when `pick`'s table
  `[quiet, loud]` contains an effectful `loud`, even though the spec only
  selects `quiet`. Storing the value in a growable array changes where it
  lives, not where it was named.
- **ABI:** no layout changes and **no version bump** (stays 13). The array
  header is generic in `T`, and the element stride is `layout_of(Ty::Fn)`, which
  has existed since v5. This is the reasoning ADR-0023 Stage B recorded for
  word-sized map values. `specification/abi.md`'s element-set paragraph gains
  the function value as documentation, not as a layout.
- **`Map[K, fn(…)]` values are a separate, later stage (C).** They are in
  scope because the router's ideal shape is a `Map[Str, fn(in Request) ->
  Response]` with no slot integer at all. They are not bundled with the array
  change because the map path crosses a C shim as a widened machine word.
  Cranelift's `value_as_word` already passes an `I64` through. LLVM's passes a
  `ptr` through unconverted, and `write_option_scalar_dest` stores the loaded
  `i64` word back into a `ptr`-typed payload. Stage C makes that an explicit
  `ptrtoint`/`inttoptr` pair rather than relying on punning through memory.

### Interaction with ADR-0024 (capturing closures)

This decision assumes a function value is a one-word `Copy` code pointer. That
already holds for the fixed arrays, fields, and payloads in the table above.
ADR-0024's D3 keeps two options open:

- **A distinct closure type** (its recommended direction): this ADR is
  unaffected. Admitting closures as elements would be a later decision. A
  closure owns its environment, so it would enter as a heap-owning element
  through the ADR-0012 deep-copy and drop machinery, like `String`.
- **Widening `Ty::Fn` in place** to two words and non-`Copy`: this would break
  programs that work today. A non-`Copy` element cannot be moved out by
  `xs[i]`, and `[f; N]`, `filled`, and `xs[i] = f` all require `Copy`. That
  breakage exists now through fixed arrays and the router. This ADR makes it
  larger, so it adds weight to ADR-0024's split-type option, and ADR-0024
  should cite it.

## Staging

**Stage A — measure, then pin what already works.** Temporarily relax the arm
and run all three engines over `push`/`get`/`set`/`pop`/`filled` with `fn`
elements and with an `Array[Route]` carrying a `String` beside the `fn`. Record
any premise above that fails, in this ADR. Whatever happens, add three-way
fixtures for what runs today: a fixed `[fn; N]` table, a `fn` struct field, a
`fn` enum/`Option` payload, and a `[Route; N]`. This half is independently
landable.

**Stage B — the widening.** The checker arm. New three-way fixtures under
`tests/codegen/fixtures/` (`fnval_array_push_get`, `fnval_array_set_pop`,
`fnval_array_filled`, `fnval_array_of_struct`, plus an out-of-bounds trap),
pinned by `crates/tuo-cli/tests/codegen_three_way.rs`. Ok/err cases in
`tests/types/fixtures/{ok,err}/array_elements.tuo` and their snapshot. The
randomized generator in `crates/tuo-cli/tests/differential.rs` learns to route
a call through an array element. Doc updates go to `abi.md`,
`static-semantics.md`, and the brief's runnable-core line
(`crates/tuo-cli/src/cheatsheet.rs`, pinned by `cheatsheet_command.rs`).

**Stage C — `Map[K, fn(…)]` values.** `is_supported_map_value` gains `Ty::Fn`.
LLVM gets the explicit pointer conversions. Fixtures follow the ADR-0023
Stage B pattern.

## Benchmark plan

Measured first, in the probe for this ADR (Apple M4, fastest of 9 runs after a
warm-up, 20,000,000 dispatches to eight dissimilar handlers chosen by an LCG,
every binary exiting 225):

| Dispatch | C `-O2` | tuo `--release` | tuo debug |
|---|---|---|---|
| table of function pointers (tuo: fixed `[fn; 8]`) | 113.9 ms | 109.7–116.5 ms | 152.9 ms |
| `switch` / `match` chain of direct calls | 59.7 ms | 123.8 ms | 127.7 ms |

Three findings come out of that table and shape the plan:

- **Table dispatch is already at C** in tuonelang today, with a fixed array.
  The question for this ADR is whether a growable `Array` keeps that.
- **tuonelang's `match` is 2.1x C's `switch`** on the same handlers.
  `tuo debug mir --opt` shows an `Int` match lowered as a chain of single-arm
  `switch` terminators. The cause is not investigated here; it is recorded for
  its own follow-up.
- **The handlers must be dissimilar.** In a first variant the handlers
  differed only by a constant, and clang folded the `switch` into arithmetic
  (24.4 ms against 113.5 ms for the table). That compared the optimizer
  against itself, not two ways of dispatching.

The existing **`indirect-calls`** workload cannot guard this change. With the
lab's own C flags (`-O2 -ffp-contract=off`, `C_FLAGS` in
`crates/tuo-cli/tests/lab_command.rs`), its C peer's `main` compiles to
`mov w0, #0x8480; movk w0, #0x1e, lsl #16; ret`, which is `return 2000000`.
`tuo build --release` produces the same three instructions. The single,
compile-time-known target is devirtualized and the loop folded away. The lab
measures the tuonelang side through `tuo run` (unoptimized Cranelift), so its
committed comparison sets an unoptimized indirect-call loop against a C
constant. This ADR records that and does not change the workload. Changing a
committed workload is a decision of its own.

The plan, fixed before implementation:

1. **New lab workload `dispatch-table`**: the program above, with its table as
   a growable `Array[fn(take Int) -> Int]` built by `push`. Peers:
   `benchmarks/runtime/programs/c/dispatch-table.c` (a `static` array of
   function pointers) and `go/dispatch-table.go` (a `[]func(int64) int64`).
   Registered in `crates/tuo-bench/src/lab/runtime.rs` and
   `crates/tuo-bench/src/lab/compare.rs`. It is unwritable before Stage B,
   which is the catalog's documented re-entry mechanism.
2. **In-language comparison**: the same workload with a fixed `[fn; 8]` and with
   a `match` chain, built `--release`. **Acceptance condition:** the growable
   table is within noise of the fixed one (expected, since only the base address
   moves from stack to heap). If it is not, the cause is investigated before
   acceptance.
3. **The router oracle**: `examples/router` rewritten to an `Array[Route]`, its
   `register(routes, path, handler)` pushing the handler and recording its
   index itself, so no slot is written by hand. Every spec stays green. `main`
   still exits 74, the dogfood wire tests are byte-identical, and both shapes'
   dispatch cost over the six sample requests is reported with no verdict.
   Stage C then removes the slot integer altogether.
4. **No regression**: the full lab and the Benchmarks Game speed table, before
   and after.

## Consequences

- The router's unchecked correspondence becomes a type: a handler and its path
  are registered by one call into one record. A program can accept a handler
  set whose size is not fixed in its source.
- The change is small for the reason ADR-0008 gave: a function value is a
  word. If Stage A confirms the reading, the whole language-side diff is one
  match arm and a diagnostic string. Most of the work is fixtures, the
  generator, and the benchmark.
- What works today becomes pinned three-way rather than covered only by one
  example's dogfood test.

## What this ADR does not propose

Capturing closures (ADR-0024), function values made from builtins or
generic functions (still `T0015`), equality on function values, and `Option`,
`Result`, nested arrays, or tuples as `Array` elements (still refused, as
`tests/types/snapshots/array_elements.snap` pins). It also does not propose a
fix for the `match` lowering or the `indirect-calls` workload, both recorded
above, or any change to `par_map`, which already takes a function value
directly.
