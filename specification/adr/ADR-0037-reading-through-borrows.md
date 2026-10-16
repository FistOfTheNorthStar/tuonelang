# ADR-0037: Reading through borrows — lent pattern bindings, and a boxed pointee lent without a copy

- **Status:** accepted (2026-10-16 — landed with this ADR; see Resolution)
- **Date:** 2026-10-16

## Context

ADR-0028 Stage B gave `Box[T]` values a constructor and a native lowering, and
its Resolution recorded the gap it could not close: **a borrowed structure
could not be read.** The cause was not `Box`; it was the pattern rule.

A `match` binding *moves* its payload (ADR-0003, `ownership.md` §10). A match
whose scrutinee sits behind an `in` parameter therefore had nowhere legal to
move to, and was refused:

```tuo
fn first_len(in o: Option[String]) -> Int {
    match o {
        Some { value } => std::string::len(value), // O0003 before this ADR
        None => 0,
    }
}
```

```
error[O0003]: cannot move `o` out of the borrowed parameter `in o`
```

So nothing could be read out of an `in` value's enum payload unless it was
`Copy` — not an `Option[String]`, not a `Result[Record, Str]`, not the
children of a tree node. Every recursive structure ADR-0028 made expressible
was walked by **consuming** it (`take node: Node`, then `std::boxed::into`),
which is what `binary-trees` does and is the wrong shape for anything read
more than once. The only alternative, `std::boxed::get`, copies — deeply, for
a heap-owning pointee — so a "borrowed" walk of a tree copied every subtree at
every level.

The two halves are separable, and both are needed for a borrowed traversal:

1. reaching *into* an `in` value's payload, and
2. reaching *through* a box to its pointee without copying it.

## Decision

Both are delivered with **no new syntax** — no reference type, no `ref`
pattern, no dereference operator — because each is a change to what an
existing spelling *means*, chosen so that it cannot change the result of any
program that was already accepted.

### D1 — A match on an `in`-rooted place lends its non-`Copy` parts

When a `match` scrutinee is a place reached through an `in` parameter — the
parameter itself, a field of it, or a binding already lent from one — each
non-`Copy` binding in an arm is a **lent binding**: a read-only alias of the
matched part, not a move and not a copy.

- It may be **read**, **passed to an `in` parameter**, **matched again** (which
  lends in turn), and have its `Copy` fields read.
- It may never be **moved**: passing it `take`, returning it, storing it,
  boxing it is `O0003`, now reported *at the move* with its own message
  ("cannot move `value`: it is lent from a borrowed value") rather than at the
  binding.
- It is never dropped — it owns nothing.
- `Copy` bindings are copies, exactly as before.

Only `in` lends. An `in` root cannot change for the duration of the call
(nothing may write through it, and v0 has no other path to the caller's place —
the same facts the LLVM backend's `noalias` marking rests on), so an alias can
never observe a mutation. A `mut` parameter is **not** lent: an arm could
assign to it under the alias. Moving out of a `mut` parameter stays `O0003`,
as before.

This only ever *accepts* programs that were refused. A match that bound a
non-`Copy` part of an `in` value was an error before; it is now legal and reads
the value in place. No accepted program's meaning changes, and the one
reported diagnostic that moves is in the fixture
`tests/ownership/fixtures/err/move_out_of_borrow.tuo`, where the error now
points at the `return` that moves the binding instead of at the binding.

### D2 — `std::boxed::get(b)` passed to an `in` parameter lends the pointee

`get` still means "a copy of the pointee". When its result is passed straight
to an `in` parameter, the callee can only read it for the duration of the call,
so a copy and the pointee itself are observationally identical — tuonelang has
no destructors (Q-0011) and no address comparison. The compiler therefore
**elides the copy** and lends the pointee. This is an optimization of an
existing spelling, not a new semantic, and it is what makes a borrowed walk
O(n):

```tuo
fn weight(in node: Node) -> Int {
    let left = match node.left {
        Some { value } => weight(std::boxed::get(value)), // lends `*value`
        None => 0,
    };
    // …
}
```

The elision applies only when no other argument of the same call could drop or
replace the box during it: every other argument must be an `in` borrow or a
`Copy` value. With a `mut` or a non-`Copy` `take` argument alongside — say
`f(std::boxed::get(b), mut b)` — the copy is kept, because the callee could
reassign `b` and free the very pointee it was lent.

### Representation

- **MIR** gains one projection, `Projection::Deref` — the pointee of a
  `Box[T]` place (`mir.md`). Lowering emits it only for D2's lending, never to
  move out of or write through. A lent binding (D1) needs nothing new: it is
  lowered as the place it aliases (`_0.1.v0.0`), with no local of its own.
- **The interpreter** reads `Deref` as the box's single field (its model of a
  box since ADR-0028).
