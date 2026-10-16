# ADR-0032: Interfaces and method dispatch — bounds first, and no `x.f()` until it is measured

- **Status:** proposed (Stage A **landed** 2026-10-16; Stages B–D open)
- **Date:** 2026-10-14

## Context

This is the ADR that ADR-0027 defers to as "a larger decision with its own ADR
to come", and that ADR-0023 names as the gate on user map keys. The compiler
calls it "the trait system" (`tuo-mir/src/lower.rs`: "method calls are not
lowered in v0 (pending the trait system)"); the Constitution's word is
**interface** (§19), and this ADR uses it.

The Constitution already fixes a good deal: interfaces are the only
abstraction over behaviour, coherence is required, static dispatch is the
default (§19, Frozen); generics are bracketed, bounded, and monomorphized
(§18, Frozen); a call resolves to exactly one function by name (§8, Frozen).
Open are the dynamic-dispatch spelling and associated items (Q-0002) and the
`?` error conversion (Q-0003). §8 describes methods as `fn` items in an `impl`
with a receiver, and `grammar.ebnf` carries a `.IDENT call_args` "method call"
postfix — but no Frozen status covers the call syntax. Whether an interface
function is called as `x.f()` at all is the question this ADR has to answer.

### Findings from probing the real binary

Every probe below was run against the debug `tuo` built from this worktree on
2026-10-13 (`tuo 0.1.0`). The front end already accepts much of §19, and what
it accepts runs on **no** engine — a worse gap than `T0022`/`T0023`'s.

**F1 — a method call passes `tuo check` with no diagnostic, whatever it says.**

```tuo
module m3;

struct Point { x: Int, y: Int }

fn main() -> Int {
    let p = Point { x: 2, y: 3 };
    let b: Bool = p.no_such_method(1, 2, 3);
    if b { 1 } else { 0 }
}
```

```
$ tuo check m3.tuo      # exit 0, no errors, no warnings
$ tuo run m3.tuo
error: codegen: the program has no function named `main` to compile
```

The type checker returns `Ty::Error` for every `MethodCall` without emitting a
diagnostic (`tuo-types/src/check.rs`, "poison the result"), and poison
suppresses every downstream error, so an invented name, a wrong arity, and a
mistyped binding are all accepted. (The ownership checker likewise treats
every receiver as a plain read.) In a spec'd function the messages are worse:

| command on a spec'd `fn size(in s: Str) -> Int { s.len() }` | result |
|---|---|
| `tuo check` | accepted, 0 diagnostics |
| `tuo spec` / `tuo verify` | `trap internal: the interpreter only executes functions present in the lowered MIR` |
| `tuo build` | `call to a function outside the lowered program (v0 has no external calls)`, with a note that "`tuo spec`/`tuo verify` can still execute it on the reference interpreter" — which the row above shows is false |

This is not the documented check-accepts-more-than-build-lowers gap: the
reference interpreter cannot run it either. Only *reaching* the call fails: a
program whose method call sits in a function nothing calls checks, passes its
specs, and `tuo run`s to its exit byte today, which matters for Stage A. The
cheat sheet's "`xs.len()` PARSES and type-checks" is true only in that the
checker gave up silently, and `tuo-cli/tests/cheatsheet_command.rs` leaves
`xs.len()` out of `ANTI_PATTERNS` because the compiler does not reject it.

**F2 — `interface`/`impl` parse and their bodies type-check, but nothing is
checked *between* them.** ADR-0027 records `impl` as a `P0002` parse refusal;
that is now true only of the inherent form `impl Point { … }`, which the grammar
deliberately omits. `impl Area for Point { … }` parses, `self` is typed as
`Point`, and a type error in the body is reported (`T0001`). But an `impl` that
omits the interface's required `area` and adds an undeclared `perimeter` is
accepted with zero diagnostics, and a qualified call to an interface member is
not resolved past the interface name: `Area::nope(p)` and
`let b: Bool = Area::area(p, 1, 2)` both pass `tuo check`. MIR lowering skips
every impl function (`lower.rs`: "impl functions are not lowered in v0"), so a
program that declares an interface and an impl and never calls it runs fine.

**F3 — bounds resolve but are not enforced.** With `interface Area` declared,
`fn twice[T: Area](in v: T) -> Int` resolves its bound, and a spec calling
`twice(5)` **passes** — `Int` implements no `Area`. A bound naming an
undeclared interface (`T: Copy`, `T: Ord`) is `R0002`, as ADR-0027 records.
The `T0023` advisory on such a function says it "runs under `tuo spec`", which
stops being true the moment its body calls the bound's member, by either
spelling (`v.area()` or `Area::area(v)`): both check clean and the spec dies on
the same internal trap as F1.

**F4 — an unbounded generic cannot return its own argument.**
`fn dup[T](in v: T) -> T { v }` is `O0003` ("cannot move out of `in v`"):
without a `Copy` bound the checker must treat `T` as owning — correctly — and
there is no way to say otherwise. This is ADR-0027's Q6 in concrete form.

### What pulls on this decision

- **`std::collections` is `Int`-only wherever it compares.** Ten of its
  public functions — `contains`, `index_of`, `max_of`, `min_of`,
  `sorted_ascending`, `sorted_descending`, `insert_ascending`, `is_sorted`,
  `binary_search`, `deduped` — compare elements, directly or through
  `insert_ascending`. ADR-0027 can make `fold`/`map_into`/`filter_into`
  generic because they take the element operation as a function value; it
  cannot make these generic without a bound.
- **ADR-0023's user map keys** need `Hash` and `Eq` attached to the *type*.
  Passing a hash function per call would let a map be built with one hash and
  queried with another — a silent wrong answer. That is the coherence argument
  in its most concrete form.

### The free-functions-only position, and what has actually been measured

The library is "FREE FUNCTIONS ONLY (`std::array::len(xs)`, never
`xs.len()`)" (`tuo cheatsheet`, ADR-0018). The evidence is real but narrower
than the slogan. `tuo-cli/tests/stdlib_hallucination.rs` shows the *symbol
surface* rejects invented names; it compares no call spellings.
`crates/tuo-types/tests/wrong_module.rs` pins the `R0002`/`T0001` hints that
name the owning module — hints that exist **because** free functions make
module choice a failure mode (`len` has four owning builtin modules; the
catalog is mostly unambiguous, 421 of 471 names unique). Receiver-typed lookup
would make that failure impossible rather than well-diagnosed; that is the
strongest argument *for* methods. The only live runs
(`benchmarks/llm/codegen/results/README.md`: two tasks, `qwen3:4b`) go from 0%
to 100% Check@1 when primed with the brief, which shows the brief works and
says nothing about method syntax — and that README records that **variants
were not honoured**: the harness scores the program, so a variant constrains
spelling only through its own specs.

