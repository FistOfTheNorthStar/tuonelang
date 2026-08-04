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
