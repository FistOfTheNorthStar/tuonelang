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
| [ADR-0008](ADR-0008-first-class-functions.md) | First-class functions | proposed |
| [ADR-0009](ADR-0009-allocator-core.md) | The allocator core — owned `String` and growable `Array` | proposed |

(`ADR-parser-strategy.md` carries number 0001 without it in the filename;
new ADRs should follow the `ADR-NNNN-…` naming above. ADR-0005 is intentionally
unallocated; ADRs 0004/0006/0007/0008 were opened together by the Prompt 39
dogfooding exercise — see [`DOGFOODING.md`](../../DOGFOODING.md). ADR-0004 and
ADR-0006 have since been accepted and landed, each having flipped its named
performance-lab workload; 0007/0008 remain `proposed` pending their normative
spec, fixtures, and performance-lab entries. ADR-0009 is the allocator ADR that
ADR-0006's first amendment promised — it must flip the `allocation` workload
before it can be accepted.)
