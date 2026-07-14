# tuonelang language specification

This directory is the home of the tuonelang language specification. The **v0
Constitution freezes the foundational language design**; the surface grammar is
not yet transcribed and several areas remain explicitly Experimental. These
documents are the authoritative record of what is decided and what is still open.

tuonelang is the language; **TDG (Test Driven Generation)** is the paradigm it is
built for (colocated, executable specifications — see `CONSTITUTION.md` §27).

## Contents

| File | Purpose |
|------|---------|
| `CONSTITUTION.md` | The tuonelang Language Constitution (v0). Freezes the foundational design; each decision records design, rationale, rejected alternatives, and Frozen/Experimental status. |
| `open-questions.md` | The unresolved design questions left open by v0 (the Constitution's Experimental/Deferred items). |
| `grammar.ebnf` | The formal grammar. Placeholder: v0 semantics are frozen but the normative EBNF is intentionally not transcribed yet (see the file for why). |
| `adr/` | Architecture Decision Records documenting *why* specific design and implementation choices were made. |

## Process

Language decisions are recorded as ADRs (see `adr/README.md`) and, once
stabilized, folded into the constitution and grammar. A feature is never
considered specified merely because a future parser accepts it — see the
project `CONTRIBUTING.md` for the full definition-of-done for language features.