No measurement here compares `p.area()` with a free-function spelling. The
decision below does not need one yet, and is built so that one is taken before
anything irreversible happens.

## Options

| | Option | Dispatch | Spellings of a call | Name resolution |
|---|---|---|---|---|
| 0 | No interfaces; pass operations as function values | indirect, per call | one | type-free |
| 1 | Interfaces as bounds, members called by path `Area::area(p)` | static, monomorphized | one | type-free; impl *selection* by type |
| 2 | Interface members are methods (`p.area()`), stdlib stays free | static | two kinds of call in one program | `.name` needs the receiver type |
| 3 | Methods everywhere, stdlib migrated (`xs.len()`) | static | one | `.name` needs the receiver type |
| — | UFCS: `f(x)` and `x.f()` both legal | static | two for everything | both |

**Option 0** needs nothing new once ADR-0027 lands (`sorted_by[T](…, take
lt: fn(in T, in T) -> Bool)` is hand-written dictionary passing) and stays
available, but fails ADR-0023's case — an operation that must be the same at
every call should not be supplied per call — and costs an indirect call per
comparison.

**Option 2** is the worst fit: a model would have to know, per function,
whether it is a method or a free function (`std::array::len(xs)` but
`p.area()`) — a second spelling decision at every call site, exactly what the
one-spelling rule removes.

