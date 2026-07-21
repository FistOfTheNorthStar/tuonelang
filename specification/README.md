# tuonelang language specification

This directory is the home of the tuonelang language specification. The **v0
Constitution freezes the foundational language design**, and the **normative v0
grammar** (lexical + concrete) is now transcribed. Several areas remain
explicitly Experimental (notably the `spec` surface syntax). These documents are
the authoritative record of what is decided and what is still open.

tuonelang is the language; **TDG (Test Driven Generation)** is the paradigm it is
built for (colocated, executable specifications — see `CONSTITUTION.md` §27).

## Contents

| File | Purpose |
|------|---------|
| `CONSTITUTION.md` | The tuonelang Language Constitution (v0). Freezes the foundational design; each decision records design, rationale, rejected alternatives, and Frozen/Experimental status. |
| `open-questions.md` | The unresolved design questions left open by v0 (the Constitution's Experimental/Deferred items). |
| `grammar.ebnf` | The **normative** v0 grammar in EBNF: the lexical grammar `(A)` and the concrete grammar `(B)`–`(I)`, cross-referenced to the Constitution. The authoritative source the lexer/parser track. |
| `lexical-grammar.md` | Prose companion to `grammar.ebnf (A)`: how bytes become tokens (encoding, trivia, identifiers, keywords, literals, operators). |
| `syntax.md` | Prose companion to `grammar.ebnf (B)`–`(I)`: the concrete grammar, its design priorities, and the explicit boundary to the semantic layer (type checking is **not** in the grammar). |
| `ownership.md` | The **normative** v0 ownership model (ADR-0003): moves, `Copy`, the `in`/`mut`/`take` borrow rules, joins, drops, wrappers, and the `O0001`–`O0009` diagnostics. Its executable counterpart is `tests/ownership/fixtures/`. |
| `adr/` | Architecture Decision Records documenting *why* specific design and implementation choices were made. |

## Process

Language decisions are recorded as ADRs (see `adr/README.md`) and, once
stabilized, folded into the constitution and grammar. A feature is never
considered specified merely because a future parser accepts it — see the
project `CONTRIBUTING.md` for the full definition-of-done for language features.
