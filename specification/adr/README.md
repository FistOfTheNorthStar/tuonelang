# Architecture Decision Records (ADRs)

An **Architecture Decision Record** captures a single significant decision — its
context, the choice made, and the consequences — so that the *reasoning* behind
tuonelang's design and implementation is durable and reviewable.

## When to write an ADR

Write an ADR when a decision is hard to reverse or shapes later work, for
example:

- a language design choice (syntax, type system, ownership model, diagnostics
  contract);
- a compiler-architecture choice (IR design, crate boundaries, backend
  strategy);
- a tooling or process choice with long-lived consequences.

Routine, easily reversible changes do not need an ADR.

## Naming convention

ADRs are numbered sequentially and named:

```
ADR-NNNN-short-kebab-case-title.md
```

For example: `ADR-0001-use-mir-as-single-semantic-ir.md`. Numbers are never
reused, even if an ADR is later superseded.

## Format

Each ADR should contain at least:

```markdown
# ADR-NNNN: <title>

- **Status:** proposed | accepted | superseded (by ADR-XXXX) | deprecated
- **Date:** YYYY-MM-DD
- **Context:** What situation forces a decision? What constraints apply?
- **Decision:** What was decided, stated plainly.
- **Consequences:** What becomes easier or harder as a result, including
  trade-offs and follow-up work.
```

Superseding an ADR does not delete it: mark the old one `superseded` and link
the replacement, preserving the decision history.

## Current ADRs

| ADR | Title | Status |
|-----|-------|--------|
| [ADR-0001](ADR-parser-strategy.md) | Parser implementation strategy | accepted |
| [ADR-0002](ADR-0002-spec-semantics.md) | First-class `spec` semantics | accepted |
| [ADR-0003](ADR-0003-ownership-model.md) | The v0 ownership model | accepted |
| [ADR-0004](ADR-0004-aggregates-in-the-runnable-core.md) | Aggregates and iteration in the runnable core | accepted |
| [ADR-0006](ADR-0006-effect-boundary-and-strings.md) | The effect boundary and runtime strings | accepted |
| [ADR-0007](ADR-0007-concurrency-model.md) | The concurrency model | proposed |
| [ADR-0008](ADR-0008-first-class-functions.md) | First-class functions | accepted (Tier 1; Tier 2 closures deferred) |
| [ADR-0009](ADR-0009-allocator-core.md) | The allocator core — owned `String` and growable `Array` | accepted |
| [ADR-0010](ADR-0010-string-to-str-view.md) | The `String` → `Str` borrowing view (Q-0012) | proposed (Stages A/B landed) |
| [ADR-0011](ADR-0011-hash-map.md) | The hash map — a keyed associative container | proposed |
| [ADR-0012](ADR-0012-generic-array-elements.md) | Generic `Array[T]` element types — widening the monomorphic builtin surface | proposed (Stages A/B/C landed, owned elements native) |

(`ADR-parser-strategy.md` carries number 0001 without it in the filename;
new ADRs should follow the `ADR-NNNN-…` naming above. ADR-0005 is intentionally
unallocated; ADRs 0004/0006/0007/0008 were opened together by the Prompt 39
dogfooding exercise — see [`DOGFOODING.md`](../../DOGFOODING.md). ADR-0004,
ADR-0006, ADR-0009, and ADR-0008 (Tier 1) have since been accepted and landed,
each having added or flipped a performance-lab workload (`collections`,
`string-processing`, `allocation`, and the new `indirect-calls`, respectively).
ADR-0008 landed **Tier 1** — non-capturing first-class function values and the
generic higher-order stdlib combinators — with **Tier 2 capturing closures
explicitly deferred** to a future ADR increment. ADR-0009 is the allocator ADR
that ADR-0006's first amendment promised — it landed the owned `String` and
growable `Array[Int]`. Of the nine runtime workloads, eight now measure, leaving
only `networking`.

Four ADRs remain `proposed`, each pending its staged spec/fixtures/benchmark:
**ADR-0007** (concurrency — its performance-lab entry is `networking`);
**ADR-0010** (the `String`→`Str` borrowing view that resolves the long-deferred
Q-0012, unblocking the stdlib's `String`-producer / `Str`-consumer composition —
its Stage B native zero-copy lowering has since landed and is three-way
differential-pinned; only the Stage C stdlib payoff remains); **ADR-0011** (the
hash map — the largest remaining Go-parity gap, gated on a new `map-lookup` lab
workload); and **ADR-0012** (generic `Array[T]` element types — widening the
ADR-0009 array builtins from `Int` to a defined element set, so
`Array[String]`/`Array[Str]`/`Array[struct]` become expressible; its
owned-element native increment has since landed — deep-copy `get` and recursive
drop glue on both backends — so `std::str::split`/`join` run natively, leaving
only the ADR-0008 combinator instantiations and the dogfood oracle before
acceptance). ADR-0010, ADR-0011, and ADR-0012 are
independent of the (still-absent) trait system, and all ride the same
monomorphic-builtin machinery ADR-0009 used for `Array[Int]` — ADR-0012 is that
machinery's element-surface widening, explicitly **not** user-written generics
(`fn f[T]`, generic structs), which stay deferred under Q-0010.)
