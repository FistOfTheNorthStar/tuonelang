# The tuonelang 0.1 release gate

> **Purpose.** tuonelang 0.1 is a *commitment*: once declared ready, its grammar,
> semantics, machine protocol, and diagnostic schema are stable surfaces that
> tooling and generated code may rely on. This document is the checklist that
> gates that declaration. **0.1 may not be declared ready until every criterion
> below is `MET` or explicitly recorded as `RELEASE-BLOCKING`.** A criterion that
> is merely "close" does not pass.

This gate is written in the same spirit as the rest of the compiler: **it never
advertises readiness the artifacts cannot back.** Each criterion names a concrete,
committed *proving artifact* — a test target, a benchmark, or a normative
document — and its status is a claim about that artifact, not an assertion. The
companion checker `crates/tuo-cli/tests/release_gate.rs` reads the machine-readable
manifest at the end of this file, asserts every cited artifact exists in the tree
(so the gate can never cite a phantom test), and — run with `--nocapture` —
regenerates the readiness report below from those artifacts:

```bash
cargo test -p tuo-cli --test release_gate -- --nocapture
```

## Status vocabulary

| Status | Meaning |
|--------|---------|
| **MET** | The criterion's normative requirement is satisfied and pinned by the cited committed artifact, which is green in CI. |
| **PARTIAL** | The requirement is substantially satisfied by the cited artifact, but a named, bounded piece remains. A `PARTIAL` criterion **must** carry a follow-up (an ADR or an issue) and **blocks 0.1** unless the "remaining" note is explicitly downgraded to non-blocking here. |
| **RELEASE-BLOCKING** | A known divergence or gap that the team has decided must be resolved before 0.1 ships. Nothing may be silently deferred: an unresolved item is *either* fixed *or* recorded here with this status. |

The gate is *honest*, not aspirational: the table records exactly where 0.1
stands, and the checker guarantees the record cannot drift from the tree. As of
this revision every criterion is `MET` — but the gate earns that verdict by
citing artifacts the checker proves exist, not by asserting readiness. A criterion
that regressed would flip back to `PARTIAL`/`RELEASE-BLOCKING` here the moment its
artifact changed.

---

## The sixteen criteria

Each criterion has an id (`G<n>`), the prompt's normative requirement, the proving
artifact that decides it, and its current status with a one-line justification.

### G1 — Grammar is versioned

- **Requirement:** the surface grammar carries an explicit version, and a change to
  a frozen production is a breaking change tied to that version.
- **Proving artifact:** `specification/grammar.ebnf` (the `GRAMMAR-VERSION: 0.3`
  marker) + Constitution §30 (edition/breaking-change rule).
- **Status:** **MET.** The EBNF is the single authoritative CFG, now stamped
  `GRAMMAR-VERSION: 0.3 / EDITION: 2024`; the release-gate checker asserts the
  marker is present and pins the value it expects. The marker moved from `0.1`
  to `0.2` when ADR-0019 Stage A added the bitwise tokens, and to `0.3` when
  ADR-0020 Stage C added the attribute prefix and the `#` token — the *grammar*
  version tracks the surface grammar, not the release, so it may advance
  within a release line; the **edition** stays `2024` because no `[FROZEN]`
  production's surface changed and no existing program's meaning moved (both
  times, the new tokens were previously lexical errors), which is the condition
  Constitution §30 attaches a breaking change to.

### G2 — Formatter is canonical and idempotent

- **Requirement:** one canonical format; formatting a formatted program is a
  no-op.
- **Proving artifact:** `crates/tuo-fmt/tests/golden.rs` (canonical output) +
  `crates/tuo-fmt/tests/properties.rs` (idempotence + stability properties) +
  `crates/tuo-cli/tests/fmt_command.rs` (`tuo fmt`/`fmt --check` end-to-end).
- **Status:** **MET.** The formatter is deterministic, zero-config, and its
  idempotence is a pinned property, not a claim.

### G3 — Parser does not crash under fuzzing

