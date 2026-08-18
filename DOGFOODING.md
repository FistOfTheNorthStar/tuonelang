# Dogfooding tuonelang

This report is the result of *using* tuonelang v0 to build real, multi-function,
multi-package programs — not testing the compiler from the inside, but writing
application code against it the way a user would. The rule the whole exercise
obeys is the one the project already lives by and Prompt 39 restates:

> Build substantial real programs **without adding syntax merely to make one
> example easier**. Every proposed language change discovered during dogfooding
> must receive an ADR and benchmark consideration **rather than being added ad
> hoc**.

So nothing below was smoothed over by extending the language. Where v0 could
express a program, it was written in v0 and is validated by the real compiler on
every `cargo test`. Where v0 could **not** express something, that became a
recorded *finding* — and the load-bearing findings each became an
[ADR](specification/adr/) with a benchmark plan, never an ad-hoc feature.

Everything here is reproducible: each figure comes from running the real `tuo`
binary, and the examples are re-validated by
[`crates/tuo-cli/tests/dogfood_examples.rs`](crates/tuo-cli/tests/dogfood_examples.rs)
so they cannot rot.

---

## The five projects

Prompt 39 asks for five kinds of program. At the time of the exercise,
tuonelang v0 ran only the **scalar, control-flow core** — `Int` arithmetic,
comparison, `if`/`else`, function calls, and recursion — with no strings, no
heap, no collections, no I/O, and no concurrency. Three of the five kinds fit
inside that core and were **fully runnable**; two (an HTTP service and a
concurrent worker) could not run in v0, and the prompt itself hedges the HTTP
service with *"when networking support exists."* For those two, the honest
dogfooding move was to build the **pure decision core** that v0 *can* run
(routing/status logic; scheduling arithmetic) and mark the effectful shell as a
documented **contract tier**, mirroring exactly how `tdg-stdlib` separates its
executable tier from its `CONTRACT:` tier. Nothing pretends to do what the
language cannot. (The ADRs this exercise opened have since moved the boundary:
ADR-0004 landed aggregates, and ADR-0006 landed `Str` and the effect boundary —
the table below reflects the examples as they stand today, with cli-stats
printing its report and http-service parsing and printing a request/response
line.)

| # | Project | Directory | v0 status | `main` exit | Specs |
|---|---------|-----------|-----------|:-----------:|:-----:|
| 1 | Command-line application | [`examples/cli-stats/`](examples/cli-stats/) | **runs natively**, prints its report | 18 | 12 |
| 2 | JSON/data-processing | [`examples/data-pipeline/`](examples/data-pipeline/) | **runs natively** | 144 | 6 |
| 3 | Medium multi-package project | [`examples/workspace/`](examples/workspace/) | **builds natively** (3-package graph) | 26 | 14 |
| 4 | HTTP service *(when networking exists)* | [`examples/http-service/`](examples/http-service/) | **runs natively**: parses + prints; sockets are contract-tier | 200 | 12 |
| 5 | Concurrent worker | [`examples/concurrent-worker/`](examples/concurrent-worker/) | scheduling core runs; execution is contract-tier | 15 | 8 |

**1 — cli-stats.** A descriptive-statistics tool: sum, min/max, range, floored
mean, a scaled dispersion measure, and a bounded integer square root, over a
fixed seven-observation dataset. Real reductions, all recursive or folded, all
spec-pinned. Since ADR-0006 it prints its four-line report (`count 7` … `report 18`)
through `std::io::println`, consuming the stdlib's `std::io` module as input
(`src/std_io.tuo`, pinned verbatim against the catalog). Reproduce:

```console
$ tuo check examples/cli-stats/src/main.tuo examples/cli-stats/src/std_io.tuo  # exit 0
$ tuo test  --manifest examples/cli-stats        # 12 passed, 0 failed
$ tuo run   examples/cli-stats/src/main.tuo examples/cli-stats/src/std_io.tuo ; echo $?   # report + 18
```

