# ADR-0034: Shared state across threads — what may cross `par_map`, atomic `Shared`, and one atomic cell

- **Status:** proposed (Stage A items 1–2 **landed** 2026-10-16; the rest open)
- **Date:** 2026-10-14

## Context

ADR-0007 resolved the concurrency model to structured fork-join over one
primitive, `std::rt::par_map`, and closed with a deferral this ADR picks up:
"Widening what may cross (shared state, `Shared` under contention, atomics) is
a future ADR's decision, and until one lands nothing else can cross at all."
ADR-0015 then added channels and mutexes without widening anything: both are
runtime-owned handles, and the only payload is a non-negative `Int`. ADR-0028
Stage C (`Shared`/`Weak`) is blocked on open question Q-0005, which asks
whether `Shared` is always atomic. So three decisions are waiting on one
question: what is allowed to be visible to two threads at once.

The probes below ran against the real `tuo` binary (a copy of
`target/debug/tuo` built from this branch) on an Apple M4 with 10 logical CPUs.

### Finding 1 — only `Int` crosses, so read-only data is rebuilt per thread

`par_map`'s signature admits a `fn(take Int) -> Int` and nothing else:

```
fn work(in t: Int) -> Int { t + 1 }
...
std::rt::par_map(work, xs, 2)
error[T0001]: mismatched types
   expected `fn(take I64) -> I64`, found `fn(in I64) -> I64`
```

A worker that needs a table can't receive it. It has to rebuild it, and the
dogfood code does exactly that. In `examples/router/src/main.tuo`, `serve_pool`
forks workers that race `accept` on one listener (the listener crosses because
a descriptor is an `Int`). Each request then reaches `route()`, whose first
line is `let table = routes();`, so the route map is constructed again for
every request on every thread. The table is immutable after it is built. The
language can't express "these four threads read this one map".

`par_map` already sends one non-`Int` value across threads: the `in tasks:
Array[Int]` buffer. Each worker reads it through a `const long long *`
(`crates/tuo-runtime/src/effect.rs`, `tuo_rt_par_ctx`). That is sound for a
reason ADR-0007 states: the caller is blocked inside the call and `in` rules
out mutation. The same reason covers any `in` borrow, not just a scalar
buffer.

### Finding 2 — a shared counter is written as a channel round-trip

There's no shared mutable memory, so a contended counter has to be a channel
that holds exactly one value. A worker receives the value, adds one, and sends
it back. This compiles, builds, and exits 0 (four workers, 10,000 increments
each, total checked):

```tuo
fn bump(take cell: Int) -> Int {
    var i = 0;
    while i < rounds() {
        let v = std::rt::chan_recv(cell);
        if std::rt::chan_send(cell, v + 1) != 0 {
            return 0 - 1;
        }
        i = i + 1;
    }
    0
}
```

It's correct, and each increment costs a mutex-and-condvar handoff. Its cost
hasn't been measured; the benchmark plan below measures it. The pattern also
has two limits that were probed:

- **The value can't go negative.** `chan_send(ch, 0 - 5)` returns `-1` by
  design (ADR-0015), so a cell holding a balance or a signed delta can't use
  this pattern.
- **Handles are capped.** Creating 300 channels yields 256 handles
  (`SYNC_REGISTRY_CAP`), and handles are never freed. A design with one
  channel per counter runs out at 256 counters for the life of the process.

### Finding 3 — worker threads have a much smaller stack than `main`

`tuo_rt_par_map` calls `pthread_create(&threads[w], 0, …)` with default
attributes. Worker 0 runs on the calling thread and the rest run on new
threads. The same non-tail recursion was run in both places, with both
backends behaving identically:

| Depth | On the calling thread | On a spawned worker |
|---|---|---|
| 20,000 | returns | returns |
| 40,000 | returns | killed, exit 138 (SIGBUS) |
| 400,000 | returns | — |
| 10,000,000 | killed, exit 139 (SIGSEGV) | — |

