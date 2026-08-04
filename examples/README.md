# tuonelang examples

Real, multi-function tuonelang programs, dogfooded against v0 (Prompt 39). These
are not toys chosen to flatter the language: each is genuine application logic
written in whatever v0 can actually express, validated by the **real compiler**,
and re-checked on every `cargo test` by
[`crates/tuo-cli/tests/dogfood_examples.rs`](../crates/tuo-cli/tests/dogfood_examples.rs)
so they cannot rot. The findings from building them — and the ADRs those
findings produced — are written up in [`../DOGFOODING.md`](../DOGFOODING.md).

tuonelang v0 runs only the **scalar, control-flow core** (`Int`, `if`/`else`,
calls, recursion): no strings, heap, collections, I/O, or concurrency. Programs
that fit inside that core run natively; programs that don't have their **pure
decision core** written in v0 (which runs) and their effectful shell marked as a
documented `CONTRACT:` tier — exactly how `tdg-stdlib` splits executable from
contract code. Nothing here pretends to do what the language cannot.

| Example | Kind | v0 status | `main` exit |
|---------|------|-----------|:-----------:|
| [`cli-stats/`](cli-stats/) | command-line application | runs natively | 18 |
| [`data-pipeline/`](data-pipeline/) | JSON/record processing | runs natively | 144 |
| [`workspace/`](workspace/) | medium multi-package project (`app → geometry → numeric`) | builds natively | 26 |
| [`http-service/`](http-service/) | HTTP service *(routing core; I/O is contract-tier)* | routing core runs | 6 |
| [`concurrent-worker/`](concurrent-worker/) | concurrent worker *(scheduling core; execution is contract-tier)* | scheduling core runs | 15 |

## Running them

```bash
# A single-file program: check, run its specs, execute it.
tuo check examples/cli-stats/src/main.tuo
tuo test  --manifest examples/cli-stats
tuo run   examples/cli-stats/src/main.tuo ; echo $?   # 18

# The multi-package workspace: check/test the graph, then build + run the binary.
# (`tuo run` is file-based, so a package binary is built and then executed —
#  see DOGFOODING.md finding D-7.)
tuo test  --manifest examples/workspace/app            # 14 specs across 3 packages
tuo build --manifest examples/workspace/app -o /tmp/app && /tmp/app ; echo $?   # 26
```

Each package resolves a `tdg.lock` on build; those lockfiles embed
machine-absolute dependency paths and are therefore **not committed** (they are
gitignored under `examples/`, and regenerated on demand — DOGFOODING.md finding
D-6).
