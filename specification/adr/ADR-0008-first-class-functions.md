# ADR-0008: First-class functions

- **Status:** accepted (2026-08-18 — Tier 1; Tier 2 closures deferred; see Resolution)
- **Date:** 2026-08-04
- **Context:** Dogfooding v0 (see [`DOGFOODING.md`](../../DOGFOODING.md), findings
  in the *standard-library gaps* section, **D-8**) found that the stdlib cannot
  offer the single most useful data-processing abstraction — a generic
  `map`/`fold`/`filter` — for two reasons, and this ADR addresses one of them.
  `tuo-stdlib`'s `std::core` documents it directly: there are **no first-class
  function values and no closures**, so a generic `map` cannot be written and the
  library ships a specialized `map_add(opt, add)` instead. In the dogfooding
  examples this forced [`examples/data-pipeline`](../../examples/data-pipeline/)
  to write its filter+map+reduce as a bespoke recursion (`fold_one`) rather than
  passing a transform to a reusable fold.

  (The *other* reason a generic `map` is hard — no element type to be generic over
  in the runnable core — is ADR-0004's aggregate/collection work. This ADR is the
  function-value half; the two together are what make a real `map` possible.)

  A function value is a genuine language-design decision: it interacts with
  ownership (a closure captures environment — by `in`/`mut`/`take`?), with the
  ABI (how a function value is represented and called), and with the memory model
  (a capturing closure may need the heap). It is not something to add ad hoc to
  shorten one pipeline.

- **Decision (proposed):** add **first-class functions** in two tiers, specified
  before implementation:

  1. **Non-capturing function items as values.** A bare `fn` name becomes a value
     of a function type `fn(T) -> U`, passable as an argument and called
     indirectly. This needs no heap and no capture analysis — it is a code
     pointer with a known ABI — so it lands first and immediately enables generic
     higher-order stdlib functions (`fold(arr, init, step)` where `step` is a
     top-level `fn`).

  2. **Closures that capture environment.** A closure captures locals under the
     ownership vocabulary (`take` to move a capture, `in`/`mut` to borrow for the
     closure's lifetime — which, given the no-references model, means the closure
     may not outlive the borrow). Capturing closures may require a heap-allocated
     environment (the `Box`/allocator seam), so they are sequenced after
     ADR-0006's allocator work and carry their own ownership fixtures.

  The acceptance oracle is concrete: `std::core` should be able to add a real
  generic `fold`/`map` over ADR-0004's array type, and `data-pipeline`'s
  hand-written fold should be rewritable to call it with identical spec verdicts.

- **Consequences:**
  - *Easier:* the stdlib gains real higher-order combinators (one `fold`, not N
    specialized folds); application code stops re-implementing the same recursion;
    LLM generation improves because `map`/`fold`/`filter` — idioms a model reaches
    for constantly — become real APIs the compiler vouches for.
  - *Harder:* the ownership checker must model captures (tier 2), the ABI must
    define function-value representation and the indirect-call convention (and both
    backends must lower it identically, pinned by the differential suites), and the
    interpreter must represent a callable value.
  - *Trade-off:* shipping non-capturing function pointers first (tier 1) delivers
    most of the stdlib benefit with none of the capture/heap complexity; closures
    are a separate, heavier increment that need not block the combinators.

- **Benchmark consideration:** an indirect call is measurably more expensive than
  a direct one, and the performance laboratory already isolates the
  **`function-calls`** runtime workload — which today measures only *direct*
  calls. This ADR requires extending that workload (or adding a sibling) to
  measure **indirect** calls through a function value, against an equivalent C
  program calling through a function pointer, so the overhead is a recorded number
  rather than a guess. No claim about higher-order-function performance is
  admissible until that measurement exists; the ADR is not "accepted" until it is
  committed.

## Stage A (Tier 1) design (2026-08-14)

This section fixes the normative design for **Tier 1 — non-capturing function
values** and is written before the code, per the project rule. Tier 1 is
delivered in three stages: **Stage A** (this amendment: the spec text, the front
end, the MIR forms + verifier, and the reference interpreter); **Stage B**
(native indirect-call lowering in both backends, pinned by the differential
suites); **Stage C** (the `std::core` `fold`, the performance-lab indirect-call
workload, and — once the workload is committed — acceptance of the ADR). **Tier 2
(capturing closures) is explicitly out of scope** and deferred to a later ADR
increment; nothing here allocates, captures an environment, or introduces a
closure value.

