# ADR-0040: Array literals for growable arrays — a literal takes its array type from context

- **Status:** proposed
- **Date:** 2026-10-19

## Context

`[1, 2, 3]` always has the type `[_; 3]`, a fixed array. A growable
`Array[T]` has no literal. It is built from `std::array::empty()` plus one
`std::array::push` call per element. ADR-0009 introduced the growable array
without a literal because only `Array[Int]` existed then. ADR-0012 widened the
element types, and ADR-0030 added `filled`, but nobody revisited the literal.
Every finding below was probed with the binary built from `e38e070`.

### Finding 1: the literal is refused wherever a growable array is expected

```tuo
fn sum(in xs: Array[Int]) -> Int { ... }

let xs: Array[Int] = [1, 2, 3];   // error[T0001]: expected `Array[I64]`, found `[_; 3]`
sum([4, 5])                       // error[T0001]: expected `Array[I64]`, found `[_; 2]`
```

The diagnostic is correct, but it carries no help and no suggestion, and the
language has no single expression the user could write instead.

### Finding 2: it is the most common recognisable type error in the baseline

`T0001` is the most frequent first-turn error in the primed core runs
(`benchmarks/llm/codegen/results/README.md`). Splitting those `T0001`s by
message, `expected Array[…], found [_; N]` reached the repair prompt in this
many runs, out of 55 tasks per run:

| Run | First turn failed | Literal for a growable array | Spec compares to an array literal |
|---|---|---|---|
| qwen3:4b before, primed | 33 | 4 | 2 |
| qwen3:4b after, primed | 22 | 2 | 1 |
| gemma4 before, primed | 38 | 7 | 7 |
| gemma4 after, primed | 32 | 4 | 4 |
| qwen3:4b before, unprimed | 48 | 1 | 0 |
| gemma4 before, unprimed | 51 | 3 | 0 |
| gemma4 after, unprimed | 51 | 3 | 0 |

It recurs in five tasks (`digits-reversed`, `evens-up-to`, `grade-report`,
`prefix-sums`, `range-sum`), all of them tasks whose inputs or outputs are
short lists. The last column counts first attempts that wrote
`then f(…) == [0, 2, 4]`. That is a second, separate gap (specs compare
scalars only), but it can be closed only once the literal has the right
type.

### Finding 3: the code base pays for it in thousands of lines

A count of runs of two or more consecutive `std::array::push(xs, <literal>)`
lines in committed `.tuo` sources finds 69 runs, **2,590 lines** in total:

| File | Literal `push` lines |
|---|---|
| `crates/tuo-stdlib/src/std/tls.tuo` | 1,450 |
| `crates/tuo-stdlib/src/std/der.tuo` | 355 |
| `crates/tuo-stdlib/src/std/chacha.tuo` | 264 |
| `crates/tuo-stdlib/src/std/sha512.tuo` | 179 |
| `crates/tuo-stdlib/src/std/hkdf.tuo` | 119 |
| `crates/tuo-stdlib/src/std/crypto.tuo` (and its two example copies) | 36 × 3 |
| `benchmarks/runtime/programs/tuo/sha256-hash.tuo` | 32 |
| everything else | 83 |

Most of these are published test vectors and round constants, written one
element per line. They are the specs that ADR-0019 and ADR-0036 call the
catalog's strongest, and they are the hardest to check against the RFC
by eye.

### Finding 4: the library ships workarounds, twice

`std::collections::of2`/`of3` and `std::ct::of2`/`of3` exist only to build two-
and three-element arrays. The brief lists all four. That breaks the library's
own rule, one obvious API per task, in two ways: they duplicate each other,
and they stand in for a literal.

## Options

- **(a) The literal takes its type from context.** `[e₁, …, eₙ]` is either
  `[T; n]` or `Array[T]`, and unification decides which. A literal that
  nothing constrains stays a fixed array, so every existing program keeps its
  meaning.
- **(b) A conversion builtin**, `std::array::from([1, 2, 3])`. Nothing in
  inference changes. But it is two constructs where one is expected, and it
  needs a function that is generic over the length `N`, which the language does
  not have.
- **(c) A separate spelling**, such as `Array[1, 2, 3]` or a `vec!`-style form.
  A second literal syntax breaks the one-spelling rule, and every model's prior
  writes `[1, 2, 3]` anyway.
- **(d) Status quo, plus a fix on `T0001`.** The only fix the compiler could
  offer is a sequence of `push` statements, which is not an expression and
  cannot replace one.

## Decision (proposed)

**Option (a).** It is the rule the language already applies to the literal
nearest to it. An integer literal is an inference variable of class `Integer`.
It becomes whichever integer type unification asks for, and defaults to `I64`
when nothing asks (`tuo-types/src/infer.rs:16`). An array literal becomes an
inference variable of a new class, **`ArrayLiteral { elem, len }`**:

- unified with `[U; m]`: requires `m == len`, unifies `elem` with `U`, and
  resolves to the fixed array;
- unified with `Array[U]`: unifies `elem` with `U`, and resolves to the
  growable array;
