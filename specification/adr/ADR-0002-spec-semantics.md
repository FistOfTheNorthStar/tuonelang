# ADR-0002: First-class `spec` semantics

- **Status:** accepted
- **Date:** 2026-07-20
- **Context:** tuonelang's distinguishing goal is colocated, executable
  specifications. The syntax (`spec name { given …; when …; then …;
  assert …; }`) has existed since the parser, and resolution attaches
  identifier-named specs to function symbols, but the *semantics* were never
  pinned down: how specs name their targets, whether several may target one
  function, what assertions are allowed, whether specs are visible or shipped,
  and how a spec runner discovers what a spec needs. These decisions shape the
  MIR interpreter's spec runner, `tuo check`, and eventually `tuo spec`, so
  they must be fixed before execution machinery exists.
- **Decision:** the rules below. They are implemented today in resolution
  (`tuo-resolve`), type checking (`tuo-types`), and `tuo check`; execution is
  deliberately deferred until the MIR interpreter exists.

## Naming and attachment

A spec is named either by an **identifier** or by a **string**:

- `spec fibonacci { … }` — the identifier is a *target reference*, not a
  binding. It resolves in the enclosing module scope to a module-level
  **function** — the very same symbol calls to `fibonacci` resolve to. The
  spec is *attached* to that function.
- `spec "parses empty input" { … }` — a string-named spec is
  **free-standing**: it has no target and the string is a human description.

Because a spec's name is never a binding, `spec classify` and `fn classify`
never collide, and no expression or import can refer to a spec.

A target that resolves to a non-function is an error (`R0006`); a target that
resolves to nothing is an error (`R0002`, with a hint to use a string name for
free-standing specs).

## Multiplicity and duplicates

- **Multiple specs may attach to one function.** They accumulate;
  `specs_for(function)` returns all of them in source order (file order, then
  declaration order within a file).
- **Duplicate specs are permitted and distinct.** A spec's identity is the
  block itself (its stable `SymbolId`), never its name: two `spec fibonacci`
  blocks are two specs of `fibonacci`, and two string-named specs may repeat
  the same description. Nothing merges, shadows, or errors. Tooling that needs
  to distinguish repeated names uses the spec's ID and span.

## Allowed assertions

- `then` and `assert` clauses take **any expression of type `Bool`** — the
  full expression language (calls, matches, operators, …). A non-`Bool`
  expectation is a type error.
- `given` introduces typed bindings (its declared type is checked against the
  initializer); `when` runs a statement; plain `let`/`var`/expression
  statements are also allowed. All of them are resolved and type-checked
  exactly like function-body code.
- No purity or effect restriction is imposed yet; if the language grows an
  effect system, spec expectations are expected to be restricted to it.

## Visibility

Specs are **never `pub`**: they cannot be imported, re-exported, or named from
any code. They are private to their module as far as the language is
concerned, but *tooling* enumerates them program-wide through the resolution
APIs — visibility hides them from programs, not from the toolchain.

## Compilation and release

- Specs are **parsed, resolved, and type-checked in every compilation**,
  including `tuo check`. A program with a broken spec does not check.
- Specs **do not ship in release binaries**. They exist to be *executed* by
  the spec runner (the MIR interpreter, and later `tuo spec`); codegen for
  release artifacts excludes spec bodies entirely.
- Specs **do not execute yet**: execution semantics arrive with the MIR
  interpreter. Until then the whole pipeline treats specs as checked,
  non-executable declarations.

## Dependency discovery

`dependencies_of(spec)` is the set of module-level items a spec's body
references — its target function plus every function, struct, enum, enum
variant, interface, and constant used anywhere in the block — deduplicated, in
first-use order. Locals, parameters, type parameters, and items declared
*inside* the spec block are not dependencies. This set is the seed of the
compilation closure a spec runner must lower before executing the spec.
Dependencies are computed during resolution, so they are available to every
later stage without re-walking syntax.

## Source order independence

Attachment and dependency discovery run after whole-program declaration
collection, so results never depend on declaration order: a spec may precede
or follow its target in the same file, or live in a different file of the same
module, with identical outcomes.

## Semantic API

The stable query surface lives on `tuo_resolve::Resolution`:

- `specs_for(function: SymbolId) -> Vec<SymbolId>` — attached specs, source
  order.
- `target_of(spec: SymbolId) -> Option<SymbolId>` — the attached function;
  `None` for free-standing or unresolved specs.
- `dependencies_of(spec: SymbolId) -> &[SymbolId]` — the dependency set above.

- **Consequences:** the spec runner can be built against a fixed contract:
  it enumerates specs, follows `dependencies_of` to know what to lower, and
  never worries about name capture (spec names bind nothing). Allowing
  duplicate/multiple specs keeps authoring friction low but means reporting
  must key on IDs and spans, not names. Excluding specs from release binaries
  commits codegen to a spec-stripping build mode. Deferring effects means
  assertions may perform arbitrary side effects for now; tightening later may
  break existing specs.
