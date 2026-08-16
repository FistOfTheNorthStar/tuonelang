# ADR-0008: First-class functions

- **Status:** proposed
- **Date:** 2026-08-04
- **Context:** Dogfooding v0 (see [`DOGFOODING.md`](../../DOGFOODING.md), findings
  in the *standard-library gaps* section, **D-8**) found that the stdlib cannot
  offer the single most useful data-processing abstraction — a generic
  `map`/`fold`/`filter` — for two reasons, and this ADR addresses one of them.
  `tdg-stdlib`'s `std::core` documents it directly: there are **no first-class
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
