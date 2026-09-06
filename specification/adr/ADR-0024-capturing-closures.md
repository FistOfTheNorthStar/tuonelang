# ADR-0024: Capturing closures — the four decisions Tier 2 actually requires

- **Status:** proposed
- **Date:** 2026-09-06

## Context

ADR-0008 landed **Tier 1**: non-capturing first-class function values. A bare
top-level `fn` name is a `Copy` code pointer of type `fn(mode T, …) -> R`,
called indirectly with the identical direct-call ABI. That was enough for the
generic higher-order combinators (`fold`, `map_into`, `filter_into`, `any`,
`all`) and for the `indirect-calls` lab workload.

It explicitly deferred **Tier 2**:

> **capturing closures** — a closure that captures locals under the ownership
> vocabulary (`take` to move a capture, `in`/`mut` to borrow for the closure's
> lifetime), which needs a **heap-allocated environment on the `Box`/allocator
> seam** (ADR-0009) and revisits the purity taint for opaque captured effects

This ADR does not implement Tier 2. It records the decisions Tier 2 requires,
because the deferral as written names a *mechanism* ("heap environment on the
`Box` seam") without settling the questions that determine whether that
mechanism is even the right one — and because at least one of its stated
premises no longer holds.

### Why now

Capturing closures are the single largest source of refusals in
`tools/py2tuo`, the Python-subset transcoder. Comprehensions and `lambda` —
two of the most common constructs in ordinary Python — are refused *solely*
because a tuonelang function value may not capture. This is the first
dogfooding pressure on Tier 2 rather than an abstract completeness argument.

### A premise that has expired

ADR-0008 says Tier 2 needs "a heap-allocated environment on the `Box`/allocator
seam (ADR-0009)". ADR-0009 landed the allocator core, but **`Box`/`Shared`/
`Weak` *values* remain refused** — `Box::new(1)` does not parse today, and
`ty_owns_heap`'s doc comment still lists wrapped values as "stay refused".

So Tier 2 as written depends on a seam that does not exist. Either Tier 2 waits
on a `Box`-values ADR, or it allocates its environment directly on the
`tuo_rt_alloc` seam the way `String` and `Array` already do — without a
user-visible `Box`. **This ADR proposes the latter**, and says so explicitly so
the dependency is not silently inherited.

## Decision

Tier 2 is **not** one feature. It is four decisions, and they should be taken
in this order, because each constrains the next. None is taken here; this ADR
fixes the *shape* of the question and the constraints on each answer.

### D1 — Syntax: what a closure looks like

The grammar reserves nothing today: `fn_type` (grammar rule D) is documented as
"the type of a **non-capturing** function value", and no closure-expression
production exists. So Tier 2 adds syntax, and the language's own rule — *there
is deliberately ONE way to write each construct* — means the choice is not
cosmetic.

Constraints any answer must satisfy:

- It must be visually distinct from a nested `fn`, which is currently refused;
  a reader must be able to tell that a value is being constructed.
- Captures should be **explicit**, not inferred. Every other ownership decision
  in tuonelang is written down (`take`/`in`/`mut` on every parameter, no
  inference of signatures), and a closure whose captures are invisible would be
  the one place ownership is implicit. This is the strongest argument for an
  explicit capture list, and it cuts against the ergonomics of Rust's `|x| …`
  or Python's `lambda`.
- It must not collide with `|` — already doing double duty as bitwise-or and
  match-pattern alternation (ADR-0019). A Rust-style `|x|` closure would make
  `|` a third thing, disambiguated by context, in a language whose stated
  virtue is that there is one spelling per construct.

That last constraint is load-bearing and is the reason this decision must come
first: **the obvious borrowed syntax is already spent.**

### D2 — Ownership: what a capture means

ADR-0008 sketches "`take` to move a capture, `in`/`mut` to borrow for the
closure's lifetime". The hard part is the second half. tuonelang has **no
reference type and no lifetime vocabulary** (ADR-0003): a borrow exists for the
duration of a *call*, which is why parameter modes are call-scoped. A closure
that borrows a local outlives the expression that created it, so "borrow for the
closure's lifetime" introduces a lifetime notion the ownership model does not
currently have.

Three answers, in increasing cost:

1. **Move-only captures.** A closure may only `take`. No borrow, no lifetime
   question, no new ownership vocabulary. Ownership checking is unchanged: the
   capture is a move at the point of closure construction, which the existing
   checker already understands.
2. **Borrowed captures with a closure-scoped region.** Needs the ownership
   checker to reason about a borrow whose end is the closure's death rather
   than a call's return — genuinely new machinery.
3. **Full lifetimes.** Out of scope for v0 by ADR-0003.

**This ADR recommends starting at (1).** Move-only captures make Tier 2
implementable without touching the ownership model's core assumption, and they
cover the dogfooding demand: a Python comprehension captures by value in every
case a transcoder can translate. (2) remains available as a later increment and
should be its own ADR, not a hidden clause of this one.

### D3 — Representation: where the environment lives

A closure value is a pair `(code pointer, environment)`. Today a function value
is a single word (`layout_of(Ty::Fn)` is a pointer), and that is pinned
three-way (interpreter, Cranelift, LLVM).

- A closure is therefore **two words**, and `Ty::Fn` either widens for
  everything (making every non-capturing function value pay) or splits into two
  types (`fn(..)` and a distinct closure type), with a conversion where a
  non-capturing function is used as a closure.
- The environment allocates on the existing `tuo_rt_alloc`/`tuo_rt_dealloc`
  seam — **not** on a user-visible `Box`, per the expired premise above.
- The environment owns its captures, so it needs **drop glue**, and the closure
  value becomes non-`Copy`. This is the same machinery `ty_owns_heap` /
  `HeapGlue::DropInPlace` already provide for arrays and maps; `Ty::Fn` would
  join the heap-owning types.
- **ABI bumps.** `abi.md` gains the closure layout, and `EntryAbi` gains the
  indirect-call-with-environment convention.

The split-type option is worth a hard look precisely because Tier 1's promise —
"a function value is a `Copy` code pointer with the identical direct-call ABI" —
is a *guarantee already relied on* by `par_map` (ADR-0007), which requires
non-capturing function values so that no data race is expressible. Widening
`Ty::Fn` in place would quietly change what `par_map` accepts.

### D4 — Purity and concurrency: what a closure may capture

Two invariants currently rest on function values being opaque code pointers:

- **The spec sandbox is pure** (`R0007`). ADR-0008 already notes that an
  effectful function reached indirectly can escape the purity taint; a captured
  environment widens that surface, since the capture itself may carry an
  effectful value.
- **`par_map` is data-race-free by construction** (ADR-0007), *because* tasks
  are `Copy` and functions capture nothing. A capturing closure passed to
  `par_map` would break that guarantee directly.

The conservative answer, and the one this ADR recommends: **`par_map` continues
to accept only non-capturing function values**, enforced by the type system if
D3 splits the types (which is an argument for splitting). Concurrency does not
inherit closures for free.

## Benchmark plan

- A new **`closure-calls`** lab workload, peer to `indirect-calls`: a hot loop
  calling through a capturing closure, against a C peer using an explicit
  struct-plus-function-pointer pair. This measures the environment indirection's
  cost as a recorded number.
- `indirect-calls` must **not regress** — the gate that D3's representation
  choice did not tax non-capturing calls to pay for capturing ones.

## Consequences

*Easier:* the combinator surface stops needing a named top-level function per
use site. `py2tuo` can translate comprehensions and `lambda`, its two largest
refusal categories.

*Harder:* function values stop being uniformly `Copy` single words. Whichever
way D3 goes, something that is simple today gets more complicated: either every
function value widens, or the language grows a second function-like type with a
conversion between them.

*Risk, stated plainly:* this is the first v0 increment that would add a
**lifetime-shaped** obligation if D2 answers (2). The recommendation to start
move-only is specifically to avoid taking that on by accident, as a clause
inside a feature that looked like it was only about syntax.

*Unblocked by this ADR:* nothing, deliberately. It converts one deferred item
into four decidable ones, each of which can be taken, rejected, or staged on its
own evidence.
