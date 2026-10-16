# differential tests — the cross-engine semantic gate

tuonelang has one reference meaning for a program: **what the MIR interpreter
computes** for its verified MIR. The native Cranelift backend must agree with
that meaning instruction for instruction. Where they diverge, the backend is
wrong — never the interpreter. This directory is the charter and the corpus for
the machinery that enforces that agreement across a whole population of
programs.

**Any disagreement between the interpreter and a native backend is a
compiler-correctness failure.** There is no "acceptable" divergence.

## What runs, and where

The executable suite lives with the CLI (it drives the real `tuo` binary):

| File | Role |
|------|------|
| `crates/tuo-cli/tests/differential.rs` | The suite entry point and the **CI gate** — generates programs, runs both engines, minimizes any failure. |
| `crates/tuo-cli/tests/differential/harness.rs` | The reusable engine: interpret a program, run it natively, compare the full observable outcome. |
| `crates/tuo-cli/tests/differential/generator.rs` | A deterministic generator of small, well-typed programs inside the backends' shared subset (scalars, fixed arrays, function values, `Box` values). |
| `crates/tuo-cli/tests/differential/shrink.rs` | The minimized-reproduction workflow: reduce a failing program and save a repro. |
| `crates/tuo-cli/tests/codegen_differential.rs` | Hand-written fixtures pinning specific shapes (recursion, traps, the refusal boundary). |

## What is compared

For every eligible program the harness compares the two engines on every
behavior the language makes **observable today**:

1. **Return value** — `main`'s integer, surfaced natively as the process exit
   status (the v0 entry ABI truncates it to a byte, exactly as a C `main`
   return becomes the exit code).
2. **Deterministic traps** — divide-by-zero and integer overflow abort the
   interpreter and, natively, terminate with the runtime's fixed trap status
   (`tuo_runtime::TRAP_EXIT_STATUS`) after printing a `tuo: trap: …` line to
   stderr. The two must trap *together*. The harness requires the stderr line
   as well as the status, because a normal return whose exit byte is 134 (for
   example `-378`) exits with the same status and is not a trap.

Two behaviors named by the differential charter are **not yet expressible**, and
the suite is honest about that rather than faking coverage:

- **stdout** — the language has no I/O facility yet. `harness::Outcome` reserves
  a slot for stdout so the comparison extends automatically the day one lands;
  until then every program's stdout is empty on both sides.
- **observable destruction** — drops run no user code (there are no user
  destructors), so no drop is observable to a program. Since ADR-0028 Stage B
  the generator does emit values with compiler-generated drop glue — `Box`
  values and the owned `String`s in a boxed list — so the glue *executes* on
  every engine, and a glue bug that corrupts memory surfaces as a wrong return
  or a crash; but nothing compares a drop directly.

## Random generation

`generator::program(seed)` is a **pure function of its `u64` seed** — same seed,
same program, on every host and every run. It uses a small SplitMix64 PRNG, not
the `rand` crate or any wall-clock/entropy source, so a failing seed reproduces
exactly. Every program it emits is **well-typed and inside the backend subset by
construction** (integers, arithmetic, comparison, `if`/`else`, `let`, direct
and indirect calls, `mut` write-back, floats folded back through `as Int`,
fixed `[Int; N]` arrays; a call graph that is a DAG, so every program
terminates). Since ADR-0028 Stage B it also emits `Box[T]` values: boxed
scalars and structs through `std::boxed::new`/`get`/`into`, boxes in struct
fields, `Option` payloads, and fixed arrays, a box across a call boundary and
behind an `in` parameter, and two bounded-depth recursive types through
`Option[Box[Self]]` — a binary tree of at most 31 nodes and a list of up to six
cells owning `String`s — built, deep-copied with `get`, consumed whole or in
part, and dropped unopened. These live in a fixed prelude every program
carries, whose recursion is bounded by literal depths. It never emits a
growable `Array[T]`, a `Shared`/`Weak` value, or anything else a backend would
refuse, so a divergence is always a real finding, never a rejected program.

The native gate sweeps a fixed seed range (each seed is a full native
compile+link+run); a separate interpreter-only corpus check sweeps a far wider
range cheaply and asserts the population exercises *both* returns and traps.
Widen either range to search harder.

## The minimized-reproduction workflow

When a seed diverges, the suite does not stop at "seed N failed":

1. `shrink::minimize` reduces the program by dropping helpers, collapsing
   arithmetic and `if` trees to literals, and keeping one branch arm — re-running
   **both engines** on each candidate and keeping only reductions that still
   type-check *and* still diverge.
2. `shrink::save_repro` writes the reduced program to
   `<target-tmp>/differential-repro/seed-<N>.tuo` alongside a `.report` naming
   what each engine did and the exact command to reproduce it:

   ```
   tuo run seed-<N>.tuo   # observe the exit status; compare to the interpreter
   ```

3. The test then fails with the minimized program printed inline, so the failure
   is small and actionable in the CI log itself.

To reproduce a reported failure locally, run the single seed the failure names —
the generator is deterministic, so it regenerates the identical program.

## Why this is a mandatory gate

These are ordinary `#[test]`s. CI runs `cargo test --workspace` with
`-D warnings` (see `.github/workflows/ci.yml`), so this suite runs on every push
and pull request and **blocks the merge** the moment the native backend and the
reference interpreter disagree. Keeping the backend faithful to the interpreter
is therefore not a manual discipline — it is enforced.
