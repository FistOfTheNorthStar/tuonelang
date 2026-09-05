# tuonelang examples

Real, multi-function tuonelang programs, dogfooded against v0 (Prompt 39). These
are not toys chosen to flatter the language: each is genuine application logic
written in whatever v0 can actually express, validated by the **real compiler**,
and re-checked on every `cargo test` by
[`crates/tuo-cli/tests/dogfood_examples.rs`](../crates/tuo-cli/tests/dogfood_examples.rs)
so they cannot rot. The findings from building them — and the ADRs those
findings produced — are written up in [`../DOGFOODING.md`](../DOGFOODING.md).

tuonelang v0 runs the **scalar, control-flow core** (`Int`, `if`/`else`, calls,
recursion) plus the ADR-0004 aggregates (structs, enums, fixed `[T; N]` arrays,
bounded `for`), the borrowed `Str` core and effect boundary (ADR-0006), the
allocator core — owned `String` and growable `Array[T]` (ADR-0009/0012) — the
hash map (ADR-0011), first-class non-capturing function values (ADR-0008
Tier 1), structured fork-join concurrency (ADR-0007), and the OS/socket effect
boundary (ADR-0013/0014/0015/0017). The `CONTRACT:` tier these examples once
needed is now **empty**: every example below runs for real. What v0 still
cannot express is documented *in place* by the example that hit it — see
`router/`, whose split table is the shape ADR-0012 forced. Dogfooding also
*fixes* the compiler when it finds a gap: `log-analytics/` was refused
natively because neither backend lowered `Rvalue::Len` (what `for x in xs`
over a **growable** `Array[T]` compiles to), so both backends gained that
lowering, pinned three-way by `tests/codegen/fixtures/arr_for_len.tuo`.
Nothing here pretends to do what the language cannot.

| Example | Kind | v0 status | `main` exit |
|---------|------|-----------|:-----------:|
| [`cli-stats/`](cli-stats/) | command-line application | runs natively, prints its report via `std::io::println` | 18 |
| [`data-pipeline/`](data-pipeline/) | JSON/record processing | runs natively | 144 |
| [`workspace/`](workspace/) | medium multi-package project (`app → geometry → numeric`) | builds natively | 26 |
| [`http-service/`](http-service/) | HTTP service *(serves a real request over a live loopback socket)* | runs natively | 200 |
| [`concurrent-worker/`](concurrent-worker/) | concurrent worker *(runs its pool live via `par_map` + a channel-drained queue)* | runs natively | 15 |
| [`router/`](router/) | declarative request router *(runtime dispatch table, indirect calls)* | runs natively | 74 |
| [`log-analytics/`](log-analytics/) | log aggregation *(one-pass keyed rollup over `Map[Int, Int]`)* | runs natively | 42 |
| [`file-report/`](file-report/) | report generator *(renders, writes, reads back, verifies, cleans up)* | runs natively | 7 |
| [`postgres-auth/`](postgres-auth/) | PostgreSQL v3 auth handshake *(wire framing + SCRAM-SHA-256 vs RFC 7677, and the legacy MD5 challenge)* | runs natively | 48 |
| [`postgres-client/`](postgres-client/) | PostgreSQL client *(connects to a **real server**, SCRAM-SHA-256 over TCP, runs a query)* | runs natively | 42 |

## Running them

```bash
# A program: check, run its specs, execute it. (cli-stats consumes the stdlib
# std::io module as input — src/std_io.tuo, a pinned verbatim copy — so the
# file-based commands take both sources.)
tuo check examples/cli-stats/src/main.tuo examples/cli-stats/src/std_io.tuo
tuo test  --manifest examples/cli-stats
tuo run   examples/cli-stats/src/main.tuo examples/cli-stats/src/std_io.tuo ; echo $?
# count 7 / mean 12 / sd 6 / report 18, then exit 18

# The router: 9 specs over the dispatch table, then a real indirect-call run.
tuo test --manifest examples/router                    # 9 passed, 0 failed
tuo run  examples/router/src/main.tuo ; echo $?        # 74

# log-analytics: the map rollup is pinned against an independent re-scan, and
# the program is asserted to give the same answer on BOTH backends.
tuo test --manifest examples/log-analytics             # 16 passed, 0 failed
tuo run  examples/log-analytics/src/main.tuo ; echo $?           # 42
tuo run  --release examples/log-analytics/src/main.tuo ; echo $? # 42

# file-report: really writes, reads back, verifies, and removes a file, so run
# it from a scratch directory. Its exit byte is the number of report lines it
# verified through the disk, and it leaves nothing behind.
tuo test --manifest examples/file-report               # 6 passed, 0 failed
cd /tmp && tuo run ~/Projects/tuonelang/examples/file-report/src/main.tuo ; echo $?
# the 7-line report, then `verified 7`, then exit 7

# The multi-package workspace: check/test the graph, then build + run the binary.
# (`tuo run` is file-based, so a package binary is built and then executed —
#  see DOGFOODING.md finding D-7.)
tuo test  --manifest examples/workspace/app            # 14 specs across 3 packages
tuo build --manifest examples/workspace/app -o /tmp/app && /tmp/app ; echo $?   # 26
```

Each package resolves a `tdg.lock` on build; those lockfiles embed
machine-absolute dependency paths and are therefore **not committed** (they are
gitignored under `examples/`, and regenerated on demand — DOGFOODING.md finding
D-6).
