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

(`ADR-parser-strategy.md` carries number 0001 without it in the filename;
new ADRs should follow the `ADR-NNNN-…` naming above.)
