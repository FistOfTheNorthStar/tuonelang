# tuonelang v0 — Syntax (Concrete Grammar)

> **Normative.** This document describes the **concrete (syntactic) grammar** of
> tuonelang v0 — how a **token stream** (produced by
> [`lexical-grammar.md`](lexical-grammar.md)) becomes a **parse tree**. It is the
> prose companion to sections `(B)`–`(I)` of [`grammar.ebnf`](grammar.ebnf), the
> authoritative EBNF. Cross-references to [`CONSTITUTION.md`](CONSTITUTION.md)
> use `§N`.
>
> The single most important thing this document does is draw the line between
> **syntax** (context-free shape, defined here) and **semantics** (types,
> resolution, ownership, exhaustiveness — *not* defined here). **We do not
> pretend type checking is part of the context-free grammar.**

## Three layers

| Layer | Input → output | Where |
|-------|----------------|-------|
| 1. **Lexical** | characters → tokens | [`lexical-grammar.md`](lexical-grammar.md), `grammar.ebnf (A)` |
| 2. **Concrete** *(this doc)* | tokens → parse tree | `grammar.ebnf (B)–(I)` |
| 3. **Semantic restrictions** | parse tree → *rejected or accepted* | [below](#semantic-restrictions) — **not** in the grammar |

The concrete grammar accepts a **superset** of valid programs. A program that
parses may still be rejected by the semantic layer. This separation is
deliberate: the parser's job is *shape*, not *meaning*.

## Design priorities (and how the grammar meets them)

Prompt-003 fixes the grammar's priorities. Each is a concrete property of the
EBNF, not an aspiration:

### Deterministic parsing

There is exactly one parse tree for every accepted token stream. The expression
grammar is a standard precedence-layered cascade (`assignment` → `or_expr` →
`and_expr` → `compare_expr` → `range_expr` → `add_expr` → `mul_expr` →
`unary_expr` → `postfix_expr` → `primary_expr`) with fixed associativity, so no
operator precedence is ambiguous. Comparison is **non-associative** (`a < b < c`
is a parse error, not a silently-grouped expression), removing a subtle class of
mistakes.

### Minimal lookahead

The grammar is designed to be parseable with **one token of lookahead** at every
decision point (recursive-descent / LL(1)-ish):

- **Items** (`grammar.ebnf (B)`): after an optional `pub`, the next token is a
  unique item-introducing keyword (`import`, `fn`, `struct`, `enum`,
  `interface`, `impl`, `const`, `spec`, `module`). One token selects the
  production.
- **Statements** (`(F)`): `let` / `var` / `const` / `;` select directly;
  otherwise the statement is an expression, and whether it needs a trailing `;`
  is decided by whether it *starts* with a block-introducing keyword (see
  [Expression statements](#expression-statements)).
- **Generics use `[ ]`, never `< >`** (§18). This is the single largest
  lookahead win: the `<`/`>` “generic-args vs. comparison/shift” ambiguity that
  forces unbounded lookahead or type-directed parsing in C++/Rust-family
  languages simply does not exist here. `<` and `>` are *only* comparison
  operators.

### One canonical spelling

Where a construct could be written more than one way, v0 admits **exactly one**:

| Construct | The one form | Excluded |
|-----------|--------------|----------|
| Struct fields | named fields | positional/tuple structs (§12) |
| Generics | `Name[T]` | `Name<T>` |
| Strings | `"…"` | raw / multi-line / interpolated |
| Doc comments | `///` | `/** */` |
| Blocks | always braced (§5) | brace-less bodies |
| Assignment | `=` | `+=`, `-=`, … (no compound assignment in v0) |
| Imports | explicit leaves | glob `::*` (§7) |

These omissions are intentional and are marked `[FROZEN by omission]` in the
EBNF; they are revisited only through a future edition (§30).

### Explicit delimiters

Every block is brace-delimited and every non-block statement is
semicolon-terminated (§5). There are **no** off-side rules and **no** automatic
semicolon insertion. Delimiters are never inferred.

### No significant whitespace, no preprocessor, no user grammar, no macros

- **No significant whitespace** (§5): whitespace is trivia
  ([`lexical-grammar.md`](lexical-grammar.md) §2).
- **No preprocessor:** there is no `#include`/`#define`/conditional-compilation
  text substitution phase. The token stream the parser sees is exactly the
  lexer's output.
- **No user-defined grammar and no macros in v0:** the grammar is fixed. There
  is no syntax-extension mechanism, no macro invocation form, and no operator
  overloading of the *grammar*. A generator can rely on the productions here
  being the complete surface.

## Compilation unit and items

A `.tuo` file is one **module** (§6) and parses as a `compilation_unit`: an
optional self-naming `module …;` followed by a sequence of `item`s. There is no
inline `module { }` block (§6) — module structure mirrors the filesystem.

An `item` is: an optional run of `///` doc comments, an optional `pub` (§6), then
one of `import` / `fn` / `struct` / `enum` / `interface` / `impl` / `const` /
`spec`. **Item order is unconstrained** — see [Forward
reference](#forward-reference).

## Types

The type grammar (`grammar.ebnf (D)`) is purely syntactic: `path_type`,
`unit_type` `()`, the ownership `wrapper_type`s `Box[T]` / `Shared[T]` /
`Weak[T]` (§25), and the `[EXPERIMENTAL]` `fixed_array_type`
`[T; N]` (ADR-0004 Stage 2). Generic arguments are bracketed (`Option[T]`,
`Map[K, V]`).

**Fixed-capacity arrays** (`[T; N]`, ADR-0004 Stage 2): the length is part of
the type and is an `INT_LITERAL` token — never a general expression — with no
type suffix admitted (a semantic check, per the superset rule). A `[` at type
position uniquely selects this production: no other type starts with `[`
(`TypeArguments` only ever follow a path/wrapper head), so disambiguation is
one-token. `[T; N]` is distinct from the growable `Array[T]`.

**There is no reference/pointer type syntax.** Aliasing is expressed *only*
through the parameter **modes** `in` / `mut` / `take` (§22), never as a
first-class `&T`. Keeping references out of the type grammar makes §22 the sole
aliasing mechanism — there is no second way to alias that could bypass the borrow
rule.

Whether a named type exists, is in scope, or satisfies a bound is **semantic**
(resolution / typing), not decided here.

## Declarations

- **Functions** (`fn name[G](mode p: T, …) -> R where … { … }`, §8): parameters
  are always typed and always carry a mode (§22); the return type may be omitted
  to mean unit (§8). Methods are `fn` items inside an `impl` whose first
  parameter is a **receiver** (`in self` / `mut self` / `take self`, §19).
- **Structs** (§12): named fields only, private by default (`pub` per field).
- **Enums** (§13): variants are bare or carry a **named-field** payload; no
  positional payloads, no implicit integer representation.
- **Interfaces / impls** (§19): an `interface` lists method signatures (and
  optional default bodies); an `impl I for T { … }` provides them. The
  **inherent-impl** form `impl T { … }` (no `for`) is **not** in the v0
  normative grammar (it is Experimental); provide behavior through interfaces.
- **`const`** (§9): type annotation required.

Generic parameters and `where` clauses use bracket syntax and interface bounds
joined by `+` (§18).

## Statements, blocks, and block-as-expression

A `block` is `{ statement* tail_expression? }` and is itself an **expression**
whose value is the trailing expression-without-`;`, or `()` otherwise (§5).
Statements are `let` / `var` / block-scoped `const` / an expression statement /
an empty `;`.

`let`/`var` allow an **omitted initializer** at the grammar level; “definitely
assigned before first use” is a [semantic](#semantic-restrictions) check (§9),
not a syntactic one.

### Expression statements

The rule that makes statement parsing single-token is the split in
`expression_statement`:

- An expression that **starts with a block-introducing keyword** (`if`, `match`,
  `while`, `loop`, `for`, `unsafe`, or a bare `{`) forms a statement **by
  itself** — no trailing `;` required.
- **Every other expression** must be terminated by `;`.

So `if c { a(); } else { b(); }` is a complete statement, while `f(x);` needs its
semicolon. This is unambiguous with one token of lookahead and avoids ASI
entirely.

## Expressions

Expressions (`grammar.ebnf (G)`) are precedence-layered as listed under
[Deterministic parsing](#deterministic-parsing). Notable points:

- `if` and `match` are **expressions** (§16); `loop` yields a value via
  `break value` (§16).
- **Postfix `?`** is error propagation (§20). The grammar allows `?` anywhere a
  postfix applies; that the enclosing function returns a compatible `Result` /
  `Option` is [semantic](#semantic-restrictions) (§20).
- **`as`** is the only conversion operator, postfix, numeric-only by the type
  checker (§10).
- Control-transfer expressions `return` / `break` / `continue` are ordinary
  expressions (§8, §16) and may carry a value / label.
- **Array literals** (`[EXPERIMENTAL]`, ADR-0004 Stage 2): the list form
  `[a, b, c]` (0+ elements, trailing comma allowed, `[]` legal) and the repeat
  form `[x; N]` with an `INT_LITERAL` length. Disambiguation is one-token: a
  `[` at *primary* position can only open a literal — `base[i]` indexing is a
  postfix op consumed after a completed primary — and inside the literal the
  token after the first expression decides (`;` → repeat form; `,` or `]` →
  list form). Because `-` cannot begin an `INT_LITERAL`, `[x; -1]` is a parse
  error. The literal is legal in `expr_no_struct` positions
  (`for x in [1, 2, 3] { … }` is unambiguous — postfix ops never include `{`).
  No new token was added: `[`/`]`, `;`, `,`, and `INT_LITERAL` all existed.

### The one context-sensitive point: struct literals in condition position

A bare `Name { … }` is a struct literal (§12), but in the *condition* of `if` /
`while` and the *scrutinee* of `match` / `for`, a `{` must begin the block, not a
struct literal. tuonelang resolves this the same way Rust does: **struct literals
are not permitted (unparenthesized) in those positions.** The EBNF models this
with `expr_no_struct` (identical to `expression` minus the `struct_literal`
primary). Write `if (Point { x: 0.0, y: 0.0 }).is_origin() { … }` if you truly
need a literal there.

This is the **only** place the concrete grammar is position-sensitive, it is
bounded and local, and it requires **no** feedback from the parser into the
lexer. Everything else is context-free.

## Patterns

Patterns (`grammar.ebnf (H)`) appear in `match` arms, `let`/`var` bindings, and
`for`. v0 patterns: literals, `_`, bindings, struct patterns (with `..`), enum
patterns, or-patterns (`A | B`), and guards (`pat if cond`). **Range patterns and
`ref` patterns are deliberately absent** (§17). `match` **exhaustiveness** is
[semantic](#semantic-restrictions), not syntactic (§17).

## Specifications (`spec`)

`spec` is a first-class, item-level construct (§27). Its **semantic contract** —
executable on the shared MIR, pure, deterministic — is **Frozen**; its **surface
syntax** is **Experimental** (§27, tracked as Q-0001 in
[`open-questions.md`](open-questions.md)). The grammar (`grammar.ebnf (I)`)
therefore marks the spec productions `[EXPERIMENTAL]`: they are normative for the
v0 parser *today* but may change within v0 without an edition bump.

Two statement styles coexist so both forms in the design record are expressible:

```tuo
spec fibonacci {
    assert fibonacci(0) == 0;
}

fn fibonacci(n: Int) -> Int {
    ...
}
```

```tuo
spec "add is commutative" {
    given a: Int, b: Int;
    when let s1 = add(a, b);
    when let s2 = add(b, a);
    then s1 == s2;
}
```

- `given` introduces typed inputs; `when` performs a step/binding; `then` and
  the compact `assert` state conditions that must hold (§27).
- A `spec` is named by an identifier **or** a description string.

### Forward reference

**A `spec` (or any item) may refer to a function declared later in the module.**
This is not a grammar concern — `item` order is unconstrained, so the parse tree
already contains both — but it is a **resolution** rule worth stating: names are
resolved **module-wide**, not strictly top-to-bottom. The `spec fibonacci`
example above references `fn fibonacci` declared *after* it, and that is valid.

## Error recovery

On a parse error the parser emits a diagnostic and **skips tokens until a
synchronization point**, then resumes — so one error does not cascade into
dozens, and the machine-readable diagnostics TDG relies on stay useful. The v0
synchronization tokens are:

| Sync token | Resumes at |
|------------|-----------|
| `;` | the next statement / clause |
| `}` | the end of the enclosing block or item body |
| an item-leading keyword at brace-depth 0 | the next top-level item |

Mandatory braces and semicolons (§5) are precisely what make these points
reliable: recovery never depends on indentation or on guessing where a statement
ended.

## Semantic restrictions

Everything below is **checked after parsing** and is **not** expressed by the
context-free grammar. The grammar accepts programs that violate these; the
semantic layers (resolution → typing → ownership) reject them. Listing them here
is the explicit statement that **type checking is not part of the grammar**:

| Restriction | Enforced by | Constitution |
|-------------|-------------|--------------|
| Name / path / import resolution; type exists & in scope | resolver | §6, §7, §10 |
| Type checking & inference; operand type compatibility | type checker | §8–§18 |
| Numeric literal fits its type; no implicit conversion | type checker | §10, §24 |
| `let`/`var` **definite assignment** before use | type checker | §9 |
| `match` **exhaustiveness** & arm reachability | type checker | §17 |
| Generic **bound satisfaction**; monomorphization | type checker | §18 |
| Interface **coherence** (one impl per type) & orphan rules | resolver/typer | §19 |
| `?` used **only** in a compatible `Result`/`Option` function | type checker | §20 |
| Ownership: **move/borrow**, use-after-move, the borrow rule | ownership checker | §21, §22 |
| No `null`; every `T` is a valid initialized `T` | type + ownership | §23 |
| Integer **overflow** traps (checked at runtime / interp) | codegen / interp | §24 |
| `Weak` must be **upgraded** to `Option` before use | type checker | §25 |
| `unsafe`-only operations confined to `unsafe` blocks | type checker | §26 |
| Spec **purity / determinism**; runs on shared MIR | spec runner | §27 |
| Receiver (`self`) only as first parameter of an `impl` method | resolver | §19 |
| Struct-literal-in-condition restriction (parser-enforced) | parser | this doc |

The last row is the one restriction the **parser** itself enforces beyond pure
shape (via `expr_no_struct`); every other row is a later stage.
