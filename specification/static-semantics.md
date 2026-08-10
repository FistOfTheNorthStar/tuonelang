# The tuonelang static semantics (v0)

> **Status.** Normative. This document specifies the **static semantics** of
> tuonelang v0 — name resolution and type checking: the rules a program must
> satisfy *after* it parses and *before* the ownership checker (which has its own
> normative document, [`ownership.md`](ownership.md)) runs. It **consolidates**
> rules already frozen in the [Constitution](CONSTITUTION.md) (§§8–24), the
> syntactic/semantic boundary drawn in [`syntax.md`](syntax.md), and the checking
> rules documented on the [`tuo-resolve`](../crates/tuo-resolve) and
> [`tuo-types`](../crates/tuo-types) crates. It introduces **no new semantics**:
> where this prose and a crate (or the Constitution) disagree, the crate — and the
> tests that pin it (`tests/types/`) — win, and this file is corrected to match.
> It is a peer of [`ownership.md`](ownership.md), [`mir.md`](mir.md), and
> [`abi.md`](abi.md).

The static semantics are computed in two stages, each a distinct crate with a
distinct diagnostic namespace:

1. **Name resolution** ([`tuo-resolve`](../crates/tuo-resolve)) — turns every
   declaration and every name occurrence into a stable semantic **symbol**, with
   **no type information** (`Rxxxx` diagnostics, §2).
2. **Type checking** ([`tuo-types`](../crates/tuo-types)) — verifies the frozen
   type system over the resolved program (`Txxxx` diagnostics, §3).

The concrete grammar deliberately accepts a **superset** of valid programs
([`syntax.md`](syntax.md)): "we do not pretend type checking is part of the
context-free grammar." A program that parses may still be rejected here. §4 is the
authoritative index of *which stage owns which rule*.

---

## 1. What is static and what is not

The static semantics decide everything about a program's well-formedness that does
**not** require running it, **except** memory safety, which
[`ownership.md`](ownership.md) owns. Concretely:

- Resolution and typing are **static**: name/import resolution, type existence and
  scope, inference, operand compatibility, exhaustiveness, generic-bound
  satisfaction, `?` placement, definite assignment.
- **Integer-overflow trapping is a _runtime_ guarantee** (Constitution §24),
  enforced by the interpreter and codegen — *not* by static checking. It is noted
  here only because the type system fixes *which* operations trap (§3.4); the trap
  itself is a MIR/runtime event ([`mir.md`](mir.md) §5, §8).
- **Ownership** (moves, borrows, use-after-move, the borrow rule) is a separate
  static stage with its own document.

---

## 2. Name resolution

`resolve` takes one program snapshot (every parsed file as typed AST) and produces
a `Resolution`:

- a **symbol table** of every declared entity, each with a stable semantic
  `SymbolId` rather than an AST pointer;
- a **reference map** from every resolved name occurrence (its exact source span)
  to its symbol — the substrate for rename, go-to-definition, and
  find-all-references;
- **spec attachments**: an identifier-named `spec fibonacci { … }` resolves to the
  *same* function symbol that calls to `fibonacci` resolve to.

**No type information lives in resolution.** Names whose meaning depends on types —
**field accesses, method calls, `Self`, associated path tails** — are deliberately
skipped, never guessed at; the type checker owns them.

### 2.1 Symbol kinds

Every declared entity is one `SymbolKind` (the parenthesized noun is what
diagnostics use):

| Kind | Noun | Notes |
|------|------|-------|
| `Module` | module | |
| `Function` | function | |
| `Struct` | struct | |
| `Enum` | enum | |
| `Variant` | enum variant | inherits its enum's visibility |
| `Interface` | interface | |
| `Const` | constant | |
| `Spec` | spec | |
| `TypeParam` | type parameter | |
| `Param` | parameter | **including the `self` receiver** |
| `Local` | local binding | covers `let`, `var`, pattern bindings, `given` bindings; never `pub` |

Each symbol records its name, kind, owning module, a `pub` flag, and its
declaration span (`None` for entities with no written declaration, such as the root
module).

### 2.2 Scoping rules (frozen)

- **Module-level declarations are collected before any use is resolved**, so
  **forward references** across items and across files always work (Constitution
  §8's "a call site resolves to exactly one function by name" holds regardless of
  order).
- **Lexical scoping is sequential inside bodies:** a `let`/`var` **shadows** from
  its binding onward, and its initializer still sees the outer name. Shadowing
  within a scope is allowed (Constitution §9).
- A receiver (`self`) may appear **only** as the first parameter of an `impl`
  method (Constitution §19).

### 2.3 Resolution diagnostics (`Rxxxx`)

| Code | Meaning |
|------|---------|
| `R0001` | Duplicate definition. |
| `R0002` | Undefined name. |
| `R0003` | Unresolved import. |
| `R0004` | Ambiguous name. |
| `R0005` | Visibility violation. |
| `R0006` | Non-function spec target (a `spec` naming something that is not a function). |

