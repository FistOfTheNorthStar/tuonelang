# The tuonelang ownership model (v0)

> **Status.** Normative. This document freezes the v0 ownership and
> memory-safety semantics that `tuo-ownership` will enforce, refining the
> Constitution's frozen commitments (§21 ownership terminology, §22 parameter
> modes, §25 `Box`/`Shared`/`Weak`, §26 `unsafe`). It was adopted by
> [ADR-0003](adr/ADR-0003-ownership-model.md). The executable counterpart of
> this document is the fixture corpus in `tests/ownership/fixtures/` —
> `ok/` programs must compile, `err/` programs must fail with the diagnostic
> code annotated at the offending line. Specification and fixtures are
> maintained together; a change to one is a change to the other.

tuonelang's model is deliberately **smaller than Rust's**: there is no
user-written lifetime syntax (Constitution §18), no reference types, and no
borrow that outlives the function call that created it. The checker is a
per-function (intra-procedural) analysis over **places** and their
**initialization states**; function signatures carry the complete ownership
contract through their parameter modes, so no interprocedural inference is
ever required.

## 1. Places and owners

- A **place** is a location the checker tracks: a local binding (`let`/`var`),
  a function parameter, or a **field path** rooted in one of those
  (`point.x`, `msg.payload.buf`). Field paths are finite: v0 has no
  dereference operator, so a path never goes *through* a `Box`, `Shared`, or
  `Weak` (the wrapper itself is the tracked value).
- Every value has exactly **one owner** at any time: the place that holds it.
  Index expressions (`items[i]`) are **not** places in v0: indexing *reads*
  an element (which therefore must be `Copy` to be used by value) and an
  element cannot be moved out of or assigned through an index in v0.
- Two places **overlap** iff one's path is a prefix of the other's (`s` and
  `s.f` overlap; `s.f` and `s.g` are disjoint).

## 2. `Copy` and owned values

- A type is **`Copy`** iff duplicating it bitwise is sound. The compiler
  derives this structurally; it is never written by hand (Constitution §21):
  - all primitive scalars (`I8`–`I64`, `Isize`, `U8`–`U64`, `Usize`, `F32`,
    `F64`, `Bool`, `Char`), the unit type `()`, and ranges of `Copy` types
    are `Copy`;
  - `Str` is `Copy` (see §13);
  - a `struct` or `enum` is `Copy` iff every field of every variant is
    `Copy`;
  - `String`, `Array[T]` (an owned heap array), `Box[T]`, `Shared[T]`, and
    `Weak[T]` are **never** `Copy`, regardless of `T`;
  - a fixed-size array `[T; N]` (ADR-0004 Stage 2) is `Copy` iff `T` is —
    the §2 aggregate rule applied to `N` identical fields; the length is
    irrelevant. Contrast: the growable `Array[T]` above is **never** `Copy`
    (it owns a heap buffer), while `[T; N]` owns no heap and inherits its
    element's `Copy`-ness. A non-`Copy` `[T; N]` moves as one whole value,
    exactly like a struct; element-wise partial moves do not exist because
    index expressions are not places (`O0007`, §below). Dropping a `[T; N]`
    drops elements front to back and frees nothing (the storage is inline);
    a `Copy` fixed array's drop is a no-op. The repeat literal `[x; N]`
    duplicates its operand `N` times and therefore requires a `Copy`
    element for **every** `N` (uniform, no `N == 0`/`1` special cases) —
    **O0010** otherwise, since duplicating a non-`Copy` value is precisely
    what this section forbids. The list literal `[a, b, c]` has no such
    restriction (each element is its own value, moved in). Iteration binds
    each element **by copy**, so a `for` over a non-`Copy`-element fixed
    array type-checks but is refused at MIR lowering, exactly as for
    `Array[T]`. `in`/`mut` parameters of type `[T; N]` alias the caller's
    inline storage like any other aggregate — no new rule;
  - a generic type parameter `T` is treated as **not** `Copy` inside a
    generic function body (checking happens once per `fn`, pre-monomorphization,
    and must be sound for every instantiation).
- Using a `Copy` place by value **copies**; the place stays initialized.
  Everything else is an **owned** (move-only) value.

## 3. Moves