**2 — data-pipeline.** The essence of a JSON/record processor: records are
*packed integers* (`category * 100000 + amount`), and the pipeline **decodes a
field** (integer division/modulo — what a JSON number scanner reduces to once the
bytes are in hand), **filters** by category, and runs a fused **filter+map+reduce**
to total and average a category. Answers "total transport spend" = 400, exits
`400 & 0xff = 144`.

**3 — workspace.** A three-package transitive graph, `app → geometry → numeric`
(and `app → numeric` directly), wired by path dependencies. `numeric` is integer
primitives; `geometry` is integer-lattice distance/area on top of it; `app`
computes a polyline's Manhattan length. Exercises cross-package `import`,
transitive resolution, the lockfile, and whole-graph spec runs (14 specs across
three packages). Built and executed via:

```console
$ tuo test  --manifest examples/workspace/app          # 14 passed (app+geometry+numeric)
$ tuo build --manifest examples/workspace/app -o /tmp/app && /tmp/app ; echo $?   # 26
```

**4 — http-service.** The routing/status core of a web service: a `(method, path)
→ status` decision with correct precedence (404 before 405, 201 on create). This
is the real decision logic of any server, and it runs. At the time of the
exercise, sockets, request-line parsing, and response writing were `CONTRACT:`
comments only — see finding D-3. Since ADR-0006 landed, request-line parsing
(pure `std::str` byte scans, spec-checked) and response writing (a thin
`std::rt::write` shell) are runnable code with a byte-asserted stdout; only
`serve` (socket setup) remains contract-tier, pending a socket effect.

**5 — concurrent-worker.** The scheduling model of a worker pool: round-robin
partition of eight uneven tasks across three workers, per-worker load, the
**makespan** (finish time of the slowest worker = 15), **speedup** (floored 2×),
and **imbalance** (3 units). These are a pool's actual observable performance
figures, computed exactly. Threads/channels/task values are `CONTRACT:` only —
see finding D-4.

---

## What we measured

Six axes, each with evidence produced by the real compiler.

### 1. Compiler usability

**Good.** The scalar core is small but genuinely composable: every project above
is real application logic assembled from small `fn`s, and the front end accepted
them once the syntax rules were learned. The friction that *did* appear was
uniform and instructive — the language forced honest workarounds rather than
letting anything sloppy through:

- **No product type in the runnable core** meant a "point" is two `Int`
  parameters and a "dataset" is a set of positional accessor functions
  (`d0()..d6()`, `p0x()..p4y()`). Every example paid this tax. → **D-1 / ADR-0004.**
- **No arrays or iteration in the runnable core** meant every fold is either
  hand-unrolled (`fold_one(a6, r7(), …)`) or written as explicit
  accumulator-passing recursion (`load_from`, `serial_from`). Correct, but
  verbose, and it does not scale past a fixed, known batch size. → **D-2 / ADR-0004.**
- **Mandatory parameter modes** (`in`/`mut`/`take`) and **mandatory braces** were
  easy to internalize and never ambiguous.
- **`[]` generics, capitalized primitives (`Int`, `Bool`), free-function calls**
  (`f(x)`, never `x.f()`): consistent, learnable, no surprises once known.

Net: the core is *usable* for scalar/control-flow programs today, and the two
recurring usability costs both trace to missing aggregate/iteration constructs,
not to the design of what exists.

### 2. Diagnostic quality

**High where it fires, with two concrete gaps.** Real diagnostics from feeding
the compiler deliberate errors:

- **Undefined name (`R0002`)** — precise and exactly what an LLM-repair loop
  needs: names the missing symbol and points at it.
  ```
  error[R0002]: cannot find `maximum` in this scope
   --> undef.tuo:2:5
    |
  2 |     maximum(3, 7)
    |     ^^^^^^^ not found
  ```
- **Use-after-move (`O0001`)** — excellent: three linked spans (use site, move
  site, declaration).
  ```
  error[O0001]: use of moved value `b`
     --> …:11:10  `b` used here after it was moved
     --> …:10:13  value moved here
     --> …:9:24   `b` declared here
  ```

