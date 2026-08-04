# ADR-0004: Aggregates and iteration in the runnable core

- **Status:** proposed
- **Date:** 2026-08-04
- **Context:** Dogfooding v0 (see [`DOGFOODING.md`](../../DOGFOODING.md),
  findings **D-1** and **D-2**) showed that the single most repeated cost across
  *every* real program was the absence of two things from the **runnable** scalar
  core:

  1. a **product type** — a way to name a group of fields as one value. Structs
     and enums parse and type-check today, but the native backend refuses any
     program that reads a struct field ("a projected place … outside the scalar
     subset", proven by
     `crates/tuo-codegen-cranelift/tests/compile.rs::a_projected_place_makes_a_body_unsupported`).
     So a runnable program cannot carry a `Point`; the
     [`examples/workspace/geometry`](../../examples/workspace/geometry/) library
     must pass every point as two separate `Int` parameters
     (`manhattan(ax, ay, bx, by)`), and [`examples/cli-stats`](../../examples/cli-stats/)
     must model its dataset as seven positional accessor functions `d0()..d6()`.

  2. an **array/collection type with iteration** — a way to hold N values and
     loop over them. v0 has no array lowering and no runnable loop over data, so
     every fold in the examples is either hand-unrolled or written as explicit
     accumulator-passing recursion (`data-pipeline`'s `fold_one` chain,
     `concurrent-worker`'s `load_from`/`serial_from`). This does not scale past a
     fixed, compile-time-known batch size — a real data processor cannot have its
     record count baked into the source.

  These are not independent asks: iteration is only useful over an aggregate, and
  an aggregate is only ergonomic with iteration. They are one decision.

  The constraint the project imposes on itself is that this must be **designed**,
  not bolted on to make an example compile. The grammar already *accepts* structs,
  enums, `for`, and `while` (it is a deliberate superset); what is undecided is
  the **runnable semantics and native lowering** — memory layout, ownership
  interaction, and how iteration bounds stay finite for the fuzz/interp guarantees.

- **Decision (proposed):** extend the **runnable** core, in two stages behind one
  design, so the reference interpreter and both native backends agree instruction
  for instruction (the existing correctness contract):

  1. **Product types.** Lower `struct` construction and field projection to the
     ABI layout already specified in `specification/abi.md`
     (`abi::layout_of` → `#[repr(C)]` packing) so a struct becomes a real runnable
     value. Enums (tagged unions) follow, using the explicit `u32` discriminant
     numbering the ABI already defines. This retires the backend's
     "projected place is unsupported" refusal for the scalar-field case first
     (fields of `Int`/`Bool`), then aggregates-of-aggregates.

  2. **A fixed-capacity array + bounded iteration.** A `[T; N]`-style value with
     compile-time length, plus a `for x in arr` loop that lowers to a
     counted loop. Compile-time-known length keeps every downstream stage's
     recursion/iteration provably bounded (the property the fuzz harness relies
     on), and defers the harder heap-backed growable collection to a later ADR
     that builds on the `Box`/allocator seam.

  Both land as normative spec text plus fixtures **before** implementation, the
  way ADR-0003 paired the ownership model with its fixture corpus. The dogfooding
  examples become the acceptance oracle: `geometry` should be rewritable to pass a
  `Point` struct, and `data-pipeline` to fold over an array, with identical spec
  verdicts and identical `main` exit bytes.

- **Consequences:**
  - *Easier:* application code stops paying the accessor-function and
    unrolled-fold tax; the stdlib can offer real aggregate helpers; ownership
    diagnostics (finding D-5c) become reachable from runnable programs because
    aggregates are the first non-`Copy` runnable values.
  - *Harder:* the native backends must lower projection and counted loops in
    lock-step with the interpreter, and the differential suites
    (`codegen_three_way.rs`, `differential.rs`) must be extended to generate
    aggregate and loop programs — a mismatch there is a release blocker.
  - *Trade-off:* fixed-capacity arrays are deliberately less capable than a
    growable `Vec`. That is intentional: it keeps the bounded-recursion invariant
    intact and lets a growable collection be its own, separately-benchmarked ADR
    on top of the allocator, rather than smuggling the heap in here.

- **Benchmark consideration:** this feature is exactly what unblocks the
  **`collections`** runtime workload the performance laboratory currently records
  as `Support::Unsupported` (`crates/tuo-bench/src/lab/runtime.rs`). The lab is
  the enforcement mechanism: when aggregates+iteration land, that workload moves
  from `Unsupported` to `Supported` **with a committed program**, and the lab
  measures insert/scan cost against its equivalent-semantics C peer under recorded
  toolchains — no number is published until both sides compile and run. The
  compiler benchmarks must also gain an aggregate/loop program so the cold-check
  and incremental-edit costs of the new lowering are tracked from day one. This
  ADR is not "accepted" until that lab entry and its C peer exist.
