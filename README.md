<img width="1402" height="1122" alt="tuonelang" src="https://github.com/user-attachments/assets/586df834-9014-4ef6-937a-d32f4d46a935" />

# tuonelang

tuonelang is an experimental statically typed, memory-safe, native programming
language and compiler, implemented in Rust and designed for both human
programmers and AI coding agents.

> **Naming.** The language is **tuonelang**. It is designed around **TDG —
> Test Driven Generation**, a development paradigm in which colocated,
> executable specifications drive and validate machine-generated code. TDG is
> the *methodology tuonelang is built for*, not the name of the language. The
> CLI binary is `tuo` and all crates use the `tuo-` prefix.

## Capabilities

The compiler is implemented end to end — front end, reference interpreter, and
both native backends. tuonelang programs written against the **v0 runnable
core** compile, spec-check, and run natively on both backends, in lock-step with
the interpreter.

- ✅ Front end: lexer → parser (lossless CST) → resolver → type checker →
  ownership checker, with human and machine-versioned diagnostics.
- ✅ Colocated executable `spec` blocks, run through a reference MIR interpreter.
- ✅ Native compilation: a Cranelift debug backend and an optimizing LLVM
  `--release` backend, kept interpreter-equivalent by differential test suites.
- ✅ A standard library **written in tuonelang** and consumed as compiler input —
  twelve modules (`core`, `collections`, `math`, `str`, `json`, `io`, `fs`,
  `net`, `time`, `process`, `sync`, `test`), each public function either spec'd
  or pinned by a native test.
- ✅ Tooling: canonical formatter, package system (manifest + lockfile + path
  deps), LSP core, an agent protocol server, a compiler-validated corpus, and
  benchmark harnesses — including a performance lab whose fifteen runtime
  workloads all measure against equivalent C and Go peers.
- 📋 The **0.1 release gate** (`specification/RELEASE-0.1-GATE.md`) currently
  reads **READY** — all sixteen criteria are `MET`, each backed by a committed
  proving artifact.

The runnable core has grown one ADR at a time: scalar arithmetic, control flow,
and recursion; **structs, enums, `Option`/`Result`, fixed `[T; N]` arrays, and
bounded `for`** (ADR-0004); **IEEE-754 floats and borrow-mode (`in`/`mut`)
calls**; the **borrowed `Str` core and effect boundary** (ADR-0006); the
**allocator core** — owned `String` and growable `Array[T]` on real heap memory
(ADR-0009, element-generic since ADR-0012); **first-class non-capturing
function values** with the generic higher-order stdlib combinators (ADR-0008
Tier 1); the **hash map** `Map[K, V]` (ADR-0011); **structured fork-join
concurrency** over the one primitive `std::rt::par_map` (ADR-0007); the **OS
effect boundary** — clock, argv, and files (ADR-0013); **TCP sockets**
(ADR-0014); **channels and mutexes** (ADR-0015); the **data increment** —
`Float` array elements, indexed writes, and `std::json` (ADR-0016); and
**bounded waits, IPv6, and UDP** (ADR-0017). Every ADR is `accepted`; **none
remains `proposed`**.

Still outside the core, and refused rather than mis-compiled: **capturing
closures** (ADR-0008 Tier 2), the heap-wrapper **values** `Box`/`Shared`/`Weak`,
recursive nominal types without a heap-wrapper indirection (a front-end error
since ADR-0016), and **TLS/DNS** — deliberately out of ADR-0017, since TLS would
need a crypto dependency this workspace avoids and DNS belongs *in tuonelang* on
the UDP primitives.

Because tuonelang v0 ships a deliberately bounded core, the compiler
**refuses** programs outside it rather than mis-compiling them — no component
advertises behavior it cannot perform.

## Quick start

Build the `tuo` binary and run a program. Save this as `hello.tuo`:

```tuo
module hello;

/// Add two integers.
fn add(take a: Int, take b: Int) -> Int {
    a + b
}

fn main() -> Int {
    add(40, 2)
}

spec add {
    then add(2, 3) == 5;
    then add(-1, 1) == 0;
}
```

Then, from the workspace root:

```bash
cargo run -p tuo-cli -- check hello.tuo   # parse, resolve, type-check, ownership-check
cargo run -p tuo-cli -- spec  hello.tuo   # run the colocated specs (1 passed)
cargo run -p tuo-cli -- run   hello.tuo   # compile natively and run; exit status = main() = 42
```

`run` exits with `main`'s value truncated to a byte. Add `--release` to compile
through the optimizing LLVM backend, or `build [-o <path>]` to produce an
executable without running it.

For real, multi-function programs written against v0, see [`examples/`](examples/):
`cli-stats` prints its four-line report through the effect boundary,
`data-pipeline` folds heap-backed `Array[Record]` values through a generic
first-class `fold` and cross-checks them against a `Map[Int, Int]` aggregation,
`http-service` **serves itself over a live loopback socket**, `concurrent-worker`
runs a real `par_map` thread pool and drains a shared channel queue, and
`workspace/` is a three-package graph that checks and tests green. Every one is
re-validated by the real `tuo` binary on each `cargo test`. The findings from
building them — and the ADRs those findings produced — are in
[`DOGFOODING.md`](DOGFOODING.md).

## CLI

Every command reports human-readable text by default, or the versioned machine
protocol with `--message-format json` / `json-lines`.

| Command | What it does |
|---------|--------------|
| `tuo check <files>` | Front end only: parse → resolve → type-check → ownership-check (specs checked, not executed). |
| `tuo spec [--target <fn>] <files>` | Execute colocated specs through the reference MIR interpreter. |
| `tuo verify [--affected-by <file>] <files>` | All static checks **and** the specs; `--affected-by` runs only the specs an edit to that file could affect. |
| `tuo fmt [--check] <files>` | Rewrite into canonical format (deterministic, idempotent, zero config). |
| `tuo build [-o <path>] [--release] <files>` | Compile to a native executable (Cranelift debug / LLVM release). |
| `tuo run [--release] <files>` | Compile and run; exit status is the program's own. |
| `tuo new <name>` · `add` · `remove` · `test` · `package symbols` | Package lifecycle over a `tdg.toml` manifest and `tdg.lock`; `package symbols` queries a package's real exported symbols. |
| `tuo corpus validate [--category <c>] <files>` | Run a program through the compiler-validated corpus pipeline and report its per-stage results. |
| `tuo bench report <tasks> <run>` | Score a recorded LLM code-generation run by recompiling its outputs through the real compiler. |
| `tuo cheatsheet` | Emit a dense, context-injectable language brief generated from the compiler's own sources. |
| `tuo agent --stdio` | Serve the versioned JSON-lines agent protocol (one long-lived compiler DB). |
| `tuo debug syntax\|ast\|hir\|mir <file>` | Developer dumps (unstable output, not a language protocol). |

See [`CLAUDE.md`](CLAUDE.md) for the full command surface and the conventions
each command upholds.

## Learning tuonelang

Material for both audiences the language is designed for.

**For humans**

| Document | What it is |
|----------|------------|
| [`REFERENCE.md`](REFERENCE.md) | The complete programmer's guide to writing tuonelang today — types, ownership, aggregates, heap collections, maps, specs, modules, the stdlib, traps, diagnostics, gotchas, and a worked example. Everything in it reflects what the compiler actually accepts, and its §16 *honesty map* says exactly what runs where. |
| [`DOGFOODING.md`](DOGFOODING.md) | What using v0 for real programs revealed, measured across compiler usability, diagnostics, incremental builds, LLM generation, stdlib gaps, and runtime performance. |
| [`specification/`](specification/) | The normative documents: the v0 [Constitution](specification/CONSTITUTION.md), grammar, [static semantics](specification/static-semantics.md), [ownership](specification/ownership.md), [MIR](specification/mir.md), and [ABI](specification/abi.md). |
| [`specification/adr/`](specification/adr/) | Why each design decision was made, one ADR at a time — the durable record of the language's growth. |