Gaps found by dogfooding:

- **D-5a — mismatched-type span points at the return annotation, not the body.**
  Returning `n < 10` from an `-> Int` function reports:
  ```
  error[T0001]: mismatched types
   --> type_err.tuo:1:26
    |
  1 | fn f(take n: Int) -> Int {
    |                          ^... expected `I64`, found `Bool`
  ```
  The caret lands on `-> Int`, not on the offending `n < 10`. Correct diagnosis,
  imprecise location. (Backlog finding, no ADR — a diagnostics-quality bug, not a
  language change.)
- **D-5b — a missing parameter mode degrades to coarse recovery (`P0002`,
  "skipped 14 tokens").** `fn f(n: Int)` yields a whole-item skip rather than a
  targeted "parameter needs a mode" hint. (Backlog finding, no ADR.)
- **D-5c — ownership diagnostics are unreachable from the runnable core.** Every
  scalar-core type is `Copy`, so no `tuo run`-able program can exercise `O0001`;
  it fires only on heap types (`Box`/`Shared`), which are interpreter-tier. This
  is not a bug — it is a coverage gap that closes automatically once heap types
  lower (D-1/D-2).

### 3. Incremental compilation

**Strong, and measured — not asserted.** The incremental engine re-executes only
the queries an edit can affect. Measured on the standard scenarios
(`cargo test -p tuo-compiler --test incremental_measure -- --nocapture`),
counting **which per-item queries re-run**, not wall-clock:

| Scenario | Queries re-executed | Per-item stage queries |
|----------|:-------------------:|:----------------------:|
| no-change re-check | **0** | 0 |
| function-body-only edit | 7 | 1 (only that fn's MIR) |
| function-signature edit | 13 | 6 (callers re-typed) |
| unrelated-file edit | 6 | 1 |
| spec-only edit | 6 | 1 |

The body-vs-signature distinction is the load-bearing result: editing a function
*body* re-lowers only that function's MIR and never re-checks its callers (the
module-interface early-cutoff seam), whereas editing its *signature* correctly
re-types the callers. `tuo verify --affected-by <file>` uses the same graph to
run only the specs an edit could have changed, and it runs green on the examples
here. This axis is in good shape; dogfooding surfaced no new gap.

### 4. LLM generation success

**The compiler is a strong grounding signal; the language's small surface both
helps and hurts.** Two observations:

- **Helps:** invented API names are caught instantly and precisely (`R0002`
  above). The existing hallucination benchmarks
  ([`tuo-cli/tests/stdlib_hallucination.rs`](crates/tuo-cli/tests/stdlib_hallucination.rs),
  [`tuo-agent/tests/generation_benchmark.rs`](crates/tuo-agent/tests/generation_benchmark.rs))
  already show a grounded policy moving a plausible-but-wrong guess from 0% to
  100% Compile@1 by keeping only calls the compiler's real symbols vouch for. The
  dogfooding corpus reinforces this: `maximum` (vs `max`), `unwrap`, `sum_range`
  are exactly the names a model reaches for and the compiler rejects by name.
- **Hurts:** the *idioms* a model most wants (a list + a loop, a struct, a
  `println`) are precisely the ones v0 lacks. A model asked to "sum a list of
  numbers" will write `for x in xs { … }` — which parses (the grammar is a
  superset) but does not lower. So generation success on *scalar* tasks is high,
  and on *aggregate* tasks is bounded by the same D-1/D-2/D-3 gaps everything else
  hits. This is a language-surface finding, not a model finding.

### 5. Standard-library gaps

Dogfooding re-derived several stdlib functions by hand because the shipped
`std::core` is `Int`-specialized and free-function-only:

