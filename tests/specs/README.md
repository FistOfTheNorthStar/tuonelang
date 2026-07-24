# specs tests

Executable-specification tests: colocated `spec` blocks compiled to verified
MIR and **run** through the reference interpreter (`tuo-spec`).

## What a spec run does

`tuo check` parses, resolves, and type-checks specs but never executes them
(ADR-0002). Execution is `tuo-spec`'s job. It lowers each spec to verified MIR
(`tuo_mir::lower_specs`): every `then` / `assert` clause becomes a synthetic
function that replays the spec's `given` / `when` setup and returns the
assertion's `Bool` value, and each `lhs == rhs` assertion additionally lowers
both operands so a failure can report concrete *actual* and *expected* values.
The runner builds one interpreter over the whole program (its constructor is
the mandatory `tuo_mir::verify` gate) and executes each selected spec's
assertions in the deterministic sandbox — bounded instruction fuel, recursion
depth, and memory, with no host filesystem/network/clock/randomness reachable.

## Commands

- `tuo spec <files>` — execute every spec.
- `tuo spec --target <name> <files>` — execute only the specs of one function
  (or the free-standing spec of that name).
- `tuo verify <files>` — perform every static check *and* run the specs (a
  superset of `tuo check` and `tuo spec`).

A program with front-end errors is refused (a broken spec does not run). None
of these commands promise any particular latency; each reports the measured
execution time so it can be observed.

## Fixtures (`fixtures/`)

- `passing.tuo` — a program whose specs all hold (`tuo spec` / `tuo verify`
  succeed).
- `failing.tuo` — one false assertion; `tuo spec` reports the expected/actual
  mismatch and exits non-zero, while `tuo check` still succeeds (it does not
  execute specs).

## Where the tests live

- `crates/tuo-mir/tests/spec_lowering.rs` — `lower_specs` emits verifiable MIR
  and records the right assertion shape.
- `crates/tuo-spec/tests/conformance.rs` — source-to-result runner behavior:
  passing/failing/trapping assertions, `given`/`when` setup, target selection,
  resource limits, determinism, and timing.
- `crates/tuo-cli/tests/spec_command.rs` — the `tuo spec` / `tuo verify` /
  `tuo check` command surface against the real binary, using the fixtures here.
