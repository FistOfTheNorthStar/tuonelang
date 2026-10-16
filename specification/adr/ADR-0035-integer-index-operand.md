# ADR-0035: The index operand — any integer type, bounds-checked, never cast

- **Status:** proposed
- **Date:** 2026-10-14

## Context

ADR-0030 made `xs[i] = v` an assignment and left one item explicitly out of
scope: *"Indexing takes a `Usize`, so a loop counted in `Int` needs
`i as Usize` at each index. That is a usability question about indexing in
general."* This ADR takes it up. Every finding below was probed with the real
binary built from `fa3e16f`. The snippets were run, and none of them is
proposed syntax.

### Finding 1: `Usize` is required by one operator and produced by nothing

| Surface | Index or length type |
|---|---|
| `xs[i]`, `xs[i] = v` on `[T; N]` and `Array[T]` | **`Usize`** (`crates/tuo-types/src/check.rs:2213`) |
| `std::array::get(xs, i)`, `set(xs, i, v)`, `filled(n, v)` | `Int` (`check.rs:2910`–`2934`) |
| `std::array::len`, `std::str::len`, `std::string::len`, `std::map::len` | return `Int` |
| `std::str::byte_at`/`slice`, `std::string::byte_at`/`slice` | `Int` (`check.rs:376`–`386`) |
| `for x in xs` | the element; there is no cursor |
| `for i in a..b` | the type of the bounds |

`static-semantics.md:161`, `abi.md:73`, and `tuo-types/src/ty.rs:20` all
call `Usize` "the index/length type". No length in the language has that
type. The compiler works around the split itself. MIR keeps two lengths, the
`Usize` `Rvalue::Len` and the `I64` `ArrayLen` (`tuo-mir/src/mir.rs:765`). The
`for` lowering also casts its `Usize` cursor back to `I64` before it calls
`array_get` (`tuo-mir/src/lower.rs:2549`).

### Finding 2: the friction appears wherever the operand meets an `Int`

A cursor that only literals constrain is inferred as `Usize`, so it works:

```tuo
var i = 0;
while i < 4 { acc = acc + xs[i]; i = i + 1; }   // checks; `i: Usize`
```

The idiomatic bound comes from `len`, which returns `Int`, and that is
enough to break the loop:

```tuo
while i < std::array::len(xs) { acc = acc + xs[i]; i = i + 1; }
```
```
error[T0001]: mismatched types
7 |         acc = acc + xs[i];
  |                        ^ expected `Usize`, found `I64`
```

`for i in 0..std::array::len(xs) { … xs[i] … }` gets the same error. So does a
`take i: Int` parameter, a write `xs[i] = 1`, and an `I32` or `U8` index. One
cursor cannot serve both of the language's spellings of the same read either:
`xs[i] + std::array::get(xs, i)` is a `T0001` at whichever use comes second.
The machine diagnostic carries `"suggestions": []`, which means that neither
`apply_safe_fix` nor the LSP offers a fix.

### Finding 3: the casts arrived with ADR-0030

| Location | `as Usize` | Before `269c928` (ADR-0030) |
|---|---|---|
| `crates/tuo-stdlib/src/std/crypto.tuo` | 43 | 1 |
| `benchmarks/runtime/programs/tuo/sha256-hash.tuo` | 10 | 0 |
| `examples/router/src/main.tuo` | 1 | — |
| `tests/` fixtures (5 files) | 18 | — |
| `corpus/` | 0 | — |

`examples/postgres-{auth,client}/src/std_crypto.tuo` are byte-identical copies
of `crypto.tuo`, so they add 86 more occurrences of the same casts. Some casts
are defensive and change nothing, as in `state[0 as Usize]`, where the literal
alone would have checked. Where the cast is easy to avoid, the code base
avoids `xs[i]` altogether. The stdlib sources call `std::array::get(` 346
times, against about 45 index expressions (a regex count). `tools/py2tuo`
translates every Python subscript into `std::array::get`
(`tools/py2tuo/src/py2tuo/compiler.py:1568`). ADR-0030 claimed `xs[i]` "becomes
the obvious spelling", and that claim holds only for code that never asks an
array for its length. The brief also teaches the workaround:
`tuo cheatsheet` says *"Indexing takes a `Usize`: `xs[i as Usize]`"*
(`crates/tuo-cli/src/cheatsheet.rs:367`).

### Finding 4: the cast is inert, and Int-index semantics already ship

An `as` cast between integer types wraps. A negative `Int` therefore becomes a
`Usize` of at least 2⁶³, which is larger than any length, so a negative index
traps `IndexOutOfBounds` on every engine:

```tuo
fn pick(in xs: [Int; 4], take i: Int) -> Int { xs[i as Usize] }
// pick([1, 2, 3, 4], 0 - 1): spec, `tuo run`, `tuo run --release` all
// trap `index out of bounds` (native exit 134)
```

