# tokenizer-lab

A data-driven harness for measuring how candidate **tuonelang** syntax
tokenizes — across **multiple** tokenizers.

## Why this exists

Syntax that is cheap under one tokenizer can be expensive under another. If
tuonelang's syntax were designed against a single tokenizer, it would silently
overfit to that tokenizer's vocabulary. The lab measures every candidate against
several tokenizers at once so that syntax decisions are informed by the spread,
not by one data point.

The measurement engine and the tokenizer-adapter interface live in the
[`tuo-bench`](../../crates/tuo-bench) crate; this directory is a thin CLI plus
the version-controlled fixtures and results.

## What it measures

For every (candidate, tokenizer) pair the harness records:

- **token count** — how many tokens the candidate encodes to;
- **byte count** and **bytes per token** — token density.

Because every candidate declares a `level` (`construct` | `function` |
`program`), these same numbers are the **tokens per construct**, **tokens per
representative function**, and **tokens per representative complete program**
figures. The `syntax-comparisons.json` fixture includes candidates at all three
levels.

## Usage

```bash
# List the built-in tokenizer adapters.
cargo run -p tokenizer-lab -- list-tokenizers

# Measure a fixture file; print JSON to stdout.
cargo run -p tokenizer-lab -- run --fixtures fixtures/syntax-comparisons.json

# Write the machine-readable report and also show a human table on stderr.
cargo run -p tokenizer-lab -- run \
  --fixtures fixtures/syntax-comparisons.json \
  --out results/syntax-comparisons.report.json \
  --table
```

The committed report (`results/syntax-comparisons.report.json`) is regenerated
by the last command. A test in `tuo-bench`
(`tests/committed_data.rs`) fails if the committed report drifts from a fresh
run, so results stay in sync with the fixtures and engine.

## Files

```
fixtures/   version-controlled INPUT: candidate syntax to compare (JSON)
results/    version-controlled OUTPUT: machine-readable measurement reports (JSON)
src/        the CLI front-end
```

## Built-in tokenizer adapters

All three are deterministic and offline (no vocabulary files, no network), so
measurements are reproducible in CI:

| id | models |
|----|--------|
| `bytes` | one token per UTF-8 byte — the baseline upper bound on token count. |
| `whitespace` | maximal identifier/whitespace runs; each punctuation char its own token. |
| `gpt-like` | a heuristic subword model (leading-space attachment + a small greedy code-fragment vocabulary). |

These are **models**, not reproductions of any specific production tokenizer's
counts. Their job is to give a *spread* of behaviors.

## Adding a new tokenizer adapter

**You never modify the measurement engine, the CLI, or the output schema to add
a tokenizer.** The only extension point is the `Tokenizer` trait.

1. Implement `tuo_bench::tokenizer::Tokenizer` for your type:

   ```rust
   use tuo_bench::tokenizer::{Token, Tokenizer};

   pub struct MyTokenizer { /* … */ }

   impl Tokenizer for MyTokenizer {
       fn id(&self) -> &str { "my-tokenizer" }
       fn description(&self) -> &str { "One-line description." }
       fn encode(&self, input: &str) -> Vec<Token> {
           // Must be deterministic. Return tokens in order.
       }
   }
   ```

   Requirements: `encode` must be **deterministic** (same input → same tokens).
   Built-in adapters are also **lossless** (concatenated token text reproduces
   the input); external-vocabulary adapters need not be lossless but must stay
   deterministic.

2. Register it. For a permanent built-in, add one line to
   `Registry::with_builtin_adapters` in
   [`crates/tuo-bench/src/tokenizer.rs`](../../crates/tuo-bench/src/tokenizer.rs).
   For an experimental or externally-sourced tokenizer, build a `Registry`
   yourself and `register(Box::new(MyTokenizer::new()))`.

That is the entire change. Because the engine iterates whatever the registry
contains, the new tokenizer automatically participates in every measurement and
appears as a new column in reports.

### Real production tokenizers (tiktoken / Claude, etc.)

These are added the same way — implement `Tokenizer` over the real BPE — but are
kept **out of the default offline set** because they require large vocabulary
data. They should live behind an optional Cargo feature or in a separate,
non-CI-blocking crate so the core harness stays deterministic and buildable
without network or large assets.

## Decision policy: token count is **not** the decision

**Do not choose tuonelang syntax solely from token count.** The lab produces one
input to a decision that must also weigh:

- **readability and learnability** for humans;
- **parsing/grammar ambiguity** (e.g. `[T]` vs `<T>` generics);
- **consistency** with the rest of the language and its conventions;
- **familiarity** and prior art;
- **robustness under machine editing**.

The fixtures deliberately include candidates that the frozen v0 Constitution did
**not** pick (e.g. `I64` costs more `gpt-like` tokens than `Int`, yet `I64`
remains the canonical explicit-width name for clarity and determinism reasons).
Keeping them makes the trade-off auditable rather than hidden. Each candidate can
carry `notes` recording exactly these non-token considerations.
