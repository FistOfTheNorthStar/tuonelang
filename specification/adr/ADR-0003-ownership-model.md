# ADR-0003: The v0 ownership model

- **Status:** accepted
- **Date:** 2026-07-21
- **Context:** the Constitution freezes the ownership *vocabulary* — single
  owners, moves, derived `Copy`, the `in`/`mut`/`take` modes and the
  shared-XOR-exclusive borrow rule, RAII drops, `Box`/`Shared`/`Weak`, a
  narrow `unsafe` (§21–§26) — but not the *checkable semantics*: what exactly
  moves and when, how initialization state flows through branches and loops,
  what partial moves permit, when destructors run, and which programs the
  checker must accept or reject. Implementing `tuo-ownership` against an
  unwritten model would reproduce Rust's checker by folklore, which the
  project explicitly forbids ("do not implement an undocumented Rust clone").
- **Decision:** the normative model in
  [`specification/ownership.md`](../ownership.md), summarized below, together
  with the executable fixture corpus in `tests/ownership/fixtures/`
  (100+ programs that must compile, 100+ that must fail with a named
  `O`-code). The specification and the fixtures are adopted as one artifact:
  implementation of the checker begins only from this agreed pair, and a
  behavioral change to either requires updating both.

## The shape of the model

tuonelang's model is deliberately smaller than Rust's, built on one
structural simplification: **no reference types and no user lifetimes, so a
borrow exists only for the duration of the call that takes it** (`in`/`mut`
parameter modes are the only borrow formers). Everything else follows:

- Checking is **per function**, flow-sensitive over places (locals,
  parameters, field paths) and their initialization states. Signatures carry
  the whole contract; there is no interprocedural inference and no region
  inference beyond call scope.
- Borrow conflicts can only arise **within one argument list** (`O0005`,
  `O0006`); nested calls' borrows end before the outer call begins.
  Overlap is path-prefixing, so disjoint fields never conflict.
- **Q-0004 (mut re-borrowing/splitting) is resolved**: re-borrowing is
  pass-through only (an `in`/`mut` parameter may be forwarded as an argument,
  including its field paths); "splitting" is nothing more than the
  disjoint-paths rule. The open-questions entry is retired.

## Key rulings

1. **Moves and `Copy`** — by-value use of a non-`Copy` place moves it;
   `Copy` is derived structurally (wrappers, `String`, and `Array` never;
   generic `T` conservatively never inside a generic body). Use after move
   is `O0001`.
2. **Borrowed parameters are never move-out sources** (`O0003`) — not even
   `mut` with reinitialization before return.
3. **Mutable places** are `var` bindings and `mut`/`take` parameters (plus
   their fields); mutating anything else is `O0004`. Deferred `let`
   initialization is initialization, not mutation.
4. **Partial moves** are per-field-path: siblings stay usable, the whole is
   unusable (`O0009`) until the moved path is reassigned.
5. **Joins are conservative**: initialized-on-every-path or
   possibly-moved (`O0002`), including loop back edges.
6. **No hidden drop flags**: initialization must be statically known at
   every drop point; unbalanced conditional moves are rejected (`O0008`)
   rather than patched with runtime flags. Drops are reverse-declaration
   order, on every exit path; assignment drops the old value; **panics abort
   without running destructors** (no unwinding in v0).
7. **Wrappers are plain values** to the checker; no operation reaches
   through them in v0. `Shared` clones are explicit; `Weak` reaches contents
   only via upgrade to `Option[Shared[T]]`; `Shared` cycles leak safely and
   are broken with `Weak`.
8. **`Str` is a `Copy` view that only ever points at literals** in v0;
   `String`→`Str` borrowing is deferred (new Q-0012).
9. **User-written destructors are deferred** (new Q-0011); v0 drop glue is
   compiler-generated only, so cleanup never runs user code.
10. **`unsafe` relaxes no ownership rule**, now or ever.

## Consequences

- `tuo-ownership` implements exactly `specification/ownership.md`; the
  fixture corpus is its acceptance suite. The diagnostic namespace `O0001`–
  `O0009` is fixed there.
- Both `ok/` and `err/` fixtures must pass the front end
  (`parse → resolve → type-check`) with zero diagnostics — err fixtures
  fail only at the ownership stage. The harness in
  `crates/tuo-ownership/tests/` enforces this, the corpus conventions
  (documented in `tests/ownership/README.md`), and — now that the checker
  is implemented — the acceptance contract itself: `ok/` produces zero
  ownership diagnostics, `err/` exactly the annotated code per annotated
  line.
- Programs relying on Rust-permitted patterns that v0 rejects (conditional
  moves healed by drop flags, move-out-and-restore of `mut` parameters,
  index-based moves) have local rewrites; the rejections are deliberate
  simplifications, revisitable by a future ADR with implementation
  experience in hand.