- unified with anything else: `T0001`, as today, with the expected type printed
  as the type that was asked for;
- unconstrained at the end of the body: **defaults to `[elem; len]`**.

The repeat form `[v; N]` stays fixed-only. `std::array::filled(n, v)` is
already the one spelling for a growable array of `n` copies, and admitting
both would give that task two spellings.

### Open questions (settled in Stage A)

1. **`[]` as an empty growable array.** The uniform rule admits it, which
   gives `std::array::empty()` a second spelling. The recommendation is to
   admit it, because a rule that depends on the element count is a special
   case. The brief then teaches `[]` with an annotation, and `empty()` where
   nothing provides one.
2. **Two literals of different lengths that meet**, as in
   `if c { [1, 2] } else { [1, 2, 3] }`. The conservative rule is a `T0001`,
   as today, even though `Array[Int]` would satisfy both arms. Inferring the
   growable type from a length mismatch would make the answer depend on
   unification order. The recommendation is the conservative rule.

### Lowering

A literal typed `Array[T]` lowers to the operations `std::array::empty` and
`push` already lower to, with the capacity reserved up front for the known
length. No MIR construct, verifier rule, backend, runtime function, or ABI
layout changes. A long literal of constants, such as a 1,000-byte test vector,
would lower to 1,000 pushes. Lowering it as a read-only static plus one copy,
the way `Str` literals are emitted, is a Stage C optimization that waits for
the compile-time measurement below.

### Ownership

The literal is a fresh owned value, as the result of `empty()` is. Its elements
are moved in, so a non-`Copy` element (`[s, t]` with `s: String`) consumes `s`
and `t`, exactly as two `push` calls would.

## Staging

**Stage A: the rule.** Add the inference class, the default, and the lowering.
Update `static-semantics.md`'s literal section, and the brief, which shows a
growable literal and drops the `push`-sequence idiom. Add three-way fixtures:
a literal passed to an `in Array[Int]` parameter, an annotated `let`, a
returned literal, an `Array[String]` literal (which pins element moves and
drop glue), an `Array[Point]` literal, and an unconstrained literal that stays
fixed. Add type-checker fixtures for the length-mismatch rule and the
`[]` decision. Gate: every committed `.tuo` keeps its verdict and exit byte,
and a re-run of the inference audit ADR-0035 used shows no literal's type
flipping.

**Stage B: the payoff.** Rewrite the 2,590 `push` lines into literals, with
the test vectors laid out as their RFCs print them. Delete `of2`/`of3` from
both modules, or deprecate them through the brief if a release has already
published them. The oracle is the existing specs: every published-vector spec
in `std::crypto`, `std::tls` and the rest must stay green, and the natively
pinned SCRAM, TLS and SHA-256 cross-checks must keep passing.

**Stage C (measured): static data for constant literals.** Land it only if
benchmark item 3 shows that the push lowering costs measurable compile time or
code size on the Stage B stdlib.

## Benchmark plan

The predictions below are fixed before implementation.

1. **LLM rescore.** Rescore the seven committed core runs with the Stage A
   compiler through `tuo bench report`. The outputs are fixed, so any delta
   comes from the rule alone. Prediction: every first turn whose only error was
   the literal `T0001` now checks. That is at least the 2–7 per run counted
   above, minus the turns that also compare arrays with `==` in a spec, which
   this ADR does not fix. Then take one fresh primed run of each model with the
   updated brief. Acceptance: no task gets worse.
2. **Source size.** Record the line count of the Stage B stdlib against
   today's. This is a readability claim rather than a performance claim, so
   the number is reported as measured, with no target.
3. **Compile time and code size.** `tuo build --release` of a program that
   names `std::tls`, and `tuo verify` of `tls.tuo`, before and after Stage B
   (fastest of five). The push lowering emits the same calls the hand-written
   pushes did, so the prediction is no change. A regression would be the
   trigger for Stage C.
4. **Three-way agreement** on the Stage A fixtures, including drop order for
   the `Array[String]` literal.

## Consequences

- `[1, 2, 3]` works wherever an `Array[Int]` is expected, which is what every
  reader and every model already writes.
- Thousands of lines of test vectors become one literal each, laid out as
  their RFCs print them, so they can be checked by eye.
- `of2`/`of3` and the `push`-sequence idiom leave the brief.
- The literal's type now depends on its context. That is already true of
  `0`, and the default keeps an unconstrained literal's meaning unchanged.
- It prepares a structural `==` for specs (roadmap P2), which would let a spec
  say `then evens_up_to(5) == [0, 2, 4]`. This ADR does not propose that.

## What this ADR does not propose

- **Structural `==` on arrays**, in specs or elsewhere.
- **Map literals.** `Map[K, V]` keeps `empty` and `insert`.
- **A growable repeat form** `[v; n]`. `filled` remains the spelling.
- **Implicit conversion** between `[T; N]` and `Array[T]`. A *value* of one
  type never becomes the other. Only an unresolved literal chooses.
- **Slices or a common supertype** of the two array kinds.
