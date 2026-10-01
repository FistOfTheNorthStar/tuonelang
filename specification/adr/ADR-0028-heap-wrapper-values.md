# ADR-0028: Heap-wrapper values — giving `Box[T]` a constructor, and recursive types a runtime

- **Status:** proposed
- **Date:** 2026-09-28

## Context

Six places in this repository defer to "a later ADR" for the
`Box`/`Shared`/`Weak` heap wrappers — both backends' refusal messages, the
`T0022` advisory, the type checker's recursion boundary, `static-semantics.md`,
and `REFERENCE.md` — and no ADR in this directory is that ADR. This one is.

It was opened by probing the real binary, and the probe found the gap is wider
than the documentation says. The documented position is that heap-wrapper
**values** type-check, execute on the reference interpreter, and are refused
only by the native backends. Two of those three claims do not survive contact
with the compiler.

### Finding 1 — a wrapper value cannot be constructed at all

`Box`, `Shared`, and `Weak` are keywords (`tuo-lexer`), lower to
`Ty::Wrapper(kind, T)` (`tuo-types`), and have a specified layout
(`abi.md`, "Box" and "Shared"). What they do not have is a constructor. There
is no builtin that produces one, and every plausible spelling is rejected:

```
let b: Box[Int] = Box::new(3);        // P0002
let b: Box[Int] = Box(3);             // P0002
let b: Box[Int] = box 3;              // P0002
let b: Box[Int] = Box[Int](3);        // P0002
let b: Box[Int] = std::boxed::new(3); // R0002: no `boxed` in `std`
```

So a `Box[T]` can appear as a parameter type, a return type, or a field type,
and the ownership fixtures move such parameters around
(`tests/ownership/fixtures/ok/moves.tuo`), but **no program can bring one into
existence**. "The interpreter remains the reference" is true of wrapper values
only vacuously: the interpreter has never executed one, because none can be
written. The wrapper is a type without inhabitants.

### Finding 2 — the recursion error recommends a dead end

`T0016` refuses a type that reaches itself by value and tells the author how to
proceed:

```
error[T0016]: recursive type: `List` reaches itself through its own fields
without a heap-wrapper indirection; v0 cannot represent this — break the cycle
with a `Box`/`Shared`/`Weak` indirection, or flatten the recursion into an
index arena (how `std::json` stores its tree)
```

Taking the first piece of advice:

```tuo
struct List {
    head: Int,
    tail: Option[Box[List]],
}

fn total(in l: List) -> Int { l.head }

fn main() -> Int {
    let l = List { head: 5, tail: Option::None };
    total(l)
}
```

```
$ tuo check list.tuo        # accepted, and no T0022 warning
$ tuo spec list.tuo         # 1 passed
$ tuo run list.tuo
error: cannot build (codegen): the native backend does not lower a `Box[T]`
heap wrapper value (heap wrappers await a later ADR)
```

The program only ever holds `Option::None` in that field — and could hold
nothing else, by Finding 1 — yet it cannot be built. It also draws no `T0022`
warning, because the advisory skips wrappers in field position on the stated
ground that "such a declaration lowers fine".

That ground is true and beside the point. Probing both backends:

