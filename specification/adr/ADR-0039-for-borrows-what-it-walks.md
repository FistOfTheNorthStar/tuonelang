# ADR-0039: `for` borrows what it walks — reading an array without an index loop

- **Status:** proposed
- **Date:** 2026-10-19

## Context

`ownership.md` §10 says *"`for` moves its iterable once, before the first
iteration"*. For a growable `Array[T]` that rule makes the most common loop in
any language an ownership error whenever the array is borrowed, and a
use-after-move whenever it is read again. The LLM-bridge roadmap lists this as
its first P0 language item, the one with no ADR. Every finding below was probed
with the binary built from `e38e070`.

### Finding 1: every `for` over a borrowed growable array is `O0003`

```tuo
fn sum(in xs: Array[Int]) -> Int {
    var t = 0;
    for x in xs { t = t + x; }
    t
}
```
```
error[O0003]: cannot move out of `in xs`
3 |     for x in xs { t = t + x; }
  |              ^^ ownership cannot leave a borrowed parameter
  = help: `for` moves the array it walks; to read a borrowed array, loop over
    its indices instead: `var i = 0; while i < std::array::len(xs) { let x =
    std::array::get(xs, i); ... i = i + 1; }`
```

The element type does not matter. `Array[Int]`, whose elements are `Copy`, is
refused exactly like `Array[String]`. The help text is accurate, and it is the
fix the stdlib uses. Even so, it replaces one line with five, and it is the
form every reader has to decode back into "for each element".

### Finding 2: on an owned local, the loop consumes the array

```tuo
var a = std::array::empty();
std::array::push(a, 2);
for x in a { t = t + x; }
t + std::array::len(a)    // error[O0001]: use of moved value `a`
```

So an owner cannot walk its own array and keep it. A second pass over the same
data, the shape of every "compute the mean, then the variance" program, needs
an index loop or a copy.

### Finding 3: fixed arrays already behave the way this ADR proposes

`for x in xs` over `in xs: [Int; 3]` checks, because a fixed array of `Copy`
elements is itself `Copy`, so "moving" it copies 24 bytes. The two array kinds
therefore disagree on the same loop. That gap is the one ADR-0012 closed for
element types and ADR-0030 closed for indexed writes.

### Finding 4: the code base writes the index loop, not the `for`

A regex count of `while <i> < std::(array|string|str)::len(` against
`for <x> in <name> {`, both in committed `.tuo` sources:

| Location | Index loops over `len` | `for` over a named array |
|---|---|---|
| `crates/tuo-stdlib/src/std/` | 55 | 0 |
| `examples/` | 40 | 4 |
| `benchmarks/runtime/programs/tuo/` | 7 | 1 |
| `benchmarks/llm/codegen/sources/` (reference solutions) | 7 | 9 |
| `tests/` | 4 | 14 |

That is 113 index loops against 28 `for` loops, and most of those 28 walk a fixed
array or a local the loop is allowed to consume. The library, written by people
who know the rule, avoids `for` entirely.

### Finding 5: the benchmark sees it, but rarely

On the 53-task core set (`benchmarks/llm/codegen/results/core/`), the
"`for` moves the array it walks" help reached a model's repair prompt 0–2
times per run (5 in total, in three of the seven runs). Every hit was in the
`range-sum` task. The task set has few functions that take an `in Array[T]`
and walk it, so this frequency measures the task set as much as the models.
It is evidence that the mistake is made when the shape occurs, not evidence of
how often the shape occurs. The case rests on Findings 1–4. The benchmark plan
adds the tasks that would measure it.

### Why the loop never needed ownership

The lowering already reads each element by copy. `for_array`
(`tuo-mir/src/lower.rs:2527`) takes the iterable's place directly when the
operand is `Operand::Copy(place)`. It reads a `Copy` element through
`Projection::Index`, and a non-`Copy` element through `HeapOp::ArrayGet`, the
same deep copy `std::array::get` lowers to. Nothing in the loop moves an
element out of the array, and nothing frees it early. The move exists only in
the ownership checker (`for_expr`, `tuo-ownership/src/check.rs:1725`, which
evaluates the iterable as `Use::Value`) and in the spec sentence quoted above.

