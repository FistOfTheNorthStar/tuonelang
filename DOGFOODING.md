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
documented **contract tier**, mirroring exactly how `tuo-stdlib` separates its
executable tier from its `CONTRACT:` tier. Nothing pretends to do what the
language cannot. (The ADRs this exercise opened have since moved the boundary:
ADR-0004 landed aggregates, ADR-0006 landed `Str` and the effect boundary,
ADR-0007 landed structured fork-join, ADR-0014 landed sockets, and ADR-0015
landed channels — the table below reflects the examples as they stand today,
with all five running natively and **no `CONTRACT:` tier left in any
example**.)

| # | Project | Directory | v0 status | `main` exit | Specs |
|---|---------|-----------|-----------|:-----------:|:-----:|
| 1 | Command-line application | [`examples/cli-stats/`](examples/cli-stats/) | **runs natively**, prints its report | 18 | 12 |
| 2 | JSON/data-processing | [`examples/data-pipeline/`](examples/data-pipeline/) | **runs natively** | 144 | 6 |
| 3 | Medium multi-package project | [`examples/workspace/`](examples/workspace/) | **builds natively** (3-package graph) | 26 | 14 |
| 4 | HTTP service | [`examples/http-service/`](examples/http-service/) | **runs natively**: serves a live loopback request | 200 | 12 |
| 5 | Concurrent worker | [`examples/concurrent-worker/`](examples/concurrent-worker/) | **runs natively**: live pool + channel-drained queue | 15 | 8 |

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
`std::rt::write` shell) are runnable code with a byte-asserted stdout. And
since ADR-0014 landed the socket effects, `CONTRACT serve` is gone too: the
real `serve_once`/`live_status` run the service against itself over a **live
loopback socket** — `main` listens on an ephemeral port, connects, sends
`GET /health HTTP/1.1`, serves it, and reads the status back, exiting 200
only when the wire agrees with the pure parser (stdout still exactly
`HTTP/1.1 200 OK\n`). No contract tier remains.

**5 — concurrent-worker.** The scheduling model of a worker pool: round-robin
partition of eight uneven tasks across three workers, per-worker load, the
**makespan** (finish time of the slowest worker = 15), **speedup** (floored 2×),
and **imbalance** (3 units). These are a pool's actual observable performance
figures, computed exactly. At the time of the exercise threads/channels/task
values were `CONTRACT:` only — see finding D-4. Since ADR-0007 landed
structured fork-join, the pool **runs live**: `main` computes the makespan
through the model *and* through a real `std::rt::par_map`. And since ADR-0015
landed channels, the dynamically-drained shared queue its contract named runs
too: `dynamic_total` fills a channel with the task ids, closes it, and lets
`par_map` workers **race to drain it** — exit 15 now requires the static pool
to match the model's makespan *and* the drained total to equal the serial
cost. Detached spawn is a documented non-goal (per ADR-0007's structured
model); no `CONTRACT:` block remains.

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
  verdicts and exit byte. Non-`Int` element polymorphism has since landed too
  (ADR-0012 made `Array[T]` element-generic, ADR-0016 added `Float`); only
  **capturing closures** (a heap-allocated captured environment) remain
  deferred to a future ADR (Tier 2).