**Option 3** is coherent, matches models' priors (`xs.len()` is in the brief's
anti-pattern table because models write it), and removes the wrong-module
failure. Its costs: every `std::*` signature, the brief, `training/breaks.py`,
the corpus, and the hallucination proxy move; and `.name` can no longer be
resolved by `tuo-resolve` (its own comment: "the method *name* is
type-dependent"), so `definition`/`references`/`rename` in the LSP and agent
protocol stop being pure projections of `Resolution`.

**UFCS** is rejected outright: two spellings of every call.

**The asymmetry decides it.** Adding `x.f()` later is additive — the form
parses today and would go from error to accepted. Removing it once programs,
the corpus, and model priming depend on it is an edition break (§30). The
reversible choice is to start without it, and measure.

## Decision

### D1 — Interfaces are bounds; dispatch is static, through monomorphization

An interface member is called by its path, `Area::area(p)`, resolved like any
other path. Within a generic body, `T: Area` makes `Area::area(v)` well-typed
for `v: T`; ADR-0027's monomorphization selects the impl per instantiation, so
the emitted call is a direct call. Outside generic code the impl is selected at
type-check time and the call is equally direct. There is no vtable, no
dictionary, and no runtime cost; the benchmark plan verifies the last claim
rather than assuming it.

Proposed, not runnable today — run through `tuo check` it is refused at
`Array[T]` ("`Array[T]` is not supported in v0", `T0001`), and everything
else in it checks and would run nowhere (F2/F3):

```tuo
// PROPOSED syntax and semantics
interface Eq {
    fn eq(in self, in other: Self) -> Bool;
}

impl Eq for Point {
    fn eq(in self, in other: Point) -> Bool { self.x == other.x && self.y == other.y }
}

pub fn contains[T: Eq](in xs: Array[T], in needle: T) -> Bool {
    var i = 0;
    while i < std::array::len(xs) {
        if Eq::eq(std::array::get(xs, i), needle) { return true; }
        i = i + 1;
    }
    false
}
```

### D2 — `x.f(…)` stays parseable and becomes an error, with a fix

The postfix form remains in the grammar (no `GRAMMAR-VERSION` change), so the
compiler can recognize it and answer it. Instead of silent poison it is an
error at the call's span. When exactly one function named `f` is visible whose
first parameter accepts the receiver's type, the diagnostic carries a
**machine-applicable** rewrite to that function (`xs.len()` →
`std::array::len(xs)`), built on the machinery `wrong_module.rs` already pins;
otherwise it lists the candidates, or none. It is an **error**, not a warning
on the `T0022` model, because no engine executes the call — a warning that
says "runs under the interpreter" would be false.

### D3 — The receiver comes first and selects the impl

Every interface member's first parameter is the receiver (`in self`,
`mut self`, `take self` — §8's form, already in the grammar), and `Self` is
the first argument's type — the same shape the wrong-module hint already keys
on. Members whose `Self` appears only in the return type (`fn zero() -> Self`)
are deferred. An impl member's signature must equal the interface's with
`Self` substituted, **modes included**.

### D4 — The minimal coherence rule

- `impl I for T` lives in the module declaring `I` or the module declaring
  `T` (builtin scalars: with `I`). Stricter than soundness needs, chosen so
  "where is this impl" has exactly two answers an agent can check.
- `T` is a concrete nominal or builtin type: no blanket (`impl[T] I for T`)
  or generic (`impl[T] Eq for Pair[T]`) impls, so overlap checking is a
  duplicate check on `(I, T)` pairs with no unification. Generic impls wait on
  ADR-0027 Q7 and conditional bounds.
- Conformance is checked: missing, extra, and mismatched members are errors.

### D5 — Compiler-known interfaces, minimally

`Copy` becomes nameable **as a bound only** — still derived (§21), never
hand-implemented — so `T: Copy` lets a generic body copy out of an `in`
parameter (F4). `Eq` and `Ord` are declared in the standard library with
compiler-provided impls for the scalars whose `==`/`<` the language defines;
`Hash` follows with ADR-0023's keys. No operator dispatches through an
interface: `==` on a `T: Eq` value stays refused, and `Eq::eq(a, b)` is the
spelling.

### D6 — No dynamic dispatch

No interface objects; Q-0002 stays open. Closed sets are served by enums and
exhaustive `match`, open ones by tables of function values — which
`examples/router` already uses natively as its dispatch table
(`[fn(in Request) -> Int; N]`). A `Box[dyn I]` would also need `Box` values,
which ADR-0028 records cannot be constructed today. Re-entry condition: a
dogfooding program neither an enum nor a function-value table can express.

### D7 — Q-0003: `?` stays exact-match

`?` already works and requires matching error types (`?` from
`Result[Int, Str]` inside a `Result[Int, Int]` function is `T0001`). A
`From`-style conversion needs generic interfaces, which are out of scope, so
this ADR recommends resolving Q-0003 as **explicit conversion only** — the
behaviour that ships.

## Staging

**Stage A — honesty, independently landable.** No new feature. (1) D2: method
calls become an error with the fix. (2) Qualified interface-member calls
(`Area::area(p)`) become an error until Stage B — they too run nowhere.
(3) `interface` and `impl` *declarations* get a `T0022`-style **warning**, not
an error: they lower to nothing, harm nothing, and a program containing one
runs today (F2), so refusing them would break working programs for no gain.
(4) The cheat sheet's "PARSES and type-checks" sentence is corrected and
`xs.len()` joins `ANTI_PATTERNS` in `tuo-cli/tests/cheatsheet_command.rs`, now
that it is really rejected. *Landed 2026-10-16* as `T0024` (method call, with a
machine-applicable rewrite when exactly one function takes the receiver's type
first — `xs.len()` → `std::array::len(xs)`), `T0025` (qualified interface-member
call), and the `T0026` warning on `interface`/`impl` declarations; pinned by
`tests/types/fixtures/err/methods.tuo` (formerly `ok/impls.tuo`) and the brief's
new anti-pattern row. Like ADR-0027's and ADR-0028's Stage A, it is
worth landing even if nothing else here is.

**Stage B — interfaces, impls, conformance, coherence; non-generic calls.**
D3, D4, and conformance. Impl members lower as ordinary MIR functions and a
non-generic `Area::area(p)` lowers to a direct call, pinned three-way. Little
use alone — it is a longer spelling of `point_area(p)` — it is staged so the
checking rules are tested before generic code depends on them.

**Stage C — bounds enforced and lowered.** Bounds are checked at every
instantiation (F3 becomes an error) and interface calls in generic bodies
dispatch per instance. Depends on ADR-0027 Stage B, and on `Array[T]` over a
generic element, which the checker refuses today (`T0001`) — a dependency
ADR-0027's own collapse of `fold`/`fold_strings` would share. The oracle is the
ten `Int`-only `std::collections` functions made generic over `T: Eq`/`T: Ord`,
the existing names kept as the `Int` instantiations: every existing spec
green, every `examples/` exit byte unchanged, no `.tuo` caller edited.

**Stage D — `Hash` and user map keys.** ADR-0023's deferred item, as its own
increment, gated on a struct-key variant of the `map-lookup` workload.

## What must be settled first

- **Q1 — Impl selection in the interpreter.** It runs generic code today
  without monomorphizing, so either a MIR call carries the instance's `Self`
  or the interpreter runs post-monomorphization MIR. Decided together with
  ADR-0027 Q1/Q2 (instance identity, pass placement), not separately.
- **Q2 — Unqualified calls.** A bare `area(p)` collides once two interfaces
  share a member name. Recommended: qualified only, like `std::array::len`.
- **Q3 — How a spec names an impl member**, which has no unique name. An
  interface-level spec checked against every impl — a *law*, such as `Ord`'s
  antisymmetry — is the TDG-native answer and the most interesting thing this
  ADR could enable; it is not proposed here.
- **Q4 — Default member bodies.** The grammar allows them; whether they cross
  §19's "no implementation inheritance" line is decided before Stage B.
- **Q5 — Incrementality.** An impl-body edit should re-check that impl and
  re-lower only instances calling it; an interface-signature edit invalidates
  every impl and bounded caller. Both become pinned scenarios in
  `tuo-compiler/tests/incremental_stages.rs` before Stage B lands.
- **Q6 — Diagnostic codes**, allocated at implementation time (Stage A needs
  one; Stage B conformance, coherence, and placement), after whatever
  concurrent ADRs claim.

## Benchmark plan

Measured before acceptance, reported with no target asserted in advance:

- **Static dispatch is free — checked.** Runtime lab workload
  `interface-dispatch`: a hot loop calling `Ord::lt` through a bounded generic
  against the same loop over a hand-written `Int` function, with a C peer
  calling directly. The two tuonelang forms should emit the same code after
  monomorphization; the lab records whether they do. `indirect-calls` is
  already the Option 0 comparison.
- **Compile time and code size.** ADR-0027's `generic-instantiation` entry
  gains a bounded case — Stage C's generic `contains`/`sorted_*` at N element
  types against N hand-written copies — plus conformance and coherence checking
  as their own cold-stage cost.
- **Incremental cost.** Q5's scenarios as hard assertions on re-executed
  queries, timed by `incremental_measure.rs`.
- **The spelling question, with the existing harness.** A new pinned task set
  under `benchmarks/llm/codegen/tasks/` (at least twenty tasks: implement an
  interface for two types, write a bounded generic over it, call builtins),
  each with `qualified` and `method` variants whose **own specs are written in
  that spelling**, run with `tuo bench run --prime` on at least two models
  within the documented hardware ceiling. Until a method prototype exists the
  `method` variant is scorable only on Parse@1 (both forms parse today) and on
  how often a model reaches for it; a Check@1 gap between an accepted and a
  rejected spelling is not a spelling comparison and must not be reported as
  one.
- **The fix-it's reach.** A deterministic proxy in the style of
  `stdlib_hallucination.rs`: method-call priors on builtin and catalog
  receivers, recording the fraction that get a machine-applicable rewrite, and
  that every applied rewrite compiles.

**Decision rule.** Method-call syntax is reconsidered in a successor ADR if,
and only if, primed live runs show the qualified spelling losing Check@1 or
Repair@1 beyond the run-to-run variation the same runs exhibit.

## Consequences

**Easier.** Generic code can compare, which is most of what a collections
library does. ADR-0023's keys and ADR-0027's Q6 lose their stated blocker.
Name resolution stays independent of types, so the LSP and agent protocol keep
answering `definition`/`references` from `Resolution` alone. The silent
acceptance in F1–F3 ends.

**Harder.** `Area::area(p)` is longer than `p.area()`, and models will keep
writing the latter; D2's fix-it is the mitigation, and the benchmark exists
to say whether it is enough. Module-level coherence will refuse some impls a
package-level rule would allow.

**Narrower, deliberately — and not for free.** Stage A rejects programs
`tuo check` accepts today. Most of them fail on every engine anyway and now
fail at `check`, with a span. But a program whose method call is in
unreached code runs today and would stop checking. That cost is accepted: the
call is a type-system hole (any name, any arity, any result type), the code
around it can never execute, and the error's fix is machine-applicable in the
common case. The Stage A change log must say this rather than describe the
change as purely additive.

**Follow-ups on acceptance.** Constitution §19 says dynamic dispatch "is
available"; it would say deferred. §8's description of methods stays true.
ADR-0027's "impl is a `P0002` refusal" and ADR-0025's "tuonelang has no `?`
operator" are both stale and get corrected. Q-0002 stays open; Q-0003
resolves as above.

## What this ADR does not propose

Interface objects or any dynamic dispatch; associated types or constants;
generic interfaces (`From[E]`, `Into[T]`); blanket or generic impls; inherent
impls (`impl Point { … }`); default member bodies (Q4); operator overloading,
including `==` on `T: Eq`; user-written `Copy` impls or a `derive`
mechanism; a user `Drop` (Q-0011); members dispatched on their return type;
method-call syntax in any form; and interface-level law specs (Q3), which are
recorded as the most promising follow-up rather than as part of this
decision.