### The function type

A function type is written `fn(P1, P2, …) -> R`, where each `Pi` is a parameter
**mode** followed by a type: `fn(take Int, in Str) -> Int`. **Modes are
mandatory** in the function-type syntax — there is no defaulting to `in`. This
is deliberate and load-bearing:

- The ownership vocabulary (`take`/`in`/`mut`) is part of a function's calling
  contract. A function *value* is a code pointer with a *known ABI*, and the
  per-argument borrow discipline at an indirect call site is driven by the
  callee's modes exactly as at a direct call. A function value that did not carry
  its modes could not be called soundly, so the modes belong to the type.
- It mirrors `fn` declarations, where modes are already mandatory on every
  parameter (Constitution §22). `fn add(take a: Int, take b: Int) -> Int` has the
  value type `fn(take Int, take Int) -> Int`.

The return arrow and type follow the parameter list; an omitted `-> R` is not
permitted in the *type* syntax (unlike a declaration, where an omitted return
type means unit) — a function type always spells its return, writing `-> ()`
for a unit-returning function.

**Type equality is exact, modes included.** Two function types are the same type
iff they have the same arity, pairwise-equal parameter types, pairwise-**equal
modes**, and equal return types. `fn(take Int) -> Int` is a *different* type from
`fn(in Int) -> Int`. This follows the existing exactness of tuonelang type
equality (`I32` never equals `I64`).

A function type is **`Copy`** (it is a single code pointer) and **non-heap**
(nothing is allocated to name a top-level function). It is not `Drop`.

### Producing a function value

