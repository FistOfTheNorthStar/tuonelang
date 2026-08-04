# ADR-0007: The concurrency model

- **Status:** proposed
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
