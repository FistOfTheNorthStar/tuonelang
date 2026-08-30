# ADR-0018: The context-injectable cheat sheet — a generated, compiler-backed language brief

- **Status:** accepted (2026-08-30)
- **Date:** 2026-08-30
- **Context:** tuonelang's stated purpose is to be friendly to *both* human
  programmers and LLM coding agents. For the agent half, the project has
  invested heavily in the **feedback** direction — the machine diagnostics
  schema, the agent protocol's generation queries, the compiler-validated
  corpus, the code-generation benchmark, the training-data generator. All of
  these help a model *recover* from a mistake it has already made.

  The **priming** direction has no artifact. A model writing tuonelang for the
  first time — a local model behind an Ollama-style runner, a coding agent with
  no tuonelang in its pretraining distribution, a fresh context window — has
  nothing dense enough to drop in front of a task. What exists today is aimed
  at humans reading linearly:

  - [`REFERENCE.md`](../../REFERENCE.md) is a 1000-line programmer's guide whose
    opening section alone spends 25 lines on ADR provenance. It is the right
    document for a person learning the language and the wrong one for a context
    window: most of its mass is prose explanation, and the facts a generator
    needs (which spellings exist, which names exist, what is refused) are
    distributed across it rather than concentrated.
  - [`specification/grammar.ebnf`](../grammar.ebnf) is precise but is a grammar,
    not a usage brief: it says what parses, not what to write.
  - The stdlib catalog holds the real symbol surface, but only as twelve `.tuo`
    module sources — a model would have to read and summarize them itself,
    spending the very context the brief is meant to save.

  So the gap is real, and the naive fix is worse than the gap. A hand-written
  `cheat-sheet.txt` would be **the one document in this repository that asserts
  rather than proves.** Every other artifact that makes a claim about the
  language derives it from the real compiler and refuses to record what the
  compiler did not produce: `tuo-corpus` admits no program on assertion,
  `tuo-codegen-bench` recompiles rather than trusting a recorded verdict,
  `tuo-bench` reports `Skipped` rather than a one-sided number, `training/`
  refuses a seed that does not compile, and the release gate proves every cited
  artifact exists. A cheat sheet is precisely the kind of document that rots
  silently: it is read by machines, not humans, so a stale signature or a
  removed function produces a confidently wrong generation rather than a visible
  error. Prose that teaches a model to write code the compiler rejects is worse
  than no prose at all — it actively poisons the generation it was meant to
  prime.

  There is also a measurement question. "LLM-friendly" is the project's central
  claim, and this ADR proposes an artifact whose entire purpose is to improve
  generation quality. The repository already holds itself to a numeric standard
  in exactly this situation — `tuo-agent`'s generation queries ship with
  `generation_benchmark.rs`, and the stdlib symbol surface ships with
  `stdlib_hallucination.rs`, both of which *really compile* each candidate to
  score it. An artifact claiming to prime a model should meet the same bar
  rather than resting on plausibility.

