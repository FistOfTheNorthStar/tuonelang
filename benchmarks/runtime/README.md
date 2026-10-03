# Runtime benchmarks

Runtime performance of *compiled* tuonelang programs, measured by really
compiling, linking, and running them — never simulated. The harness core lives
in the `tuo-bench` crate (`lab::runtime`); the committed programs live here.

## What is measured

The catalog (`tuo_bench::lab::runtime::workloads`) has two halves:

- **Language-feature workloads**: one per runtime capability as it landed
  (startup, integer computation, calls and recursion, fixed arrays, `Str`, the
  allocator, function values, maps, files, sockets, channels, JSON, SHA-256,
  constant-time comparison, wire decoding, UDP, bounded connects). Each was
  added with the ADR that made it expressible.
- **The Computer Language Benchmarks Game** (`BENCHMARKS_GAME`): `nbody`,
  `spectral-norm`, `fannkuch-redux`, `mandelbrot`, and `fasta`, plus
  `binary-trees`, which is recorded **unsupported**: its tree of individually
  allocated nodes needs native `Box` values, which v0 refuses at build time.
  These are the established cross-language programs, so a statement about
  tuonelang's speed rests on benchmarks other languages are measured by,
  not on microbenchmarks chosen here. Each runs the benchmark's own algorithm
  and folds its output (an energy, a norm, an image, a sequence) into the exit
  byte instead of printing it.

The `programs/tuo/*.tuo` files **are** the recorded source: the harness embeds
them via `include_str!`, so a file and its measurement can never drift. An
unsupported workload carries the exact reason and no number, and enters the
measured set the moment its feature lands.

## The speed table

```bash
cargo test -p tuo-cli --test lab_command benchmarks_game_speed_table -- --ignored --nocapture
```

builds every supported Benchmarks Game workload with `tuo build --release` and
with each peer's optimizing compiler (`cc -O2 -ffp-contract=off`, `rustc -O`,
`go build`), times each binary (fastest of five runs after a warm-up), and
prints the ratio of tuonelang's time to each peer's. Every run must exit with
the workload's checksum byte, so a time is only printed for a program that
computed the right answer. CI runs it on every push so the current figures sit
in the log; it asserts correctness only, never a ratio, because wall-clock on a
shared runner is too noisy to gate on.

The floating-point peers are written so all four languages perform the same
roundings: C is built with `-ffp-contract=off`, Go products are wrapped in
explicit `float64` conversions (which the Go spec forbids fusing), and neither
rustc nor tuonelang's LLVM pipeline contracts a multiply-add on its own.

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

**Rust** is the third peer, for the Benchmarks Game workloads (programs under
[`programs/rust/`](programs/rust/)): the safe, runtime-free language at the top
of that suite, compiled with `rustc -O`.

Each supported workload has an equivalent-semantics program in **both** C and Go,
computing the same result the same way (same arithmetic, same recursion, same
byte scans; the allocation peers even replicate the explicit doubling growth
rather than leaning on Go's built-in `append` heuristic). Every source set
(`programs/tuo/`, `programs/c/`, `programs/go/`, `programs/rust/`) is embedded
via `include_str!`, so a workload and its peers can never drift.

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