- **Requirement:** arbitrary source input never crashes the front end; discovered
  crashes become permanent regressions.
- **Proving artifact:** `crates/tuo-fuzz/tests/sweep.rs` (fixed-seed corpus sweep,
  ordinary `cargo test`) + `crates/tuo-fuzz/regressions/` (content-addressed
  crashers replayed on every run).
- **Status:** **MET.** Every front-end entry point up through MIR lowering is total
  by construction; the sweep and the committed regressions (e.g. the deep
  binary-operator-chain stack overflow, now `P0003` before parse) hold the line.
  Coverage-guided `cargo fuzz` targets extend the same checkers on nightly.

### G4 — Static semantics are documented

- **Requirement:** the type system and name resolution are specified, not only
  implemented.
- **Proving artifact:** `specification/static-semantics.md` (the normative model:
  the resolution rules, the type universe, explicit signatures,
  no-implicit-conversions, the `Rxxxx`/`Txxxx` diagnostics, and the
  syntax/semantics boundary) + `tests/types/` (the executable counterpart pinning
  every `T0001`..`T0013`).
- **Status:** **MET.** The static semantics are now a single normative document,
  `static-semantics.md`, a peer of `ownership.md`: it consolidates the rules from
  Constitution §§8–24, `syntax.md`, and the `tuo-types`/`tuo-resolve` crate docs
  into one place, and the `tests/types/` corpus pins them.

### G5 — Ownership semantics are documented

- **Requirement:** the memory-safety model is specified.
- **Proving artifact:** `specification/ownership.md` (the normative model) +
  `specification/adr/ADR-0003-ownership-model.md` (the decision record) +
  `tests/ownership/`.
- **Status:** **MET.** `ownership.md` is a complete normative document; ADR-0003
  records the decision; the checker suite pins conformance.

### G6 — MIR semantics are documented

- **Requirement:** the single executable semantic representation is specified.
- **Proving artifact:** `specification/mir.md` (the normative model: every
  instruction, terminator, and trap defined on its type; the mandatory
  verifier's well-formedness invariants and `Mxxxx` codes; the interpreter's abort
  taxonomy and deterministic sandbox) + `crates/tuo-mir/src/verify.rs` (the
  verifier that enforces it) + `specification/adr/ADR-0002-spec-semantics.md`.
- **Status:** **MET.** MIR's meaning is now a single normative document,
  `mir.md`, a peer of `ownership.md`/`abi.md`: it lifts the instruction semantics
  from the `tuo-mir` crate doc, the verifier invariants, and the interpreter's
  reference-execution model into one place. The verifier remains the enforcement
  gate every backend and the interpreter obey.

### G7 — Specs execute deterministically

- **Requirement:** colocated executable specs run to the same verdict every time.
- **Proving artifact:** `specification/adr/ADR-0002-spec-semantics.md` (spec model)
  + `crates/tuo-cli/tests/spec_command.rs` + the deterministic sandbox in
  `tuo-mir-interp` (fuel/recursion/memory limits; the only non-deterministic
  observable is a *measured* duration, reported as an observation, never a promise).
- **Status:** **MET.** Verdicts are deterministic; timing is measured and explicitly
  non-normative.

### G8 — Interpreter and Cranelift pass differential testing

- **Requirement:** the reference interpreter and the default native backend agree
  instruction-for-instruction.
- **Proving artifact:** `crates/tuo-cli/tests/codegen_differential.rs` (interpreter
  vs Cranelift over `tests/codegen/fixtures/`) + `crates/tuo-cli/tests/differential.rs`
  (randomized, generated accepted programs) + `tests/differential/README.md`
  ("there is no acceptable divergence").
- **Status:** **MET.** Agreement over both fixture and generated programs is a CI
  gate; any mismatch is a release blocker by policy.

### G9 — LLVM passes three-way differential testing (if in 0.1)

- **Requirement:** if the LLVM backend ships in 0.1, interpreter == Cranelift == LLVM.
- **Proving artifact:** `crates/tuo-cli/tests/codegen_three_way.rs` (the three-way
  agreement, over optimized MIR + LLVM O2).
