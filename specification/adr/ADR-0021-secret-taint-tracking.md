# ADR-0021: Secret taint tracking — marking data, not just functions

- **Status:** proposed (2026-09-05)
- **Date:** 2026-09-05
- **Context:** ADR-0020 shipped `#[constant_time]`, which the compiler verifies
  by refusing every data-dependent construct in a marked function's body:
  branches, bounds-checked indexing, trapping arithmetic, and calls to unmarked
  functions (`T0017`–`T0021`). The primitives it guards are pinned twice over —
  by that checker, and by disassembling the emitted code on both backends and,
  since the cross-target work, on x86-64 as well as ARM64.

  That is a real guarantee and it has a specific hole, recorded in ADR-0020's
  own outstanding list: **the attribute marks functions, not data.** The
  compiler verifies *how* a marked function computes; it says nothing about
  *what flows into it*. So this compiles clean today:

  ```tuo
  let key = load_secret();
  if key_len(key) > 32 { … }        // an ordinary branch on a secret
  let tag = compute_tag(key);        // an unmarked function, unchecked
  if tag == expected { … }           // == on a MAC tag: the exact leak
  ```

  Nothing here is marked, so nothing is checked. `std::crypto::verify` exists
  precisely so the last line has a safe spelling, but the language cannot
  *require* its use, because it does not know `tag` is a secret. The library
  can offer the safe primitive; only the type system can insist on it.

  This is the gap between "a constant-time subset exists" and "secrets are
  handled in constant time", and it is the difference between a library a
  careful author can use correctly and a language that catches the careless
  case. The audience this project is built for — coding agents and newcomers —
  is exactly the audience that will not know which comparison is which.

- **The question this ADR must answer:** what is the smallest addition that
  makes the compiler refuse the code above, without making ordinary programs
  harder to write?

- **Decision:** *Not yet taken.* This ADR is opened to record the design space
  and the constraints, per the project's rule that a gap discovered by
  dogfooding becomes an ADR rather than an ad-hoc feature. It should not be
  implemented until the questions in "What must be settled first" are answered.

## The design space

Four shapes, in increasing order of cost and of what they buy.

### 1. A `Secret[T]` wrapper type

A library-level newtype whose contents can only be reached through
constant-time operations. `Secret[Int]` has no `==`, no `<`, and cannot be
branched on; `std::ct` gains `Secret`-taking variants.

*Buys:* the common mistakes, with no new language machinery — this is
expressible in v0 today if generics over a nominal type land.
*Costs:* every unwrap is a hole, and there must be unwraps (a MAC tag
eventually gets written to a socket). Discipline lives in whoever writes the
unwrap, which is where it already lives. It documents intent more than it
enforces it.

### 2. Taint inference with no annotations

The checker infers taint from a small set of sources (`std::crypto::random_byte`,
anything derived from a function parameter marked secret) and propagates it
through assignment, arithmetic, and calls, refusing a data-dependent construct
on a tainted value.

*Buys:* catches the careless case with no burden on ordinary code.
*Costs:* whole-program analysis with no annotation boundary. Every call has to
be analysed in context or conservatively assumed tainted, which either loses
precision at module boundaries or requires an interprocedural summary the
compiler does not have today. Error messages get worse the further the source
is from the violation.

### 3. Annotated taint with inferred propagation

Sources are marked (`#[secret] fn load_key() -> Str`), propagation is inferred
within a function, and function *signatures* carry the taint of their
parameters and result — so a summary is written down rather than inferred
across the whole program.

*Buys:* most of what (2) buys, with a checkable boundary at each function and
error messages that can name the source.
*Costs:* a real addition to the type system — taint becomes part of a
signature, so it is part of the module interface, which touches resolution,
typing, the incremental query graph's `module interface` seam, and the agent
protocol's rendered signatures. Not a checker pass.

### 4. Full information-flow types

Taint as a lattice with declassification, the literature's answer (Jif, FlowCaml).

*Buys:* soundness, including implicit flows.
*Costs:* out of proportion to v0. It would be the largest single feature in the
language, and the fraction of tuonelang programs handling secrets does not
justify it.

**Provisional lean: (3), with (1) as a stepping stone that can ship first and
is useful regardless.** But this should not be decided from the armchair — see
below.

## What must be settled first

1. **Is there a real program that gets this wrong?** ADR-0020's own adoption
   note records that `std::crypto` shipped with *no* comparison at all, so
   callers wrote their own — the exact failure this would catch. That is
   evidence, but it is one instance and it has since been fixed by adding
   `verify`. Before adding a type-system feature, the dogfooding corpus should
   be checked for whether the mistake recurs when the safe primitive exists.

2. **Does the attribute already cover the cases that matter?** Every function
   in `std::crypto` that touches a secret could carry `#[constant_time]`
   instead. That fails today — the checker refuses loops and indexing, which
   HMAC and PBKDF2 need — so answering this means knowing whether the checker
   can be extended to accept *public-length* loops without accepting
   data-dependent ones. That is a smaller and more concrete question than taint,
   and it may dissolve most of the need.

3. **What is the interaction with `#[constant_time]`?** Two marking systems
   that do not compose would be worse than either alone. A tainted value passed
   to a marked function should presumably be fine — that is what marked
   functions are *for* — which suggests the two are complementary rather than
   overlapping, but it needs stating precisely.

4. **What does it cost an ordinary program?** The rule from ADR-0020 holds: a
   gate that rejects working code is acceptable only when the code was
   genuinely improvable. A taint system that makes non-cryptographic programs
   harder to write has failed, however sound it is.

## Consequences (if implemented)

- **A benchmark plan is required**, per the project rule. Taint is a
  compile-time property with no runtime cost, so the benchmark is a *compiler*
  one: the added checking must not measurably slow `tuo check`, and the
  incremental scenarios must not lose their early cutoff — a taint summary in
  the module interface would invalidate callers on a change that currently
  does not.
- **The corpus needs a category.** ADR-0020's checker got dedicated tests
  (`T0017`–`T0021`); taint would need the same, plus corpus entries in the
  `type-error` category proving each rejection is real.
- **The honesty rule carries over.** Whatever ships must state precisely what
  it does not cover. Implicit flows, timing through the cache, and the hardware
  question ADR-0020 already disclaims all remain out of reach, and a taint
  system is likely to *feel* more complete than it is.

## What this ADR does not propose

Deferring is a decision, and this is one: **shipping nothing is currently the
right answer.** `#[constant_time]` plus `std::crypto::verify` covers the
primitives that exist, both are verified twice over, and question (2) above may
show the remaining gap is better closed by extending the checker than by adding
a second marking system. This ADR exists so that conclusion is reached
deliberately rather than by the gap being forgotten.