So the same program can work or crash depending on which worker gets a task,
and neither crash is a defined trap. Today this only affects deep recursion
written by the user. Once ADR-0028 lands it also affects drop glue. ADR-0028
already says dropping a long `Box` chain recurses once per node. If the last
`Shared` handle to a structure is dropped on a worker, the whole drop runs on
the small stack.

### What makes this tractable: nothing writes through `in`

tuonelang has no references and no lifetimes. A borrow lasts for the call that
takes it (ADR-0003). `Shared` grants read access only, and ownership.md §12
says interior mutation "requires an explicit synchronized cell type". No such
type exists yet. With no interior mutability anywhere, two threads reading
through `in` borrows can't race. A data race needs a write, and `in` admits
none.

Only one operation in the specified language would break this: **cloning a
`Shared` through an `in` borrow writes its reference count.** `abi.md` (§Shared)
specifies plain counts and says "Making the counts atomic for a future
threaded target is a version-bumping change." Constitution §25 calls `Shared`
"atomic reference counting" and rejects "a single `Rc` that is sometimes
atomic". Q-0005 is the gap between those two documents.

## Decision

### D1 — `Shared` is always atomically counted (resolves Q-0005)

There will be one `Shared[T]`, its counts will be atomic, and there will be no
non-atomic sibling. The reasoning comes from the section above. If the count
is atomic, every type is safe to read from several threads, because the only
write a read can trigger is atomic. Then the language needs no `Sync`
analogue. A non-atomic sibling would bring back the one type for which that
fails, and the checker would need a structural "may not cross" property
(derivable the same way `Copy` is, ADR-0003 ruling 1) to keep it out of
`par_map`. That's a second ownership dimension, and authors (human or model)
would face a choice between two types, all to save one cost.

That cost is smaller here than in languages with implicit retain/release.
Ruling 7 makes `Shared` clones explicit and moves never touch the count. So
the number of atomic operations equals the number of written `clone` and drop
sites, and those are visible in the source. The size of the cost is a
measurement, not an assumption (see `shared-refcount` below). If the lab shows
it matters, a successor ADR can bring back a sibling, using the figure as
evidence.

The count protocol is internal to the runtime and is the standard one:

- Increment uses a relaxed `fetch_add`.
- Decrement uses an acquire-release `fetch_sub`, and the payload is destroyed
  when it reaches zero. A separate acquire fence is avoided because
  ThreadSanitizer models fences poorly.
- `Weak` upgrade is a compare-and-swap loop that increments `strong` only if
  it is still non-zero.

This changes `abi.md`'s `Shared` section. `ABI_VERSION` bumps when ADR-0028
Stage C first lowers a `Shared`, which is also the first moment the layout can
be observed. No program can construct a `Shared` today
(`std::shared::new` → `R0002`), so nothing that exists is affected.

### D2 — `par_map` takes `in` elements and an `in` context

The following signatures are **proposed**. The builtins are element-generic
over the checker-accepted element set, in the same way as `std::array`
(ADR-0012), so ADR-0027 monomorphization isn't needed:

```
// proposed
std::rt::par_map(take f: fn(in T) -> R, in tasks: Array[T],
                 take workers: Int) -> Array[R]
std::rt::par_map_with(take f: fn(in C, in T) -> R, in ctx: C,
                      in tasks: Array[T], take workers: Int) -> Array[R]
```

- **Tasks** are borrowed with `in`, as they already are. A worker receives
  each element `in`, and the array can't change for the length of the call.
  `take T` stays legal for a `Copy` `T`, so every existing call
  (`fn(take Int) -> Int`) type-checks unchanged. For a non-`Copy` `T`, `take`
  is refused for the reason `O0003` gives: you can't move out of a borrow.
- **The context** is one `in` value that every worker reads. The router builds
  its table once and passes it. Context is an explicit argument, not a
  capture: ADR-0024 D4 keeps `par_map` non-capturing, and this ADR agrees.
