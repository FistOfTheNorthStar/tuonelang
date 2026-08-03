# Code-generation evaluation harness

The **executable** TDG LLM evaluation harness. Where
[`tuo-bench::llm`](../../../crates/tuo-bench/src/llm.rs) (a level up) defines the
metric *schema* and does not run the pipeline, this harness
([`tuo-codegen-bench`](../../../crates/tuo-codegen-bench)) **drives the real
compiler** over everything a model produces, so every metric it reports is earned
by an actual compile rather than asserted.

## What it does

A model is reached through a host-implemented `ModelAdapter` — **no LLM is
embedded**. For each task the harness asks the model for a generation, compiles
it through the real front end and spec runner, and — while it still fails and a
repair budget remains — feeds the compiler's diagnostics back and re-compiles.
Every turn is recorded.

For each task it measures (and `BenchmarkSummary` aggregates):

| Metric | Meaning |
|--------|---------|
| **Parse@1** | first generation parses. |
| **Check@1** | first generation passes the whole front end. |
| **SpecPass@1** | first generation passes its colocated specs. |
| **TestPass@1** | the task's **held-out** tests (not shown to the model) pass on the final program. |
| **Repair@1** | reaches fully-passing within one repair turn. |
| **repair cycles** | repair turns per task. |
| **generated tokens** | the model's own reported token counts. |
| **feedback latency** | wall-clock time to produce the compiler feedback (measured, never promised). |
| **invented symbols** | undefined-name (`R0002`) references — the compiler is the authority on which names do not exist. |
| **unrelated edit rate** | fraction of repairs that edited code the compiler had **not** flagged. |

## Provenance kept

A benchmark run keeps everything needed to trust and reproduce a result: the
exact **prompts** per turn, the **model configuration**, the **compiler and
language versions**, the model's **outputs** (every turn's generated source), and
the compiler's **results** (every turn's evaluation).

## Tasks are never changed silently

Tasks are version-controlled and pinned by a content **digest**; loading a task
set verifies every pin, so a task cannot be edited without the change being loud.
A test (`tuo-codegen-bench/tests/shipped_tasks.rs`) re-verifies every pin here.

Tasks may carry comparable **syntax variants** (e.g. `double` via `x + x` vs.
`x * 2`) so a language-design decision can be evaluated with data instead of
taste.

## Scoring a recorded run

An external runner records a model's outputs into a run file. The CLI *proves*
its metrics by recompiling the recorded generations:

```bash
tuo bench report tasks/starter-tasks.json results/<run>.json          # human table
tuo --message-format=json bench report tasks/starter-tasks.json <run> # machine report
```

`bench report` verifies the task-set pins first (refusing a silently-edited
benchmark), then recomputes every metric from the real compiler's verdicts —
never from the recorded booleans — so a fabricated result cannot survive.

## Files

```
tasks/    INPUT: pinned task sets — prompt + specs + held-out tests + variants (JSON)
results/  OUTPUT: recorded benchmark runs — prompts, outputs, per-turn results (JSON)
```

- [`tasks/starter-tasks.json`](tasks/starter-tasks.json) — a starter, pinned task
  set (with syntax variants and held-out tests). Inputs only.

## Status

The harness runs the real compiler over recorded outputs and reports both ways.
A live model runner that produces fresh generations against `tasks/` plugs in
through `ModelAdapter`; none is embedded here.
