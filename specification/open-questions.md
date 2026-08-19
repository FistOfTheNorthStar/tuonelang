# tuonelang open design questions

A running register of unresolved language- and compiler-design questions. This
is a working document: questions are added as they arise and removed (or
migrated into an ADR and the Constitution) once resolved.

These are the questions left open by the v0 Constitution
(`CONSTITUTION.md`) — specifically the items it marks **Experimental** or
**Deferred**. The Constitution's **Frozen** decisions are *not* open questions;
changing them is a breaking change requiring a new edition and an ADR (§30).

## How to use this document

Each open question gets an entry using the template below. When a question is
resolved, record the decision as an ADR under `adr/`, fold the outcome into the
`CONSTITUTION.md` and/or `grammar.ebnf`, and delete the entry here (its history
remains in version control).

### Template

```
### Q-NNNN: <short title>

- **Status:** open | under discussion | resolved (→ ADR-XXXX)
- **Area:** lexical | types | ownership | diagnostics | MIR | tooling | …
- **Constitution ref:** §N
- **Question:** <what must be decided>
- **Why it matters:** <impact on the language, users, or LLM reliability>
- **Options considered:** <if any>
- **Blocks:** <what cannot proceed until this is resolved>
```

## Open questions

### Q-0001: Surface syntax for `spec` (`given`/`when`/`then`)

- **Status:** open
- **Area:** lexical / tooling
- **Constitution ref:** §27
- **Question:** What is the exact concrete syntax of a `spec` block? The
  *semantics* (executable on shared MIR, pure, deterministic) are frozen; the
  keywords and shape of `given`/`when`/`then` are not.
- **Why it matters:** Specs are the defining TDG surface; agents will generate
  them constantly, so the syntax must be maximally regular and unambiguous.
- **Options considered:** the illustrative `given/when/then` shown in §27; an
  attribute-plus-function form; a table/example-driven form.
- **Blocks:** freezing the spec grammar in `grammar.ebnf`; the `tuo-spec` crate.

### Q-0002: Spelling of dynamic dispatch and associated items

- **Status:** open
- **Area:** types
- **Constitution ref:** §19
- **Question:** How is a dynamically-dispatched interface object spelled
  (`Box[dyn I]` vs. an alternative), and does v0 include associated
  types/constants on interfaces?
- **Why it matters:** Determines how erased polymorphism and interface-relative
  types are written; affects the type checker and generics design.
- **Blocks:** finalizing §18/§19 grammar and the type-system crate design.

### Q-0003: Error-type conversion across `?`

- **Status:** open
- **Area:** types / diagnostics
- **Constitution ref:** §20
- **Question:** When `?` propagates an `Err(e: E1)` into a function returning
  `Result[_, E2]`, how does `E1` convert to `E2` — an interface-based `From`-
  style conversion, exact-match only, or explicit conversion required?
- **Why it matters:** Governs how ergonomic and how explicit error propagation
  is; over-implicit conversion hurts clarity, over-explicit hurts ergonomics.
- **Blocks:** freezing §20; error-handling stdlib design.

### Q-0005: `Shared` atomicity (atomic-only vs. atomic + non-atomic sibling)

- **Status:** open
- **Area:** ownership / runtime
- **Constitution ref:** §25
- **Question:** Is `Shared[T]` always atomically reference-counted, or is there a
  non-atomic single-thread sibling for performance?
- **Why it matters:** Atomic-only is simpler and race-safe by construction but
  costs performance in single-threaded use; a sibling adds a misuse risk.
- **Blocks:** the runtime/stdlib design of the ownership wrappers.

### Q-0006: Named overflow-operation spellings

- **Status:** open
- **Area:** types / stdlib
- **Constitution ref:** §24
- **Question:** The exact names/forms of opt-in wrapping/saturating/checked
  arithmetic (`wrapping_add`, `checked_add`, …) and whether they are methods,
  free functions, or operators with modifiers.