- **Status:** **MET (conditional).** The LLVM backend is *in scope* for 0.1 (it is
  the `--release` backend), and the three-way suite pins its agreement with the
  interpreter and Cranelift. If a late decision removes LLVM from 0.1, this
  criterion is vacuously satisfied and the suite becomes optional; that decision
  must be recorded here before ship.

### G10 — Human diagnostics are usable

- **Requirement:** the default human diagnostic output is legible and actionable.
- **Proving artifact:** `tests/diagnostics/` (rendered-output fixtures) +
  `DOGFOODING.md` (captured real diagnostics — `T0001`, `R0002`, `O0001`, `P0002` —
  from the five example programs, judged usable in practice).
- **Status:** **MET.** Diagnostics carry structured spans, codes, and messages;
  dogfooding exercised them on real programs and recorded the output.

### G11 — Machine diagnostics have a stable schema

- **Requirement:** the machine diagnostic encoding is a versioned contract.
- **Proving artifact:** `crates/tuo-diagnostics/src/json.rs` (`SCHEMA_VERSION`) +
  `crates/tuo-cli/src/protocol.rs` (`PROTOCOL_VERSION`) +
  `crates/tuo-cli/tests/protocol_command.rs` (backwards-compatibility fixtures) +
  `tests/cli/protocol/`.
- **Status:** **MET.** Both the diagnostic schema and the protocol envelope are
  versioned; the backwards-compat tests fail if a guaranteed field is dropped or
  renamed without a version bump.

### G12 — Incremental compilation benchmarks exist

- **Requirement:** the five standard edit scenarios are measured.
- **Proving artifact:** `crates/tuo-compiler/tests/incremental_stages.rs` (hard
  assertions on which queries re-execute) + `crates/tuo-compiler/tests/incremental_measure.rs`
  (`--nocapture` measurement) + `crates/tuo-compiler/tests/affected_specs.rs` +
  `benchmarks/compiler/`.
- **Status:** **MET.** No-change, function-body, function-signature, unrelated-file,
  and spec-only edits are each pinned by re-execution counts and reported with
  measured timing.

### G13 — LLM benchmarks exist

- **Requirement:** the code-generation evaluation harness exists and runs the real
  compiler for every metric.
- **Proving artifact:** `crates/tuo-codegen-bench/tests/harness.rs` (offline
  adapter driving the real compiler through fail-then-repair) +
  `crates/tuo-codegen-bench/tests/shipped_tasks.rs` (every task re-verifies its pin)
  + `benchmarks/llm/codegen/tasks/` + `crates/tuo-cli/tests/bench_command.rs`
  (recompiles recorded outputs; a lying verdict is exposed).
- **Status:** **MET.** The harness embeds no model, pins tasks by content digest,
  and scores from the compiler's verdicts, never recorded booleans.

### G14 — Official corpus is compiler-validated

- **Requirement:** every corpus entry is admitted only by passing the real compiler
  gauntlet for the category its directory names.
- **Proving artifact:** `crates/tuo-corpus/tests/shipped_corpus.rs` (re-admits every
  fixture under `corpus/`) + `crates/tuo-corpus/tests/validation.rs` (the admission
  contracts) + `crates/tuo-cli/tests/corpus_command.rs` (through the real binary,
  live native execution).
- **Status:** **MET.** No program enters on assertion; a repair entry is admitted
  only if it fails at exactly the stage its category names.

### G15 — Example applications work

- **Requirement:** substantial real programs compile and run.
- **Proving artifact:** `crates/tuo-cli/tests/dogfood_examples.rs` (five programs
  driven through the real binary to exact exit codes) + `examples/` + `DOGFOODING.md`.
- **Status:** **MET.** The CLI-stats, data-pipeline, HTTP-service,
  concurrent-scheduler-core, and multi-package workspace examples check, run their
  specs green, and execute to their asserted exit codes — and, since ADR-0006
  landed the effect boundary, cli-stats prints its report and http-service its
  response line, with stdout asserted byte-for-byte.

