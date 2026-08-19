# ADR-0010: The `String` → `Str` borrowing view (Q-0012)

- **Status:** proposed (Stage A landed 2026-08-19 — front end + MIR + interpreter
  + the ownership borrow rule; Stages B/C pending, see Progress)
- **Date:** 2026-08-19
- **Context:** ADR-0009 landed the owned, growable `String` but deliberately
  left one gap open: **there is no way to obtain a `Str` view into a live
  `String`.** `slice` *copies out* a new owned `String`, `write_string` borrows
  the header for the duration of one effect call, and nothing aliases — so
  `String`→`Str` borrowing "stays deferred (Q-0012)." That deferral is now the
  most-felt ergonomic wall in the runnable core, and dogfooding the ADR-0004…0009
  stdlib growth surfaced it concretely:

  - The `std::str` module's search/trim/case algorithms are written over
    **`in Str`** (`find`, `trim`, `starts_with`, …), because that is the borrowed,
    `Copy`, unrestricted string type. But `to_upper`/`to_lower`/`to_string`
    *produce* an owned **`String`** (they allocate), and there is **no view** to
    hand that result back into a `Str`-consuming function. A caller cannot write
    `contains(to_upper(line), "ERROR")` — the `String` result and the `in Str`
    parameter do not meet. Every such composition is blocked, and every spec that
    inspects a `String` result must compare against `std::string::from_str("…")`
    rather than a bare `Str` literal (see
    `crates/tuo-stdlib/src/std/str.tuo`).
  - The same wall stands in front of `parse_int(in Str)`: a `String` you built
    (a `read_line` result, a `concat`) cannot be parsed without a redundant
    round-trip through the `String` byte builtins.

  The representation groundwork is **already in place** — this ADR adds a
  soundness rule, not a layout:

  - [`specification/abi.md`](../abi.md) §Slices specifies `Str` as the
    `{ptr, len}` fat pointer and states plainly that "the representation is
    already the general fat pointer so that later work needs no layout change."
    A `String`'s `{ptr, len, cap}` header already carries a `ptr`/`len` prefix
    that *is* a valid `Str` value pointing at the same bytes.
  - [`specification/ownership.md`](../ownership.md) §13 ("`Str` and view types")
    already frames the exact problem: `Str` is a borrowed view "which a
    lifetime-free model must keep from dangling," resolved in v0 only "by
    construction" because "**every `Str` originates from a string literal**."
    The moment a `Str` may originate from a `String`, that by-construction
    argument no longer holds, and the model needs a real rule.

  **Why this is subtle, and why it is an ADR rather than a builtin.** Unlike a
  string literal (static storage, outlives everything), a `String` is an owned
  heap value that can be **moved, reassigned, grown, or dropped**. A `Str` view
  into it can dangle two ways: (1) the `String` is **dropped** (its buffer is
  freed) while the view is still held; (2) the `String` is **mutated**
  (`push_byte`/`append`, which may reallocate through the doubling-growth path)
  while the view is held, leaving the view pointing at a freed old buffer. A
  lifetime-free ownership model has no user-written `'a` to express "this view
  may not outlive that `String`," so the rule must come from somewhere the model
  already has: the **borrow discipline** ADR-0003 established.

