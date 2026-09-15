# ADR-0027: Monomorphization — making the generics the front end already accepts run natively

- **Status:** proposed
- **Date:** 2026-09-15

## Context

tuonelang's generics are **half-implemented, and nothing in the compiler says
so.** This ADR exists because that asymmetry was found by probing the real
binary rather than by reading the documentation, which does not record it.

A generic function parses, resolves, type-checks, ownership-checks, and
**executes on the reference interpreter** today:

```tuo
pub fn ident[T](take x: T) -> T { x }

spec ident {
    then ident(7) == 7;
    then ident(true) == true;   // two distinct instantiations, one spec
}
```

```
$ tuo spec ident.tuo
ok ident (ident.tuo:4:0, 67µs)
1 passed, 0 failed of 1 spec in 67µs
```

The same file cannot be built:

```
$ tuo run ident.tuo
error: cannot build (codegen): `ident` uses a type the Cranelift backend
does not lower yet: a generic parameter has no layout until monomorphized
note: this program is outside the native backend's current subset;
      `tuo spec`/`tuo verify` can still execute it on the reference interpreter
```

So the accepted language contains generics, the reference semantics executes
them, and both backends refuse them. This is a legitimate instance of the
documented `check`-accepts-more-than-`build`-lowers gap — but it is an
*undocumented* instance, and the only one where the front end does substantial
work that then has nowhere to go.

### What already exists

The scaffolding is further along than the absence of an ADR suggests, which is
what makes this a tractable increment rather than a new subsystem:

| Layer | State |
|---|---|
| `grammar.ebnf` | `generic_params` / `generic_param` (rules at §18) are specified, and applied to `fn`, `struct`, `enum`, `interface`, `impl` |
| `tuo-parser` | parses `[T]` on a `fn`; **rejects `impl` outright** (`P0002`, "skipped 3 tokens starting with `impl`") |
| `tuo-types` | `Ty::Param(SymbolId)`; `FnSig::type_params`; `instantiate_fn`; turbofish arguments; `struct_shape`/`enum_shape` carry `type_params` and `Ty::Struct`/`Ty::Enum` carry `args` |
| `tuo-ownership` | documents that "checking is pre-monomorphization" (`lib.rs`) |
| `tuo-mir` | `Function` is keyed by a bare `SymbolId`; `Callee::Direct(SymbolId)` |
| `tuo-runtime` | `abi::layout_of` refuses `Ty::Param` at exactly one site, and `require_monomorphic` already guards generic aggregates |

The missing step is the one this ADR names: a pass that turns a family of
generic definitions plus a set of instantiations into concrete MIR.

### Why this is the most valuable increment

Three independent lines of evidence, none of them "generics are a feature
languages have".

**1. The standard library is hand-monomorphizing right now.** `std::collections`
ships the higher-order combinators twice — `fold`/`map_into`/`filter_into` over
`Array[Int]`, and `fold_strings`/`map_into_strings`/`filter_into_strings` over
`Array[String]` — the same algorithms, duplicated because the second element
type is not expressible generically. The module's own header calls the second
set what is "expressible". `Array[Float]` and struct elements would each need
another copy. This is the cost already being paid, in the shipped library, in
the module the language most wants to be small.

**2. It is the blocker three open ADRs are waiting on, under three different
names.**

- **ADR-0023** (map value widening) defers user map *key* types to "the trait
  system", and says so twice.
- **ADR-0021** (secret taint tracking) evaluates a `Secret[T]` wrapper and notes
  it "is expressible in v0 today **if generics over a nominal type land**".
- **ADR-0024** (capturing closures) needs a generic environment type for D2/D3.

One capability unblocks all three. None of them can proceed without it, and each
currently describes it as someone else's prerequisite.

**3. The dogfooding pressure ADR-0024 cites does not survive measurement.**

ADR-0024 justifies Tier 2 closures as "the single largest source of refusals in
`tools/py2tuo`", from comprehensions and `lambda`. Measured against 14,823
functions in 596 CPython standard-library files (excluding tests, `idlelib`,
`lib2to3`, `site-packages`), counting the functions each refused construct
appears in — the function being the unit py2tuo translates:

| construct | functions | share |
|---|---|---|
| method / attribute call | 12,844 | 86.6% |
| set / tuple literal | 3,698 | 24.9% |
| `try` / `except` / `raise` | 3,682 | 24.8% |
| truthiness (`if x`) | 3,118 | 21.0% |
| `is` / `is not` | 2,812 | 19.0% |
| `in` / `not in` | 1,328 | 9.0% |
| slicing | 1,014 | 6.8% |
| f-string | 873 | 5.9% |
| **comprehension / genexp** | **620** | **4.2%** |
| `with` | 497 | 3.4% |
| nested function | 308 | 2.1% |
| generator (`yield`) | 254 | 1.7% |
| `async` / `await` | 128 | 0.9% |
| **lambda** | **117** | **0.8%** |

