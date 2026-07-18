# tuonelang benchmarks

Benchmark suites for tuonelang. These measure properties the project cares
about; a claim may only be made once the benchmark backing it exists.

| Directory | Purpose |
|-----------|---------|
| `compiler/` | Compilation throughput and latency (a core tuonelang goal: fast compilation). Holds the fixed parser corpus (`compiler/parser/`) behind ADR-0001. |
| `runtime/`  | Runtime performance of compiled tuonelang programs. |
| `llm/`      | LLM code-generation reliability: how consistently agents produce valid, compiling tuonelang. |

Implemented so far: the lexer throughput benchmark
(`crates/tuo-lexer/benches/throughput.rs`) and the parser decision-gate
benchmarks (`crates/tuo-parser/benches/parser_compare.rs` plus the
`parse_memory` example) over the fixed corpus in `compiler/parser/`.
Measured results live in `specification/adr/ADR-parser-strategy.md`.
Shared harness code will live in the `tuo-bench` crate. For anything not yet
measured, no performance claims should be made anywhere in the project.