- Every example re-implemented `min`/`max`/`abs`/`isqrt` locally. `std::core` has
  `min`/`max`/`abs` but **no `isqrt`, no `pow`, no `clamp`-of-three used here**;
  more fundamentally there was **no generic `map`/`fold`/`filter`**, because v0
  had **no first-class functions/closures** (→ **D-8 / ADR-0008**) and no element
  type to be generic over (D-1/D-2). ADR-0008 Tier 1 has since landed: a bare
  top-level `fn` name is now a first-class **function value** (`fn(mode T, …) ->
  R`, a `Copy` code pointer, called indirectly with the identical direct-call
  ABI on both backends), so `std::collections` ships the **generic** combinators
  `fold`/`map_into`/`filter_into`/`any`/`all` over a function value — one `fold`,
  not N specialized folds — and `data-pipeline`'s once-bespoke fold now calls the
  generic `fold(items, 0, add)` with `add` passed by value, identical spec
  verdicts and exit byte. Only **capturing closures** (a heap-allocated captured
  environment) and non-`Int` element polymorphism remain deferred to a future
  ADR (Tier 2).
- The effectful entry points a CLI/HTTP/worker actually needs — `println`,
  `read_line`, `now`, `exit`, `lock` — were all **contract-tier** (documented
  signatures, no runnable body) because there was no effect boundary (→ **D-3 /
  ADR-0006**), the single biggest blocker to any of these five programs
  becoming a *deployable* application. ADR-0006 has since landed: `println`,
  `print`, and `exit` are now real, natively-running implementations over the
  `std::rt` effect primitives (the stdlib's `EFFECT:` tier), while
  `read_line`/`now`/`lock` remain contract-tier, each naming the primitive it
  still awaits (owned `String`; a clock; threads).

### 6. Runtime performance

The [performance laboratory](benchmarks/) (Prompt 38) is the system of record
here. At the time of the exercise it measured exactly the four workloads the
scalar core could express — startup, integer-computation, function-calls,
recursion — against an equivalent-semantics C peer, recording
hardware/OS/toolchain/commands and **never** publishing an unsupported
"blazing fast" claim. The dogfooding examples are consistent with it: each
compiles, links, and runs to a fixed exit byte via the real Cranelift+`cc` path
([`c_comparison_agrees_where_the_toolchain_exists`](crates/tuo-cli/tests/lab_command.rs)).
The four workloads the lab then recorded as **unsupported** (allocation,
collections, string-processing, networking) were the *same* four gaps this
dogfooding exercise hit from the application side — independent confirmation
that the honest boundary was drawn in the right place. Three have since flipped
exactly as their ADRs required: `collections` when ADR-0004 landed fixed
arrays, `string-processing` when ADR-0006 landed the borrowed `Str` core, and
`allocation` when ADR-0009 landed the allocator core (owned `String` + growable
`Array[Int]`, measured against a `malloc`/`realloc`/`free` C peer with the same
doubling growth). ADR-0008 Tier 1 then *added* a ninth workload,
`indirect-calls` — a hot loop through a first-class function value, measured
against a C peer calling through a function pointer — so the indirect-call
overhead is a recorded number, not a guess. Only `networking` (no socket effect)
still publishes a reason, not a number: eight of the nine workloads now measure.
No new runtime figure is invented here; the lab remains the one measurement of
record.

---

## Findings → ADRs

Every load-bearing gap became an [ADR](specification/adr/) with a
benchmark-consideration section, per the prompt. None was patched ad hoc.

| ID | Finding (discovered by) | Disposition |
|----|-------------------------|-------------|
| **D-1** | No product type (struct/tuple) in the runnable core — *geometry points, every dataset* | [ADR-0004](specification/adr/ADR-0004-aggregates-in-the-runnable-core.md) **(accepted, landed)** — geometry now passes a real `Point` |
| **D-2** | No arrays/collections + no iteration in the runnable core — *every fold* | [ADR-0004](specification/adr/ADR-0004-aggregates-in-the-runnable-core.md) **(accepted, landed)** — the folds now run over `[T; N]` arrays |
| **D-3** | No String value + no effect boundary/I/O — *http-service shell, every CLI* | [ADR-0006](specification/adr/ADR-0006-effect-boundary-and-strings.md) **(accepted, landed)** — cli-stats now `println`s its report and http-service parses/prints its request/response line, stdout byte-asserted by the dogfood tests. The *owned, growable* `String`/`Array[Int]` half (D-3's allocator continuation) is [ADR-0009](specification/adr/ADR-0009-allocator-core.md) **(accepted, landed)** — `std::io::read_line` now builds a real `String` from stdin, and `std::collections` gained real `Array[Int]` algorithms |
| **D-2b** | No *growable* collection whose size is not compile-time-fixed — *data-pipeline filter+collect* | [ADR-0009](specification/adr/ADR-0009-allocator-core.md) **(accepted, landed)** — data-pipeline now `push`es its filtered subset onto a heap-backed `Array[Int]` and folds it, spec-pinned equal to the streaming fold |
| **D-4** | No concurrency model — *concurrent-worker execution* | [ADR-0007](specification/adr/ADR-0007-concurrency-model.md) *(proposed)* |
| **D-8** | No first-class functions/closures — *generic map/fold in stdlib & pipeline* | [ADR-0008](specification/adr/ADR-0008-first-class-functions.md) **(Tier 1 accepted, landed)** — a bare `fn` name is now a `Copy` function value called indirectly (native, three-way pinned); `std::collections` ships generic `fold`/`map_into`/`filter_into`/`any`/`all` over a function value, and data-pipeline's fold calls the generic `fold` with `add` by value, same verdicts/exit byte. Tier 2 capturing closures deferred to a future ADR |
| **D-5a** | `T0001` span points at the return annotation, not the offending body expression | backlog (diagnostics bug, no ADR) |
| **D-5b** | Missing parameter mode degrades to coarse `P0002` whole-item recovery | backlog (diagnostics bug, no ADR) |
| **D-5c** | Ownership diagnostics unreachable from the runnable core (all scalar types `Copy`) | closes with D-1/D-2 |
| **D-6** | Resolved `tdg.lock` embeds machine-absolute dependency paths → not portable; gitignored under `examples/` | backlog (package tooling, no ADR) |
| **D-7** | No package-aware `tuo run`; a multi-package binary must be `tuo build --manifest` then executed | backlog (CLI ergonomics, no ADR) |

The four ADRs (0004, 0006, 0007, 0008) are the durable output of this exercise:
they turn "the language can't do X" into a reviewed, benchmarkable decision
about *how* it should, so the next capability lands by design rather than by the
convenience of one example.

**Post-exercise update (2026-08-10).** ADR-0004 completed exactly that loop:
both stages landed (structs/enums natively lowered; the inline `[T; N]` array
with literals, checked indexing, and bounded `for`), the perf-lab `collections`
workload it named moved to `Supported` with its committed program and C peer,
and the examples this report describes were rewritten onto the new capabilities
with **identical spec verdicts and exit bytes** — the acceptance oracle this
exercise defined. The prose above intentionally still describes the *v0-at-the-
time* workarounds (accessor functions, hand-unrolled folds): that is what the
exercise found, and the findings table is the record of what became of it.
D-5c closes with it: aggregates are now the first non-`Copy` runnable values,
so ownership diagnostics are reachable from runnable programs.

**Post-exercise update (2026-08-13).** ADR-0009 (the allocator core) completed
the same loop for D-3's owned-heap continuation and for D-2b: owned `String` and
growable `Array[Int]` now allocate and free real heap memory natively on both
backends, so `std::io::read_line` builds a real line from stdin, `std::collections`
ships real `Array[Int]` algorithms with executable specs, and the
`examples/data-pipeline` oracle answers its query through a growable
filter+collect whose subset size is data-dependent — the thing a fixed `[Int; N]`
could not express — spec-pinned equal to the streaming fold with the identical
exit byte (144). The perf-lab `allocation` workload it named moved to `Supported`
with its committed program and `malloc`/`realloc`/`free` C peer, taking the
measured runtime workloads to seven of eight. Deferred, unchanged: surface
`Box`/`Shared`/`Weak` values, non-`Int` `Array[T]` operations, and `String`→`Str`
borrowing.