The two constructs ADR-0024 is built on are near the bottom of the list. This
does not make closures wrong — `tools/py2tuo` is one corpus and Python is not
tuonelang — but it does remove the *empirical* argument for taking them first,
and ADR-0024's premise should be amended to say so rather than left standing.

(Several higher-frequency rows are already settled decisions, not open gaps:
`try`/`except` is refused permanently by ADR-0025, and method calls await the
trait system. They are listed for calibration, not as a backlog.)

## Decision

*Not yet taken in full.* This ADR fixes the scope, the staging, and the
constraints; the design questions in "What must be settled first" gate the
implementation of Stage B onward.

What **is** decided here:

### The strategy is monomorphization, not boxing

Every instantiation of a generic gets its own concrete MIR function, with
`Ty::Param` substituted away before the ABI or any backend sees it. The
alternative — a uniform representation where every generic value is a pointer —
is rejected:

- It would put an allocation or an indirection behind every generic call,
  in a language whose allocator boundary is deliberately explicit (ADR-0009)
  and whose integer model is deliberately machine-shaped (ADR-0026).
- `abi::layout_of` computes layouts from `tuo-types` alone, and both backends
  consult it rather than defining their own. A uniform representation would
  mean a *second* layout rule for generic contexts — exactly the drift the
  single-ABI invariant exists to prevent.
- The performance lab's equivalent-semantics C peer comparison would stop being
  apt for any generic code.

Monomorphization keeps one layout rule, one calling convention, and the existing
`PassMode` semantics unchanged.

### Scope: generic `fn` only

`impl` is **out of scope**, and stays a `P0002` parse refusal. Method dispatch is
not lowered in v0 (`tuo-mir/src/lower.rs`: "method calls are not lowered in v0
(pending the trait system)"), and generic `impl` blocks would drag in the trait
system, which is a larger decision with its own ADR to come. Generic `struct`
and `enum` **declarations** already parse and carry `type_params`; whether their
instantiation lands in this ADR's Stage C or a successor is left open below.

### The honesty gap closes first, and independently

Stage A below is a `check`-time advisory, on the `T0022` model. It is worth
landing **whether or not** monomorphization is scheduled, because the current
behaviour — accept, check, run under the interpreter, then fail at
storage-classification time with a whole-program message — is the exact failure
mode `T0022` was introduced to eliminate for heap-wrapper values. The precedent
is established and the mechanism already exists.

## Staging

### Stage A — the advisory (`T0023`), independently landable

Extend `tuo-compiler`'s `native_core` to warn, at the span of the declaration,
when a generic `fn` is declared — the same shape as the existing `T0022`
runnable-core advisory:

- a **warning**, never an error, so the accepted language is unchanged and
  `tuo spec` / `tuo verify` are unaffected;
- at the exact span of the generic parameter list, not a whole-program message;
- naming the interpreter as the path that does execute it.

Pinned the way `T0022` is, by `crates/tuo-compiler/tests/native_core.rs`.

This makes the gap visible where it is written. It is a strict improvement over
today regardless of what follows.

### Stage B — monomorphizing generic `fn`

A collection pass over typed HIR/MIR gathers every *instantiation* — a
`(SymbolId, Vec<Ty>)` pair — reachable from a non-generic root, then lowers one
concrete `Function` per pair with `Ty::Param` substituted.

The identity question is the load-bearing one: MIR's `Function` is keyed by a
bare `SymbolId` and `Callee::Direct` names one, so an instance needs an identity
the current IR cannot express. See "What must be settled first".

### Stage C — the stdlib payoff, and the benchmark plan

Collapse `std::collections`' six duplicated combinators into three generic ones,
keeping the existing names as the instantiations that already have specs and
callers.

This is the oracle, and it is a good one: the duplicated pair *already* exists,
both halves are spec-checked, and `examples/data-pipeline` drives both the
`Int` and the `String` paths to a documented exit byte. So the correctness
criterion is exact — **same specs green, same exit byte, no `.tuo` caller
changed** — and it is a real program rather than a synthetic test.

Note the collapse is **not** a pure substitution, and Stage C cannot begin
until Q6 is answered: the two folds differ in the *mode* of the step
function's element parameter (`fn(take Int, take Int)` against
`fn(take Int, in String)`), because `Int` is `Copy` and `String` is not.
Whichever way Q6 resolves determines whether these six functions can become
three at all, so a Stage C that "just works" would be evidence that Q6 was
answered implicitly rather than decided.

Per the governing rule, the benchmark plan is part of the decision, not a
follow-up: monomorphization multiplies code size, so the lab gains a
**`generic-instantiation`** compiler-lab entry measuring compile time and
emitted code size for the collapsed combinators against today's duplicated
pair. A ratio, recorded, with no target asserted in advance.

## What must be settled first

