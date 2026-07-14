# tuonelang

tuonelang is an experimental programming language and compiler project implemented in Rust.

The long-term goal is to explore a statically typed, memory-safe, high-performance
native language designed for both human programmers and AI coding agents.

> **Naming.** The language is **tuonelang**. It is designed around **TDG —
> Test Driven Generation**, a development paradigm in which colocated,
> executable specifications drive and validate machine-generated code. TDG is
> the *methodology tuonelang is built for*, not the name of the language. The
> CLI binary is `tuo` and all crates use the `tuo-` prefix.

> **tuonelang is experimental and is not yet a usable programming language.**
> There is no lexer, parser, type checker, or code generator yet. You cannot
> compile or run tuonelang programs. This repository currently contains only the
> project foundation: a Cargo workspace, crate boundaries, documentation, and a
> CLI that exposes `--help` and `--version`.

## Status

- ✅ Cargo workspace and crate architecture established.
- ✅ `tuo` CLI builds and supports `--help` / `--version`.
- ✅ Specification structure scaffolded (no language designed yet).
- ⛔ No compiler stages implemented (lexer, parser, types, ownership, MIR,
  interpreter, backends — all deferred).

No performance or reliability claims are made. The project's goals are
aspirational until validated by an actual implementation and benchmarks.

## Repository layout

```text
crates/          Rust crates (compiler pipeline, tooling, runtime, CLI)
specification/   Language Constitution, grammar, ADRs, open questions
tests/           Stage-organized compiler tests (placeholders for now)
benchmarks/      Compiler, runtime, and LLM benchmarks (placeholders)
corpus/          Compiler-validated tuonelang programs (currently empty)
examples/        Illustrative tuonelang programs (none yet)
tools/           Developer tooling (e.g. a future tokenizer lab)
```

See [`ARCHITECTURE.md`](ARCHITECTURE.md) for the planned compiler and tooling
architecture, [`specification/CONSTITUTION.md`](specification/CONSTITUTION.md)
for the frozen v0 language design, and [`CONTRIBUTING.md`](CONTRIBUTING.md) for
development rules.

## Development

tuonelang uses the current **stable** Rust toolchain (pinned to the `stable`
channel via `rust-toolchain.toml`) and Rust **edition 2024**.

```bash
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --check
cargo run -p tuo-cli -- --help
```

The binary produced by `tuo-cli` is named `tuo`:

```bash
cargo run -p tuo-cli -- --version
```

## License

Licensed under the [MIT License](LICENSE).