`std::array::get` with an `Int` index already has the semantics this ADR
proposes for `xs[i]`. A negative index traps `IndexOutOfBounds` in the
interpreter (`index -1 out of bounds for an array of length 2`) and in both
backends. The Cranelift backend does it with a single unsigned compare
(`tuo-codegen-cranelift/src/lower.rs:3437`), which is the same `Lt` that the
index projection's `Assert` emits after the cast. No fixture pins the negative
case, though: `arr_trap_oob` and `str_trap_oob` both index past the end.

One subtlety of today's rule is that a cursor inferred as `Usize` traps
*earlier*. `var i = 0; xs[i - 1]` traps `integer_overflow: Usize arithmetic
overflowed`, not `index_out_of_bounds`.

### Finding 5: no LLM evidence either way, yet

The recorded runs under `benchmarks/llm/codegen/results/` cover two tasks
(`double`, `min`). Neither indexes, and none of the runs contains the string
`Usize`. Neither `training/breaks.py` nor the brief's anti-pattern table has an
indexing row, so the advice exists only in the "further traps" prose. The case
therefore rests on Findings 1–4, and the LLM measurement belongs in the
benchmark plan rather than in the premise.

## Options

- **(a) The index is `Int`.** This matches every builtin. It breaks every
  `as Usize` above and `collections.tuo`'s `take i: Usize`, and it gains no
  semantics. A byte-valued table lookup (`table[b]`, `b: U8`) would still need
  a cast.
- **(b) The index is any integer type.** The operand is range-checked rather
  than unified. All existing code still checks, and `Int`, `Usize`, and `U8`
  cursors all index without a cast.
- **(c) Implicit widening coercion.** This is ruled out by the frozen §3.3 of
  `static-semantics.md` ("no implicit conversions"). `Int` → `Usize` is not a
  widening anyway, since it is a change of sign. It would also turn every call
  argument into a possible coercion site, not only the index.
- **(d) Status quo, plus a machine-applicable `as Usize` fix on `T0001`.**
  This is cheap, but Compile@1 still fails, each fix still costs a repair turn,
  and the 72 authored casts stay noise that no reader needs.

## Decision (proposed)

**Option (b).** The index operand of `base[index]`, whether read or written
(ADR-0030), may have **any integer type**. It is checked to be an integer and
is never unified with a particular one. A literal or an otherwise unsolved
index defaults the way every integer literal does, to `I64`. The access is in
bounds exactly when `0 <= index < len` holds over the mathematical integers.
Otherwise it traps `IndexOutOfBounds`, the semantics `get` already has. A
non-integer index (`Float`, `Bool`) remains a `T0001` whose expected type is
"an integer type".

The rationale has three parts.

- **The language already has this rule for the one operand like it.**
  ADR-0019's shift amount names a position, is range-checked, and traps outside
  its range. It accepts any integer type and "is deliberately NOT `expect_ty`"
  (`check.rs:2427`). An index is the same kind of operand, and making it agree
  removes a special case rather than adding one.
- **It introduces no conversion.** No value of one type is ever observed as
  another. The bounds check is a predicate on the operand's value, so
  ADR-0026's position stands: arithmetic does not change type, and `+` still
  traps.
- **It leaves one spelling.** The spelling is always `xs[i]`, whatever the
  cursor's type. `get(xs, i)` remains, as ADR-0030 kept it, but nothing pushes
  an author toward it any more.

The builtins keep their declared `Int` parameters. A call takes its
parameters' declared types (no overloading, `static-semantics.md` §3.2), and
`get` is a function, not an operator. A
`Usize` cursor passed to `get` still needs `as Int`. Nothing in the language
produces a `Usize`, so such a cursor exists only where someone wrote one.

`Usize` stops being described as "the index/length type". It becomes an
ordinary pointer-width unsigned type, and `static-semantics.md`, `abi.md`, and
`ty.rs` are corrected to match.

### Lowering

`checked_index` (`tuo-mir/src/lower.rs:2005`) already stores the index into a
`Usize` temporary. For a non-`Usize` operand, it instead stores
`cast int_to_int(index)`. That is exactly the MIR an explicit `i as Usize`
produces today:

```
_5 = cast int_to_int(copy _2)     // today, from `xs[i as Usize]`
_7 = len(_0)
_8 = lt(copy _5, copy _7)
assert copy _8 [index_out_of_bounds]
```

The cast is exact on the supported hosts, which are 64-bit (`abi.md:49`).
Every `IntKind` is at most 64 bits, non-negative values survive unchanged, and
negative values sign-extend past any length. If a 32-bit target is ever added,
the bounds check must compare before it narrows. A user's `i as Usize` would
*truncate* silently on such a target, so it is better to have one conversion
point that the compiler owns. The change touches no MIR construct, the
verifier, either backend, the runtime, or the ABI. The constant-time rule
`T0018` (no indexing under `#[constant_time]`) is unchanged.

### Inference

