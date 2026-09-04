# Architecture Decision Records (ADRs)

An **Architecture Decision Record** captures a single significant decision — its
context, the choice made, and the consequences — so that the *reasoning* behind
tuonelang's design and implementation is durable and reviewable.

## When to write an ADR

Write an ADR when a decision is hard to reverse or shapes later work, for
example:

- a language design choice (syntax, type system, ownership model, diagnostics
  contract);
- a compiler-architecture choice (IR design, crate boundaries, backend
  strategy);
- a tooling or process choice with long-lived consequences.

Routine, easily reversible changes do not need an ADR.

## Naming convention

ADRs are numbered sequentially and named:

```
ADR-NNNN-short-kebab-case-title.md
```

For example: `ADR-0001-use-mir-as-single-semantic-ir.md`. Numbers are never
reused, even if an ADR is later superseded.

## Format

Each ADR should contain at least:

```markdown
# ADR-NNNN: <title>

- **Status:** proposed | accepted | superseded (by ADR-XXXX) | deprecated
- **Date:** YYYY-MM-DD
- **Context:** What situation forces a decision? What constraints apply?
- **Decision:** What was decided, stated plainly.
- **Consequences:** What becomes easier or harder as a result, including
  trade-offs and follow-up work.
```

Superseding an ADR does not delete it: mark the old one `superseded` and link
the replacement, preserving the decision history.

## Current ADRs

| ADR | Title | Status |
|-----|-------|--------|
| [ADR-0001](ADR-parser-strategy.md) | Parser implementation strategy | accepted |
| [ADR-0002](ADR-0002-spec-semantics.md) | First-class `spec` semantics | accepted |
| [ADR-0003](ADR-0003-ownership-model.md) | The v0 ownership model | accepted |
| [ADR-0004](ADR-0004-aggregates-in-the-runnable-core.md) | Aggregates and iteration in the runnable core | accepted |
| [ADR-0006](ADR-0006-effect-boundary-and-strings.md) | The effect boundary and runtime strings | accepted |
| [ADR-0007](ADR-0007-concurrency-model.md) | The concurrency model | accepted |
| [ADR-0008](ADR-0008-first-class-functions.md) | First-class functions | accepted (Tier 1; Tier 2 closures deferred) |
| [ADR-0009](ADR-0009-allocator-core.md) | The allocator core — owned `String` and growable `Array` | accepted |
| [ADR-0010](ADR-0010-string-to-str-view.md) | The `String` → `Str` borrowing view (Q-0012) | accepted |
| [ADR-0011](ADR-0011-hash-map.md) | The hash map — a keyed associative container | accepted |
| [ADR-0012](ADR-0012-generic-array-elements.md) | Generic `Array[T]` element types — widening the monomorphic builtin surface | accepted |
| [ADR-0013](ADR-0013-os-effect-boundary.md) | The OS effect boundary — clock, argv, and files | accepted |
| [ADR-0014](ADR-0014-socket-effects.md) | Socket effects — the network joins the descriptor seam | accepted |
| [ADR-0015](ADR-0015-channels-and-mutexes.md) | Channels and mutexes — communication joins the effect seam | accepted |
| [ADR-0016](ADR-0016-json-and-the-data-increment.md) | `std::json` and the data increment — Float elements, indexed writes, and the recursion boundary | accepted |
| [ADR-0017](ADR-0017-timeouts-ipv6-and-udp.md) | Timeouts, IPv6, and UDP — the socket seam's additive increment | accepted |
| [ADR-0018](ADR-0018-context-injectable-cheat-sheet.md) | The context-injectable cheat sheet — a generated, compiler-backed language brief | accepted |
| [ADR-0019](ADR-0019-bitwise-operations-and-crypto.md) | Bitwise operations and the crypto primitives | accepted |
| [ADR-0020](ADR-0020-constant-time-code.md) | Constant-time code — the branchless subset and what tuonelang can honestly promise | accepted |