- **Results** move into disjoint slots. An `R` that owns heap memory is
  allocated on the worker and freed on whichever thread drops it. That's
  sound because `tuo_rt_alloc`/`tuo_rt_dealloc` are `malloc`/`free`
  (`crates/tuo-runtime/src/alloc.rs`), which are thread-safe.

The runtime stays monomorphic. The backends generate one trampoline per
instantiation, `void body(void *env, long long i)`, which performs the typed
call for task `i` and writes its result slot. The shim becomes a
`tuo_rt_par_for(body, env, n, workers)` that knows nothing about `T`, `C`, or
`R`. Today's `tuo_rt_par_map` lowers onto it.

### D3 — one atomic cell, `Atomic`, sequentially consistent only

This is the synchronized cell type that the constitution and ownership.md §12
anticipate. It's a value, not a handle:

```
// proposed: builtin module std::atomic, over the builtin type `Atomic`
fn new(take v: Int) -> Atomic
fn load(in a: Atomic) -> Int
fn store(in a: Atomic, take v: Int)
fn add(in a: Atomic, take delta: Int) -> Int       // returns the previous value
fn swap(in a: Atomic, take v: Int) -> Int
fn compare_and_set(in a: Atomic, take expected: Int, take new: Int) -> Bool
```

- **It's a value type, not a handle.** A handle in the style of ADR-0015
  would carry Finding 2's problems: a 256-entry registry, process lifetime,
  negative results that collide with error sentinels, and handles anyone can
  forge from an `Int`. An `Atomic` lives where its owner puts it. Usually
  that's the caller's frame, passed as `par_map_with`'s context or as a field
  of it, and the blocked caller outlives every worker. Fork-join is exactly
  why a stack-held atomic is sound, and it's why atomics don't depend on
  `Shared`.
- **`Atomic` is the only type that can be mutated through `in`.** It is not
  `Copy` and has no clone (a copy would split its identity), and it is not an
  array element or map value in this ADR. Both backends must keep it as a
  memory place for its whole lifetime: no register promotion, and no
  temporary created for an `in` argument. Otherwise the workers would update a
  copy and the caller would read the original after the join.
- **Sequential consistency only.** There are no ordering parameters.
  Acquire/release and relaxed orderings are correct only under reasoning that
  the compiler can't check, and they are the orderings models get wrong most
  often. The lab measures what this costs against C's `relaxed` (benchmark
  plan). Lowering targets `atomicrmw`/`cmpxchg … seq_cst` on LLVM and
  Cranelift's `atomic_rmw`/`atomic_cas`/`atomic_load`/`atomic_store`. Stage C
  must confirm that Cranelift's documented ordering is SeqCst on both
  supported targets, using a litmus test, rather than assume it.
- **Overflow traps** (Constitution §24). `add` is a compare-and-swap loop
  around a checked add, so a wrapped value is never stored where another
  thread could act on it before the abort. A plain `fetch_add` would be
  cheaper under contention. The measured gap goes in the lab report. No
  wrapping variant is proposed.
- **These are not effects.** A spec can create an `Atomic` and exercise it.
  No threads exist inside the sandbox, so the result is deterministic, and the
  reference interpreter implements each operation as a plain load or store.
  This is the decision most open to review. The other option is to mark the
  operations effectful on the grounds that observing a cell shared with other
  threads is like I/O. That would put every function that touches a counter
  outside the specs, which costs more than it buys.

### D4 — typed channels move owned values (last, and additive)

```
// proposed
std::sync::Channel[T]
fn channel() -> Channel[T]
fn send(in ch: Channel[T], take v: T) -> Bool       // false once closed
fn recv(in ch: Channel[T]) -> Option[T]             // None: closed and drained
fn close(in ch: Channel[T])
```

