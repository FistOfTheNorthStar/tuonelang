# tuonelang architecture

This document describes the intended architecture of the tuonelang compiler and
tooling. It reflects the current *foundation* — a Cargo workspace with crate
boundaries — not implemented functionality. Where it describes stages, those
stages are planned, not built.

> **tuonelang** is the language. It is designed around **TDG — Test Driven
> Generation**, the paradigm of driving and validating (often
> machine-generated) code with colocated, executable specifications. TDG is the
> methodology; tuonelang is the language and compiler built to make it
> first-class. Crates use the `tuo-` prefix and the CLI binary is `tuo`.

## Project goals

tuonelang aims to become:

- a statically typed language;
- memory safe by default;
- native compiled;
- fast to compile;
- suitable for high-performance software;
- easy for humans;
- exceptionally reliable for LLM generation;
- deterministic where practical;
- designed around excellent structured diagnostics;
- designed around colocated executable specifications;
- designed around one semantic IR shared by execution backends.

## Planned compiler architecture

```text
Source
  ↓
Lexer
  ↓
Lossless Syntax
  ↓
AST
  ↓
HIR
  ↓
Resolution
  ↓
Type Checking
  ↓
Ownership Checking
  ↓
MIR
  ├─ MIR Interpreter
  ├─ Cranelift
  └─ LLVM
```

**MIR is intended to become the single executable semantic representation of
tuonelang.**

The eventual MIR interpreter, Cranelift backend, and LLVM backend must all
consume the *same verified MIR*. This is a deliberate architectural constraint:
it prevents tuonelang's semantics from being independently reimplemented in multiple
backends, where they would inevitably drift. There is one source of truth for
"what a tuonelang program means," and every execution path flows through it.

Consequences enforced by the crate graph:

- `tuo-mir` is backend-agnostic. No Cranelift-, LLVM-, or target-specific types
  may appear in it.
- Both `tuo-codegen-cranelift` and `tuo-codegen-llvm` depend on `tuo-mir` (via
  the shared `tuo-codegen` interface), never the reverse.
- The interpreter (`tuo-mir-interp`) and the executable-specification layer
  (`tuo-spec`) run the same MIR the native backends compile.

## Planned tooling architecture

```text
                 Compiler Database
                        │
       ┌────────────────┼────────────────┐
       ▼                ▼                ▼
      CLI              LSP          Agent Protocol
```

The compiler's semantic engine is shared, not reimplemented per tool. The
query-based compiler database (`tuo-db`) plus the compiler facade
(`tuo-compiler`) form that shared engine. The CLI (`tuo-cli`), the language
server (`tuo-lsp`), and the agent protocol (`tuo-agent`) are thin surfaces over
it. This is what makes machine-readable diagnostics and agentic workflows
first-class rather than bolted on.

## Crate organization

The workspace is a set of small, architecturally focused crates under
`crates/`. Keeping crates small enforces clear boundaries, keeps compile times
and blast radius low, and makes the dependency direction explicit and
checkable.

| Crate | Responsibility |
|-------|----------------|
| `tuo-source` | Source files, byte offsets, spans, source maps. |
| `tuo-diagnostics` | Structured, machine-readable diagnostics. |
| `tuo-db` | Incremental, query-based compiler database (shared engine core). |
| `tuo-lexer` | Tokenization. |
| `tuo-syntax` | Lossless concrete syntax tree (trivia-preserving). |
| `tuo-parser` | Builds syntax/AST from tokens. |
| `tuo-ast` | Abstract syntax tree. |
| `tuo-hir` | High-level IR (desugared). |
| `tuo-resolve` | Name and path resolution. |
| `tuo-types` | Type checking and inference. |
| `tuo-ownership` | Ownership and memory-safety analysis. |
| `tuo-mir` | Mid-level IR — the single executable semantic representation. |
| `tuo-mir-interp` | Reference interpreter over MIR. |
| `tuo-spec` | Colocated executable specifications. |
| `tuo-codegen` | Backend-agnostic codegen interface. |
| `tuo-codegen-cranelift` | Cranelift native backend. |
| `tuo-codegen-llvm` | LLVM native backend. |
| `tuo-runtime` | Minimal native runtime support. |
| `tuo-stdlib` | Standard library scaffolding. |
| `tuo-fmt` | Deterministic source formatter. |
| `tuo-package` | Package/manifest model and build orchestration. |
| `tuo-compiler` | Facade coordinating the pipeline (see below). |
| `tuo-lsp` | Language server (reuses the shared engine). |
| `tuo-agent` | Agent protocol surface (reuses the shared engine). |
| `tuo-bench` | Benchmark harness support. |
| `tuo-cli` | The `tuo` binary. |

Third-party libraries that become architecturally significant are wrapped
behind tuonelang-owned abstractions (e.g. Cranelift and LLVM live behind
`tuo-codegen`) so the rest of the compiler never depends on them directly.

## Dependency architecture

The conceptual pipeline is:

```text
tuo-source
    │
    ▼
tuo-diagnostics
    │
    ▼
tuo-lexer
    │
    ▼
tuo-syntax
    │
    ▼
tuo-parser
    │
    ▼
tuo-ast
    │
    ▼
tuo-hir
    │
    ├───────────────┐
    ▼               ▼
tuo-resolve      tuo-types
                    │
                    ▼
              tuo-ownership
                    │
                    ▼
                 tuo-mir
              ┌─────┼─────┐
              ▼     ▼     ▼
          interpreter  Cranelift  LLVM
```

