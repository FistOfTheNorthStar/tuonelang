# tuonelang benchmarks

Benchmark suites for tuonelang. These measure properties the project cares
about; a claim may only be made once the benchmark backing it exists.

| Directory | Purpose |
|-----------|---------|
| `compiler/` | Compilation throughput and latency (a core tuonelang goal: fast compilation). Holds the fixed parser corpus (`compiler/parser/`) behind ADR-0001. |
| `runtime/`  | Runtime performance of compiled tuonelang programs, and an honest account of what the v0 core cannot yet run. |
| `llm/`      | LLM code-generation reliability: how consistently agents produce valid, compiling tuonelang. |

## The performance laboratory

The `tuo-bench` crate's `lab` module is tuonelang's reproducible
performance laboratory. It runs the compiler and runtime benchmarks the project
cares about **against the real compiler** — cold `lex`/`parse`/`check`/`build`,
warm no-op check, the standard incremental edits (body / signature / single-spec
/ affected-spec), and the runtime workloads — and records the full environment
(hardware, OS, compiler versions, exact commands, source) into a versioned
`LabReport`. See:

- [`compiler/README.md`](compiler/README.md) — the compiler scenarios.
- [`runtime/README.md`](runtime/README.md) — the runtime workloads, including the
  ones v0 cannot express and why.
- [`runtime/results/example-report.json`](runtime/results/example-report.json) —
  a committed example `LabReport` (illustrative environment; run the lab locally
  for real numbers).

Two host seams the crate cannot provide itself — native compile-link-run and a
foreign compiler — are injected by the CLI in the lab's end-to-end tests, so
native builds and cross-language comparisons run through the real toolchain.

**Honesty rules, enforced by the harness:** a workload the v0 core cannot express
reports the exact reason and **no number**; a cross-language comparison is only
made against a language with **equivalent semantics** (C, for the scalar core)
and only when **both** sides actually ran; and the report carries **no
superlative and no aggregate verdict** — no "blazing fast" anywhere. The
repository is built to *prove* a claim before it is made.

Also implemented: the lexer throughput benchmark
(`crates/tuo-lexer/benches/throughput.rs`) and the parser decision-gate
benchmarks (`crates/tuo-parser/benches/parser_compare.rs` plus the
`parse_memory` example) over the fixed corpus in `compiler/parser/`.
Measured results live in `specification/adr/ADR-parser-strategy.md`.
For anything not yet measured, no performance claims should be made anywhere in
the project.
