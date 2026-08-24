# ADR-0015: Channels and mutexes — communication joins the effect seam

- **Status:** accepted (2026-08-24 — all stages landed; see Resolution)
- **Date:** 2026-08-24
- **Context:** ADR-0007 resolved tuonelang's concurrency model to structured
  fork-join over the one primitive `std::rt::par_map`, deliberately admitting
  **no shared state across threads** — the tasks are `Copy` values, the
  results land in disjoint slots, and no data race is expressible by
  construction. That decision left two recorded debts:

  - `std::sync::lock`/`unlock` are the stdlib's **last `CONTRACT:` stubs**,
    "until an ADR admits shared state";
  - `examples/concurrent-worker`'s `CONTRACT submit` names the missing piece
    exactly: "a shared, *dynamically-drained* work queue needs shared mutable
    state across threads, which stays out of v0 by design" — its live pool
    can only run static round-robin assignment.

  The Go-parity review lists channels as the remaining concurrency gap. The
  right shape is not shared mutable *memory* (which would forfeit ADR-0007's
  no-data-race property) but shared **communication**: a synchronized queue
  whose payloads cross by value, and an advisory mutex for critical sections
  over *external* resources (files, sockets — the OS effects ADR-0013/0014
  opened). Both are runtime-owned objects behind opaque `Int` handles — the
  same shape as a file descriptor — so nothing new crosses the ABI.

