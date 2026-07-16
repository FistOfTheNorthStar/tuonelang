# tuonelang parser fixtures

A version-controlled corpus of tuonelang source snippets used to exercise the
**concrete grammar** ([`specification/grammar.ebnf`](../../../specification/grammar.ebnf)).
The corpus exists *before* the parser (`tuo-parser`) does: it is the executable
target the parser will be validated against, and it pins down exactly what the
grammar accepts and rejects.

## Layout

| Path | Contents |
|------|----------|
| `valid/` | Snippets that **must parse** under the v0 grammar. |
| `invalid/` | Snippets that **must be rejected** at the **lexical or syntactic** level. |
| `valid.manifest.json` | Every valid fixture + the construct it exercises. |
| `invalid.manifest.json` | Every invalid fixture + its `stage` (`lexical`/`syntax`) and failure `reason`. |

Files are named `NNN-slug.tuo`; the numeric prefix keeps them ordered and stable.

## The contract (what "valid" and "invalid" mean here)

These fixtures test **grammar**, not **meaning**. This is the same three-layer
split documented in [`syntax.md`](../../../specification/syntax.md):

- A **`valid/`** fixture is **syntactically** well-formed. It may still be
  *semantically* wrong — reference an undeclared function, be ill-typed, have a
  non-exhaustive `match`, or violate the borrow rule. That is deliberate: the
  parser accepts a **superset** of well-formed programs, and those semantic
  errors are caught by later stages (see the semantic-restrictions table in
  `syntax.md`). Semantic-error corpora live under `tests/types`,
  `tests/ownership`, etc. — **not here**.
- An **`invalid/`** fixture fails at the **lexical** or **syntactic** layer *only*.
  Each is tagged in the manifest with the `stage` it fails at and a one-line
  `reason` cross-referenced to the Constitution. A snippet that parses fine but
  is merely ill-typed does **not** belong in `invalid/`.

## Coverage

The `valid/` set spans every Frozen surface construct: modules and imports (§6,
§7), functions and parameter modes (§8, §22), generics and bounds (§18), structs
(§12), enums (§13), interfaces and impls (§19), `const`/`let`/`var` (§9), the
full control-flow set (§16), patterns and `match` (§17), the expression/operator
grammar (§16, §20, §24), `unsafe` blocks (§26), and `spec` blocks in both the
`assert` and `given/when/then` forms (§27, Experimental surface). Several larger
fixtures combine features into whole small programs.

The `invalid/` set covers the corresponding failure modes: the deliberate
omissions (`<T>` generics, positional structs, glob imports, block comments,
compound assignment, ternary, `++`, bitwise/shift ops, range/`ref` patterns,
brace-less bodies, ASI), plus malformed forms of each construct (missing
delimiters, unclosed braces/brackets/parens, missing separators, and the lexical
errors of §1/§2/§3/§10 — tabs, CRLF, BOM, bad escapes, unterminated literals).

## Consuming these fixtures (future)

When `tuo-lexer`/`tuo-parser` land, a data-driven test will:

1. parse every `valid/*.tuo` and assert success;
2. parse every `invalid/*.tuo` and assert a diagnostic is produced at the
   `stage` the manifest records;
3. keep the manifests in lockstep with the files on disk (no orphan fixtures, no
   missing entries) — the same round-trip discipline the tokenizer-lab data uses
   (`crates/tuo-bench/tests/committed_data.rs`).

Until then the manifests are the machine-readable index and the `reason` fields
are the specification of *why* each negative case is rejected.

## Regenerating

The fixtures and manifests are generated from the checked-in scripts under
`tests/parser/fixtures/tools/` so the corpus stays consistent and reviewable.
Edit a script and re-run it rather than hand-editing individual files.
