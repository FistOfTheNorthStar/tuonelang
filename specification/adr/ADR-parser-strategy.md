# ADR-0001: Parser implementation strategy

- **Status:** accepted
- **Date:** 2026-07-19

## Context

Prompt 9 delivered the first complete tuonelang parser as a Chumsky 0.10
combinator grammar behind a TDG-owned interface (`tuo_parser::parse` →
`ParseResult`; no engine type in the API). Before the parser accretes
consumers (HIR lowering, LSP, formatter), the project must decide whether to
keep the combinator engine or move to a handwritten parser. The decision
gate demanded **measured** evidence, not ideology: cold and repeated parse
throughput, memory use, implementation complexity, malformed-input behavior,
diagnostic count and quality, partial-syntax retention, and source-range
exactness, over a fixed corpus.

### Method

A minimal handwritten recursive-descent + Pratt engine
(`crates/tuo-parser/src/handwritten.rs`) was built against the same
normative grammar (`specification/grammar.ebnf`), producing the same lossless
`SyntaxTree` over token indices and mirroring the Chumsky grammar's PEG
semantics exactly — including the recovery design (skip to `;` / `}` /
item-leading keywords, retain skipped tokens under `Error` nodes, one `P0002`
per skip) and the shared `P0003` nesting pre-scan.

- **Fixed corpus:** `benchmarks/compiler/parser/` — `clean.tuo` (72 KiB of
  varied well-formed items), `expr_heavy.tuo` (79 KiB of dense expression
  code), `error_heavy.tuo` (19 KiB with 4 deliberate error sites per 24-line
  unit). Checked in, deterministic, documented in that directory's README.
- **Throughput:** Criterion benchmark
  `crates/tuo-parser/benches/parser_compare.rs` — cold parse (fresh source
  map per iteration), repeated parse (retained snapshot), and a lex-only
  baseline. Run on an Apple M4 (32 GiB), rustc 1.97.0, release profile.
- **Memory:** `crates/tuo-parser/examples/parse_memory.rs` under the dhat
  profiling allocator (peak and total heap per single parse).
- **Recovery quality / retention / ranges:** the `oracle_parity` test suite
  asserts **byte-identical rendered trees and identical diagnostics (code,
  byte offset, message)** for both engines over every fixture, the benchmark
  corpus, 14 curated recovery scenarios, 800 randomized fragment/byte-soup
  inputs, and pathological nesting — plus the losslessness invariants
  (full token coverage, byte-exact reconstruction) on every input.

### Measured results

Throughput (Criterion means; parse includes lexing):

| Corpus | Engine | Cold | Repeated | Lex-only baseline |
|---|---|---|---|---|
| clean (72 KiB) | Chumsky | 5.81 ms — 12.1 MiB/s | 5.90 ms — 11.9 MiB/s | 0.16 ms — 431 MiB/s |
| clean | **handwritten** | **0.63 ms — 112 MiB/s** | **0.72 ms — 98 MiB/s** | |
| expr_heavy (79 KiB) | Chumsky | 9.67 ms — 8.0 MiB/s | 9.85 ms — 7.8 MiB/s | 0.21 ms — 365 MiB/s |
| expr_heavy | **handwritten** | **0.98 ms — 79 MiB/s** | **1.16 ms — 66 MiB/s** | |
| error_heavy (19 KiB) | Chumsky | 2.17 ms — 8.7 MiB/s | 2.21 ms — 8.6 MiB/s | 0.05 ms — 359 MiB/s |
| error_heavy | **handwritten** | **0.25 ms — 76 MiB/s** | **0.28 ms — 67 MiB/s** | |

The handwritten engine is **9.2×–9.9× faster end-to-end**; subtracting the
shared lexing cost, the parser stage alone is **12×–13× faster**. Cold and
repeated parses are indistinguishable for both engines (no warm-up
structure; neither engine reuses state across parses yet).

Heap use for one parse (dhat):

| Corpus | Engine | Peak bytes | Total bytes | Total allocations |
|---|---|---|---|---|
| clean | Chumsky | 2 648 096 | 13 754 896 | 239 799 |
| clean | **handwritten** | 2 283 040 | **3 649 216** | **18 097** |
| expr_heavy | Chumsky | 3 940 352 | 23 822 496 | 391 303 |
| expr_heavy | **handwritten** | 3 441 152 | **5 962 784** | **31 333** |
| error_heavy | Chumsky | 1 093 200 | 5 498 688 | 90 066 |
| error_heavy | **handwritten** | 1 033 792 | **1 786 880** | **7 532** |