There is one reason the move is load-bearing today, and it decides the shape of
the rule below. `for_array` caches `len` before the first iteration and indexes
without a per-iteration bounds check. The backends lower `Projection::Index` as
unchecked address arithmetic because MIR asserts before every other use
(`tuo-codegen-cranelift/src/lower.rs:48`). Because the loop owns the array,
nothing can shrink it mid-walk. A loop that only borrows must keep that
guarantee some other way.

## Options

- **(a) Status quo, better help.** This is what landed on 2026-10-17. The help
  text is right, but every first attempt still fails, and the index loop stays
  the idiom.
- **(b) `for` borrows a place; the body may not write it.** The iterable, when
  it is a place, is read for the duration of the loop rather than moved, and
  any write to an overlapping place inside the body is an error. The element
  binding keeps today's semantics.
- **(c) A separate borrowing form**, such as `for x in in xs`. Writing the
  mode again would match the call-site discipline, but a mode at a call site
  is a targeted parse error today (`24b9fe8`), and two loop spellings for one
  task break the one-spelling rule.
- **(d) Re-check bounds every iteration** and allow writes in the body. This
  is sound and costs one compare per element. But a loop over a collection
  that the body pushes onto or pops from is a bug in every language that
  allows it, and tuonelang would be choosing to define it.

## Decision (proposed)

**Option (b).**

### D1: a place iterable is read, not moved

When the iterable of `for` is a **place** (a local, a parameter of any mode, a
lent binding, or a field path rooted in one), the loop **reads** it, exactly as
an `in` argument reads its place. It is not moved, it stays initialized after
the loop, and an `in` parameter may be walked. When the iterable is not a place
(a call result, a literal, a range expression), the loop owns the temporary,
and the temporary is dropped when the loop exits, as today.

This applies to every iterable kind: `Array[T]`, `[T; N]`, and ranges. For
the two kinds that are `Copy` today (a `Copy`-element `[T; N]`, and a range)
nothing observable changes. For a non-`Copy` `[T; N]`, which is not lowered
yet, the rule is set now so that it does not need a second ADR.

### D2: the body may not write the iterable

For the duration of the body, the iterable's place is **frozen**. Assigning
to it, passing it as `mut`, moving it, or doing any of these to a place that
overlaps it is a new error, **O0013 `cannot modify `xs` while `for` walks
it`**, anchored at the write, with a secondary label at the loop head. The
help names both fixes: walk a copy, or walk the indices when the body really
must change the array. Writes to anything else, including another element of
an outer structure, are unaffected.

