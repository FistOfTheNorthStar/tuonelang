# LLM benchmarks

Machine-readable benchmarks for how reliably a model generates **tuonelang**.

The schema lives in [`tuo-bench::llm`](../../crates/tuo-bench/src/llm.rs). This
directory holds the version-controlled **inputs** (task sets) and **outputs**
(benchmark runs). The harness here defines and validates the schema and computes
aggregate metrics; **it does not run models** — a model/agent runner (future
work) produces the per-task results that get recorded in this shape.

## Metrics

A benchmark run summarizes, over a set of tasks:

| Metric | Meaning |
|--------|---------|
| **Parse@1** | fraction of tasks whose *first* generation parses. |
| **Check@1** | fraction whose first generation type/ownership-checks. |
| **SpecPass@1** | fraction whose first generation passes its colocated specs. |
| **Repair@1** | fraction that reach a fully-passing state within **one** repair cycle. |
| **average repair cycles** | mean number of repair iterations across tasks. |
| **generated tokens** | total tokens emitted across all tasks and repairs. |
| **hallucinated symbol count** | total references to symbols that do not exist. |

The `@1` metrics are measured on the first attempt; the schema also stores the
full **repair trajectory** per task, so richer metrics (Pass@k, SpecPass@k,
repair-token cost, …) can be derived later **without a schema change**.

## Files

```
tasks/     INPUT: task sets — prompt + colocated spec(s) per task (JSON)
results/   OUTPUT: benchmark runs — raw attempts + computed summary (JSON)
```

- [`tasks/starter-tasks.json`](tasks/starter-tasks.json) — a starter set of
  tuonelang generation tasks, each paired with the spec(s) a correct answer must
  satisfy (the TDG signal). These are inputs only.
- [`results/example-run.json`](results/example-run.json) — an **illustrative**
  benchmark run. **Its numbers are synthetic** (clearly labelled in the `model`
  field) and exist to demonstrate the schema and exercise the aggregate
  computation; no model was evaluated.

## Schema shape

A **task set** (input):

```json
{
  "schema_version": 1,
  "description": "…",
  "tasks": [
    { "task_id": "add-two-ints", "prompt": "…", "specs": ["spec \"…\" { … }"], "tags": ["easy"] }
  ]
}
```

A **benchmark run** (output):

```json
{
  "schema_version": 1,
  "model": "…",
  "attempts": [
    {
      "task_id": "add-two-ints",
      "initial_generated_tokens": 90,
      "initial_outcome": { "parsed": true, "checked": true, "specs_passed": true, "hallucinated_symbols": 0 },
      "repairs": [
        { "generated_tokens": 50, "outcome": { "parsed": true, "checked": true, "specs_passed": true, "hallucinated_symbols": 0 } }
      ]
    }
  ],
  "summary": { "task_count": 1, "parse_at_1": 1.0, "check_at_1": 1.0, "spec_pass_at_1": 1.0,
               "repair_at_1": 1.0, "average_repair_cycles": 0.0, "generated_tokens": 90,
               "hallucinated_symbol_count": 0 }
}
```

## Consistency guarantee

A test in `tuo-bench` (`tests/committed_data.rs`) loads every committed file
here and asserts that:

- task sets and runs parse against the current schema;
- each run's stored `summary` equals the summary **recomputed** from its
  `attempts`.

So a committed run cannot silently disagree with its own raw data. Regenerate a
summary by constructing a `BenchmarkRun` via `BenchmarkRun::new`, which computes
the summary for you.

## Status

The schema and example data exist; a model runner that produces real results
against `tasks/` is future work and intentionally not implemented here.