- **Why it matters:** These are the *only* escape from trapping overflow, so they
  must be discoverable and hard to use by accident.
- **Blocks:** freezing §24's surface; numeric stdlib.

### Q-0007: Reproducible-build mechanisms

- **Status:** open
- **Area:** MIR / tooling
- **Constitution ref:** §28
- **Question:** The concrete content-addressing/hashing scheme for MIR and the
  exact build-configuration surface that participates in reproducibility.
- **Why it matters:** Determinism is guaranteed (frozen); the *mechanism* that
  delivers it must be pinned down before caching and spec-identity features.
- **Blocks:** MIR representation design; spec-identity hashing (§27).

### Q-0008: Directory/parent-module convention and relative paths

- **Status:** open
- **Area:** lexical / resolution
- **Constitution ref:** §6, §7
- **Question:** The exact parent-module filename (`mod.tuo` vs. alternatives) and
  whether relative import prefixes (`super`, `self`) are added.
- **Why it matters:** Affects how large packages are laid out and how portable a
  code snippet is between locations.
- **Blocks:** finalizing the module/resolution design in `tuo-resolve`.

### Q-0009: Intrinsic set behind `unsafe`

- **Status:** open
- **Area:** runtime / stdlib
- **Constitution ref:** §26
- **Question:** Which exact operations/intrinsics `unsafe` unlocks (raw pointer
  ops, FFI calling convention, memory intrinsics).
- **Why it matters:** Defines the audited safety boundary and the FFI surface.
- **Blocks:** stdlib low-level layer; FFI design.

### Q-0010: Deferred type-system features (range/`ref` patterns, tuple structs, const generics, HKT)

- **Status:** open (deferred from v0)
- **Area:** types
- **Constitution ref:** §12, §17, §18
- **Question:** Whether and when to admit tuple/positional structs, range and
  `ref` patterns, const generics, and higher-kinded types in a future edition.
- **Why it matters:** These are intentionally excluded from v0 to keep it small;
  each is a candidate for a later edition if experience justifies it.
- **Blocks:** nothing in v0 — tracked so the exclusions are not forgotten.

### Q-0011: User-written destructors

- **Status:** open (deferred from v0 by ADR-0003)
- **Area:** ownership / stdlib
- **Constitution ref:** §21, §26
- **Question:** How user types hook resource cleanup — a compiler-known `Drop`
  interface, its receiver mode, what a destructor body may do (moves out of
  `self`, panics), and how it interacts with partial moves.
- **Why it matters:** RAII for user-defined resources needs it eventually; v0
  drop glue is compiler-generated only, so dropping can never run user code.
- **Blocks:** stdlib resource types beyond compiler-known ones; any user RAII
  guard pattern.

### Q-0012: `String` → `Str` view borrowing

- **Status:** addressed by [ADR-0010](adr/ADR-0010-string-to-str-view.md)
  (proposed) — resolves once that ADR's Stages A–C land.
- **Area:** ownership / types
- **Constitution ref:** §10, §21
- **Question:** The sound rule that lets a `Str` view borrow into a live
  `String` without user lifetimes — in v0 every `Str` originates from a string
  literal, so views cannot dangle by construction.
- **Why it matters:** Reading string data without copying is basic ergonomics;
  the lifetime-free ownership model needs a view-tracking rule before allowing
  it.
- **Blocks:** the `String`/`Str` stdlib API surface.
- **Proposed answer (ADR-0010):** an `std::string::as_str(in String) -> Str`
  builtin whose returned view is **borrow-scoped** — a `Str` derived from a
  `String` is treated as a shared borrow of that `String`, so moving, mutating,
  dropping the `String` while a derived view is live is the existing
  use-of-moved / borrow-conflict family (`O0001`/`O0002`/`O0005`), and
  returning-a-view-across-a-frame is the new appended code `O0011`. No lifetimes,
  no layout change (the `Str` fat pointer is the `String` header's `{ptr, len}`
  prefix); the checker gains its first value-provenance borrow.