This is what keeps the cached `len` and the unchecked index sound (see "Why the
loop never needed ownership"). It is also the rule a reader already assumes. An
`in` parameter cannot be written anyway, so D2 only ever fires on an owned
local or a `mut` parameter.

### D3: the element binding is unchanged

A `Copy` element is copied, as today. A non-`Copy` element is the
`HeapOp::ArrayGet` deep copy, as today, so the binding is owned and the body
may move it (`std::array::push(out, s)` keeps working). Lending the element
instead, which ADR-0037 does for `match`, would save one allocation per
iteration. But it changes what the body may do with the binding, so it is a
separately measured follow-up (Stage C), not part of the rule.

### What this costs the checker

`for_expr` evaluates a place iterable as a read (`Use::Read`) instead of
`Use::Value`, and pushes the place onto a frozen-place stack that the
assignment, `mut`-argument, and move paths consult. The MIR change is limited
to the place case already present in `for_array`: the iterable is not stored
into a fresh temporary, and it is not dropped at loop exit. No MIR construct,
verifier rule, backend, runtime function, or ABI layout changes.

## Staging

**Stage A: the rule.** Implement D1 and D2 in the ownership checker and the
matching place path in `for_array`. Rewrite §10's sentence in `ownership.md`
and add an `O0013` row with a MUST FAIL example. Retire the
`for`-specific O0003 help, since the case no longer arises, and keep the
generic one. Change the brief to show `for x in xs` over an `in` array,
and replace its trap entry with D2. New fixtures:
`tests/ownership/fixtures/ok/for_borrows.tuo` (an `in` array walked twice, an owned local
walked and then reused, a field path, a `mut` parameter walked without a
write) and `tests/ownership/fixtures/err/for_frozen.tuo` (`push`, `set`, assignment,
and a move of the iterable inside the body, each `O0013`, plus a nested loop
writing the outer iterable). Add three-way codegen fixtures for an `in` walk of
`Array[Int]` and of `Array[String]`, and for the reuse-after-loop case. Gate:
every committed `.tuo` keeps its verdict and exit byte, and the differential
suites stay green.

**Stage B: the payoff.** Rewrite the stdlib's 55 index loops that only read
into `for` (an audit separates those that need the index, such as
`index_of`, from those that don't). Do the same for the examples. This is the
oracle. Every spec must stay green and every example must keep its exit byte,
so a behaviour change is caught by the existing tests rather than by review.

**Stage C (optional, measured): lent elements.** A non-`Copy` element of a
place iterable becomes a lent binding (`Origin::LentBinding`, ADR-0037), with
no copy. A move of it is `O0003`, with help that names `std::array::get` as
the way to keep one. Land it only if the benchmark below shows the copy
matters, and only after counting how many committed loops move their element.

## Benchmark plan

The predictions below are fixed before implementation.

1. **LLM Check@1.** Add pinned core tasks that take an `in Array[T]` and walk
   it (sum, mean-then-variance two-pass, the count of `String`s matching a
   prefix, max by a struct field), each with `for`/`index-loop` syntax
   variants. Record primed and unprimed runs on qwen3:4b and gemma4 before
   Stage A. Then **rescore the same outputs** with the Stage A compiler through
   `tuo bench report`, so any delta comes from the rule alone, and take one
   fresh primed run with the updated brief. Metrics: Check@1, Repair@1, and the
   count of first-turn `O0003`/`O0001` on a loop iterable. Prediction: the
   `for`-variant outputs that fail today on `O0003` alone all pass on rescore.
   Acceptance: no task gets worse.
2. **Codegen identity.** `tuo debug mir --opt` on a `take`-parameter `for`
   loop must be identical before and after. For an `in`-parameter loop, the
   MIR must equal what the index loop produced before, apart from the bounds
   assert that the cached `len` makes unnecessary.
3. **Runtime: prediction is no change, or faster.** Time the Stage B stdlib
   rewrite through the lab's `json-parse` and `sha256-hash` workloads and the
   Benchmarks Game table (fastest of five, interleaved). An index loop pays
   one bounds compare per `get`, and the `for` loop does not. Any difference is
   recorded either way.
4. **Stage C only:** one allocation per iteration, measured as wall clock on
   a 10⁶-element `Array[String]` walk, with the copy and with the lent binding.

## Consequences

- `for x in xs` reads a borrowed array, and the five-line index loop stops
  being the idiom for "for each element".
- An owner can walk its own array more than once.
- A new error, O0013, replaces an O0003 that had no good fix. It fires only on
  code that mutates the collection it walks, which is a bug in every language
  that allows it.
- `ownership.md` §10 changes, so this is a language change, not a diagnostic
  one. Almost every program that checked before still checks, since a moved
  iterable that nothing reused is equally valid as a read one. The exception
  is a body that **reassigns** the iterable (`xs = std::array::empty()`).
  That is legal today, because the loop has already moved `xs`, and it becomes
  O0013. The Stage A gate counts such loops in committed sources. The
  prediction is zero.
- The brief loses a trap entry. That is a direct reduction in what a model
  must hold in mind to write correct code.

## What this ADR does not propose

- **An iterator protocol** or user-defined iterables. `for` still walks
  ranges and arrays only. Iterating a `Map` (`std::map::keys` returns an
  array) and a `String` (bytes through `byte_at`) is unchanged.
- **Mutation through the loop variable** (`for x in mut xs`). Writing
  elements in place stays an indexed write (ADR-0030).
- **An index together with the element** (`enumerate`). That is a library
  question once generics land (ADR-0027).
- **Lending elements** in Stage A. See D3.
- **Any change to `par_map`**, which keeps its own crossing rules (ADR-0034).