- **Decision:** add a **generated** cheat sheet — `tuo cheatsheet`, a new CLI
  subcommand that emits a dense language brief assembled from compiler-owned
  sources, plus a committed copy at the repository root that CI proves current.
  No section of the output is authored as free prose about the language; every
  factual claim is either extracted from a compiler-owned source or is a code
  sample the emitting test compiles.

  **The five sections, and where each is derived from.**

  1. **Syntax skeleton** — from [`grammar.ebnf`](../grammar.ebnf), carrying its
     `GRAMMAR-VERSION` marker. The brief shows the canonical spelling of each
     construct (module, function with parameter modes, `let`/`var`, `if`/`while`/
     `for`, `match`, `struct`/`enum`, `spec`), not the full grammar. Because the
     version marker travels with the text, a brief generated against a different
     grammar version is identifiable as such.
  2. **Real stdlib symbol surface** — from `Resolution::symbols()` and
     `TypeckResult::type_of`, exactly the query behind `tuo package symbols` and
     the agent protocol's `visible_symbols_at`. Every listed function is one the
     front end actually resolved from the twelve `tuo-stdlib` catalog modules,
     rendered with its real signature via `Ty::render`. This is the section that
     makes the brief worth its tokens: `stdlib_hallucination.rs` already
     demonstrates that grounding a model in the real symbol surface moves a
     plausible-but-wrong-name corpus from 0% to 100% Compile@1.
  3. **The runnable-core boundary** — the distinction between what `tuo check`
     accepts and what `tuo run`/`tuo build` execute. This is tuonelang's single
     most confusing property for a generator, since a program can pass the front
     end and still be refused at storage-classification time. The brief states
     the boundary explicitly, including what is deliberately absent (capturing
     closures, `Box`/`Shared`/`Weak` values, detached spawn) so a model does not
     invent them.
  4. **Anti-patterns** — from [`training/breaks.py`](../../training/breaks.py),
     which already enumerates, with rationales, the exact cross-language
     mistakes models make when writing tuonelang: `use`-for-import,
     `<>`-generics, positional `Some(x)`, `let mut`, compound assignment,
     omitted parameter modes, chained comparison. That file exists to *inject*
     these errors for repair-training data; the same list read forward is a
     precise "do not write this" block. One list, two consumers, no drift.
  5. **A complete worked program** — one short program exercising the core, which
     the emitting test compiles and runs to a pinned exit byte.

  **The honesty mechanism.** `crates/tuo-cli/tests/cheatsheet_command.rs` drives
  the real binary and asserts:

  - every `tuo` code sample in the emitted brief **really compiles** through
    `check_sources` (the worked program additionally runs to its stated exit
    byte);
  - every stdlib signature shown **matches** what the front end currently
    resolves, field for field;
  - every anti-pattern shown as wrong **is really rejected**, and — the
    load-bearing half — its corrected form **is really accepted**, so the brief
    cannot teach a model to avoid something legal;
  - the committed root copy is **byte-identical** to freshly generated output,
    so a language change that invalidates the brief fails CI in the same commit
    rather than silently shipping stale guidance.

  This mirrors `shipped_corpus.rs` re-admitting every committed fixture and
  `shipped_tasks.rs` re-verifying every task pin: the committed artifact is not
  trusted, it is re-derived and compared.

  **The measurement.** The brief's value is scored the way the repository scores
  every other generation-quality claim: a deterministic Compile@1 proxy over a
  task corpus whose naive guess is wrong, with each candidate *really* compiled,
  reporting unprimed versus primed. Consistent with `generation_benchmark.rs`
  and `stdlib_hallucination.rs`, this is a proxy for the brief's discriminative
  power, **not a live-LLM evaluation** — no model provider is embedded anywhere
  in this workspace, and the benchmark says so in-band.

  **Deliberately out of scope.** No model is embedded or called. The brief is
  not a tutorial and does not replace `REFERENCE.md` for human readers. It is
  emitted as plain text with no markdown-renderer assumptions, since its
  destination is a context window rather than a browser. Per-model token
  budgeting is left to `tools/tokenizer-lab`, which already measures tuonelang
  text across multiple tokenizers and is the right home for that question.

- **Consequences:**

  *Easier.* A local or embedded model can be primed on tuonelang in one paste,
  which is the difference between a language an agent can use today and one it
  must be fine-tuned for first. The brief also becomes the natural system-prompt
  preamble for `tuo-codegen-bench` adapters and a ready-made prompt prefix for
  the `training/` generator, so the priming and feedback halves of the project
  finally share one source of truth about what the language *is*.

  *Harder.* There is a new artifact that CI must keep current, and any language
  change now has one more committed file to regenerate. This cost is intentional
  and is the whole point: the alternative is not "no cost" but "a document that
  drifts invisibly". Regeneration is one command, and the failure mode is a loud
  test failure naming the stale section.

  *Trade-off accepted.* Deriving from compiler sources constrains what the brief
  can say — it cannot offer hand-tuned pedagogical framing where no compiler
  source backs it. That is the correct trade for this artifact: a brief that
  reads slightly drier but is never wrong beats a friendlier one that
  confidently teaches a removed function. Where genuine prose judgment is
  needed, `REFERENCE.md` remains the place for it.

  *Follow-up.* If measurement shows a section carrying little discriminative
  weight, it should be cut rather than kept for completeness — context is the
  scarce resource the brief exists to conserve. The per-tokenizer size question
  belongs to `tools/tokenizer-lab` when it is asked.
