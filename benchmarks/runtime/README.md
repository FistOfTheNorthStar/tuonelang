# Runtime benchmarks

Runtime performance of *compiled* tuonelang programs, measured by really
compiling, linking, and running them — never simulated. The harness core lives
in the `tuo-bench` crate (`lab::runtime`); the committed programs live here.

## What v0 can and cannot measure

tuonelang v0 compiles and runs the **scalar, control-flow core** — `Int`
arithmetic, comparison, `if`/`else`, function calls, and recursion — plus,
since ADR-0004 Stage 2, the **fixed-capacity array** `[T; N]` (inline,
stack-allocated; see the codegen conventions in the root `CLAUDE.md`). It has
**no heap, no runtime string values, and no effect boundary** yet.

The performance-laboratory prompt lists eight runtime workloads. Five map onto
the runnable core and are measured with real programs; three cannot be expressed
at all in v0 and are recorded — honestly — as unsupported, with the exact
reason, and **no fabricated number**. The moment a feature lands, its entry
gains a program and the lab measures it with no other change — exactly the move
`collections` made when the fixed array landed.

| Workload | Status | Program |
|----------|--------|---------|
| `startup` | measured | [`programs/tuo/startup.tuo`](programs/tuo/startup.tuo) |
| `integer-computation` | measured | [`programs/tuo/integer-computation.tuo`](programs/tuo/integer-computation.tuo) |
| `function-calls` | measured | [`programs/tuo/function-calls.tuo`](programs/tuo/function-calls.tuo) |
| `recursion` | measured | [`programs/tuo/recursion.tuo`](programs/tuo/recursion.tuo) |
| `collections` | measured | [`programs/tuo/collections.tuo`](programs/tuo/collections.tuo) |
| `allocation` | **not yet expressible** | no heap-allocating type is lowered to native code (`[T; N]` is inline and allocates nothing) |
| `string-processing` | **not yet expressible** | no runtime `String` value is lowered |
| `networking` | **not yet expressible** | no effect boundary (no FFI/syscalls) exists |

The `programs/tuo/*.tuo` files **are** the recorded source: the harness embeds
them via `include_str!`, so a file and its measurement can never drift.

## Comparison against established languages

The prompt requires comparison *only against languages with equivalent
semantics*. Two AOT-native peers are used, and together they bracket tuonelang:

- **C** — the runtime-free peer. Like tuonelang it compiles ahead-of-time to
  native code with a matching integer model and **no runtime** between the
  program and the CPU. Programs under [`programs/c/`](programs/c/).
- **Go** — the runtime-bearing peer. Also AOT-native with a matching 64-bit
  integer / byte-slice model, but it ships a **managed runtime** (garbage
  collector + goroutine scheduler), so it measures the AOT-with-a-runtime point
  that C does not. Programs under [`programs/go/`](programs/go/).

Each supported workload has an equivalent-semantics program in **both** peers,
computing the same result the same way (same arithmetic, same recursion, same
byte scans; the allocation peers even replicate the explicit doubling growth
rather than leaning on Go's built-in `append` heuristic). The three source sets
(`programs/tuo/`, `programs/c/`, `programs/go/`) are embedded via `include_str!`,
so a workload and its two peers can never drift.

A comparison is reported **only when both languages actually compiled and ran**
under recorded toolchains and produced the same result. If a peer toolchain is
absent (`cc` for C, `go` for Go), or the peer program does not produce the
semantically-equal result, that comparison is recorded as *skipped* with the
reason — never a one-sided or fabricated figure. Unsupported workloads have no
comparison for any peer, because you cannot compare a feature that does not
exist.

## What every run records

Per the prompt, a run records the hardware, OS, compiler versions, exact
commands, and source — all captured live into a `LabReport` (see
[`../README.md`](../README.md) and the committed example
[`results/example-report.json`](results/example-report.json)). Nothing is
hard-coded; an unobservable fact is reported as `unknown`, not guessed.

## No unsupported claims

The lab publishes **no** superlative and **no** aggregate verdict. There is no
"blazing fast" anywhere; the human report prints measured numbers, unmeasured
workloads with their reasons, and comparisons only where a real number backs
them. The repository is built to *prove* a claim before it is made — which for
most of v0's runtime story means proving, precisely, what is not yet measurable.
