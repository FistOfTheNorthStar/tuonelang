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
  seeds/           The seed library: one directory per concept, one seed per
                   group of files (below), authored against the v0 runnable core
                   (see ../REFERENCE.md).
  seeds.py         The seed loader: reads seeds/ into `Seed` records, refusing
                   an unknown metadata key or a file that belongs to no seed.
  breaks.py        Break rules: deterministic edits that inject the cross-language
                   mistakes a model actually makes (use-for-import, <>-generics,
                   Some(x), += , missing param mode, chained comparison, ...).
  harvest.py       Bulk harvester: mines additional examples from the repository's
                   already-green tuonelang (stdlib modules, dogfooding examples,
                   the validated corpus). Volume, on top of the seed skeleton.
  generate.py      Compiler-validated generator. Validates every seed and every
                   harvested example, then emits the datasets. Refuses to emit
                   while any seed fails to compile or overlaps (below).
  similarity.py    Near-duplicate detection: programs compared by shape (names
                   and literals normalized), tasks by wording.
  overlap.py       The overlap rules generate.py and CI enforce; run it to
                   review the closest pairs before adding seeds.
  test_overlap.py  Their tests (python3 -m unittest discover -s training).
  run_eval.py      Asks a model (any OpenAI-compatible endpoint) for a completion
                   of every held-out task; writes them to the private eval repo.
  score_eval.py    Honest scorer: compiles a model's held-out completions through
                   the real compiler and reports Compile@1 / SpecPass@1 / Run@1.
  publish.py       Checks generated files against the manifest and publishes
                   them as a GitHub release.
  dataset/         Generated output (see below). Only stats.json and
                   manifest.json are committed; regenerate with generate.py.