| Use of a type with a wrapper in a field or payload | `build` |
|---|---|
| declared, never used | builds |
| an `in` parameter (a pointer to the caller's place) | builds |
| reached two levels down, through another struct's `in` parameter | builds |
| a `let` binding holding a value, annotated or inferred | **refused** |
| a temporary (`Node { … }.value`) | **refused** |
| an `Array` element | **refused** |

The *declaration* lowers. A *value* of the declared type does not, and a type
nobody may hold a value of is not much use. `T0022`'s help text tells the
author that "a wrapper in a struct field or enum payload is fine", which is
accurate about the first three rows and silent about the last three, and the
refusal that follows carries no span.

So the one diagnostic that exists to tell an author how to write a recursive
type sends them to a construct that is unconstructible, that cannot be held by
value, and that is unwarned when it fails. The second piece of advice, the
index arena, is the only one that works.

### What the arena costs

The arena is a real technique and `std::json` is a good implementation of it.
It is also the only option, which is a different thing. Every recursive
structure written so far has paid for it:

- **`std::json`** stores its tree as parallel arrays in depth-first pre-order,
  and every navigation function takes the arena plus an index.
- **Any future parser, expression tree, or linked structure** inherits the
  shape. An AST in tuonelang is an array of node records and an array of child
  indices, and the type checker cannot tell a node index from any other `Int`.

That last point is the cost that matters for this language in particular. A
tree of `Box`ed nodes is checked by the ownership system; a tree of integer
indices is checked by nothing. tuonelang is built to be written by models that
are steered by compiler feedback, and the arena moves a whole class of
structural errors out of the compiler's sight.

### What already exists

| Layer | State |
|---|---|
| `tuo-lexer`, `tuo-hir` | `Box`/`Shared`/`Weak` are keywords and lower to `Wrapper::{Box,Shared,Weak}` |
| `tuo-types` | `Ty::Wrapper(kind, T)`; the `T0016` recursion boundary treats a wrapper as the cycle-breaker |
| `tuo-ownership` | wrapper parameters and fields are moved, borrowed, and dropped by the ordinary rules, fixture-pinned |
| `specification/abi.md` | `Box` is one non-null pointer; `Shared` is a pointer to a `{strong, weak, T}` block |
| `tuo-runtime` | `tuo_rt_alloc`/`tuo_rt_dealloc`, the seam `String`/`Array`/`Map` already allocate through |
| both backends | recursive **deep-copy and drop glue** over owned array elements (ADR-0012), emitted inline |

What is missing is a constructor, a dereference, the lowering of both, and
glue that can recurse at *run* time. The inline glue ADR-0012 landed recurses
at *compile* time over the type's structure, which terminates because the
structure is finite. A recursive type's structure is not, which is precisely
why `T0016` exists: before it, such a type hung codegen.

## Decision

Heap wrappers become real values in three stages, `Box` first. The ordering
follows the dependency: `Box` needs only unique ownership, which the checker
already enforces; `Shared` and `Weak` additionally need an answer to the open
question Q-0005.

### The constructor is a builtin function, not syntax

```tuo
let b: Box[Int] = std::boxed::new(3);
let n: Int      = std::boxed::get(b);      // `in` — reads a Copy pointee
let v: T        = std::boxed::into(b);     // `take` — moves the pointee out
```

tuonelang has no methods and no associated functions, so `Box::new` is not
available as a spelling and inventing it for one type would be the ad-hoc
syntax the dogfooding rule forbids. A builtin module follows the precedent of
`std::array`, `std::string`, and `std::map`: receiver-witnessed, no user type
parameters, element-generic over the checker-accepted set. It also needs no
grammar change and therefore no `GRAMMAR-VERSION` bump.

There is deliberately **no dereference operator**. `*b` would be the first
prefix operator that produces a place, and it would arrive alone. `get` for a
`Copy` pointee and a borrowing accessor for the rest are ordinary calls the
ownership checker already understands.

### Runtime-recursive glue is out-of-line

Drop and deep-copy for a type that contains a wrapper are emitted as **one
function per type**, called rather than inlined, so a recursive type's glue
calls itself instead of expanding forever. Non-recursive types keep the inline
glue they have; the out-of-line path is taken only where a wrapper is
reachable, so no shipped program's code changes.

Recursion depth then becomes a run-time quantity. Dropping a million-node list
recurses a million frames deep, and the native stack is finite. This ADR does
not pretend otherwise: the limit is documented, the interpreter's existing
recursion limit is the reference behaviour, and a stack-exhaustion trap is a
defined outcome rather than a crash. An iterative drop is a possible later
refinement and is not promised here.

### `T0016` stays, and its advice becomes true

The recursion boundary is correct and is kept. A by-value cycle still has
infinite size. What changes is that the remedy it names will work.

## Staging

### Stage A — honesty, independently landable

No lowering. Three corrections, each of which is true today:

1. `T0022` warns wherever a **value** whose type transitively contains a
   wrapper is held — a local, a temporary, an array element — and stays quiet
   for a declaration and for a borrow-mode parameter, which build. The trigger
   must therefore be the *type* of the stored value, read from the type
   checker's results, where today's advisory reads only written annotations
   and so cannot see an inferred `let`. The rule has to match the backends'
   refusal exactly in both directions: a missed case leaves a spanless
   refusal, and a false warning tells an author to avoid something that
   works, which this project counts as worse than silence. The help text's
   "is fine" sentence is corrected in the same change.
2. `T0016`'s message stops recommending a wrapper until Stage B lands, and
   recommends the arena alone.
3. The cheat sheet's runnable-core section states that wrapper values cannot
   be constructed, not merely that they cannot be built.

Stage A is worth landing even if Stage B never does. It costs a diagnostic
change and makes three false statements true.

### Stage B — `Box[T]`

`std::boxed::new`/`get`/`into` and a borrowing accessor; interpreter support
first, as the reference; then both backends, pinned three-way; then the
out-of-line glue. `T0022` stops firing for `Box` and `T0016`'s advice is
restored. ABI version bump, since `layout_of` gains a lowered case.

### Stage C — `Shared[T]` and `Weak[T]`

Blocked on Q-0005 (atomic reference counts, or an atomic and a non-atomic
sibling). `par_map` moves only `Copy` tasks across threads today, so a
non-atomic `Shared` would be sound under the current concurrency model, but
that is a property of ADR-0007's restrictions and not of `Shared`. The
question is settled before Stage C begins, not during it.

## Benchmark plan

Per the project rule, the change is gated on measurements, and both workloads
below **cannot be written today**, which is the re-entry mechanism the lab's
catalog documents:

- **`tree-walk`** — build a balanced binary tree of `Box`ed nodes to a fixed
  depth, sum it by recursive descent, drop it. C peer: `malloc`ed nodes and an
  explicit recursive `free`. Go peer: pointer nodes under its collector, which
  is the honest comparison and will show the collector's different cost shape.
- **`arena-vs-box`** — the same tree as an index arena and as `Box`ed nodes,
  both in tuonelang. This is the one that matters for the decision: if the
  arena is substantially faster, the ADR's case rests on checkability alone
  and should say so. It is reported as measured, with no verdict.

The oracle is `std::json`. Re-expressing its tree over `Box` must leave every
spec green and every rendered byte identical; if it cannot, the design is
wrong rather than the specs.

## Consequences

- **`std::json` does not change shape on landing.** The arena is shipped,
  spec'd, and benchmarked. A `Box`-based tree is an oracle for this ADR and
  possibly a later alternative, never a silent replacement.
- **ADR-0024 is unblocked in part.** A capturing closure's environment is a
  heap allocation with compiler-generated drop glue, which is a `Box` in all
  but name. Stage B supplies the mechanism ADR-0024's decision D3 needs.
- **The differential suite's generator must learn to build wrappers**, or the
  three-way agreement is pinned only by hand-written fixtures. That is real
  work and is counted in Stage B, not deferred past it.
- **User destructors remain out.** Drop glue stays compiler-generated (Q-0011),
  so dropping a `Box[T]` runs no user code.

## What this ADR does not propose

A garbage collector, a dereference operator, `Box` of a trait object (there are
no traits), or interior mutability. `Shared[T]` is reference-counted shared
*ownership* of an immutable value, as `abi.md` already specifies, and nothing
here widens that.
