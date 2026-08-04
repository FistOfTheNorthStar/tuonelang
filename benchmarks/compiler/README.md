# Compiler benchmarks

Compilation throughput and latency — a core tuonelang goal. Every figure comes
from driving the **real** compiler: the harness core (`tuo-bench`'s
`lab::compiler`) calls `tuo_compiler::check_sources` and
`tuo_compiler::IncrementalSession` directly, the same code paths the CLI uses.
No stage is simulated.

This directory also holds the fixed parser corpus (`parser/`, behind ADR-0001)
that the `tuo-parser` decision-gate benchmarks measure.

## Two kinds of number

**Cold stages** measure batch cost from a fresh state — a new source map per
iteration — which is what a command-line invocation pays:

| Scenario | Drives | Command |
|----------|--------|---------|
| `cold-lex` | `tuo_lexer::lex` | `tuo debug syntax <file>` |
| `cold-parse` | `tuo_parser::parse` | `tuo debug ast <file>` |
| `cold-check` | `check_sources` (resolve + type + ownership) | `tuo check <file>` |
| `cold-build` | front end → verified MIR → native object → link | `tuo build <file>` |

`cold-build` needs a native backend and the `cc` linker, which `tuo-bench` does
not name; it is measured through a host-injected builder (the CLI's real
Cranelift+`cc` path), exactly as the runtime workloads are.

**Incremental edits** measure how *little* an editor's re-check costs. For an
incremental compiler the honest figure is not wall-clock but the **count of
per-item queries that re-executed** — a deterministic property of the query
graph, reported from the session's own execution log:

| Scenario | Edit | Expectation |
|----------|------|-------------|
| `warm-no-op-check` | none | **zero** queries re-execute |
| `edit-function-body` | one function's body | only that function's parse/typeck/mir re-run |
| `edit-function-signature` | one function's signature | its callers re-check; unrelated functions do not |
| `edit-single-spec` | one spec's body | only that spec's dependency graph re-runs |
| `affected-spec-verify` | one file | only the specs whose closure touches the edit are selected |

These map directly onto the fine-grained incremental machinery documented under
the incremental-compilation convention, and reuse its `executed_queries()` log
as the load-bearing evidence.

## What every run records

The hardware, OS, compiler versions, exact reproduction command, and source —
captured live into a `LabReport`. See the committed example
[`../runtime/results/example-report.json`](../runtime/results/example-report.json)
and the top-level [`../README.md`](../README.md).

## No unsupported claims

Numbers are measurements on the recorded environment, never guarantees, and the
lab publishes no superlative. "Fast compilation is a core goal" is a *design
intent*; a *claim* about compile speed may be made only once a run of these
benchmarks backs it, against C-equivalent work where a cross-language figure is
involved.