Peak heap is similar (~6–14 % lower — both engines build the same tree,
which dominates the peak), but the handwritten engine performs **12×–13×
fewer allocations** and **3.1×–4× less total allocation churn**, which is
where the throughput gap comes from.

Recovery, retention, and ranges — **exact parity, verified not estimated**:
on `error_heavy.tuo` both engines produce the same 288 diagnostics at the
same byte offsets with the same messages, retain the same 216 intact
`FunctionItem`s around the same 288 `Error` islands, and reconstruct the
input byte-for-byte. Every source range in both trees is exact by
construction (nodes index the lossless token stream). The 800-case
randomized differential sweep found zero divergences.

Implementation complexity:

| | Chumsky engine | Handwritten engine |
|---|---|---|
| Engine-specific code | `grammar.rs` + error conversion ≈ 1 100 lines | `handwritten.rs` ≈ 1 430 lines |
| Third-party deps | chumsky + 9 transitive crates | none |
| Recursion safety | engine-internal (plus shared `P0003` pre-scan) | shared `P0003` pre-scan |

The handwritten engine is ~30 % more code but is plain first-order Rust:
every backtrack point is an explicit `mark`/`rollback`, diagnostics are
emitted directly (no `Rich`-error conversion layer), and stepping through a
parse in a debugger is straightforward. The Chumsky grammar is denser but
its behavior lives inside deeply generic combinator types: the `just()`
pattern-vs-input pitfall, the private-vs-public `Emitter` path, and the
recovery-as-`validate` encoding all cost real debugging time during
Prompt 9, and diagnostic wording is constrained by what `Rich` errors carry.

## Decision

**B — migrate to the handwritten recursive-descent + Pratt parser.**

`tuo_parser::parse` now runs the handwritten engine
(`crates/tuo-parser/src/handwritten.rs`). The interface is unchanged: the
engine choice is invisible in the API, and every pre-existing parser test
(snapshots, precedence, recovery, robustness) passes unmodified.

The Chumsky implementation is **retained temporarily as a differential
oracle** behind the same interface (`tuo_parser::oracle::parse`), used only
by the `oracle_parity` test suite and the decision-gate benchmarks.
**Removal gate:** delete the oracle (and the chumsky dependency) once the
grammar next changes materially — extending both engines in lockstep is not
worth the oracle's value once the handwritten engine has its own grammar
coverage for the new constructs. Until then the oracle is frozen: new
grammar work lands in the handwritten engine first, and if the oracle can no
longer mirror it cheaply, that triggers the removal instead of a port.

Rationale, strictly from the numbers above: 9–10× end-to-end throughput,
12–13× fewer allocations, and *identical* recovery quality remove the main
argument for keeping the combinator engine (recovery ergonomics). The
handwritten engine's extra ~330 lines buy direct control over diagnostics
(needed for the diagnostics-contract work ahead), zero third-party parser
dependencies, and debuggability. Fast parsing is a core tuonelang goal
(`benchmarks/README.md`), and at 8–12 MiB/s the Chumsky engine — 27–46×
slower than the lexer stage — would have been the pipeline's bottleneck.

## Consequences

- Easier: parser-stage performance headroom (parse is now ~5× the lexer's
  cost rather than ~40×); bespoke diagnostic wording and future
  error-message work; onboarding (no combinator-generics expertise needed);
  future incremental-reparse work (explicit cursor state is amenable to
  checkpointing).
- Harder: grammar changes are more verbose than combinator edits, and until
  the oracle is removed, deliberate grammar changes must either update both
  engines or trigger the removal gate. The PEG-mirroring discipline
  (rollback of cursor **and** diagnostics at every speculative alternative)
  is an invariant reviewers must know; it is documented at the top of
  `handwritten.rs` and enforced by the parity and robustness suites.
- The `oracle_parity` suite, the fixed corpus, the Criterion benchmark, and
  the dhat example stay in-tree so this decision remains re-measurable; any
  future engine claim (including reversing this one) should rerun them.
- Follow-up work: execute the oracle removal gate when it triggers;
  reflect measured parser throughput in `tuo-bench` once that harness
  exists.