- **Decision:** extend the seam with **seven further effect builtins in
  `std::rt`** — four for channels, three for mutexes. All keep the seam's
  contract: fixed non-generic signatures, **never trap**, errors as negative
  return values. Handles are process-lived (there is deliberately no
  free/destroy: a program creates a handful, and a bounded registry refusing
  exhaustion with `-1` is simpler and safer than a use-after-free hazard).

  | Signature | Meaning |
  |-----------|---------|
  | `fn chan_new() -> Int` | Create an unbounded FIFO channel of **non-negative** `Int` values; returns a channel handle (`>= 0`) or `-1` when the registry is exhausted. |
  | `fn chan_send(take ch: Int, take v: Int) -> Int` | Enqueue `v`; `0` on success, `-1` on an invalid handle, a closed channel, **or a negative `v`**. Negative payloads are refused so `chan_recv`'s `-1` is unambiguous — the v0 channel is a work-distribution primitive carrying non-negative values; richer payloads are a program-level encoding. |
  | `fn chan_recv(take ch: Int) -> Int` | Dequeue the oldest value, **blocking** until one is available; returns the value, or `-1` once the channel is closed **and** drained (or the handle is invalid). |
  | `fn chan_close(take ch: Int) -> Int` | Close the channel: subsequent sends are refused, and every blocked or future `chan_recv` returns `-1` after the queue drains. `0` on success, `-1` on an invalid handle; closing twice is `0` (idempotent). |
  | `fn mutex_new() -> Int` | Create a mutex; returns a handle (`>= 0`) or `-1` when the registry is exhausted. |
  | `fn mutex_lock(take m: Int) -> Int` | Acquire, **blocking** until available; `0` on success, `-1` on an invalid handle or an error the host reports (relocking a mutex the calling thread already holds included — the runtime uses an error-checking mutex, so the mistake is a `-1`, never undefined behavior). |
  | `fn mutex_unlock(take m: Int) -> Int` | Release; `0` on success, `-1` on an invalid handle or when the calling thread does not hold it. |

  **The no-data-race property is preserved.** Channels move `Copy` `Int`s
  *by value* through a runtime-synchronized queue — no tuonelang memory is
  ever shared between threads. A mutex guards nothing in tuonelang's memory
  (there is nothing shared to guard); it is the advisory critical-section
  primitive for the *external* resources the OS effects reach — two `par_map`
  workers appending to one file, for example. What ADR-0007 excluded stays
  excluded: no shared mutable variables, no detached spawn.

  **Blocking is honest:** `chan_recv` and `mutex_lock` can block forever if
  the program's protocol is wrong (a receive nothing will ever send), exactly
  as in every language with these primitives. That is a liveness property the
  type system does not claim to check; the safety contract ("never traps,
  never corrupts") holds regardless.

  **Runtime obligations (ABI):** the runtime shim gains
  `tuo_rt_chan_new`/`send`/`recv`/`close` and
  `tuo_rt_mutex_new`/`lock`/`unlock` — bounded static registries, each
  channel a `pthread_mutex_t` + `pthread_cond_t` over a heap FIFO (nodes
  through the ADR-0009 `tuo_rt_alloc`/`tuo_rt_dealloc` seam), each mutex a
  `PTHREAD_MUTEX_ERRORCHECK` pthread mutex. `specification/abi.md` documents
  each; `ABI_VERSION` bumps in the same commit.

  **Stdlib payoff (the acceptance oracle):** `std::sync` gains the
  effect-tier wrappers `channel`/`send`/`recv`/`close` and
  `mutex`/`lock`/`unlock`, and its **CONTRACT tier empties** — the stdlib no
  longer advertises anything it cannot run. *Amendment recorded at
  acceptance:* the old contract signatures `lock(in state: LockState) ->
  LockState`/`unlock(...)` were placeholders written before the primitive's
  shape was known; the real API is handle-based (`lock(take m: Int) -> Int`),
  and the pure `LockState` model stays in the executable tier as the
  documented state model. `examples/concurrent-worker` gains the
  **dynamically-drained shared work queue** its contract named: `main` fills
  a channel with the task ids, closes it, and runs `par_map` workers that
  *race to drain it* — dynamic stealing, the assignment the static model
  cannot predict — exiting successfully only when the drained total equals
  the model's serial cost (the invariant dynamic stealing must preserve).

- **Consequences:**
  - *Easier:* producer/consumer pipelines, work stealing, and cross-worker
    coordination become expressible; the stdlib's contract tier is empty;
    `concurrent-worker` stops advertising a queue it cannot run.
  - *Harder:* channel/mutex programs can deadlock (a blocked `recv`/`lock`
    with no counterpart) — a liveness bug the compiler does not catch, now
    expressible where before it was not. The docs say so plainly.
  - *Trade-off:* non-negative-only channel payloads keep the primitive
    scalar and its closed-signal unambiguous without a multi-value return;
    process-lived handles trade a bounded leak for the impossibility of
    use-after-free. Both are documented v0 simplifications, revisitable
    additively.

- **Benchmark consideration (the gating workload):** a new `channels`
  runtime workload — per round, create a channel, send 500 values, close it,
  and drain it to exhaustion, single-threaded so the number isolates the
  synchronization-crossing cost itself (the parallel category already
  measures threading) — with equivalent-semantics peers: **Go uses its
  native `chan int64`** (the direct Go-parity comparison this ADR invites)
  and C a mutex-and-condvar FIFO making the identical locked crossings.

- **Deliberately out of scope:** shared mutable memory across threads
  (`Shared[T]` values crossing threads — still ADR-0007's exclusion),
  detached spawn (deliberately absent, not pending), bounded/rendezvous
  channels, `select` over multiple channels, timeouts on `recv`/`lock`,
  condition variables as a surface primitive, and freeing channel/mutex
  handles. Each is additive on this seam when dogfooding demands it.

- **Resolution (2026-08-24):** all stages landed in one increment and every
  acceptance condition is met by a committed, test-pinned artifact:
  - *Front end + MIR:* the seven builtins resolve as real `std::rt` symbols
    (`Builtin::{RtChanNew, RtChanSend, RtChanRecv, RtChanClose, RtMutexNew,
    RtMutexLock, RtMutexUnlock}`), join the effectful set (pinned by the
    `Builtin::ALL` loop in `crates/tuo-types/tests/effects.rs`), and lower
    to the seven new `EffectOp`s, verified per-op by `check_effect_types`
    (`M0011`). The reference interpreter is untouched: its blanket effect
    refusal covers the new ops by construction.
  - *Native lowering (ABI v9):* both backends lower the new ops to the
    seven `tuo_rt_*` shims; the runtime's effect C source implements them
    (bounded static registries; per-channel `pthread` mutex + condvar over
    a heap FIFO whose nodes flow through the ADR-0009 allocation seam;
    `PTHREAD_MUTEX_ERRORCHECK` mutexes), and `ABI_VERSION` bumped 8 → 9.
    Pinned end-to-end on **both** backends by
    `crates/tuo-cli/tests/effects_native.rs`: the full single-threaded
    policy roundtrip (FIFO order, negative-payload refusal, close/drain,
    idempotent close, invalid handles, error-checked relock and non-holder
    unlock), and a **cross-thread** pin — three `par_map` workers racing
    `chan_recv` on one pre-filled closed channel drain it to exactly the
    sum sent, however the race resolves.
  - *Stdlib payoff:* `std::sync` gained the effect-tier wrappers
    `channel`/`send`/`recv`/`close` and `mutex`/`lock`/`unlock` (native CLI
    pin in `crates/tuo-cli/tests/stdlib.rs` on both backends), and the
    **contract tier is empty** — the amendment above recorded (handle-based
    signatures replace the placeholder `LockState` shapes; the pure model
    stays executable), and a new CLI test pins the tier's emptiness so
    nothing re-enters silently. `examples/concurrent-worker` gained
    `dynamic_total`: the channel-drained pool whose grand total must equal
    the model's `serial_cost` for `main` to exit 15 — pinned by
    `crates/tuo-cli/tests/dogfood_examples.rs`.
  - *Benchmark condition:* the lab's twelfth workload **`channels`** landed
    `Support::Supported` — the committed
    `benchmarks/runtime/programs/tuo/channels.tuo` (200 rounds of
    send-500/recv-500 through one process-lived channel; exit byte 244)
    with its mutex-and-condvar C peer and **native-`chan`** Go peer,
    measured live by `crates/tuo-cli/tests/lab_command.rs`.