- **Both backends** walk `Deref` by loading the box's pointer; a box held in a
  scalar local starts from that pointer directly.

## Benchmark plan

The claim is that a borrowed walk costs a walk, not a copy. The pinned
fixture `tests/codegen/fixtures/lend_bindings.tuo` walks one tree twice by
borrow and checks the result three-way. The measurement is the borrowed
`count` of a depth-12 tree of `String`-labelled nodes (8,191 nodes), repeated
2,000 times: **0.43 s** release on an Apple M4, about 38 million node visits a
second, with the tree allocated once — where the same walk through `get`
copies would have reallocated each subtree at every level. `leaks` reports
zero leaks. No peer comparison is claimed here; `binary-trees` (ADR-0028)
remains the cross-language allocator benchmark, and it consumes by design.

## Consequences

- ADR-0028's open "borrowing accessor" is resolved without one: a boxed tree
  is read through `std::boxed::get` in `in` position, with no copy.
- The ownership checker's `Origin` gains `LentBinding`, which behaves exactly
  like an `in` parameter for every existing rule (reads, `in` arguments,
  `O0003` on a move), so no rule needed a special case beyond its message.
- The cheat sheet's "a match binding MOVES its payload, so walk a boxed
  structure by taking it" is replaced by the lending rule.
- Lent bindings make aliasing observable to the backends for the first time
  inside one function: two lent bindings can name overlapping places. Both are
  read-only and the root is `noalias` *against the caller*, which is all LLVM
  is told, so no optimization assumption changes.

## What this ADR does not propose

- A reference type, `ref`/`ref mut` patterns, or a dereference operator.
- Lending from a `mut` parameter, or from an owned local (a match on an owned
  local still moves, as before — the owner is right there).
- Lending in a `match` scrutinee written as `std::boxed::get(b)` — that copies.
  Lending there would need the ownership checker to see the scrutinee as a
  place, which it does not, and agreeing on that rule is a separate change.
- Mutation through a lent binding.

## Resolution (2026-10-16)

Landed as written.

- **Ownership** (`tuo-ownership/src/check.rs`): `Origin::LentBinding`; a match
  whose scrutinee place is rooted at an `InParam` or a `LentBinding` reads the
  scrutinee and registers its non-`Copy` bindings as lent.
- **MIR** (`tuo-mir/src/lower.rs`): the same predicate (scrutinee place rooted
  at an `in` parameter local); lent bindings map to their place; D2 in
  `lower_call_args`, with the other-arguments guard. `Projection::Deref` in
  `mir.rs`, the verifier, the printer (`.*`), the interpreter, and both
  backends.
- **Pinned** three-way by `tests/codegen/fixtures/lend_bindings.tuo` (lent
  bindings, nested lending, a box pointee lent, and the `mut` case that keeps
  its copy), and by the ownership fixtures `ok/lending.tuo` and
  `err/lending.tuo` (a lent binding moved into `take`, returned, or boxed; a
  `mut` scrutinee still refused).