A cursor that only an index constrained used to be inferred as `Usize`. It now
defaults to `Int`, so `xs[i - 1]` at `i == 0` traps `IndexOutOfBounds`
instead of `IntegerOverflow`. Both are traps with the same exit status, but the
trap kind is observable. The audit was measured, not assumed. Of 140 committed
programs that `tuo debug mir` lowers standalone (100 under `examples/`,
`tests/codegen/fixtures/`, and `corpus/`, the 23 lab programs, and 17 of the
23 stdlib modules), **no** named local is inferred as `Usize`. The only
`Usize` locals are 3 explicit annotations in the ADR-0030 fixtures
`arr_index_write` and `arr_index_write_trap_oob`.

## Staging

**Stage A: the rule (additive).** Implement the checker change and the
`checked_index` cast, then update `static-semantics.md` (the type list at lines
160–179 and the indexed-assignment paragraph at line 435). The cheat sheet's "further traps" line becomes "the
index is any integer". New three-way fixtures go in `tests/codegen/fixtures/`:
an `Int` index (read and write), a negative `Int` index that traps, `I32` and
`U8` indices, and a negative `std::array::get` index that traps (this pins the
existing behaviour Finding 4 relies on). Gate: every committed `.tuo` keeps its
`check`, spec, and exit verdict, the three differential suites stay green, and
the inference audit above is re-run with zero flips.

**Stage B: delete the casts.** Remove the casts from `crypto.tuo` (and its two
example copies), `sha256-hash.tuo`, and `router`. Delete the defensive literal
casts. The ADR-0030 fixtures keep their `Usize` annotations, which still
check and which pin that `Usize` stays a valid index. Gate: benchmark items
1–2 below.

**Stage C (optional): the reverse direction.** When a `T0001` is a `Usize`
argument to an `Int` builtin parameter, the diagnostic gains a
machine-applicable `as Int` suggestion. This is option (d) for the one case
this ADR leaves in place, and it can be landed on its own.

## Benchmark plan

The predictions below are fixed before implementation.

1. **Codegen identity.** Run `tuo debug mir --opt` on `sha256-hash.tuo` and on
   a program that drives `std::crypto`, both before and after Stage B deletes
   the casts. The output must be identical up to local numbering. In addition,
   disassemble one index on both backends (the method
   `tuo-cli/tests/constant_time.rs` uses), and the bounds-check sequence must
   be unchanged.
2. **Runtime: prediction is no change.** Re-run the Benchmarks Game speed
   table (`lab_command.rs`, `benchmarks_game_speed_table`) and the full lab
   before and after. Also rewrite `fannkuch-redux` and `spectral-norm` from
   `get`/`set` to `xs[i]`/`xs[i] = v`, which is only castless after Stage A, and
   time them against their committed forms (fastest of five, interleaved).
   Both forms should lower to one unsigned compare, so any difference is a
   finding about the index path and is recorded either way.
3. **LLM Compile@1.** Add pinned indexing tasks to
   `benchmarks/llm/codegen/tasks/`: sum an `Array[Int]` bounded by `len`,
   reverse a `var [Int; 8]` in place, and count bytes through a table indexed
   by byte value. Each task carries `index`/`get` variants. Record
   `tuo bench run` before Stage A, primed and unprimed. Then **rescore those
   same recorded outputs** with the Stage A compiler through
   `tuo bench report`. Because the outputs are fixed, any delta comes from the
   rule alone. Finally, take one fresh primed run with the updated brief. The
   recorded runs show that models do not honour variants, so outputs are
   classified by the spelling they actually use. Metrics are Check@1, Repair@1,
   repair cycles, and the count of `T0001`s that expect `Usize`. Acceptance:
   no task gets worse. If the baseline contains no `Usize` failure, that null
   is recorded, as ADR-0023's Stage C recorded its null, and the decision
   rests on Findings 1–4.
4. **Three-way agreement.** The Stage A fixtures must agree across the
   interpreter, Cranelift, and LLVM, including the trap kind.

## Consequences

- `xs[i]` checks with whatever integer the program already has, and
  ADR-0030's "obvious spelling" claim becomes true for loops bounded by `len`.
- The 72 authored casts become deletable, and the brief stops teaching one.
- A cursor that was inferred as `Usize` now defaults to `Int`, which changes
  one trap kind (see Inference). No committed program is affected.
- The index rule and the shift-amount rule become one rule ("a position
  operand is any integer, range-checked"), which is easier to state in the
  brief than the special case it replaces.

## What this ADR does not propose

- **Widening the builtins' `Int` parameters** to any integer type. Calls keep
  their declared types.
- **Removing `Usize`** or deprecating it as an index. It stays a valid index.
- **Any implicit conversion**, or any change to `as` itself. Integer `as`
  wraps silently, as Finding 4 shows, and whether it should trap or saturate
  instead is a separate question.
- **Negative indices from the end** (Python's `xs[-1]`). A negative index
  traps. `tools/py2tuo` already rejects negative subscripts.
- **Indexing `Str`, `String`, or `Map` with `[]`.** `s[0]` is `T0006` today
  and stays so, while `byte_at` and `std::map::get` remain the spellings.
- **Slices or range indexing** (`xs[a..b]`).