A non-`Copy` place used **by value** is **moved**: ownership transfers and
the source place becomes uninitialized. By-value uses are:

- the initializer of a binding: `let y = x;`
- the right-hand side of an assignment: `slot = x;`
- an argument to a `take` parameter;
- a field initializer in a struct/enum literal: `Pair { first: x, … }`;
- a returned value (`return x;` or a block's tail expression);
- a `match` scrutinee when some arm binds a non-`Copy` part of it (§10);
- the iterable of a `for` loop.

Using a place whose value has been moved — reading it, moving it again,
borrowing it, or moving/reading any overlapping place — is **O0001 use of
moved value**. The state is per-path: moving `s.f` leaves `s.g` usable (§9).

```tuo
// MUST FAIL (O0001)
fn use_after_move(take b: Box[Int]) {
    let c = b;
    let d = b;      // ERROR: O0001 `b` was moved to `c`
}

// MUST COMPILE — Copy values never move
fn copies(in n: Int) -> Int {
    let a = n;
    let b = n;
    a + b
}
```

### Explicit `move`

`move place` performs exactly the move that would happen implicitly; it
exists as a clarity marker (Constitution §21). It is valid only on a place
expression of a non-`Copy` type. Applying `move` to a `Copy` value or to a
non-place expression is **O0007 invalid explicit move** — the marker would be
meaningless, and tuonelang rejects meaningless forms rather than ignoring
them.

## 4. Parameter modes: `in`, `mut`, `take`

Every parameter (and method receiver) has exactly one mode; call sites do
not repeat the mode — the callee's signature is the contract.

- **`in x: T`** — shared, read-only **borrow** for the duration of the call.
  Inside the callee, `x` is an immutable, non-owned place: it may be read
  (copying `Copy` data), passed on as an `in` argument, but never mutated
  and never moved out of.
- **`mut x: T`** — exclusive, mutable **borrow** for the duration of the
  call. Inside the callee, `x` is a mutable, non-owned place: it may be
  read, assigned (the old value is dropped, §11), and passed on as `in` or
  `mut`; it may not be moved out of. `x` must hold a value whenever the
  callee returns — and because moving out of `mut` is forbidden outright
  (§5), this holds by construction.
- **`take x: T`** — the callee **owns** `x` (the caller's value moved into
  the call). Inside the callee, `x` is a mutable, owned place: it may be
  read, mutated, moved out of, returned, or simply dropped at scope end.

**The borrow rule (frozen).** At any point a value has either any number of
`in` borrows or exactly one `mut` borrow, never both. Because v0 has no
reference types, a borrow exists **only while its call executes**; borrows
can therefore only conflict inside a single argument list (§6).

## 5. Moving out of borrows

A callee never owns its `in`/`mut` parameters, so transferring ownership out
of them (or out of any field path rooted in them) is **O0003 move out of
borrowed parameter**. This is categorical: v0 does not permit
"move out then reinitialize before returning" even for `mut` parameters.

```tuo
// MUST FAIL (O0003)
fn steal(in b: Box[Int]) -> Box[Int] {
    b               // ERROR: O0003 cannot move out of `in b`
}

// MUST COMPILE — take owns, so returning is an ordinary move
fn pass_through(take b: Box[Int]) -> Box[Int] {
    b
}
```

## 6. Calls: conflicts and re-borrowing

Arguments are evaluated left to right; the callee's borrows all begin when
the call begins and end when it returns. Consequently a nested call's borrows
end before the outer call starts, and the only possible conflicts are within
**one argument list** (the receiver counts as an argument):

- the same or overlapping places passed with two borrows where at least one
  is `mut` → **O0005 conflicting borrows in call**;
- a place moved (`take` argument) while the same or an overlapping place is
  also passed as `in`/`mut` in that list → **O0006 move of borrowed value in
  call**; two `take` arguments from overlapping places are instead an
  ordinary use-after-move (**O0001** on the second, by left-to-right
  argument order);
- disjoint field paths never conflict: `f(mut p.x, mut p.y)` is legal
  (this **resolves Q-0004**: `mut` re-borrowing is pass-through only, and
  "splitting" is simply the disjointness rule — there is no other re-borrow
  form).

```tuo
fn observe(mut a: Counter, in b: Counter) { }

// MUST FAIL (O0005)
fn conflict(take c: Counter) {
    observe(c, c);  // ERROR: O0005 `c` passed as `mut` and `in` in one call
}

// MUST COMPILE — the inner call's borrow ends before the outer call begins
fn nested(take c: Counter, in d: Counter) {
    observe(c, d);
}
```

## 7. Mutability of places

Mutable places are: `var` bindings, `mut` parameters, `take` parameters, and
field paths rooted in one of those. `let` bindings and `in` parameters (and
their fields) are immutable. Assigning to an immutable place, or passing it
as a `mut` argument, is **O0004 mutation of immutable place** — with one
exception: the *first* assignment to a `let` binding declared without an
initializer is **initialization**, not mutation (Constitution §9 definite
assignment), and each control-flow path may initialize it at most once; a
second assignment on any path is O0004.

```tuo
// MUST COMPILE — deferred initialization of a `let`
fn deferred(in flag: Bool) -> Int {
    let x: Int;
    if flag {
        x = 1;
    } else {
        x = 2;
    }
    x
}

// MUST FAIL (O0004)
fn frozen_binding() {
    let x = 1;
    x = 2;          // ERROR: O0004 `x` is a `let` binding
}
```

## 8. Returns

Returning a value (early `return e;` or a block tail) moves it to the
caller. Locals and `take` parameters may be returned; returning a non-`Copy`
`in`/`mut` parameter (or a non-`Copy` field of one) is a move out of a
borrow, **O0003**. On every return path, all still-initialized locals and
`take` parameters are dropped (§11) after the returned value has been moved
out.

## 9. Struct field moves (partial moves)

Moving a non-`Copy` field out of an **owned** struct place is legal and
leaves the struct **partially moved**:

- the moved field path is uninitialized (using it is O0001);
- the struct **as a whole** cannot be used by value, borrowed, or passed
  until restored — **O0009 use of partially moved value**;
- sibling fields remain fully usable;
- reassigning the moved field (on a mutable root) restores it; when every
  field is initialized again the whole is usable again.

Field moves out of `in`/`mut` parameters are O0003 (§5). Enum payloads move
only through `match` (§10) — there is no field access into an enum variant.

```tuo
struct Pair { left: Box[Int], right: Box[Int] }

// MUST COMPILE — siblings are independent; reinit restores the whole
fn split(take p: Pair) -> Pair {
    let l = p.left;
    let r = p.right;
    p.left = l;
    p.right = r;
    p
}

// MUST FAIL (O0009)
fn broken(take p: Pair, take q: Pair) -> Pair {
    let l = p.left;
    p               // ERROR: O0009 `p.left` was moved out
}
```

## 10. `match`, branches, and joins

- A `match` moves its scrutinee place iff **any** arm binds a non-`Copy`
  part of it (the check is static and conservative — the arm actually taken
  at runtime is irrelevant). A match whose arms bind only `Copy` data (or
  nothing) merely reads the scrutinee. Arm guards may only read; a guard
  cannot move.
- **Joins.** Initialization state is tracked per path through `if`/`else`,
  `match`, and loops, and merged at join points: a place is initialized
  after a join iff it is initialized on **every** arriving path. Using a
  place that is initialized on some paths but not others is **O0002 use of
  possibly moved value**. Paths that diverge (`return`, `break`, `continue`,
  a `?` early exit, trailing `Never`) do not contribute to the join.
- **Loops.** The body's exit state joins back into the loop head: a value
  moved in an iteration and not reinitialized before the back edge is
  possibly-moved at the next iteration's use site (O0002); the diagnostic
  is anchored at the responsible move, since that is where the fix goes.
  `for` moves its iterable once, before the first iteration; loop-local
  bindings are fresh each iteration. A deferred-initialization `let` (§7)
  may not be initialized inside a loop body: the assignment could run once
  per iteration, so it is a conservative O0004.

```tuo
// MUST COMPILE — every path replaces what it moved
fn balanced(take b: Box[Int], take c: Box[Int], in flag: Bool) -> Box[Int] {
    var slot = b;
    if flag {
        let taken = slot;
        slot = c;
        drop_box(taken);
    } else {
        drop_box(c);
    }
    slot
}

// MUST FAIL (O0002)
fn loop_carried(take b: Box[Int]) {
    var i = 0;
    while i < 3 {
        let c = b;  // ERROR: O0002 `b` may already be moved by a previous iteration
        i = i + 1;
    }
}
```

## 11. Destruction and drop order

- **RAII.** When an initialized place goes out of scope its value is
  **dropped** deterministically. Within one scope, drops run in **reverse
  declaration order**; a shadowed binding's value survives (still owned by
  the shadowed slot) and is dropped in that same reverse order at scope end.
  Temporaries drop at the end of their enclosing statement, in reverse
  creation order.
