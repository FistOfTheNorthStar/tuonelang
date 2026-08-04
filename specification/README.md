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
| `static-semantics.md` | The **normative** v0 static semantics: name resolution (the `Rxxxx` diagnostics) and type checking (the type universe, explicit signatures, no implicit conversions, exhaustiveness, and the `Txxxx` diagnostics), consolidated from Constitution §§8–24, `syntax.md`, and the `tuo-resolve`/`tuo-types` crate docs. Its executable counterpart is `tests/types/`. |
| `ownership.md` | The **normative** v0 ownership model (ADR-0003): moves, `Copy`, the `in`/`mut`/`take` borrow rules, joins, drops, wrappers, and the `O0001`–`O0009` diagnostics. Its executable counterpart is `tests/ownership/fixtures/`. |
| `mir.md` | The **normative** v0 MIR semantics: every instruction, terminator, and trap defined on its type; the mandatory verifier's well-formedness invariants and `Mxxxx` codes; and the reference interpreter's abort taxonomy and deterministic sandbox. Its executable counterpart is `tuo-mir`'s verifier + the differential suites. |
| `abi.md` | The **normative** v0 runtime ABI: value layouts, panic/trap, startup/exit, the allocation boundary, and internal calling conventions, versioned by `abi::ABI_VERSION`. |
| `RELEASE-0.1-GATE.md` | The **0.1 release gate**: the sixteen criteria that must be `MET` (or explicitly `RELEASE-BLOCKING`) before 0.1 may be declared ready, each pinned to a committed proving artifact. `crates/tuo-cli/tests/release_gate.rs` regenerates the readiness report from those artifacts and fails if the gate ever cites one that does not exist. |
| `adr/` | Architecture Decision Records documenting *why* specific design and implementation choices were made. |

## Process

Language decisions are recorded as ADRs (see `adr/README.md`) and, once
stabilized, folded into the constitution and grammar. A feature is never
considered specified merely because a future parser accepts it — see the
project `CONTRIBUTING.md` for the full definition-of-done for language features.
