# tuonelang

tuonelang is an experimental statically typed, memory-safe, native programming
language and compiler, implemented in Rust and designed for both human
programmers and AI coding agents.

> **Naming.** The language is **tuonelang**. It is designed around **TDG —
> Test Driven Generation**, a development paradigm in which colocated,
> executable specifications drive and validate machine-generated code. TDG is
> the *methodology tuonelang is built for*, not the name of the language. The
> CLI binary is `tuo` and all crates use the `tuo-` prefix.

## Status

The compiler front end, reference interpreter, and native backends are
implemented. tuonelang programs written against the **v0 runnable core**
compile, spec-check, and run today. The core is deliberate: integer
arithmetic, `if`/`else`, direct and recursive function calls, an integer
`main`, and — since ADR-0004 landed — **structs, enums, `Option`/`Result`,
fixed-capacity `[T; N]` arrays with checked indexing, and bounded `for`
iteration**, plus **IEEE-754 floats and borrow-mode (`in`/`mut`) calls**,
since ADR-0006 landed — the **borrowed `Str` string core** (literals,
equality, `std::str::{len, byte_at, slice}`) and the **effect boundary**
(`std::rt::{write, read_byte, exit}`, with `std::io::print`/`println` and
`std::process::exit` implemented over it; specs stay pure by the `R0007`
gate); and — since ADR-0009 landed — the **allocator core**: owned `String`
and growable `Array[Int]` allocating and freeing real heap memory
(`std::string::{empty, from_str, push_byte, append, concat, len, byte_at,
slice}`, `std::array::{empty, push, pop, len, get}`, `std::rt::write_string`,
with `std::io::read_line` and the `std::collections` array algorithms built on
top); and — since ADR-0008 Tier 1 landed — **first-class (non-capturing)
function values**: a bare top-level `fn` name is a `Copy` code pointer of type
`fn(mode T, …) -> R`, called indirectly with the identical direct-call ABI on
both backends, which powers the **generic higher-order stdlib combinators**
(`std::collections::{fold, map_into, filter_into, any, all}` over a function
value). All of it is compiled natively by both backends in lock-step with the
interpreter. The remaining capability gaps are **capturing closures** (a
heap-allocated captured environment — a future ADR Tier 2, non-capturing
function values already landed), concurrency (ADR-0007), the heap-wrapper
**values** `Box`/`Shared`/`Weak` (declared, construction refused — a later ADR),
and `Array[T]` for non-`Int` element types, not yet in the runnable core.

- ✅ Front end: lexer → parser (lossless CST) → resolver → type checker →
  ownership checker, with human and machine-versioned diagnostics.
- ✅ Colocated executable `spec` blocks, run through a reference MIR interpreter.
- ✅ Native compilation: a Cranelift debug backend and an optimizing LLVM
  `--release` backend, kept interpreter-equivalent by differential test suites.
- ✅ Tooling: canonical formatter, package system (manifest + lockfile + path
  deps), LSP core, an agent protocol server, a compiler-validated corpus, and
  benchmark harnesses.
- 📋 The **0.1 release gate** (`specification/RELEASE-0.1-GATE.md`) currently
  reads **READY** — all sixteen criteria are `MET`, each backed by a committed
  proving artifact.

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

For real, multi-function programs written against v0, see [`examples/`](examples/)
(four run natively — cli-stats prints its report and http-service prints its
response line through the effect boundary; the concurrent worker documents its
effectful shell over a runnable decision core) and
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
| `tuo new <name>` · `add` · `remove` · `test` | Package lifecycle over a `tdg.toml` manifest and `tdg.lock`. |
| `tuo agent --stdio` | Serve the versioned JSON-lines agent protocol (one long-lived compiler DB). |
| `tuo debug syntax\|ast\|hir\|mir <file>` | Developer dumps (unstable output, not a language protocol). |

See [`CLAUDE.md`](CLAUDE.md) for the full command surface and the conventions
each command upholds.

## Repository layout

```text
crates/          Rust crates (compiler pipeline, tooling, runtime, CLI)
specification/   Constitution, grammar, normative semantics docs, ADRs, the 0.1 gate
tests/           Stage-organized compiler tests (lexer, parser, types, ownership, mir, …)
benchmarks/      Compiler, runtime, and LLM-codegen benchmarks
corpus/          Compiler-validated tuonelang programs (six categories)
examples/        Real multi-function programs dogfooding the v0 core
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