- Drops run on **every** scope exit: normal fall-through, `return`, `?`
  propagation, `break`, and `continue` (dropping exactly the scopes being
  exited, innermost first).
- **Assignment drops the old value.** `slot = new;` on an initialized
  mutable place first evaluates `new`, then drops the old value, then stores.
- **Partial moves drop the residue**: only the still-initialized fields of a
  partially moved struct are dropped.
- **Panics abort.** A panic (trapping overflow, division by zero, explicit
  abort — Constitution §24) terminates the program **without** running
  destructors. v0 has no unwinding; this keeps panic behavior deterministic
  and drop reasoning purely local.

### No hidden drop flags

At every drop point the checker must **statically** know whether each place
is initialized. A place that is *possibly* moved at a drop point —
moved on some paths to that point but not others — is **O0008 conditional
move without reinitialization**. tuonelang refuses to insert Rust-style
runtime drop flags: drop sites are statically known, with no hidden state
and no hidden branches, which both determinism (§28) and predictable codegen
rely on. The fix is always local: move on every path, or reinitialize, or
drop explicitly on the moving path.

```tuo
// MUST FAIL (O0008)
fn maybe_move(take b: Box[Int], in flag: Bool) {
    if flag {
        consume(b); // ERROR: O0008 `b` moved here but not on the other path
    }
}

// MUST COMPILE — the moving path never rejoins
fn early_out(take b: Box[Int], in flag: Bool) {
    if flag {
        consume(b);
        return;
    }
}
```

