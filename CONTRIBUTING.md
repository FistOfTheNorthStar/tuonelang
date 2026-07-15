# Contributing to tuonelang

tuonelang is an experimental compiler project with strong architectural discipline.
These rules keep the codebase clean, deterministic, and reliable as it grows.

## Toolchain

- Use the **stable** Rust toolchain. It is pinned to the `stable` channel via
  `rust-toolchain.toml`, which also installs the required `rustfmt` and `clippy`
  components.
- tuonelang targets Rust **edition 2024**.
- Do not pin an arbitrary old Rust version.

## Formatting

All code must be formatted with the project's `rustfmt.toml`:

```bash
cargo fmt          # format
cargo fmt --check  # verify (CI runs this)
```

## Lints

The workspace enforces a curated lint set (`[workspace.lints]` in the root
`Cargo.toml`), including forbidding `unsafe` code. Clippy must pass with
warnings denied:

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

- Do **not** globally suppress Clippy warnings just to make CI green. If a lint
  must be allowed, scope the `#[allow(...)]` as narrowly as possible and explain
  why in a comment.
- Avoid `unsafe`; it is forbidden workspace-wide.

## Tests

Add meaningful tests for functionality that actually exists:

```bash
cargo test --workspace
```

- Do not add trivially true tests (e.g. `assert_eq!(2 + 2, 4)`).
- Do not write tests that imply an unimplemented stage works.

## Building and running

```bash
cargo check --workspace
cargo run -p tuo-cli -- --help
cargo run -p tuo-cli -- --version
```

## Architecture expectations

- Keep crates small and focused; avoid circular dependencies.
- Respect the dependency rules in [`ARCHITECTURE.md`](ARCHITECTURE.md). They are
  enforced by an automated dependency-policy test — a change that violates them
  will fail CI.
- Keep architecturally significant third-party libraries behind tuonelang-owned
  abstractions (e.g. backends live behind `tuo-codegen`).
- Do not add fake implementations or placeholder functions that return
  meaningless values to make future functionality look complete. A stage should
  either work or not exist yet.
- Document public APIs and record significant decisions as ADRs (see
  `specification/adr/README.md`).

## Adding language features

**A language feature is not complete merely because the parser accepts it.**

Silently adding major language features is prohibited. Language changes must
eventually be accompanied by specification and tests, and a complete feature
requires consideration of the full stack:

```text
syntax
semantics
types
ownership
MIR
interpreter behavior
native backends
diagnostics
formatter
LSP
agent protocol
tests
documentation
LLM benchmarks
```

Significant design decisions are recorded as ADRs and folded into the
`specification/CONSTITUTION.md` and `specification/grammar.ebnf`. Unresolved
questions belong in `specification/open-questions.md`.

## Before opening a pull request

Run the full local gate — the same commands CI runs:

```bash
cargo fmt --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
```