---

## 3. Type checking

`check` takes one resolved program snapshot (the parsed files plus the
`Resolution`) and verifies the Constitution's core type system. **Type equality is
exact and structural**: nominal types are identified by stable `SymbolId`, never by
name or AST position, so `I32` never equals `I64` and `String` never equals `Str`.

### 3.1 The type universe

The surface primitive set is frozen (Constitution §10):

- **Signed integers** `I8 I16 I32 I64 Isize`; **unsigned** `U8 U16 U32 U64 Usize`.
  `Isize`/`Usize` are pointer-width and are the **index/length type**.
- **Floats** `F32 F64`.
- `Bool`; `Char` (a Unicode scalar value); `String` (owned UTF-8); `Str` (borrowed
  UTF-8 slice); `()` (unit, one value).
- **`Int` is a type alias for `I64`; `Float` for `F64`** (Constitution §11), and
  they are the inference defaults for an otherwise-unconstrained literal.

The checker realizes this surface set plus a small number of **checker-internal**
types that give the frozen rules a home; none is a new surface type:

- **`Never`** — the type of `return`/`break`/`continue`/an endless `loop`;
  **unifies with every type** (the realization of Constitution §16's control-flow
  rule).
- **`Tuple`** — supports `.N` field access in the core but has **no v0 surface
  syntax** (tuple structs are excluded, §12).
- **`Range`** — the internal type of `a .. b`, iterable by `for` (§16).
- **`Array[T]`** — the builtin homogeneous array, **indexed by `Usize`**.
- **`[T; N]`** (`[EXPERIMENTAL]`, ADR-0004 Stage 2) — the inline fixed-length
  array; `N` elements of `T`, the length part of the type, indexed by `Usize`
  and iterable by `for`. Lengths are concrete `u64`s — there are **no length
  variables and no length inference**: `[Int; 3]` and `[Int; 4]` are distinct
  types, and a length mismatch is an ordinary `T0001` rendering both types. The
  list literal `[a, b, c]` has its element count as the length (elements unify
  against one fresh element variable, mismatches reported at the offending
  element; an unconstrained `[]` needs an annotation, `T0011`); the repeat
  literal `[x; N]` duplicates a **`Copy`** operand (`O0010` otherwise). The
  written length is a plain `INT_LITERAL` — decimal/`0x`/`0o`/`0b`, no type
  suffix — parsed to `u64` and capped at `MAX_FIXED_ARRAY_LEN = 65_536`
  (`T0014` on violation). `[T; N]` never enters name resolution; `Array` stays
  the growable heap sequence.
- **`Option[T]` / `Result[T, E]`** — the canonical stdlib enums (§14, §15).
- **`Struct` / `Enum`** (by `SymbolId`), **`Wrapper`** (`Box`/`Shared`/`Weak`, §25),
  **`Param`** (a generic type parameter, §18), **function types**.
- **`Var`** — an unsolved inference variable (renders as `{integer}`, `{float}`, or
  `_` by class).
- **`Error`** — the **poison type**, produced after an error and **unifying with
  everything**, so one mistake is reported once, not cascaded.

### 3.2 Signatures and inference (frozen)

- **Explicit signatures (§8):** parameter and return types are **never inferred**;
  an absent `->` means the function returns `()`. There is no parameter type
  inference, no default parameters, no variadics, no overloading — a call resolves
  to exactly one function by name.
- **Local inference by unification** where unambiguous: `let`/`var` bindings,
  numeric literals (defaulting to `I64`/`F64`), and generic instantiations.
  Anything still unknown afterwards is **"type annotation needed"** (`T0011`),
  **never a guess** (§9, §11).
- **Definite assignment:** no binding may be read before it is assigned; a binding
  is either initialized at declaration or definitely-assigned before first use
  (§9). `const` and any initializer-less binding require an explicit annotation.

### 3.3 No implicit conversions (frozen)

There are **no implicit conversions**, numeric or otherwise (Constitution §10).
`I32` and `I64` meet **only** through an explicit `as` cast, and `as` is
**numeric-only** — any other cast is `T0008`. This is the load-bearing determinism
rule of the type system.

### 3.4 What the checker verifies

Collection lowers every module-level signature first (so bodies can call forward in
any order), then body checking walks each function, constant, and spec with a fresh
inference context. It verifies:

- call checking (**arity + argument types**), return checking, operator checking;
- field access (structs and tuple indices), struct-literal and struct-pattern field
  sets;
- casts (numeric-only, §3.3);
- `?` propagation — permitted **only** in a function whose return type is a
  compatible `Result` (or `Option`, for `None`-propagation), §20;
- **exhaustiveness** foundations for `match` over enums, `Option`/`Result`, and
  `Bool` (§16, §17), reporting the **specific missing cases**;
- generic arity and instantiation (§18).

Every mismatch diagnostic carries **structured expected and actual types** (not
just a rendered string), so tools can consume them precisely.