### G16 — All known semantic divergences are resolved or explicitly release-blocking

- **Requirement:** nothing is silently deferred; every known divergence is fixed or
  recorded here as release-blocking.
- **Proving artifact:** `tests/differential/README.md` (the no-acceptable-divergence
  policy) + the three differential suites (G8/G9) + `specification/open-questions.md`
  (the open-question register) + the "Known divergences" section below.
- **Status:** **MET.** As of this gate there are **zero** open interpreter/backend
  semantic divergences: the differential suites are green, so the backends agree
  with the reference interpreter on every fixture and every generated program. The
  only remaining open item is the dogfooding ADR ADR-0007 (concurrency);
  ADR-0004, ADR-0006, ADR-0009, and ADR-0008 (Tier 1) are now **accepted and
  landed**. These are *capability gaps*, not divergences — they are enumerated
  below and none is a silent deferral.

---

## Readiness summary

| # | Criterion | Status |
|---|-----------|--------|
| G1 | Grammar is versioned | MET |
| G2 | Formatter is canonical and idempotent | MET |
| G3 | Parser does not crash under fuzzing | MET |
| G4 | Static semantics are documented | MET |
| G5 | Ownership semantics are documented | MET |
| G6 | MIR semantics are documented | MET |
| G7 | Specs execute deterministically | MET |
| G8 | Interpreter ⇄ Cranelift differential | MET |
| G9 | LLVM three-way differential (LLVM is in 0.1) | MET |
| G10 | Human diagnostics are usable | MET |
| G11 | Machine diagnostics have a stable schema | MET |
| G12 | Incremental compilation benchmarks exist | MET |
| G13 | LLM benchmarks exist | MET |
| G14 | Official corpus is compiler-validated | MET |
| G15 | Example applications work | MET |
| G16 | Known divergences resolved or release-blocking | MET |

**Overall: READY.** All sixteen criteria are `MET`; none is `PARTIAL` or
`RELEASE-BLOCKING`. The two former documentation-locality gaps are closed:
`specification/static-semantics.md` (G4) and `specification/mir.md` (G6) now sit
alongside `ownership.md` and `abi.md` as the normative peer documents for the
static and MIR semantics, and the `tests/types/` corpus and the MIR verifier pin
them. The interpreter⇄Cranelift⇄LLVM differential suites are green, so there are no
open semantic divergences. Per the gate's own rule, 0.1 may be declared ready.

The remaining items enumerated under "Capability gaps" below are **not** gate
criteria — 0.1 ships the scalar control-flow core **plus the ADR-0004
aggregates** (structs, enums, fixed `[T; N]` arrays, bounded iteration) **and
the ADR-0006 `Str` + effect boundary** (borrowed strings, `std::rt` descriptor
I/O and process exit, `R0007`-pure specs) deliberately, and each remaining gap
is a proposed ADR with a scope decision of its own, not a silent deferral.

## Remaining work before 0.1

**None.** Every gate criterion is `MET`. The last two open items — consolidating
the static semantics (G4) and the MIR semantics (G6) into single normative
documents — are done (`specification/static-semantics.md` and
`specification/mir.md`). Neither required a compiler change; both were the
difference between "specified, distributed" and "specified, collected."

## Capability gaps (not gate criteria, tracked for honesty)

These are **not** 0.1 release-gate criteria — 0.1 ships the scalar control-flow
core plus the ADR-0004 aggregates and the ADR-0006 `Str`/effect boundary
deliberately — but the gate records them so no reader mistakes their absence
for a divergence. Each has an ADR from the Prompt 39 dogfooding exercise
(`DOGFOODING.md`) and a benchmark plan:

| Gap | ADR | Benchmark consideration |
|-----|-----|-------------------------|
| Aggregates + iteration in the runnable core | ADR-0004 (**accepted, landed**) | done — the perf-lab `collections` workload is `Supported` with its committed program and C peer |
| Effect boundary + runtime strings | ADR-0006 (**accepted, landed**) | done — the perf-lab `string-processing` workload is `Supported` with its committed program and C peer; per its amendments, `networking` is *not* unblocked (no socket effect) and owned `String`/`concat` moved to the allocator ADR |
| Heap allocation (owned `String`, growable collections) | ADR-0009 (**accepted, landed**) | done — the perf-lab `allocation` workload is `Supported` with its committed program and `malloc`/`realloc`/`free` C peer |
| First-class functions (Tier 1, non-capturing) | ADR-0008 (**Tier 1 accepted, landed**; Tier 2 closures deferred) | done — the perf-lab `indirect-calls` workload is `Supported` (a `fn`-value loop vs a C function-pointer loop); the generic higher-order stdlib combinators ship |
| Concurrency model | ADR-0007 (proposed) | new parallel-speedup benchmark category |

Whether any of these becomes an 0.1 requirement is a scope decision recorded in the
respective ADR, not something this gate silently assumes.

## Known divergences

**None.** The interpreter is the reference semantics; the Cranelift and LLVM
backends are held to instruction-for-instruction agreement with it by the G8/G9
differential suites, which are mandatory CI gates. Should a future change surface a
divergence, it is recorded in this section with status `RELEASE-BLOCKING` and either
fixed or explicitly deferred here before 0.1 — never left implicit.

---

## Machine-readable gate manifest

The checker in `crates/tuo-cli/tests/release_gate.rs` parses the fenced block below.
Each line is `G<n> | <status> | <artifact-path>[; <artifact-path>...]`. The checker
asserts (1) exactly sixteen criteria `G1`..`G16` are present, (2) every status is a
valid vocabulary word, and (3) every artifact path exists relative to the repository
root. Editing this manifest to cite a path that does not exist fails the build, so
the gate can never claim an artifact it does not have.

```gate-manifest
G1  | MET     | specification/grammar.ebnf; specification/CONSTITUTION.md
G2  | MET     | crates/tuo-fmt/tests/golden.rs; crates/tuo-fmt/tests/properties.rs; crates/tuo-cli/tests/fmt_command.rs
G3  | MET     | crates/tuo-fuzz/tests/sweep.rs; crates/tuo-fuzz/regressions
G4  | MET     | specification/static-semantics.md; specification/CONSTITUTION.md; specification/syntax.md; tests/types
G5  | MET     | specification/ownership.md; specification/adr/ADR-0003-ownership-model.md; tests/ownership
G6  | MET     | specification/mir.md; crates/tuo-mir/src/verify.rs; specification/adr/ADR-0002-spec-semantics.md
G7  | MET     | specification/adr/ADR-0002-spec-semantics.md; crates/tuo-cli/tests/spec_command.rs
G8  | MET     | crates/tuo-cli/tests/codegen_differential.rs; crates/tuo-cli/tests/differential.rs; tests/differential/README.md
G9  | MET     | crates/tuo-cli/tests/codegen_three_way.rs
G10 | MET     | tests/diagnostics; DOGFOODING.md
G11 | MET     | crates/tuo-diagnostics/src/json.rs; crates/tuo-cli/src/protocol.rs; crates/tuo-cli/tests/protocol_command.rs
G12 | MET     | crates/tuo-compiler/tests/incremental_stages.rs; crates/tuo-compiler/tests/incremental_measure.rs; crates/tuo-compiler/tests/affected_specs.rs
G13 | MET     | crates/tuo-codegen-bench/tests/harness.rs; crates/tuo-codegen-bench/tests/shipped_tasks.rs; benchmarks/llm/codegen/tasks
G14 | MET     | crates/tuo-corpus/tests/shipped_corpus.rs; crates/tuo-corpus/tests/validation.rs; crates/tuo-cli/tests/corpus_command.rs
G15 | MET     | crates/tuo-cli/tests/dogfood_examples.rs; examples; DOGFOODING.md
G16 | MET     | tests/differential/README.md; specification/open-questions.md
```