A path that resolves to an **ordinary top-level user `fn`**, used in value
position (i.e. *not* immediately called), evaluates to a function value of that
function's signature-as-function-type. Given `fn add(take a: Int, take b: Int)
-> Int`, the expression `add` (in `let f = add;`, or as a call argument) has type
`fn(take Int, take Int) -> Int`.

Only ordinary top-level user functions are first-class in Tier 1. Using a
**builtin** (`std::rt`/`std::str`/`std::string`/`std::array`) as a value is a
clean error (`T0015`): builtins are lowered to dedicated MIR ops, not to a
callable code pointer, so there is no function value to take. A **generic**
function used as a value is likewise rejected in Tier 1 (`T0015`): a function
value is a single monomorphic code pointer, and monomorphizing a function value
is deferred. Method calls are already v0 no-ops (no trait system), and a **spec**
name is not a value at all (an ordinary "expected a value, found spec" error,
`T0012`) — no new rule is needed for either.

### Calling a function value

`f(args)` where `f` is an expression of function type (not a direct function
name) is an **indirect call**. The type checker checks arity, argument types, and
— through the ownership checker — the per-argument modes against the function
type. A direct call of a named function keeps its dedicated fast path (better
diagnostics, generics); an indirect call goes through the function-type detour.
Calling a value that is not of function type is `T0003` ("expected a function").

### MIR and purity

MIR gains a compile-time function-value constant and an indirect call form; see
[`mir.md`](../mir.md) §"Function values (ADR-0008 Tier 1)". A function value is
`Const::Fn(SymbolId)` — a compile-time-known code pointer naming a top-level
function. `Statement::Call`'s callee becomes `Callee::{Direct(SymbolId),
Indirect(Operand)}`; every existing argument/borrow/return mechanism is reused
unchanged — only the *target* differs.

**Purity rule (the spec-gate, R0007).** In Tier 1 the *only* way to obtain a
function value is a compile-time-known top-level `fn` name. The type checker
therefore records a call-graph edge for a **value reference** to a function
exactly as it does for a direct call — a reference *taints conservatively*. As a
consequence, any function that names an effectful function as a value (to pass it,
store it, or call it indirectly) is itself in the effectful closure, and a spec
whose execution could reach that value is refused by R0007 **before** any
interpreter runs. This makes the honest, sound rule precise:

- An indirect call through a **compile-time-known `Const::Fn(sym)`** has exactly
  the target's effectfulness (the target is named).
- An indirect call through a value that flowed through a variable or parameter is
  **conservatively assumed to be possibly effectful** — but because the
  conservative call-graph edge was *already* recorded where the value was
  produced, an opaque indirect call **cannot arise inside a spec-reachable pure
  function without that function already being tainted**. In Tier 1 there is no
  way to manufacture a function value except from a named top-level `fn`, so the
  conservative taint is complete: a spec can never reach an effect through a
  function value undetected. (Tier 2's captured environments will revisit this.)

### ABI

A function value is a single **code pointer** — pointer-width, `Copy`, no heap —
and the indirect-call convention is byte-identical to the direct one (same
argument/`sret`/borrow ABI; only the call target differs). This is specified in
[`abi.md`](../abi.md) §"Function values (Tier 1)"; `layout_of(Ty::Fn)` returns a
pointer layout, and the ABI version is bumped **4 → 5** for the newly-layoutable
type. Native lowering of the indirect call **lands with ADR-0008 Stage B**; until
then both backends refuse the new MIR forms cleanly.

## Stage C (Tier 1) design (2026-08-18)

Stage C delivers the *payoff* of Tier 1 — the reason the ADR exists — on top of
the Stage A/B machinery: the generic higher-order stdlib combinators, the
dogfooding oracle, and the performance-lab indirect-call measurement that the
"Benchmark consideration" above makes a hard acceptance condition. It introduces
**no new language mechanism**: it is all ordinary tuonelang written *against*
Tier 1's function values, plus one lab workload.

### The generic combinators

`std::collections` gains one generic left fold and the map/filter family, each
taking a **function value** and each with a worked example and an executable
`spec` (pure, so a spec builds the array it folds and passes a **named** module
function by value — the only way to make a function value in Tier 1):

- `fold(in xs: Array[Int], take init: Int, take step: fn(take Int, take Int) -> Int) -> Int`
- `map_into(in xs: Array[Int], take f: fn(take Int) -> Int) -> Array[Int]`
- `filter_into(in xs: Array[Int], take pred: fn(take Int) -> Bool) -> Array[Int]`
- `any(in xs, take pred: fn(take Int) -> Bool) -> Bool` / `all(...)`

The element modes on the step/predicate are `take Int`/`take Bool`: because those
scalars are `Copy`, moving by copy is the cheapest and most natural per-element
contract, and it is fixed in the function *type*. The combinators are
monomorphic over `Array[Int]` in v0 — element-type polymorphism is ADR-0004's
separate axis, not this ADR — so they generalize with no meaning change once the
type system gains element generics. This is exactly the "one `fold`, not N
specialized folds" the Consequences promised: `sum(xs)` is `fold(xs, 0, add)`.

### The oracle

`examples/data-pipeline` rewrites its hand-written subset fold to call the
generic `fold` over a named `add` function value, with **identical** spec
verdicts and the **same** exit byte (144). The example carries its own copy of
the generic fold rather than vendoring `std::collections`, because that catalog
module also defines the generic `Pair[A, B]` struct, which the native backend
cannot lower yet (monomorphization deferred), so the whole module cannot pass
through `tuo run`; the catalog `fold` is proven by its own module spec, and the
example dogfoods the identical fold in a program that runs natively.

### The benchmark condition

The performance lab gains a ninth runtime workload, **`indirect-calls`**, the
function-value sibling of `function-calls` (which measures only direct calls): a
hot loop invoking a `fn(take Int, take Int) -> Int` value, with an
equivalent-semantics C peer that calls through a function pointer, same
iteration count and same exit byte. It is `Support::Supported`, measured live on
both host seams — this is the committed measurement the "Benchmark
consideration" requires before acceptance.

## Resolution (2026-08-18 — Tier 1 accepted; Tier 2 deferred)

All three Tier 1 stages landed and every acceptance condition — including the
benchmark condition — is met by a committed, test-pinned artifact:

- *Stage A (spec text + front end + MIR + interpreter, commit `10cc6a9`):* the
  function type `fn(mode T, …) -> R` with mandatory param modes and exact,
  modes-included type equality; a bare top-level user `fn` name in value
  position becomes `Const::Fn(SymbolId)`, a `Copy` pointer-width code pointer
  (`layout_of(Ty::Fn)` is a pointer, ABI v5); `Statement::Call`'s callee becomes
  `Callee::{Direct, Indirect(Operand)}`, reusing the entire arg/`sret`/borrow
  machinery; a builtin or generic used as a value is `T0015`; purity stays sound
  because a value reference taints the call graph exactly like a direct call, so
  an effectful indirect call in a spec is refused by `R0007`. Pinned by the
  fn-value type/ownership fixtures, spec build-and-call tests (incl. the `R0007`
  gate and a copy-prop golden), and unit tests across resolve/types/verify/
  interp/runtime.
- *Stage B (native indirect-call lowering, both backends, commit `7df153c`):*
  `Const::Fn` materializes the named function's address as a pointer-width
  scalar and `Callee::Indirect` emits an indirect call (Cranelift
  `func_addr` + `call_indirect`; LLVM the function pointer +
  `build_indirect_call`), sharing signature construction and argument
  marshalling with the direct path so `sret` aggregate returns and `in`/`mut`
  borrow args flow through a function value identically. Pinned by the six
  `fnval_*` fixtures agreeing interpreter == Cranelift == LLVM
  (`crates/tuo-cli/tests/codegen_three_way.rs`,
  `crates/tuo-cli/tests/codegen_differential.rs`) and the randomized
  differential generator routing calls through a fn-value local.
- *Stage C (combinators, oracle, lab; this change):* `tuo-stdlib`'s
  `std::collections` gained the generic higher-order combinators —
  `fold`/`map_into`/`filter_into`/`any`/`all` over a first-class function value,
  plus the named transforms `add`/`mul`/`double`/`is_even` the specs pass by
  value — each with a worked example and an executable `spec`, pinned by
  `crates/tuo-cli/tests/stdlib.rs` (which runs every spec through the interpreter
  and enforces the three-tier rule: each is pure-executable and spec-exercised).
  This is the "one `fold`, not N specialized folds" the ADR promised.
- *Oracle:* `examples/data-pipeline` — the ADR's named oracle — now folds its
  materialised subset with the generic `fold(items, 0, add)`, passing a named
  `add` as a **function value** rather than a hand-written loop, with identical
  spec verdicts and the same documented exit byte (144). Kept honest by
  `crates/tuo-cli/tests/dogfood_examples.rs` (`check` accepts it, `test` runs its
  specs to `0 failed`, and `run` compile-link-runs it to exit 144 — a native
  indirect call).
- *Benchmark condition (met):* the performance lab's **`indirect-calls`**
  workload is `Support::Supported` with the committed
  `benchmarks/runtime/programs/tuo/indirect-calls.tuo` (a 2,000,000-iteration
  loop calling through a `fn` value, expected exit 128) and its
  equivalent-semantics C peer `benchmarks/runtime/programs/c/indirect-calls.c`
  (the same loop through a function pointer, same exit), measured live on both
  host seams by `crates/tuo-bench/tests/lab.rs` and
  `crates/tuo-cli/tests/lab_command.rs`. The supported count is now **eight of
  nine**; only **`networking`** stays unsupported (no socket-open effect —
  ADR-0007 or a successor), publishing a reason, never a number. The overhead of
  the indirect dispatch is a *recorded measurement*, not a claim — no superlative
  appears anywhere in the report (a test forbids it).
- *Deliberately deferred to a future ADR (Tier 2), each a whole increment, never
  a silent half-feature* — taken up by
  [ADR-0024](ADR-0024-capturing-closures.md), which decomposes Tier 2 into four
  separately decidable questions and records that the `Box`-seam premise below
  has expired (`Box`/`Shared`/`Weak` **values** are still refused):*
  **capturing closures** — a closure that captures locals
  under the ownership vocabulary (`take` to move a capture, `in`/`mut` to borrow
  for the closure's lifetime), which needs a **heap-allocated environment on the
  `Box`/allocator seam** (ADR-0009) and revisits the purity taint for opaque
  captured effects; and **non-`Int` monomorphization** of function values and of
  the combinators (a function value is a single monomorphic code pointer today;
  `map_into`/`fold` are `Int`/`Bool`-monomorphic — element-type generics are
  ADR-0004's axis). Nothing in Tier 1 allocates, captures an environment, or
  introduces a closure value.