- **Decision (proposed):** land a **restricted, borrow-scoped `String`→`Str`
  view** — the minimal sound rule that unblocks the stdlib composition above —
  as a single language-provided builtin, reusing the existing `in`/`mut`
  borrow machinery rather than introducing lifetimes.

  1. **The view builtin.** `std::string` gains one operation:

     | Signature | Meaning |
     |-----------|---------|
     | `fn as_str(in s: String) -> Str` | A borrowed `Str` view of `s`'s bytes — the `{ptr, len}` prefix of `s`'s header, **no copy**. The returned `Str` is valid only within the borrow of `s` from which it was derived. |

     `as_str` is **pure** and **never traps** (the empty string yields a
     zero-length `Str`, which is well-formed). It is the *single obvious* way to
     view a `String` as a `Str`; there is deliberately no second `borrow`/`view`
     spelling and no slicing-view variant (a viewing `slice` is out of scope,
     below).

  2. **The soundness rule — a view does not outlive its borrow.** The value
     returned by `as_str(in s)` is **derived from the borrow of `s`**, and the
     borrow checker treats it exactly as it already treats a place borrowed
     `in`: for as long as any `Str` derived from `s` is **live** (reachable and
     used later), `s` is considered **shared-borrowed**, which means within that
     region `s` may **not** be:

     - **moved** (passed to a `take` parameter, reassigned away, returned),
     - **mutably borrowed** (`push_byte`/`append`/any `mut s` use), or
     - **dropped** (its scope may not end, nor may it be overwritten).

     The rule is *"a `Str` derived from a `String` is a shared borrow of that
     `String`, live for as long as the derived `Str` is."* This is a **new
     ownership concept** — the v0 model's standing invariant is that "a borrow
     lives exactly as long as the call that takes it" (`ownership.md`; every
     borrow to date is call-scoped, and `Str` is `Copy` with no borrow link), so
     a `Str` that *outlives* its call is the first **value-provenance borrow** the
     checker must track. Concretely: the checker records that the binding holding
     an `as_str` result **borrows-from** the argument's root place, and while that
     view binding is live the `String` root is treated as shared-borrowed. A move
     of it is then the existing use-of-moved family (`O0001`/`O0002`); a `mut`
     borrow of it is a borrow conflict (`O0005`); and a scope-end drop while a
     view is live is refused. One **new diagnostic code, appended** —
     `O0011` ("a view of this `String` is still in use") — names the view site and
     the conflicting move/mutate/drop site, because no existing O-code means
     "a derived view outlives its source" (the trap taxonomy and the `Oxxxx` set
     are append-only).

  3. **Returning a view is refused, by the same rule.** A function may **not**
     return a `Str` derived from a `String` it owns or borrows — the borrow would
     outlive the callee's frame. `fn f(in s: String) -> Str { as_str(s) }` is
     rejected (the returned view outlives the `in s` borrow) with the new
     escaping-view error `O0011`. This is the precise analogue of Rust's "cannot
     return reference to local," expressed through the borrow scope rather than a
     named lifetime. A view is a **within-expression / within-block** convenience;
     to carry string data across a frame boundary you still pass or return the
     owned `String` (or copy with `slice`). This keeps the feature sound without
     any lifetime-inference machinery.

  The load-bearing rules:

  - **No layout change, no ABI bump.** `as_str` reads the `{ptr, len}` prefix of
    the `String` header and yields the existing `Str` fat pointer
    (`abi.md` §Slices). Both are already specified; `abi::ABI_VERSION` does not
    move. The builtin lowers to a pair of field reads (a `Rvalue::HeapOp` view
    variant that borrows the subject **place**, exactly as `Len`/`byte_at` already
    do), never an allocation and never a copy.
  - **`Str` stays `Copy`, but a `String`-derived `Str` is borrow-scoped.** The
    *type* `Str` is unchanged and remains `Copy` (ownership.md §13). The
    restriction is not on the type but on the **provenance**: a `Str` value whose
    provenance is a `String` borrow participates in that borrow's region. A `Str`
    from a literal borrows nothing and is unrestricted, exactly as today. The
    checker already tracks borrow provenance for `in`/`mut` place borrows; this
    extends that tracking to the value `as_str` returns.
  - **The interpreter already shares the buffer.** `Value::Str` and the
    `String`'s buffer are the *same* byte representation in `tuo-mir-interp`
    (ADR-0009 noted "`Value::Str` is already a byte buffer that `Str` and
    `String` share"), so `as_str` in the reference interpreter is a zero-copy
    projection — and, critically, the interpreter must **honor the borrow**: a
    mutate-through-view or use-after-drop that the checker was meant to reject can
    never reach the interpreter (the front end refuses it), so no
    `TrapKind::Internal` path is added. No new trap code.
  - **Purity is unchanged.** `as_str` is pure (it is a read), so specs may build a
    `String` and view it freely. This is what lets `to_upper`/`to_lower`/
    `to_string` specs finally compare against a `Str` literal
    (`as_str(to_upper("Hi!")) == "HI!"`) instead of `from_str`, and lets the
    stdlib compose `String` producers into `Str` consumers.

  **Deliberately out of scope** (each stays refused or a plain type error, never
  a silent half-feature):

  - **A viewing `slice`** — `fn slice_view(in s: String, a, b) -> Str`. The
    borrow rule extends to it unchanged, but it is a second spelling of the same
    idea; ADR-0009's copying `slice` stays, and `as_str` + the existing `Str`
    `slice` (`std::str::slice(as_str(s), a, b)`) already expresses a viewed
    sub-slice. Adding `slice_view` later is additive.
  - **Views into `Array[T]`** (an `[T]` slice view of a growable array). The
    `[T]` fat-pointer layout is reserved in `abi.md` but uninhabited; a growable
    `Array` view rides the *same* soundness rule and is a natural follow-up, but
    it needs the slice type inhabited first — a separate increment.
  - **Storing a view in a struct field or returning it across a frame** — refused
    by rule 3. Named lifetimes / view-carrying data structures remain future work;
    v0 keeps views within a borrow region.
  - **Mutable views** (`&mut`-style aliasing into a `String`). Only shared (`in`)
    views land; a mutable view would need the exclusive-borrow machinery to also
    track derived places, which is a larger step.

- **Consequences:**
  - *Easier:* the stdlib's `String` producers and `Str` consumers finally
    compose — `contains(as_str(to_upper(line)), "ERROR")`,
    `parse_int(as_str(built))`, and `to_upper("Hi!") == "HI!"` (via `as_str`)
    all become expressible. The ergonomic wall this session's `std::str` growth
    hit is removed; the specs that compare against `std::string::from_str("…")`
    can compare against `Str` literals through `as_str`, and the note that "you
    can't feed a computed `String` back into a `Str`-consuming function" is
    retired.
  - *Harder:* the borrow checker gains its **first value-provenance borrow** —
    until now every borrow was of a *named place* (`in x`/`mut x`); a `Str`
    returned by `as_str` is a borrow that flows through an expression. The
    data-flow that already computes place-borrow liveness must now also mark the
    returned value as borrow-derived and propagate its liveness. This is real
    checker work (and is why it is an ADR); it reuses the existing use-of-moved
    and borrow-conflict diagnostics (`O0001`/`O0002`/`O0005`) for the
    move/mutate-while-viewed cases and adds one appended code, `O0011`, for the
    escaping-view case — rather than inventing a lifetime system.
  - *Trade-off:* the within-borrow-region restriction (rule 3) is deliberately
    conservative — it forbids returning or storing a `String`-derived view, which
    a full lifetime system would permit. The conservative rule is **sound with no
    lifetime inference** and **additive**: a later ADR can relax it (named view
    lifetimes, view-carrying structs) without changing the meaning of code that
    compiles today. Honest and narrow beats a broken general feature.

- **Staging (spec-before-code, per the ADR-0004/0006/0009 precedent):**
  - **Stage A** — this normative text; the front end (`as_str` resolves as a
    `std::string` symbol; the type rule `in String -> Str`; the borrow-provenance
    rule and its `O0001`/`O0002`/`O0005`/`O0011` refusals); MIR (the view `HeapOp`
    borrowing a place, verifier unchanged or a single `M00xx` if the view needs
    its own well-formedness invariant); and the reference interpreter (zero-copy
    projection). Pinned by `tests/types/fixtures/{ok,err}/string_view.tuo`,
    `crates/tuo-ownership/tests/` borrow-conflict fixtures (move/mutate/return a
    live view → the right `Oxxxx`), and `crates/tuo-mir-interp/tests/conformance.rs`.
  - **Stage B** — native lowering on both backends: `as_str` is two field reads
    yielding the `Str` fat pointer, matching the interpreter instruction for
    instruction, pinned by a `str_view` fixture agreeing interpreter == Cranelift
    == LLVM in `crates/tuo-cli/tests/codegen_three_way.rs`. No allocation, no ABI
    change.
  - **Stage C** — the stdlib payoff: `std::str`/`std::string` gain the composition
    this unblocks (and the `to_upper`/`to_lower`/`to_string` specs switch to
    `as_str(…) == "<literal>"`), pinned by `crates/tuo-cli/tests/stdlib.rs`; a
    dogfood example composes a `String` producer into a `Str` consumer natively.
    At that point this ADR can be accepted.

- **Benchmark consideration:** this ADR unblocks no *new* performance-lab
  workload — it adds no runtime capability the lab measures (no I/O, no new
  allocation pattern; a view is strictly cheaper than the copy it replaces). So,
  unlike ADR-0004/0006/0009, its acceptance is **not** gated on flipping a lab
  entry. Instead its acceptance is gated on the **static** promise: the
  borrow-conflict fixtures must prove the checker refuses every dangling-view
  shape (move-while-viewed, mutate-while-viewed, return-a-view), and the
  three-way differential must prove the accepted views compute identically on the
  interpreter and both backends. Where it *does* touch the lab: the compiler
  lab's cold/incremental figures should show `as_str`-using programs re-check
  through the existing per-function queries with no new whole-program pass — a
  measurement, not a promise. This ADR is not "accepted" until Stages A–C land
  with those fixtures committed.

- **Open sub-questions (to resolve during Stage A, recorded so they are not
  silently decided):**
  - Whether a `Str` derived from a `mut`-borrowed `String` is allowed at all in
    v0 (the safe answer: `as_str` takes `in String`, so the source is always a
    shared borrow, and mixing a view with a concurrent `mut` on the same
    `String` is the `O0005` conflict — no special case needed).
  - Whether the view's liveness is computed with the existing per-function borrow
    data-flow unchanged, or needs a small extension to track value-carried
    borrows through `let`-bindings (expected: the latter, a bounded extension).
  - The exact diagnostic wording for "a view of this `String` is still in use
    here," so the error names the *view* site and the *conflict* site the way the
    other borrow errors do.

- **Progress — Stage A (2026-08-19):** the front end, MIR, interpreter, and the
  ownership borrow rule landed; the sub-questions above resolved as anticipated:
  - *Builtin + types:* `std::string::as_str(in String) -> Str` resolves as a real
    `std::string` symbol (`tuo-resolve`, `Builtin::StringAsStr`, pinned by
    `crates/tuo-resolve/tests/resolve.rs`), and type-checks `in String -> Str`
    (`tuo-types::builtin_signature`, pinned by `tests/types/fixtures/{ok,err}/
    allocator.tuo` — a `Str` argument or a `String` result is `T0001`). It is
    **pure** (not in `Builtin::is_effect`), so specs use it freely.
  - *MIR + interpreter:* lowered as `HeapOp::StringAsStr` — a subject-reading,
    zero-operand op producing `Str` (`tuo-mir`, verifier `M0012` result type
    `Str`, `specification/mir.md` §5.7). The reference interpreter models the view
    as a byte copy of the subject (sound because the checker pins the source),
    pinned by `crates/tuo-mir-interp/tests/conformance.rs`
    (`string_as_str_views_the_whole_string`).
  - *Ownership — the value-provenance borrow:* the checker gained its first borrow
    whose source is a **value**. A binding initialized exactly with `as_str(s)`
    records that its root borrows `s`'s root; while that view binding is live, `s`
    is shared-borrowed. Moving `s` (`O0001`/`O0002`), `mut`-borrowing it, dropping
    it at scope end, or overwriting it → **`O0011`** (new, appended); a view in
    return/tail/`break` position → **`O0011`** (escaping view). An **inline**
    `as_str(s)` borrows only for its expression, so a later mutation is fine. The
    rule is deliberately conservative (a bound view keeps `s` borrowed for its
    whole lexical extent, not to last-use). Pinned by `tests/ownership/fixtures/
    {ok,err}/string_view.tuo` and documented in `ownership.md` §13/§15 and
    `abi.md`.
  - *Deferred to Stage B/C, unchanged:* native lowering (a true zero-copy
    `{ptr, len}` view on both backends, three-way differential), and the stdlib
    payoff (the `std::str` `to_upper`/`to_lower`/`to_string` specs switching to
    `as_str(…) == "<literal>"`, plus a dogfood example composing a `String`
    producer into a `Str` consumer natively). No ABI version bump (the `Str`
    fat-pointer layout is unchanged).
