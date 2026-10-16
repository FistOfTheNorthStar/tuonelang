# ADR-0028: Heap-wrapper values — giving `Box[T]` a constructor, and recursive types a runtime

- **Status:** proposed (Stages A and B **landed** 2026-10-14; Stage C open — see Resolution)
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

## Resolution (2026-10-14) — Stages A and B landed, Stage C open

**Stage B landed as proposed**, and with it most of Stage A became moot rather
than separately necessary.

- **The constructor is `std::boxed`**: `new(take v: T) -> Box[T]`,
  `get(in b: Box[T]) -> T` (a copy — deep for a heap-owning pointee, as
  `std::array::get` copies an element), and `into(take b: Box[T]) -> T`.
  Three builtins, receiver-witnessed and generic over the pointee; no grammar
  change, no dereference operator. MIR gained `HeapOp::BoxNew`/`BoxGet`/
  `BoxInto` (`mir.md` §5.7); the interpreter models a box as a one-field
  aggregate, so Rust ownership of the value tree *is* the unique ownership.
- **Both backends lower it.** A box is a pointer-width scalar. Drop and deep
  copy of a boxed pointee are emitted **out of line**, one function per
  pointee type, declared before its body so a recursive type's glue calls
  itself instead of expanding forever (LLVM defines the function on the spot;
  Cranelift declares on first call and defines after the program's functions).
  Non-boxed types keep their inline glue, so no shipped program's code
  changed. Pinned three-way by `box_values` and `box_recursive` (a list of
  owned strings deep-copied and consumed, a binary tree built and counted);
  20,000 rounds of build/copy/drop report **zero leaks** under macOS `leaks`
  on both backends.
- **ABI version 14.** `layout_of` already gave `Box` its one-pointer layout
  (`abi.md` "Box"), so the lowering alone changed no layout; the bump to 14
  landed with the stack-overflow trap (a new trap code and runtime symbols),
  and records code relying on the `Box` layout in the same version.
- **`T0016`'s advice is true**: it names `Box[Name]` and `std::boxed::new`,
  and no longer `Shared`/`Weak`. **`T0022`** looks through `Box` and warns
  only for `Shared`/`Weak`, or a `Box` inside an `Array`/`Map` type (still
  refused natively). Stage A's item 1 — driving `T0022` from inferred types —
  was not done: with `Box` lowered, the remaining unlowered wrappers cannot be
  constructed at all, so an inferred binding of one can only come from a
  parameter that is itself already warned about. The brief now teaches
  `std::boxed` and says plainly that `Shared`/`Weak` values do not exist.

**Measured** (release, fastest of five after a warm-up, Apple M4): the
Benchmarks Game **`binary-trees`** (maximum depth 16, every node its own `Box`,
each freed as the walk counts it) — **173.6 ms** against C 149.0 ms (1.16x),
Rust 169.1 ms (1.03x), Go 130.5 ms (1.33x; its collector frees in bulk, the
different cost shape this ADR predicted). All four agree on exit byte 174.
It was the lab's last `Unsupported` entry; the catalog has none now.

**What did not land, stated plainly:**

- **The borrowing accessor** was a language-wide gap rather than a `Box`
  one — a match binding always moved its payload, so nothing could be read
  out of an `in` value's enum payload. **Resolved (2026-10-16) by ADR-0037**
  without an accessor: a match on an `in`-rooted place lends its non-`Copy`
  parts, and `get(b)` passed to an `in` parameter lends the pointee instead
  of copying it, so a tree is walked by borrow in O(n).
- **The stack.** Out-of-line glue recurses once per node, so its depth is a
  run-time quantity: a 1,000,000-node list's drop overflows the 8 MB main
  stack. It first **segfaulted (exit 139)**; since 2026-10-16 it is the
  defined trap this ADR promised — the runtime's guard-page handler reports
  `tuo: trap: stack overflow` and aborts with the trap status, on the main
  thread and on `par_map` workers alike (`TrapCode::StackOverflow`, ABI 14,
  pinned by `tuo-cli/tests/stack_overflow.rs`). An iterative drop for the list
  shape is still not promised.
- **`arena-vs-box`**, the **`std::json` oracle**, and the **differential
  generator** were not done when Stage B landed; they have since been done
  (below).
- **Stage C** (`Shared`/`Weak`) waits on Q-0005, which ADR-0034 proposes to
  settle as atomic-only.

### The three Stage B follow-ups (2026-10-16)

**The differential generator builds boxes.** Every generated program now
carries a fixed prelude (`BOX_PRELUDE` in
`crates/tuo-cli/tests/differential/generator.rs`), and a new expression form
draws on it: boxed scalars and structs through `new`/`get`/`into`, a box in a
struct field behind `Option` opened by a partial move, a box in a box, a box as
an `Option[Box[Int]]` payload, boxes as fixed-array elements, a `var` box
overwritten, a box passed to and returned from a call and borrowed by an `in`
parameter, and two recursive types through `Option[Box[Self]]` — a binary tree
of depth 0–4 and a list of up to six cells owning `String`s — built,
deep-copied with `get`, consumed whole or in part, and dropped unopened. The
operands are generated expressions, so a trap can fire with live boxes in
scope. 68% of the generator's programs use a `Box` form and 48% build a
recursive one (`the_corpus_exercises_boxes_and_recursive_box_types` keeps both
above a floor). The committed sweeps (150 seeds on Cranelift, 60 on LLVM) pass,
as did a one-off wider sweep of 2,000 further seeds on Cranelift and 1,000 on
LLVM: **no cross-engine mismatch**. That sweep did surface one harness false
positive, unrelated to `Box`: a program returning `-378` exits with byte 134,
the trap status, and the harness called it a trap. It now also requires the
runtime's `tuo: trap:` line on stderr, pinned by
`a_return_of_the_trap_status_byte_is_not_a_trap`.

**`arena-vs-box`, measured** (`arena_vs_box_timing` in
`crates/tuo-cli/tests/lab_command.rs`; programs
`benchmarks/runtime/programs/tuo/arena-vs-box-{arena,box}.tuo`). A balanced
tree of depth 20 (2,097,151 nodes), four rounds of build, sum by recursive
descent, and drop, both built with `tuo build --release`, best of five after a
warm-up, Apple M4, three runs of the table: **arena 52.6–53.2 ms, box
130.8–136.0 ms, box/arena 2.46–2.59x**. Both reach the same checksum. The two
programs do not do identical work, and the difference is the language's, not
the benchmark's: the arena grows three `Array[Int]` columns by doubling and
frees them once, while the box tree makes one allocation per node, and its
walk must consume the tree (a match binding moves its payload), so each node
is freed as it is summed. The arena is the faster representation by this
measure. Per the benchmark plan, the case for `Box` therefore rests on what
the compiler can check — a child is a typed, owned value rather than an `Int`
any other `Int` can be confused with — and not on speed.

**The `std::json` oracle holds.** `tests/codegen/oracles/box_json.tuo` defines a
`Value` enum whose containers hold `Option[Box[Item]]` / `Option[Box[Member]]`
lists, converts `std::json`'s arena into it node by node, and renders it. For
all 27 documents (every document `std::json`'s specs parse, plus the
json-parse workload's) the `Box` tree renders **byte-identically** to
`std::json::render`, both directly and through a deep copy taken with
`std::boxed::get`; and `std::json`'s spec questions (kinds, numbers, decoded
strings, node and child counts, element order, member lookup, nesting) asked
of the `Box` tree get `std::json`'s answers. `crates/tuo-cli/tests/box_json_oracle.rs`
compiles it with the unmodified catalog `std::json` and runs it on all three
engines: 22 specs pass on the interpreter (`std::json`'s ten and the oracle's
twelve) and `main` returns 27 on the interpreter, Cranelift, and LLVM.
`std::json` was not changed. Two costs showed up and are written into the
oracle's header: every reader takes its tree by value, so asking a document a
second question means converting it again or deep-copying it first; and
`std::json`'s scalar formatting is private, so the oracle carries a copy of it
and the comparison pins the tree's structure rather than two independent
number formatters. Nothing the oracle needed was inexpressible.

**One gap found on the way, not `Box`-specific:** a `for` loop over a fixed
array of non-`Copy` elements (`[Box[Int]; 2]`, and equally `[String; 2]`) is
accepted by `tuo check`, but MIR lowering skips the enclosing function
(`debug mir`: "iterating a fixed `[T; N]` of non-`Copy` elements is not
lowered in v0"), and `tuo run` then fails with the misleading "the program has
no function named `main` to compile". Indexed reads of such an array work, and
that is what the generator uses.

*Note (2026-10-16, after ADR-0037).* The two measurements above consume their
trees because, when they were written, a match binding could only move its
payload. ADR-0037 has since landed: a match on an `in`-rooted place lends its
parts, and `std::boxed::get(b)` passed to an `in` parameter lends the pointee,
so a boxed tree can now be read by borrow, any number of times, without a
copy. The `arena-vs-box` figures stand as a build-sum-drop comparison (the
consuming walk *is* the drop); the oracle's "every reader takes its tree by
value" cost is now a choice of that file rather than a limit of the language.
