# tuonelang benchmarks

Benchmark suites for tuonelang. These measure properties the project cares about but
does not yet make claims about; no benchmarks are implemented yet.

| Directory | Purpose |
|-----------|---------|
| `compiler/` | Compilation throughput and latency (a core tuonelang goal: fast compilation). |
| `runtime/`  | Runtime performance of compiled tuonelang programs. |
| `llm/`      | LLM code-generation reliability: how consistently agents produce valid, compiling tuonelang. |

Shared harness code will live in the `tuo-bench` crate. Until benchmarks exist,
no performance claims should be made anywhere in the project.
