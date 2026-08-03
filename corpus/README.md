# tuonelang corpus

This directory holds tuonelang source programs that have been **compiler-validated**
by the corpus pipeline. No program is trusted on assertion: an entry earns its
place only by passing the required, ordered validation gauntlet driven over the
real compiler stages.

```
format → parse → resolve → type check → ownership → MIR verify →
    specs/tests → native execution (where applicable)
```

The pipeline, the metadata each entry records, and the admission rules live in
the `tuo-corpus` crate; `tuo corpus validate <files>` runs it from the CLI (the
CLI injects native execution — compile, link, and run — as the final stage).

## The six corpora

The corpus is not one bucket. Candidates come from four origins — humans,
generators, LLMs, and transformed benchmark tasks — and are sorted into six
categories, each with its **own** admission contract:

| Directory | Contract |
|-----------|----------|
| `correct/` | passes the whole pipeline |
| `syntax_repair/` | fails at parse (a lexical/parse error) |
| `type_repair/` | parses and resolves, fails at type check |
| `ownership_repair/` | type-checks, fails at ownership |
| `spec_repair/` | statically valid, a spec fails |
| `repository_change/` | a multi-file change validated as a whole (one candidate per subdirectory) |

A repair program is admitted **only if it fails at exactly the stage its
category names** — so a "type-error" entry is a genuine type error, not a
mislabeled parse error. Every fixture stored here is checked against its
category's contract by `crates/tuo-corpus/tests/shipped_corpus.rs`, so the corpus
can never silently drift into dishonesty.

## Metadata

Each admitted entry carries a full metadata record: the language version it was
validated against, its source origin, the language features it uses, the
per-stage validation results, a coarse complexity measure, and token counts
under every deterministic tokenizer the research harness ships. In a machine
format, `tuo corpus validate` emits the whole record as a protocol item.

## Formatting

All corpus programs must be canonically formatted (`tuo fmt --check` clean) — the
formatter defines tuonelang's single canonical source representation, and it is
the pipeline's first stage. A repair fixture that is broken *after* the format
stage is still canonically formatted (the formatter is conservative on the
malformed region), so it reaches the stage its category tests.