**Deferred to the trait system, never guessed:** method calls, interface bounds,
`Self` types, and interface-member bodies **poison to `Ty::Error`** (which unifies
with everything) rather than producing speculative diagnostics. This is a
deliberate honesty boundary — the core type system does not pretend to check what
the trait system will own.

### 3.5 Type diagnostics (`Txxxx`)

| Code | Meaning |
|------|---------|
| `T0001` | Mismatched types (carries structured expected/actual). |
| `T0002` | Wrong number of arguments. |
| `T0003` | Call of a non-function. |
| `T0004` | Unknown field. |
| `T0005` | Struct-literal / struct-pattern field-set error. |
| `T0006` | Operation not supported for a type. |
| `T0007` | Non-exhaustive match. |
| `T0008` | Invalid cast. |
| `T0009` | Invalid `?` propagation. |
| `T0010` | Wrong number of type arguments. |
| `T0011` | Type annotation needed. |
| `T0012` | Expected a value/constructor, found something else. |
| `T0013` | `break`/`continue` outside a loop. |
| `T0014` | Invalid fixed-size array length: suffixed, unparsable, or over `MAX_FIXED_ARRAY_LEN` (ADR-0004 Stage 2). |

Ownership-flavored array rules live in `specification/ownership.md`: indexing
reads an element out of `Array[T]` **and** `[T; N]` alike (only `Copy`
elements by value, `O0007`), and the repeat literal `[x; N]` requires a `Copy`
element (`O0010`).

---

## 4. Ownership of each rule (the syntax/semantics boundary)

This table (from [`syntax.md`](syntax.md)) is the authoritative index of which
stage enforces which restriction. Everything below the parser row is a *semantic*
rule the grammar deliberately does not decide.

| Restriction | Enforced by | Constitution |
|-------------|-------------|--------------|
| Name / path / import resolution; type exists & in scope | resolver | §6, §7, §10 |
| Type checking & inference; operand type compatibility | type checker | §8–§18 |
| Numeric literal fits its type; no implicit conversion | type checker | §10, §24 |
| `let`/`var` definite assignment before use | type checker | §9 |
| `match` exhaustiveness & arm reachability | type checker | §17 |
| Generic bound satisfaction; monomorphization | type checker | §18 |
| Interface coherence (one impl per type) & orphan rules | resolver / typer | §19 |
| `?` used only in a compatible `Result`/`Option` function | type checker | §20 |
| Ownership: move/borrow, use-after-move, the borrow rule | ownership checker | §21, §22 |
| No `null`; every `T` is a valid initialized `T` | type + ownership | §23 |
| Integer overflow traps (runtime / interp) | codegen / interp | §24 |
| `Weak` must be upgraded to `Option` before use | type checker | §25 |
| `unsafe`-only operations confined to `unsafe` blocks | type checker | §26 |
| Spec purity / determinism; runs on shared MIR | spec runner | §27 |
| Receiver (`self`) only as first parameter of an `impl` method | resolver | §19 |
| Struct-literal-in-condition restriction | parser | `syntax.md` |

The last row is the **only** restriction the parser itself enforces beyond pure
shape; every other row is resolution, typing, ownership, or a runtime stage.

---

## 5. What pins these rules

The static semantics are executable: `tests/types/` is the fixture corpus that
holds this document honest.

- `tests/types/fixtures/ok/` — programs that must resolve **and** type-check with
  **zero diagnostics** (`adts`, `arrays`, `casts`, `control_flow`, `functions`,
  `impls`, `inference`, `option_result`, `specs`).
- `tests/types/fixtures/err/` — programs that resolve cleanly but contain type
  errors; their diagnostics (code, span, message, structured expected/actual) are
  snapshotted under `tests/types/snapshots/`. The err stems map onto the code
  groups above: `mismatch` → `T0001`, `arity` → `T0002`, `constructors` →
  `T0005`/`T0010`, `fields` → `T0004`, `operators` → `T0006`, `exhaustive` →
  `T0007`, `casts` → `T0008`, `fallibility` → `T0009`, `annotations` → `T0011`,
  `loops` → `T0013`.

Every code `T0001`..`T0013` is exercised by the snapshot corpus (harness
`crates/tuo-types/tests/fixtures.rs`; re-bless with
`TUO_BLESS=1 cargo test -p tuo-types --test fixtures`). Resolution diagnostics are
pinned by `tuo-resolve`'s own suites.

---

## Cross-references

- **Constitution §§8–24** — the frozen type system these rules realize.
- **[`syntax.md`](syntax.md)** — the syntax/semantics boundary and the ownership
  table (§4 above).
- **[`ownership.md`](ownership.md)** — the memory-safety static stage that runs
  after typing.
- **[`mir.md`](mir.md)** — where accepted programs acquire their executable meaning
  (overflow trapping lives there, §3.4 / §1).
- Peer normative documents: [`ownership.md`](ownership.md), [`mir.md`](mir.md),
  [`abi.md`](abi.md).
