# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**tuonelang** is an experimental statically typed, memory-safe native language and
compiler, designed to be friendly to both human programmers and LLM coding agents.
The repository directory is `tuonelang`; the crate/package prefix is `tuo-` and the
compiler binary is `tuo`.

The language is built around **TDG — Test Driven Generation**: a development paradigm
in which colocated, executable specifications drive and validate machine-generated
code. TDG is the *methodology tuonelang is built for*, not the name of the language,
so it survives in the methodology framing and in the `tdg.toml` / `tdg.lock` package
filenames — but every crate is `tuo-*` and the binary is `tuo`.

**Current state: the compiler is implemented.** The front end (lexer → parser →
resolver → type checker → ownership checker), the reference MIR interpreter, both
native backends (Cranelift debug, LLVM release), and the tooling surfaces all work;
tuonelang programs written against the **v0 runnable core** compile, spec-check, and
run today. Each `lib.rs` doc comment states what the crate owns and which layers it
may depend on. When adding functionality, respect the boundary each stub describes
rather than reshaping the graph.

## Commands

```bash
cargo build                    # build the whole workspace
cargo build -p tuo-cli         # build a single crate (produces the `tuo` binary)
cargo run -p tuo-cli -- --help # run the CLI
cargo run -p tuo-cli -- check file.tuo         # parse, resolve, type-check, ownership-check (specs included)
cargo run -p tuo-cli -- spec file.tuo          # execute the program's specs (MIR interpreter)
cargo run -p tuo-cli -- spec --target f file.tuo # run only the specs of function `f`
cargo run -p tuo-cli -- verify file.tuo        # all static checks + execute specs
cargo run -p tuo-cli -- verify --affected-by a.tuo a.tuo b.tuo # …only specs an edit to a.tuo could affect
cargo run -p tuo-cli -- debug syntax file.tuo  # dump the lossless CST (dev tool)
cargo run -p tuo-cli -- debug ast file.tuo     # dump the typed AST views (dev tool)
cargo run -p tuo-cli -- fmt file.tuo           # rewrite into canonical format
cargo run -p tuo-cli -- fmt --check file.tuo   # verify canonical formatting (exit 1 if not)
cargo run -p tuo-cli -- build file.tuo         # compile to a native executable (Cranelift, debug)
cargo run -p tuo-cli -- build -o out file.tuo  # …to a chosen path
cargo run -p tuo-cli -- build --release file.tuo # …optimized, via the LLVM backend
cargo run -p tuo-cli -- run file.tuo           # compile and run; exit status = the program's result
cargo run -p tuo-cli -- run --release file.tuo # …compile with the optimizing LLVM backend, then run
cargo run -p tuo-cli -- new app                # scaffold a new package (tdg.toml + src/main.tuo)
cargo run -p tuo-cli -- add util --path ../util # add a path dependency; re-resolve tdg.lock
cargo run -p tuo-cli -- remove util            # drop a dependency; re-resolve tdg.lock
cargo run -p tuo-cli -- check                  # …with no files: resolve+check the package in `.`
cargo run -p tuo-cli -- build [--release]      # …resolve+compile the package (checksum-verified deps)
cargo run -p tuo-cli -- verify                 # …resolve+static-check+run the package's specs
cargo run -p tuo-cli -- test                   # run the package's tests (its specs) across the graph
cargo run -p tuo-cli -- --message-format=json package symbols # a package's real exported symbols
cargo run -p tuo-cli -- corpus validate file.tuo # validate a program through the compiler-validated corpus pipeline
cargo run -p tuo-cli -- corpus validate --category type-repair a.tuo # …proving it fails at exactly one stage
cargo run -p tuo-cli -- --message-format=json corpus validate file.tuo # …with the full metadata record as a protocol item
cargo run -p tuo-cli -- cheatsheet             # emit the context-injectable language brief (ADR-0018)
cargo run -p tuo-cli -- --message-format=json cheatsheet # …the brief as a protocol item
cargo run -p tuo-cli -- bench report tasks.json run.json # score a code-gen benchmark by recompiling the model's outputs
cargo run -p tuo-cli -- --message-format=json bench report tasks.json run.json # …the metric summary as a protocol item
cargo run -p tuo-cli -- debug hir file.tuo     # dump the lowered HIR (dev tool)
cargo run -p tuo-cli -- debug mir file.tuo [fn] # dump the lowered MIR (dev tool)
cargo run -p tuo-cli -- debug mir --opt file.tuo # …after the MIR optimization passes
cargo run -p tuo-cli -- --message-format=json verify file.tuo      # machine protocol: one versioned envelope
cargo run -p tuo-cli -- --message-format=json-lines spec file.tuo  # machine protocol: streamed, one event per line
cargo test                     # run all tests
cargo test -p tuo-cli          # test one crate
cargo test -p tuo-cli command_definition_is_valid  # run a single test by name
cargo clippy --all-targets     # lint; workspace lints are -D warnings in CI
cargo fmt                      # format (stable rustfmt, config in rustfmt.toml)
```

Toolchain is pinned to **stable** (`rust-toolchain.toml`), edition **2024**, resolver 3.

## Architecture

The workspace is a strictly **layered compiler pipeline**. A crate must never depend on
a crate later in the pipeline than itself — the stub doc comments spell out each crate's
allowed dependencies, and this discipline is the core invariant to preserve.

```
source → lex → parse → resolve → type check → ownership → MIR → codegen
```

Crates, lowest layer first:

- **`tuo-source`** — source files, byte offsets, spans, source maps. Foundation; depends on nothing.
- **`tuo-diagnostics`** — structured, machine-readable diagnostics.
- **`tuo-db`** — incremental, query-based compiler database. The shared computation layer
  that CLI, LSP, and the agent protocol all drive, so semantics are computed once. Depends
  only on `tuo-source` and `tuo-diagnostics`, so it owns the *stage-agnostic* red-green
  engine (`QueryEngine`, driven by a host's `QueryHost::compute`), **not** the stage wiring —
  the real per-stage queries are registered by `tuo-compiler` (which sees every stage). No
  engine type leaks into a host's public API; the boundary speaks source identities, plain
  values, and `QueryError`.
- **`tuo-lexer`** → **`tuo-syntax`** (lossless CST) → **`tuo-parser`** → **`tuo-ast`**.
- **`tuo-hir`** — desugared HIR lowered from the AST; shared input to resolution and typing.
- **`tuo-resolve`** (name/path resolution) → **`tuo-types`** (inference/checking) →
  **`tuo-ownership`** (memory-safety checking).
- **`tuo-mir`** — the single executable semantic representation; **`tuo-mir-interp`** is its
  reference interpreter.
- **`tuo-codegen`** — backend-agnostic codegen interface, with two backends behind it:
  **`tuo-codegen-cranelift`** and **`tuo-codegen-llvm`** (both consume MIR).
  **`tuo-runtime`** provides minimal native runtime support.

**`tuo-compiler`** is the facade: the single orchestration seam that wires the stages end
to end so the CLI, LSP, and agent protocol don't each re-wire the pipeline. It depends on
the semantic-stage crates up through the `tuo-codegen` abstraction, **never on a concrete
backend and never on CLI presentation**. Keep it to orchestration + re-exports; stage
*logic* lives in the individual stage crates.

Tooling surfaces on top of the facade: **`tuo-cli`** (the `tuo` binary), **`tuo-lsp`**
(language server), **`tuo-agent`** (exposes compiler feedback to coding agents),
**`tuo-fmt`** (formatter), **`tuo-package`** (package/build orchestration),
**`tuo-stdlib`**, **`tuo-spec`** (colocated executable specs), **`tuo-bench`**,
**`tuo-corpus`** (the compiler-validated corpus pipeline),
**`tuo-codegen-bench`** (the code-generation evaluation harness), and
**`tuo-fuzz`** (the whole-compiler fuzzing harness).

Test corpora and fixtures live in top-level `tests/` (by stage: `lexer`, `parser`,
`types`, `ownership`, `mir`, `codegen`, `diagnostics`, `differential`, `specs`),
plus `benchmarks/`, `corpus/`, `examples/`, and `specification/adr/`.

## Conventions

- **`unsafe_code` is `forbid`** workspace-wide. `missing_docs` is `warn` — every public
  item needs a doc comment. Lint levels are centralized in root `Cargo.toml`
  (`[workspace.lints]`); `clippy.toml` only holds tunables.
- `print_stdout`, `print_stderr`, `dbg_macro`, `todo`, and `unimplemented` are lint-warned
  (and CI is `-D warnings`). Don't leave them in committed code.
- **The CLI must never advertise behavior the compiler can't perform.** New
  subcommands appear only once their functionality exists; the `Command` enum in
  `tuo-cli/src/cli.rs` is the extension point. Implemented so far:
  `tuo check <files>` (the parse → resolve → type-check → ownership-check front end,
  specs included per ADR-0002 — specs are checked but **not** executed here),
  `tuo spec [--target <name>] <files>` and `tuo verify [--affected-by <file>] <files>`
  (execute the program's colocated specs through the reference MIR interpreter — `spec`
  runs the selected specs, `verify` runs all static checks *and* the specs; both refuse a
  program with front-end errors, run each spec in the interpreter's deterministic sandbox
  with configurable fuel/recursion/memory limits, and report measured timing with no
  latency promise; `verify --affected-by <file>` runs only the specs whose semantic
  dependency closure touches a symbol defined in `<file>` — the specs an edit to it could
  have changed, selected through the incremental dependency graph — and a cold `verify`
  with no such flag runs every spec), `tuo fmt [--check] <files>` (the canonical
  formatter — deterministic,
  idempotent, zero configuration), `tuo build [-o <path>] [--release] <files>` and
  `tuo run [--release] <files>` (compile the accepted program to verified MIR, run the
  tuonelang-native MIR optimization passes over it, and then to native code, linking the
  runtime trap shim into an executable — the default debug build uses the Cranelift
  backend, `--release` uses the optimizing LLVM backend; `run` additionally executes it
  and exits with the program's own status, the integer its nullary `main` returns; both
  backends lower the **v0 runnable core** — the scalar control-flow core plus the
  ADR-0004 aggregates (structs, enums, `Option`/`Result`, fixed `[T; N]` arrays
  with checked indexing, bounded `for`), floats (IEEE-754, saturating `as`
  casts, `%` via the C `fmod`), borrow-mode (`in`/`mut`) calls (a pointer
  to the caller's place, per `specification/abi.md`), — since ADR-0006 —
  the borrowed `Str` (a two-word fat pointer; literals as read-only static
  data; equality; the trapping `std::str` byte ops) and the `std::rt` effect
  primitives (`write`/`read_byte`/`exit`, via the runtime's effect shim), and
  since ADR-0009 — the **allocator core**: owned `String` and growable
  `Array[Int]` (the three-word `{ptr, len, cap}` header, allocating and freeing
  real heap memory through the linked `tuo_rt_alloc`/`tuo_rt_dealloc` shim with
  drop glue; the `std::string`/`std::array` builtin ops and
  `std::rt::write_string`), laid out by `tuo-runtime`'s `abi` module (ABI v4);
  since ADR-0012 — the **element-generic array surface** over the whole
  checker-accepted element set (`Int`/`Bool`/`Str`/`String` and structs/enums
  whose fields are supported), including the owned-element increment: `get` of
  a heap-owning element is a recursive **deep copy** and drop is recursive
  per-element glue matching the interpreter's clone/drop, with
  `std::string::as_str` lowered as a zero-copy two-word view (ADR-0010 Stage
  B); — since ADR-0008 Tier 1 — **first-class (non-capturing) function
  values**: a bare top-level `fn` name is a `Const::Fn` `Copy` code pointer of
  type `fn(mode T, …) -> R` (`layout_of(Ty::Fn)` a pointer), called
  indirectly through `Callee::Indirect` with the identical direct-call ABI
  (sret + borrow args included), pinned three-way; — since ADR-0011 — the
  **hash map** `Map[Int, Int]`/`Map[Str, Int]` (the same three-word header
  over dense insertion-ordered entries; every op beyond `len`/`empty` lowers
  to the linked `tuo_rt_map_*` shim, whose hidden open-addressing index uses
  the vector-pinned splitmix64/FNV-1a hashes — unobservable, since `keys` is
  insertion order — ABI v6); — since ADR-0007 — the one concurrency
  primitive `std::rt::par_map` (structured fork-join over POSIX threads via
  `tuo_rt_par_map`, a typed effect); — since ADR-0013 — the **OS effect
  boundary**: the six further `std::rt` primitives `now_nanos` (the
  monotonic clock), `arg_count`/`arg_byte` (argv, captured by the runtime
  before `main`), and `open`/`close`/`remove_file` (files, composing with
  the ADR-0006 descriptor seam); — since ADR-0014 — the **socket
  effects** `listen`/`bound_port`/`accept`/`connect` (IPv4 TCP descriptor
  producers on the same seam — a socket is a descriptor, so
  `write`/`read_byte`/`close` move and release the bytes); — since
  ADR-0015 — the **channels and mutexes**
  `chan_new`/`chan_send`/`chan_recv`/`chan_close` and
  `mutex_new`/`mutex_lock`/`mutex_unlock` (runtime-owned process-lived
  handles; non-negative payloads cross threads by copy, so ADR-0007's
  no-data-race property survives — ABI v9); and — since ADR-0016 — the
  **data increment**: `Float` array elements, the in-place
  `std::array::set` (bounds-trapping indexed write with old-element drop),
  and the `T0016` recursion boundary (a struct/enum reaching itself without
  a heap-wrapper indirection is now a *front-end* error — previously it
  type-checked and hung codegen); and — since ADR-0017 — the **socket
  seam's additive increment** (ABI v10): the bounded waits
  `accept_timeout`/`connect_timeout`/`read_byte_timeout` (a distinct `-3`
  timeout sentinel, so a timeout is never confused with a host error or end
  of input), **IPv6** (`connect` infers the family from a numeric address,
  so `"::1"` needs no new spelling; the server side gains
  `listen6`/`peer_family`), and **UDP**
  (`udp_bind`/`udp_send`/`udp_recv`/`udp_byte_at`/`udp_peer_port` — a
  datagram is a *message*, so a receive reports its boundary and stages the
  payload, which `udp_byte_at` indexes; the stream-side `read_byte` is
  deliberately untouched); and — since ADR-0019 Stage A — the **bitwise
  operators** `&`/`|`/`^`/`~`/`<<`/`>>` on integers (conventional
  precedence, integers-only — never `Float` or `Bool`; `>>` is arithmetic on
  a signed type and logical on an unsigned one; a shift amount outside
  `0..width` **traps** `InvalidShift` rather than adopting the target's
  shift-masking, so all three engines agree; `|` is one token serving both
  pattern alternation and bitwise-or, disambiguated by grammatical context;
  no new type, no ABI change) — and *refuse* — never
  mis-compile — anything outside it (the `Box`/`Shared`/`Weak` heap-wrapper
  **values**, array elements containing one, and **capturing closures** — Tier
  2, deferred), refusing at storage-classification time with a message naming
  the type and pointing the user back to the interpreter as the
  reference), and
  `tuo debug syntax|ast|hir|mir [--opt] <file>` (diagnostic developer
  tools with unstable output, not language protocols; `mir` requires an accepted
  program, since MIR is only defined once the front end passes, and the lowered MIR is
  verified (`tuo_mir::verify`, mandatory) before it is dumped — every backend and the
  interpreter must reject unverified MIR; `mir --opt` shows the MIR after the
  optimization passes the native build applies), `tuo agent --stdio` (serve the
  versioned JSON-lines agent protocol over stdio — a long-lived compiler-intelligence
  server reusing one database across requests; see the agent-protocol convention below),
  and the **package commands** — `tuo new <name>`, `tuo add <name> --path <p>`,
  `tuo remove <name>`, `tuo test`, the package-aware forms of `check`/`build`/`verify`
  (invoked with no file arguments, optionally `--manifest <dir>`, they resolve the
  package graph rooted at that directory and drive the same front end / backend / spec
  runner over its loaded sources), and `tuo package symbols` (a machine-only query of a
  package's real exported symbols); see the package-system convention below.
  Also `tuo corpus validate [--category <c>] [--origin <o>] <files>` (run a program
  through the compiler-validated corpus pipeline and report its per-stage results plus
  metadata; see the corpus-pipeline convention below).
  And `tuo bench report <tasks> <run>` (score a recorded code-generation benchmark
  run by recompiling the model's outputs through the real compiler — never trusting
  the recorded verdicts — and report the metric summary; see the codegen-benchmark
  convention below).
- **The trusted corpus is compiler-validated, six-cornered, and self-honest —
  no program enters on assertion.** `tuo-corpus` (layer 115, just above the
  layer-110 tools it composes — the formatter and the research harness's
  tokenizers — and below the CLI) owns tuonelang's official corpus pipeline. A candidate is
  admitted only after clearing the required, ordered gauntlet driven over the **real**
  compiler stages — format → parse → resolve → type check → ownership → MIR verify →
  specs/tests → **native execution where applicable** — short-circuiting at the first
  failure (every later stage recorded as skipped). Native execution is the one stage the
  crate cannot perform alone (it needs a concrete backend and the `cc` linker), so it is
  a **host-injected `NativeExecutor` seam** — the CLI wires in its compile-link-run
  machinery (`codegen::native_run`), mirroring how the agent injects a `Formatter`; the
  crate itself names no concrete backend, only the backend-agnostic `tuo-codegen`
  interface. Candidates come from four origins (human, generator, LLM, transformed
  benchmark) and are sorted into **six corpora**, each with its own admission contract:
  **correct** (passes everything), **syntax-error / type-error / ownership-error /
  failing-spec repair**, and **repository-level changes** (multi-file, validated as a
  whole). A repair candidate is admitted **only if it fails at exactly the stage its
  category names** (a "type-error" entry is a real type error, not a mislabeled parse
  error), and, when a corrected program is attached, only if that fix validates clean —
  so the corpus stores real (broken → repaired) pairs, never an unverified claim. Every
  admitted entry carries a full `EntryMetadata` record: **language version**, **source
  origin**, the **features used** (exact for declarations from the resolved symbol table,
  conservative/token-based for the rest, skipping prelude symbols so nothing false is
  reported), the **per-stage validation results**, a coarse **complexity** measure, and
  **token counts** under every deterministic tokenizer the research harness ships (reused
  from `tuo-bench`, so there is one measurement of record). The promise is *enforced*, not
  asserted: `crates/tuo-corpus/tests/shipped_corpus.rs` re-admits every fixture under
  top-level `corpus/` to the category its directory names (so the shipped corpus can never
  drift into dishonesty), `crates/tuo-corpus/tests/validation.rs` pins the pipeline and the
  admission contracts, and `tuo-cli/tests/corpus_command.rs` drives the whole thing through
  the real binary including live native execution. The dependency-policy guard keeps
  `tuo-corpus` at layer 115 (it may drive the compiler facade and every crate below it,
  including the layer-110 tools it composes, and injects native execution from the CLI
  above).
- **The code-generation benchmark drives the real compiler for every metric,
  embeds no model, and never changes a task silently.** `tuo-codegen-bench`
  (layer 116, just above `tuo-corpus` and below the CLI) owns tuonelang's complete
  LLM code-generation evaluation harness. It benchmarks a model through a
  **pluggable `ModelAdapter`** seam — an LLM behind an API, a local runner, or a
  deterministic generator — and **no LLM provider is embedded** anywhere; the
  adapter turns a `Prompt` (and, on repair turns, the compiler's diagnostics) into
  tuonelang source, and the harness compiles that source itself, so a metric is
  only ever claimed when the *real* compiler produced it (mirroring how the corpus
  injects a `NativeExecutor` and the agent a `Formatter`). For each task
  `run_task` drives a repair loop and records every turn; `BenchmarkSummary`
  aggregates the metrics the prompt names — **Parse@1 / Check@1 / SpecPass@1 /
  TestPass@1** (the last scored against **held-out** tests the model was not
  shown), **Repair@1**, repair cycles, generated tokens (the model's own
  accounting), wall-clock **feedback latency** (measured, never promised),
  **invented symbols** (undefined-name `R0002` diagnostics — the compiler is the
  authority on which names do not exist), and the **unrelated-edit rate**
  (repairs that touched code the compiler had not flagged, judged by comparing the
  edited lines against the previous turn's error lines). A `BenchmarkRun` keeps
  full **provenance** — the exact prompts, the `ModelConfig`, the compiler and
  language versions, the model's outputs, and the compiler's per-turn results —
  and both a machine report (`BenchmarkSummary::to_json`, versioned by the crate's
  `SCHEMA_VERSION`) and a human report (`render_human`) come from the one summary.
  Benchmark **tasks are never changed silently**: a `BenchTask` is pinned by a
  content digest and a `TaskSet` verifies every pin on load, so an edit without a
  re-pin is a loud error; tasks may carry comparable **syntax variants** so a
  language-design decision can be evaluated empirically across spellings. The CLI
  cannot generate live (no model is embedded), so `tuo bench report <tasks> <run>`
  *proves* a recorded run's metrics instead: it verifies the task-set pins
  (refusing a silently-edited benchmark), then **recompiles every recorded output**
  (`tuo_codegen_bench::rescore`) and computes the summary from the compiler's
  verdicts, never the recorded booleans — a fabricated result cannot survive. The
  promise is pinned by `tuo-codegen-bench`'s unit tests, its end-to-end
  `tests/harness.rs` (a scripted, offline `ModelAdapter` driving the real compiler
  through a fail-then-repair loop, held-out-test scoring, variants, and provenance
  round-trip), `tests/shipped_tasks.rs` (every task under
  `benchmarks/llm/codegen/tasks/` re-verifies its pin), and
  `tuo-cli/tests/bench_command.rs` (the whole thing through the real binary,
  including a run whose recorded verdict is a *lie* that recompilation exposes).
  The dependency-policy guard keeps `tuo-codegen-bench` at layer 116.
- **The context-injectable brief is generated from the compiler, never authored
  as prose about it.** `tuo cheatsheet` (ADR-0018) emits a dense language brief
  meant to be pasted into a coding agent's or a local model's context before it
  writes tuonelang — the *priming* counterpart to the feedback surfaces (machine
  diagnostics, the agent protocol, the corpus, the codegen benchmark). Its five
  sections are assembled from compiler-owned sources: the syntax skeleton
  carries `grammar.ebnf`'s own `GRAMMAR-VERSION`; the standard-library section
  drives the sixteen `tuo-stdlib` catalog modules through the **real** front end
  (`check_sources` is the acceptance gate — the brief refuses to describe a
  library that does not compile) and lists each public `fn` as its *declaration*,
  parameter names and written type spellings included, because that is the form
  a caller must type (`Ty::render` would normalize `Int` to `I64` and drop the
  names); the runnable-core section states what `tuo check` accepts versus what
  `run`/`build` execute, including what is deliberately absent (capturing
  closures, `Box`/`Shared`/`Weak` values, detached spawn, method calls) so a
  model does not invent it; and the anti-pattern table is the forward reading of
  `training/breaks.py` — one list, two consumers, so the generator's error model
  and the brief's advice cannot drift apart. A committed copy lives at
  `tuonelang-cheat-sheet.txt`. The promise is *enforced*, not asserted:
  `tuo-cli/tests/cheatsheet_command.rs` compiles every `tuo` sample in the brief
  (the worked program additionally runs to its documented exit byte), proves
  every listed signature is callable as shown, proves every anti-pattern is
  really rejected **and its correction really accepted** (so the brief cannot
  teach a model to avoid something legal), verifies no listed name is absent
  from its module, and pins the committed copy byte-for-byte against fresh
  output — because a stale brief fails *silently*, producing confidently wrong
  generations rather than a visible error. Whether the brief actually helps is
  measured, not claimed: `tuo-cli/tests/cheatsheet_benchmark.rs` (`--nocapture`)
  is a deterministic Compile@1 proxy over a corpus whose cross-language prior is
  wrong, each pick *really* compiled, reporting unprimed (0%) versus primed
  (100%); the primed policy reads the real generated brief, so a brief that
  stopped carrying a fact would stop scoring for it. It is a proxy for the
  brief's discriminative power, **not** a live-LLM eval (no provider is
  embedded); the doc says so plainly.
- **The performance laboratory drives the real compiler for every number,
  reports only what it measured, and never publishes an unsupported claim.**
  `tuo-bench`'s `lab` module (in the layer-110 research crate, which may sit
  atop the `tuo-compiler` facade) owns tuonelang's reproducible compiler and
  runtime benchmarks. Every figure comes from the **real** compiler:
  `lab::compiler` times the cold stages by driving `check_sources` and the
  cold `lex`/`parse` entry points, and measures the incremental edits through
  the shared `IncrementalSession` — reporting each edit's cost as the
  **deterministic set of per-item queries that re-executed** (`executed_queries`),
  not just wall-clock, so *warm no-op* (zero), *function-body*, *function-signature*,
  *single-spec*, and *affected-spec* scenarios are pinned by re-execution counts,
  not timing noise. `lab::runtime` runs compiled programs through a host-injected
  `NativeRunner` seam (the crate names no backend and no `cc`; the CLI wires in the
  real Cranelift+`cc` `tuo run`, mirroring the corpus's `NativeExecutor`). The
  honesty rule the prompt demands is enforced structurally: all **eighteen**
  runtime workloads (**startup, integer-computation, function-calls,
  recursion**, — since ADR-0004's fixed arrays landed — **collections**, an
  `[Int; 8]` insert/scan with its C peer, — since ADR-0006's `Str` core
  landed — **string-processing**, a byte-level tokenize/scan/slice-compare
  over a fixed request-log line with its C peer, — since ADR-0009's
  allocator core landed — **allocation**, a bounded allocate/grow/free loop over
  a growable `Array[Int]` and an owned `String` with its `malloc`/`realloc`/`free`
  C peer, same doubling growth, — since ADR-0008 Tier 1's function values
  landed — **indirect-calls**, a hot loop calling through a `fn` value with a C
  peer calling through a function pointer, same iteration count and exit, —
  since ADR-0011's hash map landed — **map-lookup**, an insert-1000/lookup-1000/
  remove-500 churn over `Map[Int, Int]` with open-addressing C and built-in-map
  Go peers, — since ADR-0013's OS boundary landed — **file-io**, per round
  an open/write/close of a 240-byte scratch file, a byte-at-a-time read-back,
  and a remove, with C and Go peers making the identical calls — the deferred
  ADR-0006 effect-crossing benchmark, — since ADR-0014's sockets landed —
  **networking**, per round a listen/connect/accept over an ephemeral
  loopback port with 128 bytes read back byte-at-a-time, the catalog's last
  `Unsupported` entry flipped exactly as its comment promised, — since
  ADR-0015 — **channels**, a single-threaded send-500/recv-500 churn
  isolating the locked-FIFO crossing (C peer a mutex-and-condvar queue, Go
  peer its **native buffered `chan`**), and — since ADR-0016 —
  **json-parse**, a recursive-descent parse of a fixed document into a
  kind/number arena (C peer `strtod`-based, Go peer its standard
  **`encoding/json`**), and — since ADR-0017 — **udp-echo**, per round two
  ephemeral loopback UDP sockets exchanging 8 datagrams with an echo back to
  `udp_peer_port` (C peer `sendto`/`recvfrom`, Go peer
  `net.ListenPacket`/`WriteTo`/`ReadFrom`), and **connect-timeout**, the cost
  of a *bounded failure* — 200 rounds of `connect_timeout` to a port nothing
  listens on, each required to come back rather than hang (C peer
  non-blocking `connect`+`poll`, Go peer `net.DialTimeout`) — a workload that
  **could not be written before ADR-0017**, since a blocking `connect` has no
  bounded outcome to measure), and — since ADR-0019 — **sha256-hash**, the
  full FIPS 180-4 compression function over a fixed 64-byte message (C peer
  on `uint32_t`, Go peer its standard **`crypto/sha256`**), and
  **wire-decode**, the length-prefixed framing walk that motivated the ADR —
  a big-endian length and type decoded, re-encoded, and round-trip checked
  per frame (C peer the identical shifts, Go peer **`encoding/binary`**) —
  both workloads that **could not be written before ADR-0019**, since
  SHA-256 is *defined* in rotations and shifts and masking has no arithmetic
  spelling at all, and — since ADR-0020 — **constant-time**, the one entry
  measuring a cost *deliberately paid* rather than a throughput to improve:
  a 32-byte tag comparison per round done twice over the same inputs, once
  branchlessly and once with the early-returning form that is the textbook
  timing vulnerability, with rounds alternating between the naive form's
  best case (differ at byte 0) and its worst (equal tags) so the gap is not
  measured at one extreme — the C peer using the same sign-smearing mask
  tuonelang is forced into (`0 - bit` traps on `i64::MIN`), the Go peer its
  standard **`crypto/subtle.ConstantTimeCompare`**) carry a real program and are
  `Support::Supported` —
  none remains `Unsupported`, and the mechanism (an entry with the *exact
  reason* and **no number**, flipping the moment its feature lands) stays as
  the documented re-entry path for any future workload.
  Since ADR-0007, the lab also owns the **parallel-speedup category**
  (`lab::parallel`): one CPU-bound reduction committed four ways (tuonelang
  serial + `par_map`, C serial + pthreads, same thread count, same exit byte),
  measured through a host-injected `TimedRunner` that builds first and times
  only the binary's execution (warm-up run first), `Measured` only when a
  side's serial *and* parallel runs hit the expected exit — raw nanoseconds
  recorded, the ratio derived at render time, never promised. The compiler
  lab's cold stages measure the aggregate/loop program
  (`lab::compiler::COLD_AGGREGATE`, acceptance test-pinned) so the ADR-0004
  lowering's compile cost is tracked too. Cross-language
  comparison is **equivalent-semantics only** (C is the apt peer for the runnable
  core: AOT-native, matching integer model, `#[repr(C)]`-compatible aggregates)
  via a `ComparisonRunner` seam, and a
  `Verdict` reaches `Measured` **only when both sides actually compiled and ran**
  under recorded toolchains and produced the same observable exit — otherwise it is
  `Skipped` with the reason, never a one-sided or fabricated figure. Each workload's
  source (tuonelang *and* its C peer) is the committed file under
  `benchmarks/runtime/programs/`, embedded via `include_str!` so the recorded
  source cannot drift; a run captures the full environment (hardware, OS, tuonelang
  and rustc versions, exact commands) into a versioned `LabReport`
  (`SCHEMA_VERSION`), whose human render (`render_human`) carries **no superlative
  and no aggregate verdict** ("blazing fast" appears nowhere and a test forbids it).
  The contract is pinned by `tuo-bench`'s unit tests, `tuo-bench/tests/lab.rs`
  (committed programs equal the embedded sources; every supported workload passes
  the real front end; the honesty/no-superlative rules; the committed example report
  parses, round-trips, and its deterministic parts regenerate; a `--nocapture`
  measurement prints the full report), and `tuo-cli/tests/lab_command.rs` (the two
  host seams end-to-end: the supported workloads really compile-link-run via `tuo
  run` to their expected exit byte, and a live `cc` C comparison agrees where the
  toolchain exists, recording a skip where it does not). The benchmark repository
  itself lives under `benchmarks/` (`compiler/`, `runtime/programs/`,
  `runtime/results/example-report.json`). The dependency-policy guard keeps
  `tuo-bench` at layer 110.
- **The whole compiler is fuzzed through its real entry points, one invariant is
  written once, and a discovered bug becomes a committed regression forever.**
  `tuo-fuzz` (layer 117, above the tooling surfaces it composes and below the
  CLI) owns tuonelang's whole-compiler fuzzing harness. It holds **no compiler
  machinery** — it is a pure consumer that drives every listed stage through its
  *real* public entry point (lexer, syntax-tree operations, parser, formatter,
  AST→HIR lowering, resolve+type-check+ownership, HIR→MIR lowering + MIR verify,
  and the MIR interpreter) and asserts that stage's invariants. Each stage's
  contract is written **once** as a `check_*` function in `stages`, and **two
  drivers exercise the same checker** so they can never drift: a stable,
  fixed-seed corpus sweep (`tests/sweep.rs`, ordinary `cargo test`, no nightly)
  and coverage-guided `cargo fuzz` targets (`fuzz/`, nightly, workspace-opted-out,
  one target per stage). The load-bearing invariants are enforced, not asserted:
  **arbitrary source input must not crash the compiler** (every stage entry point
  up through MIR lowering is total by construction — malformed input becomes error
  tokens, recovery islands, poison nodes, or diagnostics), **formatting is
  idempotent** and **valid formatted source stays parseable with the same
  diagnostics** (`check_fmt`), lowered MIR of an accepted program **verifies**,
  and **verified MIR never triggers an interpreter structural panic** —
  `check_interp` only runs MIR the interpreter's mandatory verify gate accepted
  and rejects any `TrapKind::Internal` (the interpreter's own impossible-state
  signal). **Differential cross-engine agreement** already lives as a mandatory
  CI gate over the *accepted-program* generator in `tuo-cli/tests/differential.rs`
  (interpreter == Cranelift == LLVM); this crate does not duplicate the native
  build and says so rather than re-asserting a weaker copy. **Regression fixtures
  are added automatically:** when a sweep or a `cargo fuzz` run finds an input
  that panics a checker, `regression::record` (called from the `guarded` driver
  every target wraps) writes the exact input to `regressions/<stage>/` — content-
  addressed, idempotent, the file's bytes *are* the crashing input — and
  `tests/sweep.rs::committed_regressions_stay_fixed` replays every committed
  fixture through its stage's checker on each `cargo test`, so a fixed bug can
  never silently return. This harness already earned its keep: the sweep found a
  stack overflow on deeply nested binary-operator chains (`x + x + … + 0`), which
  parse iteratively but build a left-nested `BinaryExpr` tree the front-end walk
  overflowed on; the fix extends the parser's depth pre-scan to bound
  binary-operator chain length (rejected as `P0003` before parsing, keeping every
  downstream stage's recursion bounded), pinned in `tuo-parser/tests/recovery.rs`
  and by the committed fixtures under `regressions/front-end/`. `tuo-fuzz` exposes
  **no CLI subcommand** — fuzzing is a developer/CI activity, never a promise the
  `tuo` binary makes — and the dependency-policy guard keeps it at layer 117 with
  no edge into the pipeline.
- **The language is dogfooded with real programs; a discovered gap becomes an ADR
  with a benchmark plan, never ad-hoc syntax.** Top-level `examples/` holds nine
  real, multi-function programs written *against* v0, not to test it from inside:
  `cli-stats` (a command-line statistics tool), `data-pipeline` (a record/JSON-style
  processor that decodes packed-integer fields and runs a filter+map+reduce),
  `workspace/` (a medium three-package graph `app → geometry → numeric` wired by
  path dependencies), `http-service` (a request-routing/status core),
  `concurrent-worker` (a worker-pool scheduling model), `router` (a declarative
  dispatch table over indirect calls), `log-analytics` (a one-pass keyed rollup),
  `file-report` (a report generator that really touches the disk), and
`postgres-auth` (the PostgreSQL v3 authentication handshake — big-endian wire
  framing plus SCRAM-SHA-256 checked against RFC 7677's published vector and
  the legacy MD5 challenge, the ADR-0019 motivating case as a whole program;
  exits 48 only when every step agrees, and it caught a real bug that no
  self-consistent spec would have: an empty `n=` username field, invisible
  structurally but fatal to the proof), and `postgres-client` (the other half:
  the same protocol driven over TCP against a **real PostgreSQL server** —
  startup packet, the live SASL exchange, the server's signature verified in
  constant time, then `SELECT 42` decoded out of a `DataRow` frame — and the
  **extended query protocol** (`Parse`/`Bind`/`Describe`/`Execute`/`Sync`),
  which is what makes parameters *safe*: a value travels as its own
  length-prefixed field rather than interpolated into SQL text, demonstrated
  with `'; DROP TABLE users; --` bound as a parameter and returned verbatim;
  its
  protocol layer is pure and spec-checked so `check`/`test` always run, while
  the live exchange skips cleanly when no server is reachable and its exit byte
  *names the failing step* when one is, so a rejected proof reports 20 and a
  failed server-signature check 21 rather than a bare mismatch). The three that fit the
  runnable core **run natively** — and since ADR-0004 landed they use it for
  real: `geometry` passes a `Point` struct, `data-pipeline` folds an `[Int; 8]`
  batch, and `cli-stats` holds an `[Int; 7]` dataset. Since ADR-0009 landed the
  allocator core, `data-pipeline` also answers its query through a
  **growable-collection oracle** — it `push`es its filtered subset (a
  data-dependent size a fixed `[Int; N]` cannot express) onto a heap-backed
  `Array[Int]` and folds it; and since ADR-0008 Tier 1 landed first-class
  function values, that fold is the **generic higher-order fold** (`fold(items,
  0, add)`, passing a named `add` as a function value — the ADR-0008 oracle), so
  `run` also exercises a native indirect call, spec-pinned equal to the streaming
  fold with the same exit byte. Since ADR-0012 landed the element-generic array
  surface, `data-pipeline` additionally carries the **record-struct oracle**:
  the packed batch decodes into `Record { category, amount, label: String }`
  structs held in a growable `Array[Record]`, filtered and folded through the
  struct instantiation of the generic fold (`fold_records` over an `in Record`
  function value), with `main` routed through that path — same exit byte,
  spec-pinned equal to both packed-`Int` paths — so the native binary exercises
  the owned-element deep-copy `get`, the recursive drop glue, and the ADR-0010
  `String`→`Str` composition (`label_is`/`label_len` over `as_str`) for real.
  Since ADR-0011 landed the hash map, `data-pipeline` also carries the
  **keyed-aggregation oracle** — `totals_by_category` folds the whole batch
  into a `Map[Int, Int]` in one scan, spec-pinned equal to the per-category
  re-scans, and `main` cross-checks the map path against the record path at
  runtime; and since ADR-0007 landed structured fork-join,
  `concurrent-worker` **runs its pool live** — `main` computes the makespan
  through the pure scheduling model AND through a real `std::rt::par_map`
  (one OS thread per worker running the model's own `worker_load`) — and,
  since ADR-0015 landed channels, **drains a real dynamically-drained shared
  work queue too**: `dynamic_total` fills a channel with the task ids,
  closes it, lets the workers race `chan_recv` to drain it, and `main`
  exits 15 only when the static pool matches the model's makespan AND the
  drained total equals the model's serial cost — both live paths against
  the spec-checked oracle (the old `CONTRACT submit` discharged; detached
  spawn remains a documented non-goal, not a contract).
  Since ADR-0006 landed the
  effect boundary, `http-service` **runs natively too**: its request-line
  parsing is pure `std::str` byte scanning (spec-checked), and since
  ADR-0014 landed sockets its old `CONTRACT serve` is gone — `serve_once`
  serves a real request (read the request line off an accepted connection,
  parse, route, respond over the wire, close), and `main` proves it by
  **serving itself over a live loopback socket** (`live_status`: listen
  ephemeral → connect → accept → the client reads the status back),
  printing `HTTP/1.1 200 OK` and exiting 200 only when the wire agrees
  with the pure parser; `cli-stats` prints its four-line
  report through `std::io::println`, consuming the stdlib's `std::io` module
  as input (`src/std_io.tuo`, a verbatim copy pinned byte-for-byte against
  the catalog). What still cannot run (recursive nominal types await a
  successor ADR's runtime-recursive glue; detached spawn is deliberately
  absent) is documented in place; nothing advertises behavior the compiler
  cannot perform. The examples are kept
  honest by `tuo-cli/tests/dogfood_examples.rs`, which drives the **real**
  `tuo` binary over every one (`check` accepts it, `test` runs its specs to
  `0 failed`, each runnable program `run`s/`build`s to the exact documented
  exit byte, and the two printing examples' stdout is asserted
  byte-for-byte), so a committed example can never rot. The exercise's measurements across the six axes
  the prompt names — compiler usability, diagnostic quality, incremental
  compilation, LLM generation success, stdlib gaps, runtime performance — are
  written up in top-level [`DOGFOODING.md`](DOGFOODING.md) with evidence from the
  real compiler (the incremental figures reuse `incremental_measure`, the runtime
  figures defer to the performance lab as the system of record). The governing rule
  is the project's own: **every language change discovered by dogfooding gets an ADR
  and a benchmark plan, never an ad-hoc feature to make one example compile** — so
  the exercise opened `specification/adr/ADR-0004` (aggregates + iteration in the
  runnable core — since **accepted and landed**, having unblocked its named
  `collections` workload), `ADR-0006` (the effect boundary + runtime strings —
  since **accepted and landed**, having unblocked its named
  `string-processing` workload; per its amendments, owned-`String`/`concat`
  moved to the allocator ADR and `networking` was explicitly *not* unblocked),
  `ADR-0009` (the allocator core — owned `String` + growable `Array[Int]`,
  since **accepted and landed**, having unblocked its named `allocation`
  workload; `Box`/`Shared`/`Weak` **values** and non-`Int` `Array[T]` stay
  deferred), `ADR-0008` (first-class functions — **Tier 1 accepted and landed**:
  non-capturing function values and the generic higher-order stdlib combinators,
  having added its named `indirect-calls` workload; **Tier 2 capturing closures
  deferred** to a future ADR), and `ADR-0007` (the concurrency model — since
  **accepted and landed**: the deferred model resolved to structured fork-join
  over the one primitive `std::rt::par_map` (a typed effect; non-capturing
  function values over `Copy` tasks, round-robin, join-before-return, so no
  data race is expressible by construction), `concurrent-worker` now runs its
  pool **live** with the spec-checked scheduling model as the runtime oracle
  (exit 15 survives only when the live run agrees), and the gating
  **parallel-speedup** benchmark category landed (`lab::parallel`, serial vs
  `par_map` wall clock with a same-thread-count pthreads C peer)). The successor ADRs the dogfooding
  chain produced — `ADR-0010` (the `String`→`Str` view) and `ADR-0012`
  (generic `Array[T]` element types) — are both since **accepted and landed**
  (all stages, including ADR-0010's Stage C stdlib payoff and ADR-0012's
  owned-element increment, combinator instantiations, and the record-struct
  oracle above), and `ADR-0011` (the hash map) is since **accepted and
  landed** too (all three stages: `Ty::Map` with the `Map[Int, Int]`/
  `Map[Str, Int]` surface, insertion-ordered reference semantics, the native
  `tuo_rt_map_*` table on both backends at ABI v6, `std::collections::counts`,
  data-pipeline's keyed-aggregation oracle, and its gating `map-lookup`
  workload). The 2026-08-24 Go-parity sweep closed the remaining chain:
  `ADR-0013` (the OS effect boundary: clock, argv, files — the `file-io`
  workload), `ADR-0014` (socket effects — `networking`, the lab's last
  unsupported entry, flipped; `http-service` serves live), `ADR-0015`
  (channels + mutexes — the stdlib's contract tier emptied,
  `concurrent-worker`'s dynamic queue real, the `channels` workload with
  Go's native `chan` as its peer), and `ADR-0016` (the data increment +
  `std::json` — the `T0016` recursion boundary, `Float` elements,
  `std::array::set`, and the `json-parse` workload against `encoding/json`)
  are all since **accepted and landed** — as is `ADR-0017` (timeouts, IPv6,
  and UDP: three of the four items ADR-0014 had listed as additive
  *"when its need is demonstrated by dogfooding"*, landed in three stages at
  ABI v10 with the gating `udp-echo` and `connect-timeout` workloads;
  **TLS and DNS stay out** — TLS needs a crypto dependency this workspace
  avoids, and DNS is properly written *in tuonelang* on the new UDP
  primitives). `ADR-0019` (bitwise operations and crypto) is **accepted, both
  stages landed**: the PostgreSQL-connector target was the first dogfooding
  case the language could not express *at all*, and it needed two separable
  things — Stage A the operator surface (see the runnable-core entry above),
  Stage B the `std::bits`/`std::crypto` library written *in tuonelang* on it.
  Its headline claim is discharged by a real test: a **native** tuonelang
  binary's SHA-256 agrees byte-for-byte with `tuo-package`'s own **Rust**
  `sha256`, so the language reproduces its own package manager's checksum
  function. The entropy primitive
  `std::rt::random_byte` (ABI v12) landed with it, so a **full
  SCRAM-SHA-256 client proof** now computes natively and is pinned against
  RFC 7677's published vector — the PostgreSQL authentication path the ADR
  was opened for. That path is now a *program* rather than a test vector:
  `std::crypto` carries the SCRAM client exchange
  (`scram_salted_password`/`scram_client_proof`/`scram_server_signature`)
  and the constant-time `verify` (delegating to `std::ct::bytes_eq`, so the
  safe comparison is the convenient one and `==` on a MAC tag is never the
  obvious spelling — the catalog's second declared dependency edge,
  `std::crypto → std::ct`), and `examples/postgres-auth` computes the whole
  v3 handshake end to end — big-endian message framing, the startup packet,
  the SASL challenge parsed off the wire format, the proof, and the server's
  signature verified in constant time — exiting 44 only when every step
  matches RFC 7677's published values. Its two gating benchmark
  workloads landed too — **`sha256-hash`** (against a `uint32_t` C peer and
  Go's `crypto/sha256`) and **`wire-decode`** (against `encoding/binary`) —
  both unwritable before Stage A, taking the lab catalog to seventeen
  workloads with none `Unsupported` (eighteen since ADR-0020 added
  `constant-time`). `md5` has since landed too — the legacy
  `AuthenticationMD5Password` challenge is supported, shipped **documented as
  broken for security** (the ADR's own requirement, since a stdlib that ships
  MD5 without that sentence teaches the wrong thing to exactly this language's
  audience) and spec'd against RFC 1321's published suite, with
  `md5_password` pinning the protocol composition against an independent
  implementation — so nothing from ADR-0019 remains unimplemented. SCRAM
  stays the primary path regardless, being the default on every current
  server. Note Stage B weakens but does not overturn the TLS
  exclusion above: SHA-256/HMAC written *in tuonelang* need no external
  dependency, but TLS additionally needs X.509, a certificate store, and AEAD
  ciphers, so it stays out. Resolved
  `examples/**/tdg.lock` files embed machine-absolute dependency paths and
  are therefore gitignored, not committed.
- **The 0.1 release gate is a checklist backed by artifacts, and the report is
  generated, never asserted.** `specification/RELEASE-0.1-GATE.md` fixes the sixteen
  criteria that must be `MET` (or explicitly `RELEASE-BLOCKING`) before tuonelang 0.1
  may be declared ready — grammar versioned, formatter canonical/idempotent, parser
  crash-free under fuzzing, static/ownership/MIR semantics documented, specs
  deterministic, interpreter⇄Cranelift (and, since LLVM ships in 0.1, three-way
  ⇄LLVM) differential-clean, human diagnostics usable, machine diagnostics
  schema-versioned, incremental/LLM benchmarks present, corpus compiler-validated,
  examples working, and every known semantic divergence resolved or release-blocking.
  Each criterion names a concrete committed *proving artifact* (a test target, a
  benchmark, or a normative doc) in a machine-readable `gate-manifest` block, and
  `tuo-cli/tests/release_gate.rs` parses that block to assert there are exactly
  sixteen criteria `G1`..`G16`, every status is from the gate's vocabulary, the
  manifest and the prose summary table agree, the `GRAMMAR-VERSION: 0.1` marker G1
  relies on is present in `grammar.ebnf`, and — the load-bearing check — **every
  cited artifact path actually exists**, so the gate can never advertise readiness it
  cannot back. Run with `--nocapture` it regenerates the readiness report from the
  manifest and live `Path::exists` probes. The gate is honest, not aspirational —
  a criterion earns `MET` only by citing artifacts the checker proves exist. As of
  the current revision **all sixteen criteria are `MET`** (verdict **READY**): the
  last two documentation-locality gaps are closed by `specification/static-semantics.md`
  (G4 — resolution + type-checking rules, the `Rxxxx`/`Txxxx` diagnostics,
  consolidated from Constitution §§8–24, `syntax.md`, and the `tuo-resolve`/`tuo-types`
  crate docs, pinned by `tests/types/`) and `specification/mir.md` (G6 — every MIR
  instruction/terminator/trap defined on its type, the verifier's `Mxxxx`
  invariants, and the interpreter's abort taxonomy + sandbox), both now normative
  peers of `ownership.md`/`abi.md`. A criterion that regressed would flip back to
  `PARTIAL`/`RELEASE-BLOCKING` the moment its artifact changed.
- **Codegen is behind a tuonelang-owned interface; no backend type leaks upward.**
  `tuo-codegen` defines `CodegenBackend` (verified MIR + `TypeckResult` → a
  relocatable `ObjectArtifact`) and the plain values that cross the boundary
  (`TargetSpec`, `ObjectArtifact`, `CodegenError`, `EntryAbi`). Two backends implement
  it — `tuo-codegen-cranelift` (the default debug build) and `tuo-codegen-llvm` (the
  `--release` build, wrapping LLVM 19 via inkwell, pinned by the `llvm19-1` feature and
  located at build time through `LLVM_SYS_191_PREFIX` / `llvm-config-19` / a Homebrew
  `llvm@19`; the local default prefix is set in `.cargo/config.toml`, CI installs LLVM
  19 and sets the env var); **no Cranelift, inkwell, or LLVM type appears in anything
  either exports**, so neither can leak into MIR, type checking, the CLI protocol, or
  the runtime's public surface. A backend consumes only *verified* MIR (it never
  re-checks) and must agree with the MIR interpreter instruction for instruction —
  where they diverge, the backend is wrong. The reference semantics stays the
  interpreter; correctness comes before optimization — the Cranelift backend emits
  unoptimized code, and the LLVM backend prioritizes semantic equivalence, using LLVM's
  **standard** `default<O2>` pipeline (`OptLevel::Release`) rather than a custom pass
  stack. The interpreter-vs-native agreement is pinned by the differential suites in
  `tuo-cli/tests/codegen_differential.rs` (interpreter vs default backend), the
  **three-way** `tuo-cli/tests/codegen_three_way.rs` (interpreter == Cranelift == LLVM),
  and the randomized `tuo-cli/tests/differential.rs` (both backends over generated
  programs), all over `tests/codegen/fixtures/`; any mismatch is a release blocker.
  `tuo-runtime` is the minimal native runtime linked into
  every built binary: it owns the deterministic trap (a stable `TrapCode` → stderr
  message → `abort()` with a fixed status), emitted as C so a generated executable
  needs no Rust runtime.
- **MIR optimization is a set of isolated, meaning-preserving, re-verified passes;
  the interpreter stays the reference on _unoptimized_ MIR.** `tuo-mir`'s `opt` module
  (`tuo_mir::optimize`) runs a small pipeline — constant folding, simple copy
  propagation, unreachable-block removal, dead-local elimination — to a bounded fixed
  point on the native build path (`tuo build`/`tuo run`), before the backend and *after*
  the mandatory verifier. Each pass is one `Pass` with a declared purpose and
  preconditions; the driver calls `tuo_mir::debug_assert_verified` after every pass, so a
  pass that corrupts MIR panics (named) in debug/test rather than reaching a backend.
  Every pass is a rewrite that leaves observable behavior (return value, and which/whether
  it traps) unchanged — in particular constant folding **never** folds a trapping
  operation (`1/0`, `MIN/-1`, overflow, `MIN` negation), since that would erase an
  observable abort. Nothing speculative lives here (no inlining, no loop transforms); the
  release backend layers LLVM's own optimizer on top. The contract is pinned by the
  before/after golden suite (`tests/mir/opt/fixtures/*.mir` / `*.opt.mir`,
  `tuo-mir/tests/opt_golden.rs`), the MIR-level semantic differential and compile-time /
  code-size **measurement** (`tuo-cli/tests/opt_semantics.rs`, interpreter agrees on raw
  vs optimized MIR), and the existing native differential suites — which now compile
  *optimized* MIR, so they pin interpreter (unopt) == Cranelift (opt) == LLVM (opt +
  LLVM O2). Any divergence is a bug in the pass, never the interpreter.
- **The runtime ABI is tuonelang-owned, backend-independent, and versioned before it is
  frozen.** `tuo-runtime` is the single normative home of the ABI compiled programs
  obey — value layouts, panic/trap, startup/exit, the allocation boundary,
  destruction, and internal calling conventions — specified in prose in
  `specification/abi.md` and implemented in the crate's `abi` and `alloc` modules.
  Layouts (`abi::layout_of` → `Layout{size,align}`, computed from `tuo-types` alone;
  `#[repr(C)]` packing, explicit `u32` enum discriminants numbered in declaration
  order to match the interpreter's `Value::Variant`, no niche packing in v0) carry no
  Cranelift or LLVM type: a backend *consults* them, never defines its own, so the two
  backends and the interpreter cannot drift into incompatible memory models. The ABI
  carries an explicit `abi::ABI_VERSION` (currently `0`), bumped on any
  layout-affecting change in the same commit that moves the pinning tests. The heap
  allocator is one swappable seam — the C-ABI `tuo_rt_alloc`/`tuo_rt_dealloc`
  (`alloc_runtime_c_source`, OOM traps, never returns null) — through which every
  `Box`/`Shared`/`String`/`Array` allocation flows. ABI layout tests
  (`tuo-runtime/tests/abi_layout.rs`, checked against real `#[repr(C)]` Rust types) and
  interpreter⇄ABI equivalence tests (`tuo-cli/tests/abi_equivalence.rs`) pin the
  contract; the crate stays independent of any concrete backend.
- **Machine output is a versioned contract, human output is not.** A global
  `--message-format` selects `human` (default), `json` (one envelope), or
  `json-lines` (streamed, one event per line) for every result-producing command
  (`check`, `spec`, `verify`, `fmt`, `build`, `run`). The wire shape lives in `tuo-cli`'s
  `protocol` module, versioned by `PROTOCOL_VERSION`; every machine message
  carries the protocol version, event kind, command, status, stable diagnostics
  (serialized with the independently-versioned `tuo_diagnostics::json` schema),
  relevant IDs, and source ranges. In a machine format **stdout carries protocol
  output only**, and internal logging reaches **stderr only under `--log`**. The
  `debug` dumps have no machine encoding (unstable developer output) and reject a
  non-human format. The contract is pinned by `tests/cli/protocol/` fixtures and
  the backwards-compatibility tests in `tuo-cli/tests/protocol_command.rs`:
  additive changes are allowed without a bump, but dropping/renaming a guaranteed
  field or changing the version must move `PROTOCOL_VERSION` and the schema
  fixture together.
- **Incremental compilation is fine-grained at the query-graph level, and
  correctness beats invalidation reduction.** `tuo-db` owns a generic red-green
  engine (`QueryEngine`): opaque `QueryKey`s, opaque host-comparable `StoredValue`s
  (the host's own `PartialEq` decides **early cutoff**), dependency recording, cycle
  detection, and cooperative cancellation — pinned by `tuo-db/tests/incremental.rs`.
  Because `tuo-db` may not see a stage crate, the real stage wiring lives in
  `tuo-compiler`'s `IncrementalSession`, which implements `QueryHost` and registers the
  seven tracked queries — source file, parse, module interface, resolution, per-function
  type-check, per-function MIR, and per-spec dependency graph. The whole-program stage
  passes (`resolve`, `check`, `lower`) are not yet internally per-item incremental, so the
  session recomputes each **whole-program** result at most once per revision (memoized by
  revision) and slices per-symbol results out of it; the per-symbol queries store
  comparable values (a signature `Ty`, a rendered MIR string, a dependency list) so early
  cutoff fires. Two deliberate precision seams shield downstream work: the
  signature-level **module interface** (bodies excluded, so a body edit does not re-check
  callers) and a per-function **body fingerprint** (so a one-function body edit re-lowers
  only that function's MIR), plus a spec-graph digest kept separate from the resolution
  digest (so a spec's dependency change never re-checks function typing). Everywhere else
  the session errs toward recomputing. The five edit scenarios — no-change,
  function-body-only, function-signature, unrelated-file, spec-only — are pinned as **hard
  assertions on which queries re-execute** (`tuo-compiler/tests/incremental_stages.rs`) and
  reported with measured timing (`.../incremental_measure.rs`, run with `--nocapture`).
  Affected-spec selection (`IncrementalSession::affected_specs`, `Selection::Affected` in
  `tuo-spec`, `tuo verify --affected-by`) runs only the specs an edit may have changed and
  is proven sound, precise, and verdict-preserving in
  `tuo-compiler/tests/affected_specs.rs`. Interactive hosts read the snapshot's semantic
  results — resolution, types, the source map, and diagnostics with their spans — through
  the scoped-closure accessor `IncrementalSession::with_semantics(|Semantics| …)`; the
  snapshot retains the full `Vec<Diagnostic>` (not just an `accepted` boolean) so spans
  survive to the surface.
- **The LSP is a projection of the shared query engine, never a second
  front end.** `tuo-lsp` implements every language feature — diagnostics, hover,
  go-to-definition, find-references, rename, document symbols, completion, signature help,
  semantic tokens, quick-fix code actions, and navigation both ways between a function and
  its colocated specs — as a read-only translation of what `IncrementalSession` already
  computed. It **reimplements no stage**: each feature delegates to an existing query
  (`Resolution::resolved_at` / `references_to` / `rename_spans` / `specs_for` / `target_of`,
  `TypeckResult::type_of` / `expr_ty`, `Ty::render`, the collected diagnostics), so the CLI,
  the LSP, and the agent server all drive the *same* compiler queries — the whole
  point of the shared engine. The crate is three layers: `wire` (the LSP JSON vocabulary,
  `serde`-serializable, carrying no compiler types), `convert` (the **only** place UTF-8
  byte `Span`s meet LSP's UTF-16 line/character `Position`s — both directions, clamping
  out-of-range input rather than failing), and `analysis` (the `Analysis` request surface,
  one method per feature over the session). Code actions offer **only** compiler-authored,
  machine-applicable suggestions — the server never invents a fix the compiler did not
  vouch for. `Analysis` is the testable semantic core (answers are plain typed values,
  pinned in `tuo-lsp/tests/features.rs`); a JSON-RPC/stdio transport is a thin future
  addition, so no wire server is advertised before it exists. The read-only
  `SourceMap::file_id(name)` lookup lets a host resolve a document URI to its `FileId`
  without mutating the map.
- **The agent protocol is the third projection of the shared engine — a compiler
  intelligence protocol, not an AI model.** `tuo agent --stdio` speaks a versioned,
  JSON-lines request/response protocol (`tuo-agent`'s `protocol` module,
  `PROTOCOL_VERSION`) so a coding agent drives the compiler by writing one request per
  line and reading one response per line. Like the LSP it **reimplements no stage**: every
  method is a read-only projection of what `IncrementalSession` already computed —
  `initialize`, `check`, `verify`, `format`, `diagnostics`, `type_at`, `definition`,
  `references`, `symbols`, `signature`, `members`, `available_imports`, `specs_for`,
  `run_spec`, and `apply_safe_fix` (which offers **only** compiler-authored,
  machine-applicable fixes, never an invented one). The crate is three layers mirroring the
  LSP's: `protocol` (the versioned `Request`/`Response`/`ResponseError` envelopes, carrying
  no compiler types), `convert` (the one place wire positions meet compiler byte offsets —
  a wire `Position` carries an `offset` **and** a one-based `line:column`, so an agent may
  anchor either way, and clamps out-of-range input rather than failing), and `session` (the
  `Session`/`Server`, a long-lived database over `IncrementalSession` answering every
  method). **The compiler database is reused across requests**: one `Server` owns one
  `Session` owns one incremental engine, kept alive for the process's life, so an agent
  editing a program never restarts the compiler per edit. Every response is **deterministic
  where the underlying operation is** — the same request against the same open-document
  state yields the same `result`; the only non-deterministic field is a *measured* spec
  duration, reported as an observation in its own field, never a promise. **No LLM provider
  is embedded** anywhere. The protocol core is transport-agnostic and directly testable
  (`tuo-agent/tests/protocol.rs`); the `tuo agent --stdio` transport lives in the CLI (which
  also injects the canonical formatter through a `Formatter` seam, since `tuo-fmt` sits at
  the agent's own dependency layer and cannot be imported there) and is pinned end-to-end by
  `tuo-cli/tests/agent_command.rs`. `--stdio` is required, so the CLI never advertises a
  transport it does not have.
- **The agent's generation queries help an agent write the *next* token, and keep
  syntactic guidance strictly apart from semantic guidance — never over-claiming.**
  `tuo-agent`'s `generation` module (`GenerationQueries`, implemented for `Session`)
  adds seven compiler-guided methods on top of the descriptive ones: `context_at`,
  `expected_type_at`, `visible_symbols_at`, `valid_members_of`, `call_signature`,
  `imports_for_symbol`, `expected_syntax_at`. Like every other method they **reimplement
  no stage** — the *semantic* answers (`expected_type_at`, `visible_symbols_at`,
  `valid_members_of`, `call_signature`, `imports_for_symbol`, and `context_at`'s semantic
  block) are read-only projections of the shared `Resolution`/`TypeckResult`. Two honesty
  rules are load-bearing and pinned by tests (`tuo-agent/tests/generation.rs`): (1)
  **syntactic** guidance (`expected_syntax_at`, `context_at`'s `syntactic` block) is a
  **conservative lexical heuristic** over raw text, *always* flagged `"exhaustive": false`
  with a note that the compiler does **not** enumerate every valid next token — because the
  pipeline exposes no grammar-recovery/expected-token oracle, so claiming one would be
  dishonest; and (2) queries that can only approximate say so in-band — `visible_symbols_at`
  is an over-approximation (`"complete": false`, block-scoping unmodeled) and
  `expected_type_at` reports the type the checker *recorded* for the enclosing expression or,
  as a fallback, the enclosing function's declared return type (its `source` field names
  which), never a general hole-typing power the compiler lacks. `valid_members_of` is precise
  (`"exhaustive": true`) because the type checker's struct/enum shapes are complete. Whether
  the queries actually raise an agent's **Compile@1**/**Repair@1** is measured
  deterministically in `tuo-agent/tests/generation_benchmark.rs` (`--nocapture`): a fixed
  task corpus where the naive text-only guess is wrong, scored by *really* compiling each
  pick through `check_sources` under a baseline (no guidance) vs a guided policy (keep only
  candidates consistent with the queries' evidence) — the test asserts guidance is never
  worse and, on that corpus, strictly better. It is a proxy for the queries' discriminative
  power, not a live-LLM eval (no provider is embedded); the doc says so plainly.
- **The standard library is written in tuonelang, consumed as input, and split
  into three honest tiers — executable, effect, and contract — it never
  advertises an effect the compiler cannot perform.** `tuo-stdlib` is a
  *catalog* crate (no
  compiler machinery, layer 90): each of the sixteen modules — `std::core`,
  `std::collections`, `std::math`, `std::bits`, `std::bignum`, `std::ct`,
  `std::crypto`,
  `std::str`,
  `std::json`, `std::io`,
  `std::fs`, `std::net`, `std::time`, `std::process`, `std::sync`,
  `std::test` — is a `.tuo` source
  file under `src/std/`, embedded
  via `include_str!` and exposed as `Module { path, name, source }`
  (`MODULES`, `module(path)`), so any host loads them into its own `SourceMap`
  and runs its own pipeline. Every public API carries an exact signature, a doc
  comment, a worked example, machine-queryable symbol information (the same
  `Resolution` symbols the agent/LSP project), and — where pure-executable — an
  executable `spec`; there is deliberately **one** obvious API per fundamental
  task, never competing spellings. Because **methods are not lowered** (`impl`
  method calls are v0 no-ops pending the trait system), the library is *free
  functions only*, and each module separates an **executable tier** (pure
  computation — ordering, `Option`/`Result` combinators, `Duration` arithmetic,
  error classification, the pure state models of a latch/lock — which runs and
  whose specs run — including, since ADR-0009, the `std::collections`
  `Array[Int]` algorithms `sum`/`max_of`/`contains`/`index_of`/`reversed` over
  the allocator core, and, since ADR-0008 Tier 1, the **generic higher-order
  combinators** `fold`/`map_into`/`filter_into`/`any`/`all` over a first-class
  function value — one `fold`, not N specialized folds — whose specs build the
  arrays they fold and pass a named top-level `fn` by value, joined since
  ADR-0012 by their `String`-element instantiations
  `fold_strings`/`map_into_strings`/`filter_into_strings` with the non-`Copy`
  modes that element demands, and, since ADR-0011, `counts` — the frequency
  table over the builtin `Map[Int, Int]`, spec'd through the map's own
  observables, and, since ADR-0013, the rest of the `strconv` layer —
  `std::str::parse_float`/`float_to_string`, honest about binary-`Float`
  precision — the `std::core` **error chains** (`error_new`/`error_wrap`/
  `error_is`/`error_root`/`error_message`, the concrete `Array[String]`
  context-wrapping model — v0 has no traits, so no error interface is
  pretended), `std::time::render` (the one duration spelling) and the now-pure
  `elapsed`/`instant_at`, and, since ADR-0016, the whole of **`std::json`** —
  decode/navigate/render over an index arena (parallel arrays in DFS
  pre-order; positioned parse errors; documented `Float`-number and
  escape-set limits), entirely pure and spec-checked, plus natively pinned
  on both backends, and — since ADR-0019 Stage B — the whole of
  **`std::bits`** (width masking, the **modular** `add32`/`mul32` hashes are
  defined over — plain `+` traps, which is right for arithmetic and wrong for
  a checksum — rotation/logical shifts, and big-endian assembly/splitting)
  **`std::bignum`** (arbitrary-precision non-negative integers over **28-bit
  limbs** — the size is a correctness property, not a tuning knob: schoolbook
  multiplication accumulates `limb + limb*limb + carry`, which is 56 bits at
  28-bit limbs and would overflow — i.e. *trap* — at 32; deliberately **not
  constant time**, so it is safe for public values and unsafe for secrets),
  — since ADR-0020 Stage A —
  **`std::ct`** (the branchless subset, for code whose timing must not
  reveal its data: `mask`/`select`, the fixed-time tests
  `nonzero`/`is_zero`/`eq`/`ne`/`is_negative`/`lt`/`gt`, and the array
  operations that scan rather than index — `select_array` and `bytes_eq`,
  the fixed-time tag comparison whose early-returning form is a textbook
  vulnerability; written with **no arithmetic whose overflow check could
  depend on secret data**, since a trap is a branch and `0 - bit` traps on
  `i64::MIN`, so `mask` is the sign-smearing `(bit << 63) >> 63` instead.
  Its claim is deliberately narrow and *pinned* at two levels: the ten
  scalar primitives carry **`#[constant_time]`** (ADR-0020 Stage C) so the
  **compiler** verifies the source contains no data-dependent construct
  (`T0017`–`T0021` — branches, indexing, trapping arithmetic, and calls to
  unmarked functions are all refused), and
  `tuo-cli/tests/constant_time.rs` disassembles the native binary on
  **both** backends to check the *emitted* code is branchless too, since an
  optimizer may rewrite a branchless idiom into a conditional. Whether those
  instructions execute in data-independent time is a hardware property beyond
  a compiler's reach, so this is still short of a constant-time *guarantee*
  and says so.
  The array scans are honestly weaker and **deliberately unmarked**: a scan
  needs a loop and indexing by nature, and there is no `#[allow]` escape
  hatch — a function that cannot satisfy the checker simply goes unmarked, so
  the attribute's presence is what tells a reader the compiler verified it.
  They promise only that their control flow depends on array *lengths*, never
  on contents),
  and **`std::crypto`** (SHA-256, HMAC-SHA-256, PBKDF2-HMAC-SHA-256, Base64,
  hex, the byte/text bridge, the constant-time `verify` — which delegates to
  `std::ct::bytes_eq`, making the safe comparison the convenient one so `==`
  on a MAC tag is never the obvious spelling, and giving the catalog its
  second declared dependency edge `std::crypto → std::ct` — and the
  SCRAM-SHA-256 client exchange
  `scram_salted_password`/`scram_client_proof`/`scram_server_signature`,
  the PostgreSQL authentication path ADR-0019 was opened for), whose specs
  are unique in the catalog for
  asserting **published vectors** (FIPS 180-4, RFC 4231, RFC 4648) rather
  than the module's own reasoning — RFC 7677's own SCRAM vector needs 4096
  PBKDF2 iterations, more than the spec sandbox's instruction fuel allows,
  so the sandbox specs check structure and the published vector is pinned
  **natively** instead (the cost being the whole point of an iteration
  count) — and whose headline pin,
  `tuo-cli/tests/crypto_cross_check.rs`, compares a **native** tuonelang
  binary's digests against `tuo-package`'s own **Rust** `sha256` across nine
  padding-boundary inputs, so the language demonstrably reproduces its own
  package manager's checksum function), an **effect
  tier** (`std::io::print`/`println` over `std::rt::write`, `std::process::exit`
  over `std::rt::exit`, — since ADR-0009 — `std::io::read_line`, which builds
  an owned `String` from the bytes `std::rt::read_byte` yields, — since
  ADR-0007 — `std::sync::par_map`, the structured fork-join wrapper over
  `std::rt::par_map`, — since ADR-0013 — `std::time::now` over
  `now_nanos`, `std::process::arg_count`/`arg` over the argv primitives, and
  the whole `std::fs` disk tier `read`/`write`/`exists`/`remove` over
  `open`/`close`/`remove_file` composed with the descriptor seam, — since
  ADR-0014 — the whole `std::net` socket tier
  `listen`/`bound_port`/`accept`/`connect`/`close`, — since ADR-0015 —
  `std::sync`'s channels `channel`/`send`/`recv`/`close` and mutexes
  `mutex`/`lock`/`unlock`, — since ADR-0019 Stage B — `std::crypto`'s
  `random_byte`/`nonce` over `std::rt::random_byte` (the platform CSPRNG via
  `getentropy`; drawing randomness is an effect by nature, since a function
  whose purpose is to differ on every call cannot be pure, so `R0007` refuses
  it in a spec with no new mechanism), and — since ADR-0017 — `std::net`'s bounded
  waits `accept_timeout`/`connect_timeout`/`read_byte_timeout`, its IPv6
  pair `listen6`/`peer_family`, and its UDP tier
  `udp_bind`/`udp_send`/`udp_recv`/`udp_byte_at`/`udp_peer_port` (joined in
  the executable tier by the `is_timeout`/`is_ipv6` classifiers) — real
  tuonelang
  implementations that run natively but can carry **no** spec, since `R0007`
  keeps the spec sandbox pure; each is marked `EFFECT:` and names the native CLI
  test that pins it), and a
  **contract tier** that is now **empty**: ADR-0015 discharged the last
  stubs (`std::sync::lock`/`unlock` are real, handle-based; the pure
  `LockState` model stays executable), the mechanism remains (`CONTRACT:`
  marker, exact signature, no spec) for any future entry, and a CLI test
  pins the tier's emptiness so nothing re-enters silently. The promise
  is enforced, not asserted: `tuo-cli/tests/stdlib.rs` really compiles every
  module (with exactly its **declared** dependencies, and together) with zero
  errors, runs every shipped spec to
  green with **no skips** (a skipped spec would mean a dishonest, unrunnable
  contract slipped into the executable tier), enforces the three-tier rule
  textually per public function (pure ⇒ spec'd; `EFFECT:` ⇒ no spec + a named
  native test; `CONTRACT:` ⇒ no spec), and proves the effect tier natively on
  both backends (`println` prints `hi\n` exactly; `exit` really exits with the
  status's code; `now` never runs backwards; `arg`/`arg_count` read a real
  command line; the `std::fs` roundtrip really touches the disk; the
  `std::net` roundtrip really touches the network over loopback; the
  `std::sync` channels/mutexes really synchronize). Since ADR-0019 Stage B
  the catalog is no longer flat: `std::crypto` uses `std::bits` (rather
  than ship two copies of `rotr32`/`add32`/`be32` free to drift) and, since
  the SCRAM surface landed, `std::ct` (so the constant-time comparison has
  one implementation, not a re-derived copy in the module that most needs it
  to be right), so the test
  carries a `DECLARED_DEPENDENCIES` table — each module is checked with
  exactly its declared dependencies and **nothing else**, so an undeclared
  use still fails to resolve, and `the_dependency_graph_is_declared_and_acyclic`
  proves the listed edges are the only ones and form no cycle. The invariant
  is therefore "the dependency graph is declared and acyclic", not the older
  "every module stands alone". And
  `tuo-cli/tests/stdlib_hallucination.rs` (`--nocapture`) is the API-hallucination
  benchmark — a deterministic Compile@1 proxy over a corpus whose naive guess is a
  plausible-but-wrong name (`maximum`/`unwrap`/`sum_range`/`is_abs`), scored by
  *really* compiling each pick, showing a baseline (priors only) at 0% versus a
  grounded policy (keep only calls to functions the module's real symbols export)
  at 100%. It is a proxy for the symbol surface's discriminative power, not a
  live-LLM eval (no provider is embedded); the doc says so plainly. The
  dependency-policy guard pins `tuo-compiler → tuo-stdlib` (the stdlib is input,
  never the reverse) and keeps the catalog crate free of any stage dependency.
- **The package format is data-and-filesystem only; the compiler and agent query a
  package's real symbols by compiling its resolved sources, never by guessing.**
  `tuo-package` (layer 110) owns tuonelang's first package format and holds **no
  compiler machinery** — it models the manifest and lockfile, resolves a path-dependency
  graph off disk, and computes content checksums, all as plain values, so it depends on
  no tuonelang stage crate. A **package** is a directory with a `tdg.toml` manifest and a
  **module root** (`[modules].root`, default `src`) of `.tuo` sources. The format defines:
  **identity** — `(name, version)`, the name validated (`PackageName`: `[a-z][a-z0-9_]*`)
  so it is a safe directory name, module prefix, and CLI token; **module roots** — the
  one directory whose `.tuo` files are the package's modules; **dependency resolution** —
  `resolve::resolve` follows path dependencies transitively, detecting cycles and
  duplicate names, and returns the whole graph (`ResolvedGraph`) in deterministic name
  order; **lockfile semantics** — `Lockfile` (`tdg.lock`, format `LOCKFILE_VERSION`) pins
  every resolved package's checksum and direct dependencies and is always written in name
  order, so a workspace resolves to a byte-identical lock; **checksums** — each package's
  content is SHA-256'd (`sha256`, a hand-rolled, FIPS-180-4-vector-pinned implementation,
  no new dependency) over its module names+text, and a compile refuses a **dependency**
  whose bytes drifted from the lock (`verify_against_lock`) while deliberately exempting
  the **root** package under active development; **edition selection** — `[package].edition`
  (`Edition`, only `2024` in v0) is an enum so an unknown edition is a load-time error,
  never silently accepted. v0 supports **local/path dependencies only** — a remote
  registry is a later addition and the format advertises no dependency kind the resolver
  cannot fetch. The manifest/lockfile use a small **hand-rolled TOML subset** codec
  (`toml`) — top-level key/values, `[table]`/`[[array-of-tables]]` headers, strings,
  non-negative integers, string arrays, inline tables — that reports a precise error on
  anything outside the subset rather than a silent misparse (no full-TOML dependency).
  The CLI (`tuo-cli`, layer 120) is the only host that reaches up to the compiler: its
  package commands resolve the graph, load every module source across it into one
  `SourceMap`, and drive the **exact same** `check_sources` / codegen / `tuo_spec::run`
  the file-based commands use — the package layer only decides *which sources* form the
  program. `tuo package symbols` compiles the resolved sources and reports the actual
  public, module-level symbols (`Resolution::symbols()`, the same surface the agent
  protocol and LSP project), which is what lets a tool *query installed package symbols
  without guessing*. Pinned by `tuo-package`'s unit tests + `tuo-package/tests/resolve.rs`
  (transitive graphs, cycle/duplicate/missing-dep detection, checksum drift vs. root-edit
  exemption, deterministic re-resolution) and `tuo-cli/tests/package_command.rs`
  (the whole lifecycle through the real binary: scaffold → check/test green, add/remove,
  a build resolving a dependency graph and running its specs, dependency-drift refusal,
  and the machine `symbols` query). The dependency-policy guard keeps `tuo-package` free
  of any stage dependency.
- Third-party deps and tuonelang crate paths are declared once in `[workspace.dependencies]`;
  members opt in with `dep.workspace = true`. Add shared versions there, not per-crate.
- `Cargo.lock` **is** committed (this is an application/toolchain workspace).
- New crates inherit metadata via `field.workspace = true` and should set `[lints] workspace = true`.

## Branching

Use `./new-branch.sh <suffix>` to create a branch named `DD-MM-YYYY-N-<suffix>`, where `N`
auto-increments across today's existing local and remote branches.