A channel value is shared like an `Atomic`, through `par_map_with`'s context.
`send` moves the value, so the value has exactly one owner at every moment.
The queue's mutex creates a happens-before edge from the sender to the
receiver. `Option` replaces the `-1` sentinel, so payloads are no longer
restricted to non-negative values. When the channel is dropped, its
compiler-generated glue drains and drops any items left in it. ADR-0015's
`Int`-handle channel stays, since it is accepted, landed, and benchmarked.
Whether it is retired is decided when Stage D is accepted, using the
`channels` workload measured both ways. Two channel APIs left side by side
with no decision would break the one-obvious-API rule.

### The invariant that replaces `Send`/`Sync`

There are no traits, so the no-data-race property rests on facts about the
language. Each one is pinned below.

1. Every way a value reaches a worker is an `in` borrow (tasks, context), a
   moved `Copy` scalar, or a channel receive. No `par_map` signature admits
   `mut`.
2. Nothing mutates through `in` except `Atomic` operations and `Shared`
   counts, and both use hardware atomics.
3. Every other write is to a uniquely owned place. A uniquely owned place is
   either created on the thread that writes it, or arrives through a channel
   (synchronized by the queue) or a result slot (synchronized by
   `pthread_join`).

A data race needs two unsynchronized accesses to the same location, at least
one of them a non-atomic write. Points 1–3 leave no place where that can
happen. Points 1 and 3 hold by construction (signatures and ownership).
Point 2 is a property of the builtin catalog and could be broken silently by
a future builtin, so it gets a test.

## Staging

### Stage A — honesty and stacks (independent, no language change)

1. Spawn workers with the stack reserve that the main thread gets from the
   default rlimit (8 MiB on macOS and Linux; `ulimit -s` reported 8176 KiB on
   the probe machine) via `pthread_attr_setstacksize`. Then the Finding 3
   program behaves the same whichever worker runs a task. A native test pins
   a depth that passes on `main` and used to SIGBUS on a worker.
2. ADR-0028's stack-exhaustion trap, when it lands, must cover worker threads
   too. This is recorded as a constraint on that ADR rather than duplicated
   here.
3. Record Q-0005 as resolved by D1 in `open-questions.md`, and correct
   `abi.md`'s `Shared` note. This is a documentation change only, because no
   `Shared` is lowered yet.

*Items 1 and 2 landed 2026-10-16:* `par_map` workers are created with an 8 MiB
stack, and the stack-overflow trap (`TrapCode::StackOverflow`, ABI 14) arms
every worker as it starts, so the Finding 3 program returns on a worker and an
unbounded recursion there traps rather than SIGBUS — pinned by
`a_worker_has_the_main_threads_stack` and
`unbounded_recursion_on_a_par_map_worker_traps` in
`crates/tuo-cli/tests/stack_overflow.rs`. Item 3 waits on this ADR's
acceptance.

### Stage B — `in` elements and context (D2)

This stage doesn't depend on ADR-0028. If ADR-0028 Stage B (`Box[T]`,
`std::boxed::new`/`get`/`into`) has landed by then, `Box` simply joins the
element set like any owned type. Before shipping, Stage B adds the point-2
audit. Every builtin that takes an `in` receiver is listed with its runtime
shim, and a test confirms the shim's parameter is `const`-qualified, so a
future builtin that writes through `in` fails the build. The router example
builds its table once (the dogfood oracle is that served counts are
unchanged), and `concurrent-worker` stays green.

### Stage C — `Atomic` (D3)

This stage needs Stage B, because `par_map_with`'s context is how a cell gets
shared. The interpreter goes first as the reference, then both backends are
pinned three-way on single-threaded semantics. Cross-thread behaviour is
pinned natively, on both backends, by stress tests whose totals must be
exact. The differential suite can't cover it, because the interpreter has no
threads, and ThreadSanitizer instruments only the C shims, not
Cranelift/LLVM output. So each stress test needs a **sabotage check**:
replace `add` with a load, add, and store, and confirm the test fails. A
stress test that passes against the racy version doesn't prove anything.
Bumps the ABI version.

### Stage D — atomic `Shared` crossing (D1, executed)