## 12. `Box`, `Shared`, `Weak`, and cycles

All three wrappers are ordinary non-`Copy` values to the checker: they move,
borrow, and drop by the rules above. Their *contents* add no aliasing rules
in v0 because no operation reaches through a wrapper (no dereference
operator; the access surface arrives with the stdlib).

- **`Box[T]`** — unique ownership of a heap allocation. Dropping the `Box`
  drops the `T`, then frees. Moving a `Box` moves the handle; the heap value
  never moves.
- **`Shared[T]`** — reference-counted shared ownership. Duplicating a
  `Shared` handle is **explicit** (a stdlib clone operation taking the
  handle `in` and returning a new handle); assignment and passing move the
  handle like any value — there is no implicit copy. Dropping a handle
  decrements the count; the `T` is dropped (deterministically, at that
  drop site) when the last handle drops. `Shared` grants **shared read
  only**: no `mut` access to the contents ever goes through a `Shared`;
  interior mutation requires an explicit synchronized cell type (stdlib,
  deferred).
- **`Weak[T]`** — a non-owning handle obtained from a `Shared[T]`. It never
  keeps the value alive and cannot touch the contents at all: the only
  content-reaching operation is **upgrade**, which yields
  `Option[Shared[T]]` — `None` if the value is already gone. `Weak` is
  non-`Copy` (it manages a weak count); it is cloned explicitly and dropped
  like any value.
- **Cycles.** A cycle of `Shared` handles keeps itself alive: it **leaks**.
  A leak is *safe* — memory-safety is never violated, destructors of the
  cycle simply never run — and this is defined behavior, not an error the
  checker detects in v0. Cycles are broken by demoting one edge to `Weak`
  (the normative pattern: owners point down with `Shared`/`Box`, back-edges
  point up with `Weak`).

```tuo
// MUST COMPILE — a cyclic shape is expressible only via Weak back-edges
struct Node {
    value: Int,
    parent: Weak[Node],
}
```

## 13. `Str` and view types

`Str` is a borrowed view, which a lifetime-free model must keep from
dangling. A `Str` that originates from a **string literal** cannot dangle by
construction — literals live for the whole program — so such a `Str` is `Copy`
and unrestricted: it can be stored, returned, and kept freely.