```

Three rules decide where each thing lives: **sources live with the
compiler, generated outputs are published, and held-out tasks stay
private.** The seeds are here so a language change that breaks one fails in
the same PR. The datasets are deterministic outputs of the seeds and the
compiler, so committing them would make every regeneration a large, noisy
diff. And this repository is public: anything committed may end up in a
scraped training set, which is what training examples are for and exactly
what an eval task must avoid.

## The datasets

Regenerate everything (fast path uses a prebuilt binary):

```bash
cargo build -p tuo-cli
TUO_BIN="$(pwd)/target/debug/tuo" python3 training/generate.py
```

This writes, under `training/dataset/`:

| File | Purpose | Format | Committed |
|------|---------|--------|-----------|
| `sft_oneshot.jsonl` | Supervised fine-tuning for **fluency**: task → correct program. | Chat: `system`, `user`, `assistant`. | no, published |
| `sft_repair.jsonl` | SFT for the **feedback loop**: task → buggy attempt → *real* compiler diagnostic → corrected program. | Chat with a `tool` (name `tuo_compiler`) turn between the two `assistant` turns. | no, published |
| `stats.json` | Coverage: seed/example counts, per-concept and per-break breakdown, harvest kept/dropped counts. | JSON. | yes |
| `manifest.json` | The source commit, whether the tree was clean, the compiler version, and a SHA-256 per file above; for the eval set, only its hash and size. | JSON. | yes |

Every public seed is a training seed. Generation is byte-reproducible, so
anyone can check a published dataset: check out the manifest's
`source_commit`, build `tuo` there, regenerate, and run
`python3 training/publish.py verify` (or `verify --dir <downloads>` against
the release's files).

### Publishing

```bash
# after the seeds and code are merged to main, from a clean checkout of it:
TUO_BIN="$(pwd)/target/debug/tuo" TUO_EVAL_DIR=../tuonelang-eval python3 training/generate.py
git commit -m "Regenerate the training datasets" training/dataset   # stats + manifest
python3 training/publish.py release --dry-run
python3 training/publish.py release   # gh release create dataset-<commit> ...
```

`release` refuses unless the files match the committed manifest, the
manifest came from a clean tree, and its commit is on `origin/main`, so every
published dataset names a commit anyone can check out. It uploads the two
JSONL files, `stats.json`, and the manifest, and never the eval set.

### The held-out eval set is private

The eval tasks live in a **separate private repository** (`tuonelang-eval`)
using the same seed layout, plus a `<name>.heldout.tuo` of specs the model
is never shown. A model trained on scraped GitHub may have seen anything
public, so a score on public tasks would measure memorisation. The 16 tasks
that used to be committed here as `eval_heldout.jsonl` are treated as burned
and were replaced by a fresh private set on 2026-10-23.

Point `TUO_EVAL_DIR` at a checkout of it. `generate.py` then validates its
seeds too (held-out specs must pass; mutants are refused, since an eval seed
emits no transcripts), refuses to emit if any eval task resembles the
training material, a benchmark task, or another eval task (see
[Overlap](#overlap)), writes
`$TUO_EVAL_DIR/dataset/eval_heldout.jsonl` (records of `{task, concept,
system, reference_solution, runnable, run_exit, tier, difficulty,
heldout_specs}`), and records only that file's hash and task count in the
public manifest. Without the variable the training sets are generated
exactly as before and the manifest's `eval` is `null`. In this repository a
`.heldout.tuo` file is an error: published, its specs are not held out.

### Harvested examples

Beyond the hand-authored seeds, `generate.py` mines `sft_oneshot.jsonl` from
source the repository already keeps green (`harvest.py`):

| Origin | What | Kept green by |
|--------|------|---------------|
| `crates/tuo-stdlib/src/std/*.tuo` | every documented, specced `pub fn` as a **call site**: the task names the function and its declaration, the solution is the library's own spec moved into a user module and written with full paths (`std::core::min(2, 5)`) | `tuo-cli/tests/stdlib.rs` |
| `examples/**/src/*.tuo` | whole multi-function programs | `tuo-cli/tests/dogfood_examples.rs` |
| `corpus/correct/*.tuo` | compiler-validated correct programs | `tuo-corpus/tests/shipped_corpus.rs` |

Every harvested item is compiled **and** spec-run through the real `tuo` before
it is emitted, exactly like a seed. One that does not stand alone is *dropped*,
not shipped with hidden context: a library spec that reaches a module-private
type or helper cannot compile from a user module, and an example only teaches
if the target program compiles as shown.

The stdlib harvest used to copy each function's *body*, which taught a model to
re-implement the library — the opposite of the brief's "one obvious API, call
it". Since ADR-0038 a program calls the library directly (the toolchain loads
the modules it names), so the harvest shows the call instead. `stats.json` records
`harvested_kept`, `harvested_dropped`, and the per-origin breakdown, so the drop
rate is visible rather than silent.

Harvested records join the **one-shot set only**. The contamination check
compares every eval task and solution against them as well as the seeds.
A harvested program that copies a seed or an earlier harvested example (the
`std_*.tuo` modules several examples vendor, say) is dropped and counted as
`harvested_duplicates`.

### Overlap

An exact-match test misses the duplicates that matter: two seeds written
apart converge on the same program under different names, and tasks are
easily reworded. `similarity.py` compares programs by **shape**, with specs,
comments and `pub` dropped, local names renamed by first occurrence and
literals replaced by their kind, as the Jaccard index of their 5-token
shingles. It compares tasks by their word trigrams. `overlap.py` then refuses:

| Pair | Refused at | Why |
|------|-----------|-----|
| an eval task and training material, a benchmark task, or another eval task | program ≥ 0.7 or task ≥ 0.5, or the same function name as a benchmark task or a training seed of its concept | a held-out score on it measures recall |
| a training program and a public codegen-benchmark reference | program ≥ 0.8 or task ≥ 0.5 | a fine-tuned model would score on the benchmark by recall |
| two training seeds | program ≥ 0.9 | the copy adds nothing and doubles one solution's weight |

Renamed copies score 0.9 to 1.0. The closest pairs of different programs,
such as two loops over an array, score up to about 0.76. CI runs
`python3 training/overlap.py` on the public material. Run it with `--top 10`
to see the closest pairs before adding seeds, and with `TUO_EVAL_DIR` set to
check the eval seeds (keep that output private).

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
output. For a compile break (parse, type, or ownership) it is the first
diagnostic from `tuo --message-format=json check`; for a **spec break** — a
program that checks but whose logic is wrong (an off-by-one bound, a swapped
operator, or a seed's own `mutants`) — it is the real `tuo spec` failure report,
and the turn reads `$ tuo spec program.tuo`. Timings and temp-file paths are
normalized so regeneration is byte-reproducible. `stats.json` breaks the
transcripts down by `repair_by_kind` (parse / type / ownership / spec) as well
as by break rule.

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

1. **Add seeds** (`seeds/`) — new concepts or more tasks per concept. A seed
   is a group of files in `seeds/<concept>/`, named after the function it is
   about:

   ```
   seeds/hash-map/most_common.toml          task, notes, run_exit, tier, ...
   seeds/hash-map/most_common.tuo           the solution: a complete program
   seeds/hash-map/most_common.heldout.tuo   optional: held-out spec blocks
   seeds/hash-map/most_common.mutant1.tuo   optional: wrong programs, 1..n
   ```

   The `.toml` carries `task` (required), `notes`, `run_exit` (present exactly
   when the program has a `main` to run natively), `stdlib_deps`, a `tier`
   (`executable` / `effect` / `interpreter`; inferred when omitted and checked
   when declared, and an `executable` seed with no `main` must still build
   natively once a stub `main` is added, so a generic struct, which has no
   native layout in v0, is `interpreter`), a `difficulty` 1–5, and an
   `origin`. Each mutant must
   check and must fail a spec, and becomes a spec-failure repair transcript.
   Every `.tuo` file must be canonically formatted (`tuo fmt`), since a model
   learns its style from them. A held-out spec belongs to an eval seed in the
   private set, never here.

   `crates/tuo-cli/tests/training_seeds.rs` admits every seed on each
   `cargo test` — formatting, `check`, `spec`, `run_exit`, mutants, and stray
   files — so a language change that breaks a seed fails in the same PR, and
   `generate.py` validates them again before emitting. With `TUO_EVAL_DIR`
   set it admits the private eval seeds too; CI has no copy of them, so run
   it locally after a language change.
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
`{"task": "<verbatim task string>", "completion": "<model output>"}` — then
score it. `run_eval.py` produces one from any OpenAI-compatible endpoint
(local Ollama by default), sending each record's own system prompt and task
at temperature 0 with `tuo bench run`'s fixed seed; `--prime` adds the
language brief exactly as `tuo bench run --prime` does:

```bash
export TUO_BIN="$(pwd)/target/debug/tuo" TUO_EVAL_DIR=../tuonelang-eval
python3 training/run_eval.py --model qwen3-coder:30b --prime
python3 training/score_eval.py "$TUO_EVAL_DIR/results/qwen3-coder-30b-primed.jsonl" \
    --summary "$TUO_EVAL_DIR/results/qwen3-coder-30b-primed.score.json"
```

The completions repeat the tasks, so `run_eval.py` writes them into the
private repo and refuses a path inside this one. `--summary` writes the
metrics alone (counts and concept names, no task text), which is what may be
quoted publicly.

The scorer prints the SHA-256 of the eval set it read; quote it with a
result, and compare it with the manifest's `eval.sha256`, so a number always
names the exact set it was measured on. Do not paste eval tasks or model
completions of them anywhere public: a leaked task is burned.

It extracts the ```` ```tuo ```` block from each completion, compiles it, runs
its specs, and (for runnable tasks) runs it natively — reporting:

- **Compile@1** — fraction whose `tuo check` passes.
- **SpecPass@1** — fraction whose specs all pass.
- **TestPass@1** — fraction of tasks with held-out specs whose held-out specs
  also pass.
- **Run@1** — fraction of runnable tasks hitting the expected exit byte.

TestPass@1 and Run@1 count a completion that does not compile as a failure:
their denominators are every task with held-out specs and every runnable
task, not only those that compiled. (`tuo bench report`'s TestPass@1 is
measured over the programs that checked, so the two are not comparable.)
An empty completion fails every metric (`tuo check` accepts an empty file,
and a reasoning model that spends its token cap thinking returns nothing),
and a completion with no spec fails SpecPass@1 unless the task's reference
has none either.

The private set covers every training concept, and the scorer reports
Compile@1 and SpecPass@1 per concept, so a result can be read area by area
rather than as one number.

Sanity-check the harness itself (reference solutions must score 100%):

```bash
python3 training/score_eval.py --reference
```

### Baselines (2026-10-28)

Untuned local models on the private set (sha256 `d0b87c53…96ca1b`, 37
tasks; 30 with held-out specs, 7 runnable), one completion per task at
temperature 0 and seed 7, local Ollama 0.35.1 on a 32 GB Mac, 32k context.
"Brief" is `--prime`: the system prompt plus the language brief, about 13k
prompt tokens.

| Model | Brief | Compile@1 | SpecPass@1 | TestPass@1 | Run@1 |
|---|---|---|---|---|---|
| qwen3-coder:30b | no | 4/37 (10.8%) | 0/37 | 3/30 | 0/7 |
| qwen3-coder:30b | yes | 13/37 (35.1%) | 8/37 (21.6%) | 5/30 | 2/7 |
| gemma4:latest | no | 0/37 | 0/37 | 0/30 | 0/7 |
| gemma4:latest | yes | 11/37 (29.7%) | 7/37 (18.9%) | 6/30 | 1/7 |
| qwen3:4b | yes | 9/37 (24.3%) | 4/37 (10.8%) | 6/30 | 1/7 |

- gemma4 ran with `--reasoning-effort none`, which it honours. qwen3:4b
  reasons regardless (see `benchmarks/llm/codegen/results/README.md`) and
  ran with `--max-tokens 8192`: 19 of its 37 replies hit the cap before any
  code and score as failures, so its row measures the cap as much as the
  model. The run took four hours; the others took 4 to 18 minutes.
- Unprimed, almost nothing parses: 33 of qwen3-coder's 37 replies stop at a
  parse error. That gap, closed without a 13k-token brief, is what a
  fine-tune is for.
- Primed, the most common parse failure is `let mut` (6 of qwen3-coder's 11),
  which the system prompt rules out by name; then mismatched types (`T0001`)
  and invented library names (`R0002`).

The completions and per-task scores are in the private repo's `results/`.

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
- The standard library is called by **full path** from any program —
  `std::core::abs(x)` — with nothing to import or copy: the toolchain loads
  every module a program names (ADR-0038). The `std::array` / `std::string` /
  `std::map` heap ops and the `std::rt` effects are compiler builtins.

## Provenance and honesty

- Every emitted program compiled clean through `tuo` at generation time.
- Every repair diagnostic is the compiler's real output, not hand-written.
- The eval set never overlaps the SFT sets or the public benchmark, even
  renamed or reworded (checked, not assumed), and it is never published. No
  training seed copies a benchmark task.
- `stats.json` records exactly what was produced, including which break rules
  fired and how often — silent truncation is visible, not hidden.

If a seed ever stops compiling (say, after a language change),
`training_seeds.rs` fails in that change's own CI run, and `generate.py`
fails loudly and emits nothing rather than shipping a stale example.