This is conceptual: not every crate depends directly on its predecessor. The
**hard invariant** is a total layering rule: every tuonelang-owned crate is
assigned to a numbered layer, and a crate may only depend on crates in
**strictly lower** layers. Lower compiler layers can never depend on higher
ones.

| Layer | Crates | Role |
|-------|--------|------|
| 0 | `tuo-source` | Foundation. |
| 10 | `tuo-diagnostics` | Structured diagnostics. |
| 20 | `tuo-db`, `tuo-lexer`, `tuo-syntax`, `tuo-ast` | Infrastructure & front-end data structures. |
| 30 | `tuo-parser`, `tuo-hir` | Front-end passes. |
| 40 | `tuo-resolve` | Name resolution. |
| 50 | `tuo-types` | Type checking. |
| 60 | `tuo-ownership` | Memory-safety analysis. |
| 70 | `tuo-mir` | The single executable semantic representation. |
| 75 | `tuo-runtime` | Native runtime support (below the backends, above MIR). |
| 80 | `tuo-mir-interp`, `tuo-codegen` | MIR consumers: interpreter, backend interface. |
| 90 | `tuo-codegen-cranelift`, `tuo-codegen-llvm`, `tuo-spec`, `tuo-stdlib` | Concrete backends, spec runner, stdlib. |
| 100 | `tuo-compiler` | Orchestration facade. |
| 110 | `tuo-fmt`, `tuo-package`, `tuo-lsp`, `tuo-agent`, `tuo-bench` | Tooling surfaces. |
| 120 | `tuo-cli` | The `tuo` binary; may orchestrate anything below it. |

Numbers are spaced by 10 so a new crate can slot between layers without
renumbering. The familiar rules all follow from the table: the lexer cannot
depend on type checking, syntax cannot depend on code generation, `tuo-mir`
cannot depend on Cranelift or LLVM (backend-specific types cannot appear in
it), no semantic crate can depend on CLI presentation, and the LSP and agent
tooling sit *above* the shared engine (`tuo-db` + `tuo-compiler`) and reuse it
rather than reimplementing it.

A few constraints point *downward* in the table and therefore need explicit
extra rules on top of layering:

- `tuo-compiler` stops at the `tuo-codegen` abstraction — it must never depend
  on `tuo-codegen-cranelift` or `tuo-codegen-llvm`, even though they sit lower.
- `tuo-compiler` must not link `tuo-stdlib`; the standard library is written
  *in* tuonelang and is compiler *input*, not a compiler component.

### Dependency cycles

Cycles between tuonelang crates are impossible by construction, twice over:

1. **Cargo** rejects any cyclic crate dependency graph outright — a cycle
   cannot even build.
2. **The layering rule** is stricter: every edge must strictly *decrease* the
   layer number, so a cycle (which would have to return to its starting layer)
   cannot be expressed even between crates Cargo would allow, and crates within
   the *same* layer may not depend on each other at all.

What Cargo alone would still permit — and the layer test exists to prevent —
is an *inverted* edge (e.g. `tuo-mir` → `tuo-codegen`) that is acyclic today
but would forbid the legitimate downstream edge forever after.

### Enforcement

The layer table and the extra forbidden edges are enforced automatically by a
lightweight dependency-policy test in the `tuo-cli` crate
(`crates/tuo-cli/tests/dependency_policy.rs`), which mirrors the table above —
keep the two in sync. It parses the workspace's `Cargo.toml` manifests
(including dev- and build-dependencies), asserts every crate under `crates/`
has a layer assignment (adding a crate forces an explicit layering decision),
and asserts every tuonelang-to-tuonelang edge points strictly downward. This is
intentionally small — it reads manifests and checks a table; it is **not** a
general-purpose architecture framework.

## The `tuo-compiler` facade crate — decision

**Decision: create `tuo-compiler` now.**

A small facade/orchestration crate is justified because three separate surfaces
— the CLI, the LSP, and the agent protocol — must all drive the pipeline
(`source → lex → parse → resolve → type check → ownership → MIR`) through *one*
shared entry point. Without a facade, each surface would re-wire the stages,
duplicating orchestration and inviting semantic drift — precisely what the
single-MIR principle exists to prevent. Establishing the crate boundary now
means later stages slot into a known coordination point rather than forcing a
restructure.

To keep it from becoming a dumping ground, `tuo-compiler` is constrained to
orchestration and re-exports only. Stage *logic* lives in the individual stage
crates; the facade wires them together and stops at the `tuo-codegen`
abstraction so it never learns about a specific backend, and it never depends on
CLI presentation.

## Explicit non-goals for the initial implementation

The following are explicitly out of scope for the initial implementation:

- self-hosting;
- macros;
- reflection;
- compile-time metaprogramming;
- GPU compilation;
- distributed runtime syntax;
- arbitrary compiler plugins.

## Workspace conventions

- **Edition & toolchain:** Rust edition 2024 on the stable channel.
- **Resolver:** Cargo `resolver = "3"`.
- **Lints:** Shared correctness/quality lints are configured workspace-wide in
  the root `Cargo.toml` (`[workspace.lints]`). `unsafe_code` is **forbidden**.
  We enable the Clippy `all` group plus a curated set of high-signal lints, and
  deliberately avoid enabling controversial/noisy groups (`pedantic`,
  `nursery`, `restriction`) wholesale. Warnings fail CI (`-D warnings`).
- **Release profile:** `lto = "thin"` with `codegen-units = 1`, trading release
  build time for faster, smaller compiler/tooling binaries. This affects only
  the release profile, not day-to-day dev builds.
- **Lockfile:** `Cargo.lock` is committed; tuonelang is an application/toolchain
  workspace and pins exact dependency versions for reproducibility.