Since ADR-0010 a `Str` may also originate from **`std::string::as_str(in s:
String) -> Str`**, a zero-copy view of a live `String`'s bytes (resolving
Q-0012). A view *into a `String`* **can** dangle — the `String` may be moved,
mutated (a `push_byte`/`append` that reallocates its buffer), dropped, or
overwritten — so it is not unrestricted. The rule that keeps it sound, without
user lifetimes, is a **value-provenance borrow**:

> A `Str` value produced by `std::string::as_str(s)` is a **shared borrow of
> `s`** for as long as that `Str` value is live. While it is live, `s` may not
> be moved, mutably borrowed, dropped, or overwritten. A `Str` derived from a
> `String` may not **escape the frame** that created it (it may not be returned,
> `break`-ed out, or stored where it outlives the borrow).

This is the model's first borrow whose source is a **value** (an expression's
result) rather than a named place; it is tracked flow-sensitively per body, just
like place borrows. A view bound with `let v = as_str(s)` keeps `s` borrowed
until `v` leaves scope or is overwritten; a view used **inline** (e.g.
`std::str::len(as_str(s))`) borrows `s` only for that expression, exactly like
any other `in` argument. The rule is deliberately **conservative** — it keeps
`s` borrowed for the *whole* lexical extent of a bound view, not only up to the
view's last use (NLL-style last-use shrinking is future work), and it forbids
returning or storing a `String`-derived view entirely. Both restrictions are
sound with no lifetime inference and are purely additive to relax later.

Violations are diagnosed as: moving `s` while a view is live — the use-of-moved
family **O0001**/**O0002**; a `mut` borrow of `s` while a view is live —
**O0011**; dropping/overwriting `s` while a view is live — **O0011**; and a view
escaping its frame — **O0011**. See §15.

## 14. Resource cleanup and `unsafe`

- **Cleanup is RAII** (§11): stdlib resource types (files, sockets, …) release
  their resource in their destructor at their statically known drop site.
  **User-written destructors are deferred from v0** (Q-0011): v0 drop glue is
  entirely compiler-generated (fields in declaration order, wrappers as in
  §12), so dropping can never run user code, observe partially-moved state,
  or panic.
- **`unsafe` relaxes nothing here.** Ownership and borrow checking apply
  *identically* inside `unsafe { … }` blocks — every rule and diagnostic in
  this document is unaffected. `unsafe` unlocks only the operations the
  Constitution §26 names (raw-pointer use, FFI; surface deferred, Q-0009),
  none of which exist in v0 source yet. This is frozen: no future intrinsic
  may be specified to disable move or borrow checking of safe places.

## 15. Diagnostics

Ownership diagnostics use the `O` namespace. The v0 codes, each exercised by
the negative fixture corpus:

| Code | Meaning |
|------|---------|
| `O0001` | Use of moved value. |
| `O0002` | Use of possibly moved value (branch join or loop back edge). |
| `O0003` | Move out of a borrowed (`in`/`mut`) parameter. |
| `O0004` | Mutation of an immutable place (`let` binding, `in` parameter, or a field of one). |
| `O0005` | Conflicting borrows of overlapping places in one argument list. |
| `O0006` | Move of a place that is borrowed in the same argument list. |
| `O0007` | Invalid explicit `move` (a `Copy` value or a non-place expression). |
| `O0008` | Conditional move without reinitialization at a drop point (no hidden drop flags). |
| `O0009` | Use of a partially moved value. |
| `O0010` | Repeat array literal `[x; N]` of a non-`Copy` element (ADR-0004 Stage 2). |
| `O0011` | A `String` is moved, mutated, dropped, or overwritten while a `Str` view of it (from `std::string::as_str`) is still live, or such a view escapes its frame (ADR-0010, §13). |

Every diagnostic names the place, the earlier action that produced the state
(the move site, the borrow, the conflicting argument), and — where one
exists — the local fix, following the structured-diagnostic conventions of
`tuo-diagnostics`.

## 16. Checking order

Ownership checking runs after type checking and consumes its results; a
function with type errors is not ownership-checked (poisoned types would
produce noise, not signal). Specs (`spec` blocks) are ownership-checked
exactly like function bodies, per ADR-0002's "specs are checked in every
compilation".
