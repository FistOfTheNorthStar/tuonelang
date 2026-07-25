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
cargo run -p tdg-cli -- debug syntax file.tuo  # dump the lossless CST (dev tool)
cargo run -p tdg-cli -- debug ast file.tuo     # dump the typed AST views (dev tool)
cargo run -p tdg-cli -- fmt file.tuo           # rewrite into canonical format
cargo run -p tdg-cli -- fmt --check file.tuo   # verify canonical formatting (exit 1 if not)
cargo run -p tdg-cli -- debug hir file.tuo     # dump the lowered HIR (dev tool)
cargo run -p tdg-cli -- debug mir file.tuo [fn] # dump the lowered MIR (dev tool)
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
  only on `tdg-source` and `tdg-diagnostics`.
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
- **The CLI must never advertise behavior the compiler can't perform.** Subcommands
  (`build`, `run`) are deliberately absent until their functionality exists; the
  `Command` enum in `tdg-cli/src/cli.rs` is the extension point. Implemented so far:
  `tdg check <files>` (the parse → resolve → type-check → ownership-check front end,
  specs included per ADR-0002 — specs are checked but **not** executed here),
  `tdg spec [--target <name>] <files>` and `tdg verify <files>` (execute the program's
  colocated specs through the reference MIR interpreter — `spec` runs the selected
  specs, `verify` runs all static checks *and* the specs; both refuse a program with
  front-end errors, run each spec in the interpreter's deterministic sandbox with
  configurable fuel/recursion/memory limits, and report measured timing with no latency
  promise), `tdg fmt [--check] <files>` (the canonical formatter — deterministic,
  idempotent, zero configuration), and `tdg debug syntax|ast|hir|mir <file>`
  (diagnostic developer tools with unstable output, not language protocols; `mir`
  requires an accepted program, since MIR is only defined once the front end passes,
  and the lowered MIR is verified (`tuo_mir::verify`, mandatory) before it is dumped —
  every backend and the interpreter must reject unverified MIR).
- **Machine output is a versioned contract, human output is not.** A global
  `--message-format` selects `human` (default), `json` (one envelope), or
  `json-lines` (streamed, one event per line) for every result-producing command
  (`check`, `spec`, `verify`, `fmt`). The wire shape lives in `tdg-cli`'s
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
- Third-party deps and TDG crate paths are declared once in `[workspace.dependencies]`;
  members opt in with `dep.workspace = true`. Add shared versions there, not per-crate.
- `Cargo.lock` **is** committed (this is an application/toolchain workspace).
- New crates inherit metadata via `field.workspace = true` and should set `[lints] workspace = true`.

## Branching

Use `./new-branch.sh <suffix>` to create a branch named `DD-MM-YYYY-N-<suffix>`, where `N`
auto-increments across today's existing local and remote branches.
