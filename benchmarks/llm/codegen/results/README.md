# Recorded runs

Every file here was produced by `tuo bench run` against a live model and is
re-verified, not trusted: `tuo bench report tasks/starter-tasks.json results/<run>.json`
recompiles every recorded output and recomputes the metrics from the compiler's
verdicts. The numbers below are the rescored ones.

The task set is the two-task starter set (`double` with two syntax variants,
`min`), so these are four runs per pass, not a benchmark of the language. They
are the first live measurements the harness has produced, and they are here for
what they show about the method, not for their statistical weight.

## qwen3:4b, local Ollama, temperature 0, seed 7, up to 2 repair turns

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

### What the runs show

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

### What was tried and could not be measured

`qwen3.6:35b` (22 GB resident) and `gemma4:31b` were run through the same
command on the same 32 GB machine. Neither returned a single completion within
the 30-minute per-request timeout, and the gemma backend failed to start at all.
Those run files were discarded as measuring nothing. On this hardware the
practical ceiling for this benchmark is a model that stays well under ~10 GB
resident; a larger model needs a larger machine, not a longer timeout.

The live benchmark also cannot share the machine with `cargo test`: one rerun
timed out purely from that contention. Run it alone.