**For AI coding agents**

tuonelang treats machine-generated code as a first-class use case, so the
compiler exposes its knowledge rather than making an agent guess:

- `tuo agent --stdio` serves a versioned JSON-lines **compiler-intelligence
  protocol** over one long-lived database — diagnostics, types, definitions,
  references, symbols, signatures, available imports, spec execution, and
  compiler-authored safe fixes. Its *generation* queries (`expected_type_at`,
  `visible_symbols_at`, `valid_members_of`, `call_signature`, …) help write the
  next token, and each one states in-band whether its answer is exhaustive —
  never over-claiming a power the compiler lacks.
- [`tuonelang-cheat-sheet.txt`](tuonelang-cheat-sheet.txt) is a dense language
  brief to paste into a model's context before asking it to write tuonelang —
  syntax skeleton, the real stdlib surface, the runnable-core boundary, and the
  cross-language anti-patterns. It is **generated** by `tuo cheatsheet` from the
  compiler's own sources (ADR-0018): every listed signature is a declaration the
  compiler accepted, every sample is compiled by CI, and the committed copy is
  pinned byte-for-byte against fresh output — so it cannot drift into teaching a
  model something the compiler rejects.
- `--message-format json` / `json-lines` gives every command a versioned machine
  protocol, so feedback is parsed, not scraped.
- [`training/`](training/) is a **compiler-validated** fine-tuning corpus
  generator: task → correct program, multi-turn repair transcripts (buggy
  attempt → the compiler's real diagnostic → fix — the TDG loop), and a held-out
  eval set scored by really compiling the model's output. Nothing is emitted
  that the real `tuo` did not accept, and examples harvested from the stdlib,
  the dogfooding examples, and the validated corpus are dropped rather than
  shipped with hidden context.
- [`corpus/`](corpus/) holds compiler-validated programs in six categories
  (correct plus four repair categories and repository-level changes); a repair
  entry is admitted only if it fails at *exactly* the stage its category names.
- `tuo bench report` scores a recorded code-generation run by **recompiling**
  the model's outputs — a fabricated result cannot survive.

## Repository layout

```text
crates/          Rust crates (compiler pipeline, tooling, runtime, CLI)
specification/   Constitution, grammar, normative semantics docs, ADRs, the 0.1 gate
tests/           Stage-organized compiler tests (lexer, parser, types, ownership, mir, …)
benchmarks/      Compiler, runtime, and LLM-codegen benchmarks
corpus/          Compiler-validated tuonelang programs (six categories)
examples/        Real multi-function programs dogfooding the v0 core
training/        Compiler-validated fine-tuning material for a tuonelang model
tools/           Developer tooling
```

The workspace is a strictly **layered compiler pipeline**
(`source → lex → parse → resolve → type check → ownership → MIR → codegen`); a
crate never depends on one later in the pipeline than itself. See
[`ARCHITECTURE.md`](ARCHITECTURE.md) for the crate graph,
[`specification/CONSTITUTION.md`](specification/CONSTITUTION.md) for the frozen
v0 design, [`specification/README.md`](specification/README.md) for the
normative documents (grammar, static semantics, ownership, MIR, ABI), and
[`CONTRIBUTING.md`](CONTRIBUTING.md) for development rules.

## Development

tuonelang uses the current **stable** Rust toolchain (pinned via
`rust-toolchain.toml`) and Rust **edition 2024**.

```bash
cargo build --workspace
cargo test  --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --check
cargo run -p tuo-cli -- --help
```

The LLVM `--release` backend requires LLVM 19; the debug (Cranelift) path and
every command above need no external toolchain beyond a C compiler for linking.

## License

Licensed under the [MIT License](LICENSE).
