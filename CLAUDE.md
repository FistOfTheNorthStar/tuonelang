# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**TDG** is an experimental statically typed, memory-safe native language and compiler,
designed to be LLM-friendly. The repository directory is `tuonelang`; the crate/package
prefix is `tdg-` and the compiler binary is `tdg`.

**Current state: scaffolding.** Nearly every crate is a documented stub — the crate
boundaries, dependency edges, and intended responsibilities are established, but the
pipeline logic is not implemented yet. Each `lib.rs` doc comment states what the crate
*will* own and which layers it may depend on. When adding functionality, respect the
boundary each stub describes rather than reshaping the graph.

## Commands

```bash
cargo build                    # build the whole workspace
cargo build -p tdg-cli         # build a single crate (produces the `tdg` binary)
cargo run -p tdg-cli -- --help # run the CLI
cargo run -p tdg-cli -- check file.tuo         # parse, resolve, type-check, ownership-check (specs included)
cargo run -p tdg-cli -- spec file.tuo          # execute the program's specs (MIR interpreter)
cargo run -p tdg-cli -- spec --target f file.tuo # run only the specs of function `f`
cargo run -p tdg-cli -- verify file.tuo        # all static checks + execute specs
cargo run -p tdg-cli -- verify --affected-by a.tuo a.tuo b.tuo # …only specs an edit to a.tuo could affect
cargo run -p tdg-cli -- debug syntax file.tuo  # dump the lossless CST (dev tool)
cargo run -p tdg-cli -- debug ast file.tuo     # dump the typed AST views (dev tool)
cargo run -p tdg-cli -- fmt file.tuo           # rewrite into canonical format
cargo run -p tdg-cli -- fmt --check file.tuo   # verify canonical formatting (exit 1 if not)
cargo run -p tdg-cli -- build file.tuo         # compile to a native executable (Cranelift, debug)
cargo run -p tdg-cli -- build -o out file.tuo  # …to a chosen path
cargo run -p tdg-cli -- build --release file.tuo # …optimized, via the LLVM backend
cargo run -p tdg-cli -- run file.tuo           # compile and run; exit status = the program's result
cargo run -p tdg-cli -- run --release file.tuo # …compile with the optimizing LLVM backend, then run
cargo run -p tdg-cli -- debug hir file.tuo     # dump the lowered HIR (dev tool)
cargo run -p tdg-cli -- debug mir file.tuo [fn] # dump the lowered MIR (dev tool)
cargo run -p tdg-cli -- debug mir --opt file.tuo # …after the MIR optimization passes
cargo run -p tdg-cli -- --message-format=json verify file.tuo      # machine protocol: one versioned envelope
cargo run -p tdg-cli -- --message-format=json-lines spec file.tuo  # machine protocol: streamed, one event per line
cargo test                     # run all tests
cargo test -p tdg-cli          # test one crate
cargo test -p tdg-cli command_definition_is_valid  # run a single test by name
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

- **`tdg-source`** — source files, byte offsets, spans, source maps. Foundation; depends on nothing.
- **`tdg-diagnostics`** — structured, machine-readable diagnostics.
- **`tdg-db`** — incremental, query-based compiler database. The shared computation layer
  that CLI, LSP, and the agent protocol all drive, so semantics are computed once. Depends
  only on `tdg-source` and `tdg-diagnostics`, so it owns the *stage-agnostic* red-green
  engine (`QueryEngine`, driven by a host's `QueryHost::compute`), **not** the stage wiring —
  the real per-stage queries are registered by `tdg-compiler` (which sees every stage). No
  engine type leaks into a host's public API; the boundary speaks source identities, plain
  values, and `QueryError`.
- **`tdg-lexer`** → **`tdg-syntax`** (lossless CST) → **`tdg-parser`** → **`tdg-ast`**.
- **`tdg-hir`** — desugared HIR lowered from the AST; shared input to resolution and typing.
- **`tdg-resolve`** (name/path resolution) → **`tdg-types`** (inference/checking) →
  **`tdg-ownership`** (memory-safety checking).
- **`tdg-mir`** — the single executable semantic representation; **`tdg-mir-interp`** is its
  reference interpreter.
- **`tdg-codegen`** — backend-agnostic codegen interface, with two backends behind it:
  **`tdg-codegen-cranelift`** and **`tdg-codegen-llvm`** (both consume MIR).
  **`tdg-runtime`** provides minimal native runtime support.

**`tdg-compiler`** is the facade: the single orchestration seam that wires the stages end
to end so the CLI, LSP, and agent protocol don't each re-wire the pipeline. It depends on
the semantic-stage crates up through the `tdg-codegen` abstraction, **never on a concrete
backend and never on CLI presentation**. Keep it to orchestration + re-exports; stage
*logic* lives in the individual stage crates.

Tooling surfaces on top of the facade: **`tdg-cli`** (the `tdg` binary), **`tdg-lsp`**
(language server), **`tdg-agent`** (exposes compiler feedback to coding agents),
**`tdg-fmt`** (formatter), **`tdg-package`** (package/build orchestration),
**`tdg-stdlib`**, **`tdg-spec`** (colocated executable specs), and **`tdg-bench`**.

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
  `tdg-cli/src/cli.rs` is the extension point. Implemented so far:
  `tdg check <files>` (the parse → resolve → type-check → ownership-check front end,
  specs included per ADR-0002 — specs are checked but **not** executed here),
  `tdg spec [--target <name>] <files>` and `tdg verify [--affected-by <file>] <files>`
  (execute the program's colocated specs through the reference MIR interpreter — `spec`
  runs the selected specs, `verify` runs all static checks *and* the specs; both refuse a
  program with front-end errors, run each spec in the interpreter's deterministic sandbox
  with configurable fuel/recursion/memory limits, and report measured timing with no
  latency promise; `verify --affected-by <file>` runs only the specs whose semantic
  dependency closure touches a symbol defined in `<file>` — the specs an edit to it could
  have changed, selected through the incremental dependency graph — and a cold `verify`
  with no such flag runs every spec), `tdg fmt [--check] <files>` (the canonical
  formatter — deterministic,
  idempotent, zero configuration), `tdg build [-o <path>] [--release] <files>` and
  `tdg run [--release] <files>` (compile the accepted program to verified MIR, run the
  TDG-native MIR optimization passes over it, and then to native code, linking the
  runtime trap shim into an executable — the default debug build uses the Cranelift
  backend, `--release` uses the optimizing LLVM backend; `run` additionally executes it
  and exits with the program's own status, the integer its nullary `main` returns; both
  backends lower the **scalar, control-flow core** today and *refuse* — never
  mis-compile — anything outside it, pointing the user back to the interpreter as the
  reference), and `tdg debug syntax|ast|hir|mir [--opt] <file>` (diagnostic developer
  tools with unstable output, not language protocols; `mir` requires an accepted
  program, since MIR is only defined once the front end passes, and the lowered MIR is
  verified (`tuo_mir::verify`, mandatory) before it is dumped — every backend and the
  interpreter must reject unverified MIR; `mir --opt` shows the MIR after the
  optimization passes the native build applies).
- **Codegen is behind a TDG-owned interface; no backend type leaks upward.**
  `tdg-codegen` defines `CodegenBackend` (verified MIR + `TypeckResult` → a
  relocatable `ObjectArtifact`) and the plain values that cross the boundary
  (`TargetSpec`, `ObjectArtifact`, `CodegenError`, `EntryAbi`). Two backends implement
  it — `tdg-codegen-cranelift` (the default debug build) and `tdg-codegen-llvm` (the
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
  `tdg-cli/tests/codegen_differential.rs` (interpreter vs default backend), the
  **three-way** `tdg-cli/tests/codegen_three_way.rs` (interpreter == Cranelift == LLVM),
  and the randomized `tdg-cli/tests/differential.rs` (both backends over generated
  programs), all over `tests/codegen/fixtures/`; any mismatch is a release blocker.
  `tdg-runtime` is the minimal native runtime linked into
  every built binary: it owns the deterministic trap (a stable `TrapCode` → stderr
  message → `abort()` with a fixed status), emitted as C so a generated executable
  needs no Rust runtime.
- **MIR optimization is a set of isolated, meaning-preserving, re-verified passes;
  the interpreter stays the reference on _unoptimized_ MIR.** `tdg-mir`'s `opt` module
  (`tuo_mir::optimize`) runs a small pipeline — constant folding, simple copy
  propagation, unreachable-block removal, dead-local elimination — to a bounded fixed
  point on the native build path (`tdg build`/`tdg run`), before the backend and *after*
  the mandatory verifier. Each pass is one `Pass` with a declared purpose and
  preconditions; the driver calls `tuo_mir::debug_assert_verified` after every pass, so a
  pass that corrupts MIR panics (named) in debug/test rather than reaching a backend.
  Every pass is a rewrite that leaves observable behavior (return value, and which/whether
  it traps) unchanged — in particular constant folding **never** folds a trapping
  operation (`1/0`, `MIN/-1`, overflow, `MIN` negation), since that would erase an
  observable abort. Nothing speculative lives here (no inlining, no loop transforms); the
  release backend layers LLVM's own optimizer on top. The contract is pinned by the
  before/after golden suite (`tests/mir/opt/fixtures/*.mir` / `*.opt.mir`,
  `tdg-mir/tests/opt_golden.rs`), the MIR-level semantic differential and compile-time /
  code-size **measurement** (`tdg-cli/tests/opt_semantics.rs`, interpreter agrees on raw
  vs optimized MIR), and the existing native differential suites — which now compile
  *optimized* MIR, so they pin interpreter (unopt) == Cranelift (opt) == LLVM (opt +
  LLVM O2). Any divergence is a bug in the pass, never the interpreter.
- **The runtime ABI is TDG-owned, backend-independent, and versioned before it is
  frozen.** `tdg-runtime` is the single normative home of the ABI compiled programs
  obey — value layouts, panic/trap, startup/exit, the allocation boundary,
  destruction, and internal calling conventions — specified in prose in
  `specification/abi.md` and implemented in the crate's `abi` and `alloc` modules.
  Layouts (`abi::layout_of` → `Layout{size,align}`, computed from `tdg-types` alone;
  `#[repr(C)]` packing, explicit `u32` enum discriminants numbered in declaration
  order to match the interpreter's `Value::Variant`, no niche packing in v0) carry no
  Cranelift or LLVM type: a backend *consults* them, never defines its own, so the two
  backends and the interpreter cannot drift into incompatible memory models. The ABI
  carries an explicit `abi::ABI_VERSION` (currently `0`), bumped on any
  layout-affecting change in the same commit that moves the pinning tests. The heap
  allocator is one swappable seam — the C-ABI `tuo_rt_alloc`/`tuo_rt_dealloc`
  (`alloc_runtime_c_source`, OOM traps, never returns null) — through which every
  `Box`/`Shared`/`String`/`Array` allocation flows. ABI layout tests
  (`tdg-runtime/tests/abi_layout.rs`, checked against real `#[repr(C)]` Rust types) and
  interpreter⇄ABI equivalence tests (`tdg-cli/tests/abi_equivalence.rs`) pin the
  contract; the crate stays independent of any concrete backend.
- **Machine output is a versioned contract, human output is not.** A global
  `--message-format` selects `human` (default), `json` (one envelope), or
  `json-lines` (streamed, one event per line) for every result-producing command
  (`check`, `spec`, `verify`, `fmt`, `build`, `run`). The wire shape lives in `tdg-cli`'s
  `protocol` module, versioned by `PROTOCOL_VERSION`; every machine message
  carries the protocol version, event kind, command, status, stable diagnostics
  (serialized with the independently-versioned `tuo_diagnostics::json` schema),
  relevant IDs, and source ranges. In a machine format **stdout carries protocol
  output only**, and internal logging reaches **stderr only under `--log`**. The
  `debug` dumps have no machine encoding (unstable developer output) and reject a
  non-human format. The contract is pinned by `tests/cli/protocol/` fixtures and
  the backwards-compatibility tests in `tdg-cli/tests/protocol_command.rs`:
  additive changes are allowed without a bump, but dropping/renaming a guaranteed
  field or changing the version must move `PROTOCOL_VERSION` and the schema
  fixture together.
- **Incremental compilation is fine-grained at the query-graph level, and
  correctness beats invalidation reduction.** `tdg-db` owns a generic red-green
  engine (`QueryEngine`): opaque `QueryKey`s, opaque host-comparable `StoredValue`s
  (the host's own `PartialEq` decides **early cutoff**), dependency recording, cycle
  detection, and cooperative cancellation — pinned by `tdg-db/tests/incremental.rs`.
  Because `tdg-db` may not see a stage crate, the real stage wiring lives in
  `tdg-compiler`'s `IncrementalSession`, which implements `QueryHost` and registers the
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
  assertions on which queries re-execute** (`tdg-compiler/tests/incremental_stages.rs`) and
  reported with measured timing (`.../incremental_measure.rs`, run with `--nocapture`).
  Affected-spec selection (`IncrementalSession::affected_specs`, `Selection::Affected` in
  `tdg-spec`, `tdg verify --affected-by`) runs only the specs an edit may have changed and
  is proven sound, precise, and verdict-preserving in
  `tdg-compiler/tests/affected_specs.rs`.
- Third-party deps and TDG crate paths are declared once in `[workspace.dependencies]`;
  members opt in with `dep.workspace = true`. Add shared versions there, not per-crate.
- `Cargo.lock` **is** committed (this is an application/toolchain workspace).
- New crates inherit metadata via `field.workspace = true` and should set `[lints] workspace = true`.

## Branching

Use `./new-branch.sh <suffix>` to create a branch named `DD-MM-YYYY-N-<suffix>`, where `N`
auto-increments across today's existing local and remote branches.
