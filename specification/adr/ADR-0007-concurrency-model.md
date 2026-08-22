# ADR-0007: The concurrency model

- **Status:** accepted (2026-08-22 — the model is resolved and landed:
  structured fork-join over one primitive, `std::rt::par_map`; see Resolution)
- **Date:** 2026-08-04
- **Context:** Dogfooding v0 (see [`DOGFOODING.md`](../../DOGFOODING.md), finding
  **D-4**) built the [`examples/concurrent-worker`](../../examples/concurrent-worker/)
  scheduling model — round-robin partition, per-worker load, makespan, speedup,
  imbalance — entirely in the runnable core, because those are pure arithmetic
  over a schedule. What it could **not** build is the pool itself: v0 has no way
  to start a second thread of control, no channel/queue to hand work across it,
  and no synchronization type to share state safely. `spawn_pool`, `submit`, and
  `join` are `CONTRACT:` comments.

  Concurrency in a memory-safe, single-owner language is one of the highest-stakes
  design decisions there is: it interacts directly with the ownership model
  (ADR-0003) — what may cross a thread boundary, how `Shared` behaves under
  contention, whether there are atomics — and with the effect boundary (ADR-0006),
  since spawning is itself an effect. Getting it wrong reintroduces data races,
  the exact class of bug the language exists to prevent. This is precisely the
  "hard to reverse, shapes later work" decision the ADR process is for, and it
  must not be improvised to make one worker example run.

  It is also **downstream** of two other proposed ADRs: a work queue needs a
  collection (ADR-0004) and a channel needs an effect/synchronization primitive
  (ADR-0006). So this ADR is sequenced last of the four dogfooding ADRs and is
  recorded now to capture the requirement and the constraints, not to decide the
  model prematurely.

- **Decision (proposed):** *defer the model choice*, but fix the **constraints**
  it must satisfy, so whoever designs it starts from an agreed contract rather
  than folklore:

  1. **No data races, checked.** Whatever crosses a thread boundary must be
     checked by the ownership system, extending the per-function borrow model of
     ADR-0003 to a `Send`/`Sync`-like discipline expressed in tuonelang's own
     `in`/`mut`/`take` + `Shared`/`Weak` vocabulary — not a bolted-on copy of
     Rust's markers "by folklore" (the project's stated prohibition).

  2. **Spawning is a typed effect.** Starting a worker is an effect under
     ADR-0006's boundary, so the pure spec sandbox and the fuzz harness keep
     seeing total, effect-free stage functions, and a program's concurrency is
     visible in its types.

  3. **The deterministic scheduling model is the oracle.** The
     `concurrent-worker` executable tier already computes a pool's *observable*
     performance (makespan, speedup, balance). When real concurrency lands, that
     model becomes the correctness oracle a live run is checked against —
     dogfooding produced the test before the feature.

  4. **One obvious primitive.** Following the stdlib's "one obvious API per task"
     rule, v0-next should ship a single, well-specified concurrency primitive
     (e.g. a bounded task pool over a channel) rather than a menu, with richer
     patterns layered on top.

  The full normative model + fixtures land under this ADR (or a successor that
  supersedes it) **before** implementation, exactly as ADR-0003 did for ownership.

- **Consequences:**
  - *Easier:* the worker-pool example becomes a running program; server request
    handling (ADR-0006 + this) can be concurrent; CPU-bound data pipelines can
    parallelize.
  - *Harder:* the ownership checker grows a cross-thread dimension, the biggest
    single extension to its model since ADR-0003; the interpreter must define a
    deterministic concurrency semantics (or an explicitly non-deterministic one
    with a pinned scheduler for specs) so `tuo spec` stays reproducible.
  - *Trade-off:* deferring the model now (recording only constraints) risks
    under-specification, but committing to a model before strings/collections/
    effects exist would be deciding on top of unbuilt foundations — strictly
    worse.

- **Benchmark consideration:** concurrency is the one capability whose entire
  *point* is performance, so it may not land without benchmarks. It has no single
  existing lab workload to unblock; instead it requires a **new benchmark
  category**: parallel speedup of a CPU-bound workload versus the serial baseline,
  measured by the performance lab under recorded hardware (core count is part of
  the environment the lab already captures via
  `Environment::logical_cpus`). The equivalent-semantics peer is a C program using
  the same thread count on the same machine. The dogfooding model's predicted
  makespan/speedup become the *expected* figures the live measurement is checked
  against — a real speedup that disagrees with the model is a scheduling bug, and
  a "scales linearly" claim is inadmissible until the lab measures it against C.
  This ADR cannot reach "accepted" until that benchmark category exists.

