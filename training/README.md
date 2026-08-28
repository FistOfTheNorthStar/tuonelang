# Training a tuonelang programmer model

Material for fine-tuning a language model into a professional **tuonelang**
programmer — one that writes code the real compiler accepts, runs, and
spec-verifies, and that repairs its own mistakes against genuine compiler
feedback (the **TDG — Test Driven Generation** loop tuonelang is built for).

The governing rule of everything here is the project's own rule: **nothing is
trusted on assertion.** Every training example is produced by compiling real
tuonelang source through the real `tuo` compiler. A program that does not
`check`, whose specs do not pass, or that does not run to its stated exit byte is
*refused* by the generator — it never reaches the dataset. This mirrors how
`tuo-corpus`, `tuo-codegen-bench`, and `tuo-bench` refuse to record a metric the
compiler did not produce.

## What's here

```
training/
  seeds.py         The seed library: (concept, task, canonical solution) triples,
                   authored against the v0 runnable core (see ../REFERENCE.md).
  breaks.py        Break rules: deterministic edits that inject the cross-language
                   mistakes a model actually makes (use-for-import, <>-generics,
                   Some(x), += , missing param mode, chained comparison, ...).
  harvest.py       Bulk harvester: mines additional examples from the repository's
                   already-green tuonelang (stdlib modules, dogfooding examples,
                   the validated corpus). Volume, on top of the seed skeleton.
  generate.py      Compiler-validated generator. Validates every seed and every
                   harvested example, then emits the datasets. Refuses to emit
                   while any seed fails to compile.
  score_eval.py    Honest scorer: compiles a model's held-out completions through
                   the real compiler and reports Compile@1 / SpecPass@1 / Run@1.
  dataset/         Generated output (see below). Regenerate with generate.py.
```

## The datasets

Regenerate everything (fast path uses a prebuilt binary):

```bash
cargo build -p tuo-cli
TUO_BIN="$(pwd)/target/debug/tuo" python3 training/generate.py
```

This writes, under `training/dataset/`:

| File | Purpose | Format |
|------|---------|--------|
| `sft_oneshot.jsonl` | Supervised fine-tuning for **fluency**: task → correct program. | Chat: `system`, `user`, `assistant`. |
| `sft_repair.jsonl` | SFT for the **feedback loop**: task → buggy attempt → *real* compiler diagnostic → corrected program. | Chat with a `tool` (name `tuo_compiler`) turn between the two `assistant` turns. |
| `eval_heldout.jsonl` | **Held-out** tasks never emitted into the SFT sets, for scoring. | `{task, concept, system, reference_solution, runnable, run_exit, stdlib_deps}`. |
| `stats.json` | Coverage: seed/example counts, per-concept and per-break breakdown, harvest kept/dropped counts. | JSON. |

The eval split is deterministic (a stable hash of each task string), so the same
seeds always produce the same train/eval partition — no leakage between runs.

### Harvested examples

Beyond the hand-authored seeds, `generate.py` mines `sft_oneshot.jsonl` from
source the repository already keeps green (`harvest.py`):

| Origin | What | Kept green by |
|--------|------|---------------|
| `crates/tuo-stdlib/src/std/*.tuo` | every documented `pub fn` — the doc comment becomes the task, the committed body the solution | `tuo-cli/tests/stdlib.rs` |
| `examples/**/src/*.tuo` | whole multi-function programs | `tuo-cli/tests/dogfood_examples.rs` |
| `corpus/correct/*.tuo` | compiler-validated correct programs | `tuo-corpus/tests/shipped_corpus.rs` |

Every harvested item is compiled **and** spec-run through the real `tuo` before
it is emitted, exactly like a seed. One that does not stand alone is *dropped*,
not shipped with hidden context: a stdlib function referencing a module-private
type (`Pair`) or a private helper cannot compile as presented, and an example
only teaches if the target program compiles as shown. `stats.json` records
`harvested_kept`, `harvested_dropped`, and the per-origin breakdown, so the drop
rate is visible rather than silent.

Harvested records join the **one-shot set only** and never the eval split — the
held-out tasks stay the hand-authored seed slice, so scoring is never
contaminated by material that also appears verbatim in the repository.

Skip the harvest with `--no-harvest` to regenerate from seeds alone.

### Chat record shapes

One-shot (`sft_oneshot.jsonl`):

```json
{"messages": [
  {"role": "system", "content": "You are a professional tuonelang programmer..."},
  {"role": "user", "content": "Write `double(x)` returning x + x, specced..."},
  {"role": "assistant", "content": "```tuo\nfn double(take x: Int) -> Int {...}\n```"}
]}
```

Repair (`sft_repair.jsonl`) — the middle `tool` turn is the compiler's **actual**
output, captured from `tuo --message-format=json check`:

```json
{"messages": [
  {"role": "system",    "content": "You are a professional tuonelang programmer..."},
  {"role": "user",      "content": "Write `square(n)`... (the compiler will give feedback)"},
  {"role": "assistant", "content": "```tuo\nfn square(n: Int) -> Int {...}\n```"},
  {"role": "tool", "name": "tuo_compiler",
   "content": "$ tuo check program.tuo\nerror[P0002]: malformed item: skipped 14 token(s)..."},
  {"role": "assistant", "content": "```tuo\nfn square(take n: Int) -> Int {...}\n```"}
]}
```

If your fine-tuning stack does not support a `tool` role, flatten it into the
`user` turn (prefix the diagnostic with `Compiler output:\n`). The signal — *code,
then real error, then fix* — is what matters.

## Adapting to your fine-tuning stack

The records use the common OpenAI-style `messages` schema. Convert as needed:

- **Anthropic / Claude fine-tuning or few-shot:** map `system` → the system
  prompt, alternate `user`/`assistant`; render the `tool` turn as a `user`
  message carrying the compiler output.
- **Raw completion format:** concatenate `system` + `\n\n` + `user` +
  `\n\n` + `assistant` and train on the assistant span.
- **Preference/RLAIF:** use `score_eval.py` as the reward signal — a completion
  that compiles and passes its specs is preferred over one that does not.

## Scaling up

The seed library is the *coverage skeleton*, not the final corpus. To scale:

1. **Add seeds** (`seeds.py`) — new concepts or more tasks per concept. Each new
   seed is validated the moment you run `generate.py`; a non-compiling seed
   fails loudly, so you cannot poison the set by accident.
2. **Add break rules** (`breaks.py`) — more mistake patterns produce more repair
   transcripts from the same seeds. A break that fails to actually break a
   program (or that the compiler still accepts) is skipped, never faked.
3. **Feed real programs** — `harvest.py` already ingests `../examples/`,
   `../corpus/correct/`, and `../crates/tuo-stdlib/src/std/` directly. To widen
   it, teach the harvester a new origin (or raise its keep rate by emitting a
   stdlib function together with the module context its prompt shows).
4. **Generate → repair with a live model** — the highest-value data is a real
   model's *own* mistakes. Run your candidate model over the eval/seed tasks,
   capture the completions it gets wrong, compile them to harvest the real
   diagnostics, and add those (attempt → diagnostic → reference fix) as new
   repair transcripts. `score_eval.py` already does the compile step; the
   `first_diagnostic` helper in `generate.py` captures the structured error.

Because validation is the real compiler, the corpus can grow without ever
drifting away from what tuonelang actually is.

## Evaluating a fine-tuned model

Produce a completions file — one JSON line per held-out task,
`{"task": "<verbatim task string>", "completion": "<model output>"}` — then:

```bash
TUO_BIN="$(pwd)/target/debug/tuo" python3 training/score_eval.py completions.jsonl
```

It extracts the ```` ```tuo ```` block from each completion, compiles it, runs
its specs, and (for runnable tasks) runs it natively — reporting:

- **Compile@1** — fraction whose `tuo check` passes.
- **SpecPass@1** — fraction whose specs all pass.
- **Run@1** — fraction of runnable tasks hitting the expected exit byte.

Sanity-check the harness itself (reference solutions must score 100%):

```bash
TUO_BIN="$(pwd)/target/debug/tuo" python3 training/score_eval.py --reference
```

## The language, in one screen

The single source of truth for *what to teach* is
[`../REFERENCE.md`](../REFERENCE.md) — a complete, compiler-accurate programmer's
guide. The system prompt in `generate.py` distills the highest-frequency
correctness rules a model coming from other languages gets wrong:

- Mutable bindings are `var`, not `let mut`; assignment is plain `=` (no `+=`).
- `import`, not `use`; generics use `[]` (`Option[Int]`), never `<>`.
- Option/Result payloads are **named fields**: `Some { value: x }`,
  `Ok { value: x }`, `Err { error: e }` — there is no `Some(x)`.
- Every parameter has a **mode** (`take`/`in`/`mut`) and a type.
- No negative literals (`-5` is unary minus); comparisons don't chain; integer
  overflow **traps**.
- **No methods** — free functions only: `area(r)`, never `r.area()`.
- The standard library is **consumed as input**: a program using `std::core`,
  `std::test`, or `std::collections` is compiled *alongside* that module's
  source (`std/core.tuo`, …). The `std::array` / `std::string` heap ops and the
  `std::rt` effects are compiler builtins and need no such dependency.

## Provenance and honesty

- Every emitted program compiled clean through `tuo` at generation time.
- Every repair diagnostic is the compiler's real output, not hand-written.
- The eval split never overlaps the SFT sets.
- `stats.json` records exactly what was produced, including which break rules
  fired and how often — silent truncation is visible, not hidden.

If a seed ever stops compiling (say, after a language change), `generate.py`
fails loudly and emits nothing rather than shipping a stale example. Keep the
seeds green the same way the rest of the repository stays green.
