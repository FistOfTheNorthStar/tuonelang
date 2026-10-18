# Recorded runs

Every file here was produced by `tuo bench run` against a live model and is
re-verified, not trusted: `tuo bench report <tasks> results/<run>.json`
recompiles every recorded output and recomputes the metrics from the
compiler's verdicts. The numbers below are the rescored ones.

## The core task set, before and after ADR-0038 (2026-10-17/18)

`core/` holds seven runs over `tasks/core-tasks.json`: 53 tasks, 55 runs per
pass (`range-sum` is run as written and in its two named styles), levels L1
to L4, up to two repair turns, temperature 0, seed 7, local Ollama on a 32 GB Mac. Rescore any of them
with `tuo bench report tasks/core-tasks.json results/core/<run>.json`.

*Before* is the compiler at `4dd8a24` with only `bench run`'s
`--reasoning-effort` and `--max-tokens` flags added, so it sends the same
requests; *after* is the ADR-0038 revision: the standard library in scope, the
builtins listed in the brief, and variant notes reaching the model. Because
`bench report` recompiles, a *before* run rescored by the *after* compiler
separates the two effects: same replies, new compiler.

How each model was run:

- **gemma4:latest** (9.6 GB) with `--reasoning-effort none`, which this
  model honours.
- **qwen3:4b** with `--max-tokens 8192`. Its reasoning cannot be turned off
  on Ollama 0.35.1: `think: false` and `reasoning_effort: "none"` only move the
  reasoning into the reply. So it reasons on every turn, under a cap; a reply
  cut off before its code counts as a failure. One run took 5½ to 8 hours.
  `qwen3-4b-after-plain` was not run: unprimed, the prompt is the same before
  and after (gemma4's two unprimed runs score identically), so it would have
  re-measured `qwen3-4b-before-plain`.

| Run file | Primed | Parse@1 | Check@1 | SpecPass@1 | TestPass@1 | Repair@1 | Invented symbols | Generated tokens |
|---|---|---|---|---|---|---|---|---|
| `gemma4-before-plain.json` | no | 20.0% | 7.3% | 7.3% | 100% | 29.1% | 270 | 22,477 |
| `gemma4-after-plain.json` | no | 20.0% | 7.3% | 7.3% | 100% | 29.1% | 315 | 23,181 |
| `gemma4-before-primed.json` | yes | 54.5% | 41.8% | 34.5% | 89.3% | 45.5% | 165 | 65,190 |
| `gemma4-after-primed.json` | yes | 54.5% | 41.8% | **38.2%** | 96.3% | 43.6% | 196 | 64,822 |
| `qwen3-4b-before-plain.json` | no | 21.8% | 12.7% | 12.7% | 100% | 18.2% | 343 | 616,562 |
| `qwen3-4b-before-primed.json` | yes | 89.1% | 43.6% | 43.6% | 97.2% | 58.2% | 107 | 562,087 |
| `qwen3-4b-after-primed.json` | yes | 90.9% | **60.0%** | **60.0%** | 95.9% | **74.5%** | **35** | 401,401 |

TestPass@1 is over the runs whose *final* program checked (after any repairs),
scored against the held-out tests the model never saw. gemma4's two unprimed runs
diverge only on `range-sum`: its variant prompts now carry the note, and its
repair turn quotes the *after* compiler's diagnostics.

What the old compiler reported for the same *before* replies, before
rescoring: gemma4 primed Check@1 30.9%, SpecPass@1 27.3%, 315 invented
symbols; qwen3:4b primed Check@1 40.0%, 151 invented symbols. The difference is
calls such as `std::core::abs(x)`, which the brief listed and the old compiler
rejected as `R0002`.

### What the runs show

- **Loading the library alone is worth 3 to 11 points of Check@1** on replies
  that did not change: gemma4 30.9% → 41.8%, qwen3:4b 40.0% → 43.6%.
- **The new brief helped qwen3:4b a lot and gemma4 barely.** On qwen3:4b,
  SpecPass@1 rose from 43.6% to 60.0% (24 → 33 of 55; 12 runs gained, 3
  lost), and its first-turn `R0002` errors fell from 9 to 1 once the builtins
  (`std::str::len`, `std::array::push`, ...) were listed. On gemma4 the gain is
  two runs (19 → 21), within noise: one run is 1.8 points.
- **Priming is still the largest effect** for both models, 31 to 47 points of
  Check@1, as on the starter set below.
- **By level** (SpecPass@1, after, primed): qwen3:4b L1 8/8, L2 19/30, L3
  5/11, L4 1/6; gemma4 L1 6/8, L2 14/30, L3 1/11, L4 0/6. L3 and L4 (strings and
  parsing, maps, growable arrays, structs, enums, `Box`) are where the next
  work is.
- **The remaining first-turn errors are mostly types**: `T0001` (mismatched
  types) is the most common on both runs of qwen3:4b (13 before, 10 after),
  then parse errors and `R0003`.

## The starter set (2026-10-16)

The task set is the two-task starter set (`double` with two syntax variants,
`min`), so these are four runs per pass, not a benchmark of the language. They
are the first live measurements the harness has produced, and they are here for
what they show about the method, not for their statistical weight.

### qwen3:4b, local Ollama, temperature 0, seed 7, up to 2 repair turns

| Run file | Primed with the brief | Parse@1 | Check@1 | SpecPass@1 | TestPass@1 | Repair@1 | Invented symbols | Generated tokens |
|---|---|---|---|---|---|---|---|---|
| `qwen3-4b-plain-feedback-v1.json` | no | 0% | 0% | 0% | n/a | 0% | 32 | 32,319 |
| `qwen3-4b-plain.json` | no | 0% | 0% | 0% | n/a | 0% | 28 | 40,290 |
| `qwen3-4b-primed.json` | **yes** | **100%** | **100%** | **100%** | **100%** | 100% | 0 | 8,693 |

The two unprimed files differ only in the repair feedback the harness rendered.
`feedback-v1` sent the model `CODE: message` per diagnostic; `plain` is the
current renderer, which adds the location, the compiler's primary label, its
notes and help, and attributes an error located in the harness-appended spec to
the spec rather than to the model's code. Both are kept so the change is
measured rather than assumed.

#### What the runs show

- **The brief is decisive for a model that does not know the language.** The
  same 4B model, the same tasks, the same seed: every first attempt fails
  without the brief and every first attempt passes with it, including the
  held-out tests. Unprimed, it writes other languages' spellings (`fn double(x:
  Int)`, `def double(...)`, `double: (Int take) -> Int`) and keeps writing them.
- **Diagnostics alone did not rescue it, but the label did reach it.** Under
  the current renderer the `min` task's second repair placed the modes
  correctly (`in a: Int, in b: Int`) and **checked**, the only unprimed turn in
  any run to do so; the program then failed its spec on the model's own logic
  (it returned the larger value). Every other repair stayed unparseable. A
  model that has never seen the syntax does not infer it from error messages
  about the syntax; the feedback loop is worth measuring on a model that already
  clears Parse@1.
- **Feedback must say whose code it is about.** Under the first renderer, an
  error located in the appended spec carried help addressed to a spec author
  ("use a string name for a free-standing spec"), and the model obeyed it,
  producing `fn "double"`. That is the origin of the attribution rule in the
  current renderer, under which the quoted names did not recur.
- **Variants were not honoured.** Primed, the `add` variant still produced
  `x * 2`. The harness scores the program, not its adherence to the requested
  spelling, so variant runs on this task say nothing about the spelling question
  yet.

#### What was tried and could not be measured

`qwen3.6:35b` (22 GB resident) and `gemma4:31b` were run through the same
command on the same 32 GB machine. Neither returned a single completion within
the 30-minute per-request timeout, and the gemma backend failed to start at all.
Those run files were discarded as measuring nothing. On this hardware the
practical ceiling for this benchmark is a model that stays well under ~10 GB
resident; a larger model needs a larger machine, not a longer timeout.

The live benchmark also cannot share the machine with `cargo test`: one rerun
timed out purely from that contention. Run it alone.