Stage B should not be implemented until these are answered. Each changes the
shape of the pass.

**Q1 — Instance identity in MIR.** `Function.symbol: SymbolId` and
`Callee::Direct(SymbolId)` assume one body per symbol. Options: a new
`InstanceId` interned per `(SymbolId, Vec<Ty>)`; extending `Callee::Direct`
with a substitution; or a pre-MIR expansion that mints fresh symbols. This
choice propagates into the MIR verifier, `tuo-mir-interp`, the golden `.mir`
fixtures, both backends, and the incremental query keys — so it is the first
question, not an implementation detail.

**Q2 — Where the pass runs.** Before lowering (expand HIR, lower concrete
bodies) or after (lower once generically, then clone-and-substitute MIR).
After-lowering keeps one lowering path and is more testable; before-lowering
avoids ever constructing MIR the verifier must be taught to accept. Note the
verifier is mandatory and runs on *every* body, so a generic MIR body must
either verify or never exist.

**Q3 — Incrementality.** `IncrementalSession` tracks per-function MIR keyed by
symbol. A generic function's MIR is now per-instantiation, and editing a generic
body invalidates every instantiation. This must be stated rather than
discovered: the five pinned edit scenarios in
`tuo-compiler/tests/incremental_stages.rs` are hard assertions on which queries
re-execute, and a sixth is needed.

**Q4 — Recursive instantiation.** `f[T]` calling `f[Array[T]]` generates
infinitely many instances. Every monomorphizing language needs a bound. The
tuonelang-shaped answer is a **hard error naming the cycle**, consistent with
`T0016`'s treatment of infinitely-sized recursive types — not a silent depth cap.

**Q5 — Function values.** `T0015` currently refuses a generic function as a
first-class value ("a function value is a single monomorphic code pointer"). Once
instances exist, `ident[Int]` *is* monomorphic and could be one. Whether taking a
value of an instantiation is in scope, and what the syntax is, is open.

**Q6 — Parameter modes across instantiations.** The duplicated combinators do
not differ only in type: `fold` takes `fn(take Int, take Int) -> Int` while
`fold_strings` takes `fn(take Int, in String) -> Int`. The mode differs because
`Int` is `Copy` and `String` is not, so collapsing them needs either a
mode-polymorphic signature, a rule deriving the mode from the type's `Copy`-ness,
or a decision that the generic form always borrows (`in T`) and `Copy` types
tolerate it. This is a *language* question, not an implementation detail, and it
is the one Stage C cannot avoid — the ownership model (ADR-0003) makes modes
explicit everywhere else, so an inferred mode would be a new kind of implicitness.

**Q7 — Do generic `struct`/`enum` instantiations land here?** `require_monomorphic`
already refuses them and `Ty::Struct` already carries `args`. Including them makes
Stage C more useful (`Option[T]`-shaped user types) and the pass substantially
larger. Recommended: defer to a successor, and keep this ADR to functions.

## Consequences

**Easier.** The stdlib stops duplicating algorithms per element type. ADR-0021's
`Secret[T]`, ADR-0023's user map keys, and ADR-0024's closure environments each
lose their stated blocker. `tools/py2tuo` gains nothing directly — its top
refusals are elsewhere — which is worth stating plainly rather than claiming a
benefit that measurement does not support.

**Harder.** Compile time and code size grow with instantiation count, which is
why Stage C carries a benchmark rather than an assertion. The incremental
engine's per-symbol MIR keying needs revisiting (Q3). The MIR golden fixtures and
the three-way differential suites all gain generic cases.

**Unchanged.** The interpreter stays the reference semantics — it already runs
these programs, and monomorphization must agree with it, not the other way
round. `Int` remains `I64` (ADR-0026). No uniform representation, no boxing, one
ABI.

**Explicitly not decided here.** The trait system; generic `impl`; bounds;
variance; and whether generic aggregates land in this ADR or a successor (Q7).

On bounds specifically: `generic_param` allows `T: Bound`, and the bound is
**not** ignored — the resolver looks it up and reports `R0002: cannot find
`Ord` in this scope`, because v0 declares no interface types for a bound to
name. So bounds are already wired into resolution and blocked on the trait
system having something to resolve *to*, rather than being unparsed syntax.
That is the right failure (an unknown name is a real error) and it means the
unbounded `[T]` form is the only one that can be written today — which is
exactly the surface this ADR monomorphizes.

## Amendment to ADR-0024

ADR-0024's "Why now" section states that capturing closures are "the single
largest source of refusals in `tools/py2tuo`". The survey above does not support
that, on the corpus available: comprehensions appear in 4.2% of CPython
standard-library functions and `lambda` in 0.8%, against 86.6% for method calls
and 24.8% for `try`/`except`. ADR-0024 should be amended to cite the measured
figures, so its priority argument rests on data rather than on impression. This
does not change any of ADR-0024's four design questions, which stand on their
own merits.