- The effectful entry points a CLI/HTTP/worker actually needs — `println`,
  `read_line`, `now`, `exit`, `lock` — were all **contract-tier** (documented
  signatures, no runnable body) because there was no effect boundary (→ **D-3 /
  ADR-0006**), the single biggest blocker to any of these five programs
  becoming a *deployable* application. ADR-0006 has since landed: `println`,
  `print`, and `exit` are now real, natively-running implementations over the
  `std::rt` effect primitives (the stdlib's `EFFECT:` tier). `read_line`
  followed when ADR-0009 landed the owned `String`, structured spawning
  (`par_map`) when ADR-0007 resolved the concurrency model, and `now` — plus
  argv (`arg_count`/`arg`) and the whole `std::fs` disk tier — when ADR-0013
  landed the OS effect boundary (clock, argv, file open/close/remove). The
  last holdout, `lock`, was discharged when ADR-0015 landed channels and
  mutexes: `std::sync::lock`/`unlock` are now real handle-based effect
  wrappers over error-checked mutexes (alongside `channel`/`send`/`recv`/
  `close`), and ADR-0014's `std::net` and ADR-0016's `std::json` filled the
  networking and data-format gaps this exercise's HTTP/JSON projects implied.
  **The stdlib's contract tier is now empty** — pinned by
  `the_contract_tier_is_empty` in
  [`tuo-cli/tests/stdlib.rs`](crates/tuo-cli/tests/stdlib.rs), so nothing can
  re-enter it silently.

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
that the honest boundary was drawn in the right place. All four have since
flipped exactly as their ADRs required: `collections` when ADR-0004 landed
fixed arrays, `string-processing` when ADR-0006 landed the borrowed `Str` core,
`allocation` when ADR-0009 landed the allocator core (owned `String` + growable
`Array[Int]`, measured against a `malloc`/`realloc`/`free` C peer with the same
doubling growth), and — last — `networking` when ADR-0014 landed the socket
effects, exactly as the entry's recorded reason promised. ADR-0008 Tier 1 then
*added* a workload of its own, `indirect-calls` — a hot loop through a
first-class function value, measured against a C peer calling through a
function pointer — so the indirect-call overhead is a recorded number, not a
guess, and the later ADRs kept the pattern: `map-lookup` (ADR-0011), `file-io`
(ADR-0013), `channels` (ADR-0015, whose Go peer is Go's native buffered
`chan`), and `json-parse` (ADR-0016, whose Go peer is `encoding/json`), then
`udp-echo` and `connect-timeout` (ADR-0017), then `sha256-hash` and
`wire-decode` (ADR-0019, whose Go peers are `crypto/sha256` and
`encoding/binary`), and `constant-time` (ADR-0020, whose Go peer is
`crypto/subtle.ConstantTimeCompare`). All **eighteen** workloads now
measure, each against equivalent-semantics C *and* Go peers; none is
unsupported.

`constant-time` is the odd one out and deliberately so: it measures a cost
*deliberately paid* rather than a throughput to improve. A 32-byte tag
comparison done branchlessly against the same comparison done with an early
return — the textbook timing vulnerability — over rounds alternating between
the naive form's best case and its worst, so the gap is not read off one
extreme. On this host tuonelang lands at parity with Go and roughly 2.2× the
hand-written C peer, and the lab records that without an aggregate verdict. No new runtime figure is invented here; the lab
remains the one measurement of record.

---

## Findings → ADRs

Every load-bearing gap became an [ADR](specification/adr/) with a
benchmark-consideration section, per the prompt. None was patched ad hoc.

| ID | Finding (discovered by) | Disposition |
|----|-------------------------|-------------|
| **D-1** | No product type (struct/tuple) in the runnable core — *geometry points, every dataset* | [ADR-0004](specification/adr/ADR-0004-aggregates-in-the-runnable-core.md) **(accepted, landed)** — geometry now passes a real `Point` |
| **D-2** | No arrays/collections + no iteration in the runnable core — *every fold* | [ADR-0004](specification/adr/ADR-0004-aggregates-in-the-runnable-core.md) **(accepted, landed)** — the folds now run over `[T; N]` arrays |
| **D-3** | No String value + no effect boundary/I/O — *http-service shell, every CLI* | [ADR-0006](specification/adr/ADR-0006-effect-boundary-and-strings.md) **(accepted, landed)** — cli-stats now `println`s its report and http-service parses/prints its request/response line, stdout byte-asserted by the dogfood tests. The *owned, growable* `String`/`Array[Int]` half (D-3's allocator continuation) is [ADR-0009](specification/adr/ADR-0009-allocator-core.md) **(accepted, landed)** — `std::io::read_line` now builds a real `String` from stdin, and `std::collections` gained real `Array[Int]` algorithms. The socket half of the shell followed via [ADR-0014](specification/adr/ADR-0014-socket-effects.md) **(accepted, landed)** — http-service now serves itself over a live loopback socket, exit 200 only when the wire agrees with the pure parser |
| **D-2b** | No *growable* collection whose size is not compile-time-fixed — *data-pipeline filter+collect* | [ADR-0009](specification/adr/ADR-0009-allocator-core.md) **(accepted, landed)** — data-pipeline now `push`es its filtered subset onto a heap-backed `Array[Int]` and folds it, spec-pinned equal to the streaming fold |
| **D-4** | No concurrency model — *concurrent-worker execution* | [ADR-0007](specification/adr/ADR-0007-concurrency-model.md) **(accepted, landed)** — the model resolved to structured fork-join over one primitive (`std::rt::par_map`, a typed effect); concurrent-worker now runs its pool live, exiting with the model's makespan only when the real parallel run agrees — the scheduling model as the runtime oracle, exactly as this table predicted. The dynamically-drained shared queue its contract named followed via [ADR-0015](specification/adr/ADR-0015-channels-and-mutexes.md) **(accepted, landed)** — `dynamic_total` drains a real channel with racing workers, exit 15 only when the drained total equals the serial cost |
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

**Post-exercise update (2026-08-24).** ADR-0014, ADR-0015, and ADR-0016 closed
the loop on everything this exercise had to leave as a contract. Sockets
joined the descriptor seam (ADR-0014), so http-service replaced `CONTRACT
serve` with the real `serve_once`/`live_status`: `main` now serves
`GET /health HTTP/1.1` to itself over a live loopback socket and exits 200
only when the wire agrees with the pure parser, stdout still byte-asserted.
Channels and mutexes joined the effect seam (ADR-0015), so concurrent-worker
gained `dynamic_total` — the dynamically-drained shared work queue its
contract named, drained by racing `par_map` workers — and exit 15 now also
requires the drained total to equal the model's serial cost; detached spawn is
a documented non-goal, and the stdlib's last `CONTRACT:` stubs
(`lock`/`unlock`) became real handle-based wrappers over error-checked
mutexes, leaving **the contract tier empty** (pinned by
`the_contract_tier_is_empty`). ADR-0016 landed the data increment — `Float`
array elements, the in-place `std::array::set`, and the `T0016` recursion
boundary (a self-reaching struct/enum is now a declaration-time error instead
of a compiler hang) — plus `std::json`, the entirely-executable twelfth
stdlib module (index-arena `parse`/`render` with positioned errors). The
perf-lab entries the three ADRs gated all measure: `networking` flipped from
the lab's last `Unsupported`, and `channels` and `json-parse` landed
supported — thirteen workloads at that point, each with C and Go peers
(ADR-0017 has since taken the catalog to fifteen, ADR-0019 to
seventeen, and ADR-0020 to eighteen). Still deferred,
honestly: recursive nominal types (`T0016` is the boundary until a successor
ADR gives the backends runtime-recursive clone/drop glue), DNS and TLS,
`\uXXXX` escapes, `Map[Str, V]` beyond `Int` values, building JSON from
scratch, detached spawn, and capturing closures (Tier 2).

**Post-exercise update (2026-08-25).** [ADR-0017](specification/adr/ADR-0017-timeouts-ipv6-and-udp.md)
took up three of the four items ADR-0014 had listed as out of scope *"when
its need is demonstrated by dogfooding"*, on the strength of a gap this
exercise's own committed code carries: `http-service` reads its request line
with an unbounded `read_byte`, and the `networking` workload blocks in
`accept`/`connect`, so any peer that goes away wedges the process with no
recourse — not a missing capability but an unbounded wait in shipped code.
Timeouts (`accept_timeout`/`connect_timeout`/`read_byte_timeout`, with a
distinct `-3` sentinel so a timeout is never confused with an error or EOF),
IPv6 (`connect` infers the family, so `"::1"` needs no new spelling; the
server side gains `listen6`/`peer_family`), and UDP (`udp_bind`/`send`/
`recv`/`byte_at`/`peer_port` — a datagram is a message, so a receive reports
its boundary) all landed across three stages at ABI v10. Two gating lab
workloads came with them, taking the catalog to **fifteen** measured:
`udp-echo` and `connect-timeout`, the latter measuring a *bounded failure* —
a program that could not have been written before the ADR. (ADR-0019 has
since added `sha256-hash` and `wire-decode`, and ADR-0020
`constant-time`, taking the catalog to **eighteen**.) **DNS stays out**, and so does TLS — though for a narrower
reason since ADR-0019 Stage B: SHA-256/HMAC are now written *in tuonelang*
and need no cryptographic dependency at all, but TLS additionally needs
X.509, a certificate store, and AEAD ciphers. (The original framing here —
that TLS would need a cryptographic dependency this workspace avoids on
purpose — was true when written and is recorded as superseded rather than
silently rewritten.) DNS is properly written *in tuonelang* on the UDP
primitives this ADR added, rather than bolted into the runtime shim.

### ADR-0019 — bitwise operations and the crypto primitives

The first dogfooding target the language could not express **at all**, rather
than merely express awkwardly: a PostgreSQL client. Two separable gaps blocked
it, and only the first was a language change.

The absence of bitwise operators was not an oversight but a documented v0
commitment, stated in three places — `grammar.ebnf`'s punctuation table (`|`
is "pattern alternative only; NOT a bitwise operator"), the lexer's
`TokenKind::Pipe` doc, and the grammar's note that `<`/`>` are "shift-free".
Reversing a deliberate commitment is exactly what an ADR is for.

The sharpest statement of the gap was internal: **this workspace already
contained a SHA-256**, hand-rolled in Rust (`tuo-package/src/sha256.rs`)
precisely so the workspace need not take a crypto dependency — and tuonelang
could not express its own package manager's checksum function, because
SHA-256 is *defined* in rotations, xors, and shifts. The framing question the
ADR had to get right was what the framing workaround actually cost: on
well-formed input `b0*16777216 + b1*65536 + b2*256 + b3` is genuinely
equivalent to the shift form (a spec proves it), so the honest argument is not
that the arithmetic form is *wrong* but that it **does not generalize** —
masking, field extraction, and rotation have no arithmetic spelling at all.

Stage A added six operators at `GRAMMAR-VERSION` 0.2. Three things were worth
the care they took. The `|` overload works because `pattern` and the
expression chain are disjoint productions, proven by parsing `1 | 2 => n | 8`
and asserting one `OrPattern` and one `BinaryExpr` in the same program. `>>`
is signedness-directed rather than a second operator, which is what makes the
existing `IntKind` signedness carry weight. And an out-of-range shift
**traps**: x86 masks the amount to 6 bits, so an unguarded `1 << 64` would be
`1` natively while the interpreter trapped — a silent three-way divergence,
now pinned by the differential suites.

Three real bugs surfaced by testing paths that had been reasoned about but
not exercised, each invisible to the tests that existed at the time. The C
trap shim built its message table from a hand-maintained array *and the test
meant to catch drift hardcoded the same list*, so a new trap printed
`unknown` natively while the test passed; both now iterate one
`TrapCode::ALL`. The interpreter's operand-kind `debug_assert` predated
shifts not unifying their operands, so `U32 >> Int` panicked — invisible in
release builds and invisible for `Int` operands. And an unsigned shift amount
with its top bit set bypassed the guard, because a signed comparison read it
as negative; that one was found by re-reading the guard rather than by a
failing test.

Stage B wrote `std::bits` and `std::crypto` *in tuonelang* on the new
operators. Their specs are unique in the catalog for asserting **published
vectors** (FIPS 180-4, RFC 4231, RFC 4648) rather than the module's own
reasoning — a spec that reproduces a published vector cannot be
self-consistently wrong. The headline claim is discharged by a real test: a
native tuonelang binary's SHA-256 agrees byte-for-byte with the toolchain's
Rust `sha256` across nine padding-boundary inputs. Two decisions were forced
by the language rather than chosen: `+` traps on overflow but every hash is
defined over modular arithmetic, so `add32` is the named way to ask for
wraparound (made once, in the library, rather than rediscovered per call
site); and drawing entropy cannot be pure, so `random_byte`/`nonce` are the
effect tier and `R0007` refuses them in a spec with no new mechanism.

One catalog invariant changed deliberately. `std::crypto` uses `std::bits`,
making it the first non-standalone module; rather than ship two copies of
`rotr32`/`add32`/`be32` free to drift, the stdlib test gained a
`DECLARED_DEPENDENCIES` table and the invariant weakened from "every module
stands alone" to "the dependency graph is declared and acyclic" — the honest
statement of what is now true.

The end-to-end proof is a **full SCRAM-SHA-256 client proof** computed
natively and checked against RFC 7677's published vector: Base64 both ways,
PBKDF2 over 4096 iterations, two HMACs, a SHA-256, and a byte-wise XOR. Since
PostgreSQL has defaulted `password_encryption` to `scram-sha-256` since
version 14, that is the exchange a connector actually needs.

That proof has since become **library surface and a running program**.
`std::crypto` carries the SCRAM exchange
(`scram_salted_password`/`scram_client_proof`/`scram_server_signature`) plus
the constant-time `verify`, and `examples/postgres-auth` computes the whole v3
handshake — big-endian framing, the startup packet, the SASL challenge parsed
off the wire format, the proof, and the server's signature verified in
constant time.

Two findings came out of writing it, both worth recording:

* **A library that offers no comparison is not neutral.** Before `verify`,
  `std::crypto` exposed no way to compare two digests at all, which sounds
  safe and is not: the caller writes the comparison instead, and the obvious
  spelling returns early on the first mismatched byte — the exact timing leak
  `std::ct::bytes_eq` exists to prevent. The fix was to make the safe
  comparison the *convenient* one, in the module the caller already imports.
* **A published vector caught a bug no self-consistent spec would have.** The
  example's client-first message hardcoded an empty username. Every structural
  property still held — the message parses, the proof is 32 bytes, the XOR
  round-trip recovers the client key — but the auth message both sides sign
  differed, so a real server would reject the proof with no useful diagnostic.
  This is the argument for external vectors in one concrete instance.

The connector has since been driven against a **real PostgreSQL 18 server**
(`examples/postgres-client`): startup packet, the live SASL exchange, the
server's signature verified in constant time, then `SELECT 42` decoded out of a
`DataRow` frame. Three findings came out of that, none of which the hermetic
version could have surfaced:

* **`String` is the byte container the wire needs, and that is not a
  workaround.** v0 has no `[u8]`, and a PostgreSQL frame is full of zero bytes
  and high bytes. `String` holds both and `std::rt::write_string` puts them on
  a socket unchanged, so the question "how do we send arbitrary bytes" had no
  answer to invent — it was already the type's behaviour. Worth recording
  because the obvious assumption is the opposite.
* **The default `trust` auth would have made the test pass without
  authenticating.** A cluster set up the usual way never issues a challenge, so
  the client "succeeds" having proved nothing. The test provisions a cluster
  with `--auth-host=scram-sha-256` for exactly this reason, and the README says
  so — a green test against a `trust` server is the security equivalent of an
  empty assertion.
* **An exit byte that names the failing step is worth the arithmetic.** The
  program returns `10 - step`, so a rejected proof reports 20 and a failed
  server-signature check reports 21. Both were verified load-bearing by
  substituting a wrong password and by tampering with the expected signature;
  a single boolean would have said only "it didn't work".
* **The extended query protocol is a safety feature, not a performance one.**
  The simple `Query` message carries SQL text, so a value can only reach it by
  interpolation — which is how SQL injection happens. `Parse`/`Bind` separates
  them: the statement carries `$1` placeholders and the value travels as its
  own length-prefixed field the server never re-parses. The client proves this
  against the live server by binding `'; DROP TABLE users; --` and requiring it
  back verbatim; the server's log never shows the text as SQL. The specs pin
  the structural half the runtime check cannot see — that the dangerous text
  appears in the `Bind` message and never in `Parse`'s SQL.

**`md5` has since landed too**, so the legacy `AuthenticationMD5Password`
challenge is supported for servers too old for SCRAM — shipped documented as
broken for security (ADR-0019's own requirement), spec'd against RFC 1321's
published suite, and with `md5_password` pinning the protocol composition
against an independent implementation. SCRAM stays the primary path: it is the
default on every current server, and MD5 auth is disabled outright on several
managed providers.