(`ADR-parser-strategy.md` carries number 0001 without it in the filename;
new ADRs should follow the `ADR-NNNN-…` naming above. ADR-0005 is intentionally
unallocated; ADRs 0004/0006/0007/0008 were opened together by the Prompt 39
dogfooding exercise — see [`DOGFOODING.md`](../../DOGFOODING.md). ADR-0004,
ADR-0006, ADR-0009, and ADR-0008 (Tier 1) have since been accepted and landed,
each having added or flipped a performance-lab workload (`collections`,
`string-processing`, `allocation`, and the new `indirect-calls`, respectively).
ADR-0008 landed **Tier 1** — non-capturing first-class function values and the
generic higher-order stdlib combinators — with **Tier 2 capturing closures
explicitly deferred** to a future ADR increment. ADR-0009 is the allocator ADR
that ADR-0006's first amendment promised — it landed the owned `String` and
growable `Array[Int]`. Of the ten runtime workloads, nine now measure —
`map-lookup` joined with ADR-0011 — leaving only `networking`; the
parallel-speedup category (ADR-0007) measures alongside them.

**ADR-0010** and **ADR-0012** have since been accepted together (2026-08-21):
ADR-0010 (the `String`→`Str` borrowing view that resolves the long-deferred
Q-0012) landed all three stages — the borrow rule, the three-way-pinned native
zero-copy lowering, and the Stage C stdlib payoff (the
`to_upper`/`to_lower`/`to_string` specs compare `as_str(…) == "<literal>"`, and
`data-pipeline` composes a `String` producer into `Str` consumers natively) —
and ADR-0012 (generic `Array[T]` element types — widening the ADR-0009 array
builtins from `Int` to a defined element set) landed its full staging: the
checker widening, native lowering including the owned-element increment
(deep-copy `get` + recursive drop glue on both backends, so
`std::str::split`/`join` run natively), the ADR-0008 combinators'
`String`/struct instantiations, and the dogfood oracle (`data-pipeline` holding
parsed records in an `Array[Record]`, spec-pinned equal to its packed-`Int`
predecessor). Neither needed a trait system: both ride the same
monomorphic-builtin machinery ADR-0009 used for `Array[Int]` — ADR-0012 is that
machinery's element-surface widening, explicitly **not** user-written generics
(`fn f[T]`, generic structs), which stay deferred under Q-0010.

**ADR-0011** and **ADR-0007** have since been accepted as well (2026-08-22):
ADR-0011 (the hash map) landed all three stages — `Ty::Map` with the
monomorphic `Map[Int, Int]`/`Map[Str, Int]` surface, the insertion-ordered
reference semantics, the native `tuo_rt_map_*` table on both backends (ABI
v6, vector-pinned FNV-1a/splitmix64), `std::collections::counts`, the
data-pipeline keyed-aggregation oracle, and the gating `map-lookup` lab
workload with C and Go peers — and ADR-0007 (the concurrency model) resolved
its deferred decision to **structured fork-join over one primitive**
(`std::rt::par_map`: non-capturing function values over `Copy` tasks,
round-robin, join-before-return — no data race expressible by construction),
landed it as a typed effect through both backends' pthreads runtime,
turned `concurrent-worker` into a live pool whose exit survives only if the
run agrees with the spec-checked scheduling model, and added the gating
**parallel-speedup** benchmark category (serial vs `par_map` wall clock with
a same-thread-count C peer).

**ADR-0013** has since been accepted as well (2026-08-23): the OS effect
boundary — six further `std::rt` primitives (`now_nanos`, `arg_count`,
`arg_byte`, `open`, `close`, `remove_file`, ABI v7) that made
`std::time::now`, `std::process::arg_count`/`arg`, and the whole `std::fs`
disk tier real EFFECT-tier code, and landed the gating **`file-io`** lab
workload (with C and Go peers) — the effect-crossing benchmark ADR-0006's
acceptance had deferred to exactly this ADR.

**ADR-0014, ADR-0015, and ADR-0016** were accepted together (2026-08-24) as
the Go-parity closing sweep. ADR-0014 (socket effects, ABI v8) added
`listen`/`bound_port`/`accept`/`connect` on the descriptor seam, the
`std::net` module, http-service's live loopback `serve_once` (replacing its
last CONTRACT), and flipped the lab's `networking` entry — the catalog's
final `Unsupported` — to measured. ADR-0015 (channels and mutexes, ABI v9)
added `chan_new`/`send`/`recv`/`close` and `mutex_new`/`lock`/`unlock` as
runtime-owned handles preserving ADR-0007's no-data-race property, **emptied
the stdlib's contract tier** (`std::sync` channels and handle-based
`lock`/`unlock` are real, pinned emptiness), gave `concurrent-worker` its
dynamically-drained shared work queue, and landed the gating `channels`
workload (whose Go peer is Go's native `chan`). ADR-0016 (the data
increment) closed the recursive-declaration compiler-hang hole with the
`T0016` recursion boundary, admitted `Float` array elements, added
`std::array::set` (the indexed write), and shipped `std::json` — decode,
navigate, render over an index arena, entirely spec-checked and natively
pinned — with the gating `json-parse` workload (whose Go peer is
`encoding/json`). That sweep took the catalog to thirteen measured
workloads. **ADR-0017** (2026-08-25) is the first ADR opened since that sweep, and takes
up three of the four items ADR-0014 listed as out of scope and additive
*"when its need is demonstrated by dogfooding"*: **timeouts** (the strongest
case — blocking `accept`/`connect`/`read_byte` are an unbounded wait in code
that already ships, so `examples/http-service` and the `networking` lab
workload can wedge with no recourse), **IPv6** (a strict widening of `connect`
plus an explicit `listen6`, since a server cannot infer a family from a port),
and **UDP** (a transport the seam cannot reach at all). **TLS and DNS stay
out** — TLS would need a crypto dependency this workspace deliberately avoids,
and DNS is properly written *in tuonelang* on the UDP primitives this ADR adds.
It landed in three stages (timeouts → IPv6 → UDP, ABI v10) and is since
**accepted**: both gating performance-lab workloads are committed and
measuring — `udp-echo` (against `sendto`/`recvfrom` C and Go's
`net.ListenPacket`) and `connect-timeout` (against non-blocking
`connect`+`poll` C and Go's `net.DialTimeout`), the latter a workload that
**could not be written before this ADR**, since a blocking `connect` has no
bounded outcome to measure. Of the runtime workloads, **all fifteen now
measure**. Two design errors were caught by the compiler and the tests rather
than by review, and both are recorded in the ADR: the timeout sentinel had to
become `-3` (`-1` and `-2` were already spent, and `read_byte_timeout` is the
call where all four outcomes are simultaneously possible), and the staged
datagram needed its own `udp_byte_at` indexer rather than reusing
`read_byte`, which calls `read(2)` directly and cannot see a runtime buffer.
**No ADR remains `proposed`.**