- **Resolution (2026-08-22) — the model, decided and landed.** The deferred
  choice resolves to the narrowest sound model: **structured fork-join over
  exactly one primitive**,

  ```
  std::rt::par_map(take f: fn(take Int) -> Int, in tasks: Array[Int],
                   take workers: Int) -> Array[Int]
  ```

  — apply the non-capturing function value `f` (ADR-0008 Tier 1) to every
  task, distributed **round-robin** over `workers` OS threads (task `i` on
  thread `i % workers`), join every thread, and return the results **in task
  order**. Each of the four recorded constraints is satisfied, and two of
  them resolved by *construction* rather than by a checker extension:

  1. **No data races, checked.** Nothing the primitive admits can race: the
     only values that cross a thread boundary are a `Copy` code pointer, a
     read-only borrowed `Array[Int]` (immutable for the whole call — the
     caller is blocked inside it, and `in` forbids concurrent mutation by
     construction), `Copy` `Int` tasks, and each thread's own disjoint
     result slot. The `Send`/`Sync`-like discipline the constraint asked for
     is expressed in the primitive's own `in`/`take` + non-capturing-`fn`
     signature — the v0 rule is "only `Copy` values and an immutably lent
     scalar buffer cross" — with no new ownership dimension needed. Widening
     what may cross (shared state, `Shared` under contention, atomics) is a
     future ADR's decision, and until one lands nothing else can cross at
     all.
  2. **Spawning is a typed effect.** `Builtin::RtParMap` is in
     `Builtin::is_effect`, so a spec whose closure could reach it is `R0007`,
     the spec sandbox and fuzz harness keep seeing total, effect-free stage
     functions (the interpreter still executes **no** effect), and a
     program's concurrency is visible in its types via the transitive
     effect computation. MIR carries it as `EffectOp::ParMap` — the one
     effect whose destination is an `Array[I64]` — verified under `M0011`
     (`mir.md` §4.2); both backends lower it to the runtime's
     `tuo_rt_par_map` (POSIX threads, `abi.md` §Effect symbols), linked like
     every other shim (with `-pthread`).
  3. **The deterministic scheduling model is the oracle — executed.**
     `examples/concurrent-worker` now runs its pool live: `main` computes
     the makespan through the pure spec-checked model AND through a real
     `par_map` (one OS thread per worker, each running the model's own
     `worker_load` over its round-robin partition — the primitive implements
     exactly the partition the model predicts), and exits with the model's
     answer (15) only when the live run agrees. A scheduling bug flips the
     exit; the native dogfood test pins it on both backends.
  4. **One obvious primitive.** `par_map` is the *only* way to start a
     thread — no bare spawn, no detached handles, no channel menu. The
     stdlib effect tier wraps it once (`std::sync::par_map`, pinned natively
     by `par_map_runs_natively`); `std::sync`'s `lock`/`unlock` remain
     honest `CONTRACT:` entries (shared-state locking is exactly what the
     structured primitive deliberately does not admit), and the old `spawn`
     contract is superseded — detached spawn is *deliberately absent*, not
     pending.

  Determinism: the result array is deterministic (task order, pure `f`), so
  `tuo spec` reproducibility is untouched (specs cannot reach effects) and
  the effectful shell's observable — the exit byte — is deterministic in
  every committed program.

- **Benchmark resolution:** the required category exists and measures. The
  performance lab gained `lab::parallel` — the **parallel-speedup** category:
  one CPU-bound reduction (`benchmarks/runtime/programs/parallel/`, four
  committed programs: tuonelang serial + `par_map`, C serial + pthreads with
  the same thread count, all exiting 64), measured through a host-injected
  `TimedRunner` that builds first and times **only the binary's execution**
  (with a warm-up run so first-exec host costs never pollute the figure). A
  side is `Measured` only when its serial *and* parallel programs really ran
  to the expected exit — raw nanoseconds recorded, the ratio derived at
  render time, never a stored or promised figure — and the `LabReport`
  carries the category (additively; the committed example records honest
  skips). Core count is already part of the recorded environment
  (`Environment::logical_cpus`). Pinned by `tuo-bench`'s unit tests,
  `tuo-bench/tests/lab.rs` (committed programs equal embedded sources; both
  tuonelang programs pass the real front end), and the live
  `tuo-cli/tests/lab_command.rs::parallel_speedup_measures_live_through_the_real_cli`
  (real `tuo build` + `cc -O2 -pthread`, both sides measured on a live
  host; `--nocapture` prints the figures). The `networking` lab entry is
  **not** this ADR's gate and stays honestly `Unsupported` — its missing
  primitive is a socket-open *effect* (ADR-0006's amendment 2), a future
  effect ADR.