This is ADR-0028 Stage C, unblocked, with the count protocol above.
`Shared[T]` becomes a legal task, context, and result element. The
cross-thread pin is many workers cloning and dropping one `Shared` through an
`in` context, with the final strong count and payload drop checked exactly.
The same sabotage check applies (plain counts must fail it).

### Stage E — typed channels (D4)

This stage needs Stage B, and Stage D for `Shared` payloads. ADR-0015's
cross-thread drain pin (`crates/tuo-cli/tests/effects_native.rs`) is ported
to `Channel[String]`. Its exact total becomes a byte-for-byte reassembly
check.

## Benchmark plan

Every workload below **can't be written today** except the baselines marked
as current spellings. Every figure comes from `lab` running the real
compiler, and none is promised here.

- **`contended-counter`** (Stage C): 4 workers × N increments of one cell.
  - tuonelang `std::atomic::add`.
  - Today's channel-as-cell spelling (Finding 2), which can be committed now
    as the baseline it replaces.
  - C `atomic_fetch_add` under `seq_cst` and under `relaxed`. The gap between
    the two is the price of D3's no-orderings rule.
  - Go `sync/atomic.AddInt64`.
- **`shared-refcount`** (Stage D), single-threaded, which is where atomic-only
  costs the most: a clone/drop loop over `Shared[T]`, against a `Box[T]` move
  loop as the zero-count floor. Peers are a C loop with plain and with atomic
  counters, and Rust `Rc` versus `Arc` through `PeerLanguage::Rust`. That last
  pair is the closest public measurement of what Q-0005's other answer would
  have saved. If the gap is material, this ADR's case for D1 rests on
  simplicity alone, and the report says so.
- **`lab::parallel`** (Stage B) gains a second reduction. It reads a shared
  lookup table through `par_map_with`, against C pthreads sharing a `const`
  table at the same thread count. It is measured beside the existing
  `parallel-reduction` (`benchmarks/runtime/programs/parallel/`) to show
  whether sharing context costs anything beyond the pointer. The measurement
  goes through the existing `TimedRunner`, which times only execution.
- **`channels`** (Stage E) is measured with both the handle form and
  `Channel[Int]`, plus a `Channel[String]` variant against Go's `chan string`.
  This is the evidence the handle channel's retirement decision needs.

## Consequences

- **ADR-0007's determinism note needs one qualification.** Today it reads "the
  result array is deterministic (task order, pure `f`)". After this ADR it
  holds only when `f` reads no `Atomic`, which matches channels already, where
  the router documents its winner as scheduler-dependent. Specs are
  unaffected, since they can't reach `par_map`.
- **The ownership checker gains no cross-thread dimension.** ADR-0007
  predicted this would be "the biggest single extension to its model since
  ADR-0003". With D1, that extension isn't needed, and the remaining risk is
  concentrated in point 2's audit.
- **`Atomic` is the first exception to "`in` is read-only".** Diagnostics,
  `ownership.md`, and the cheat sheet need to say so explicitly, because a
  model that has learned `in` means "can't change" will reason wrongly about
  it.
- **Liveness is still unchecked.** A spin loop on `compare_and_set`, or a
  `recv` that no sender will ever satisfy, can hang the program, as ADR-0015
  already notes.

## What this ADR does not propose

The following are out of scope:

- Weaker memory orderings.
- A data-guarding `Mutex[T]`. That needs a scoped `mut` borrow obtained from a
  shared one, which is the shape ADR-0024's closures would provide.
  ADR-0015's advisory handle mutex stays.
- A non-atomic `Rc` sibling. Its re-entry path is the `shared-refcount`
  measurement.
- `Atomic` of `Bool`, other widths, or pointers, and `Array[Atomic]`.
- Thread-locals, `select`, and condition variables.
- Lock-free collections.
- `Shared` with interior mutability.
- Detached spawn and unbounded concurrency. Fork-join still has to join, and
  that is the property that makes stack-held shared state sound.
