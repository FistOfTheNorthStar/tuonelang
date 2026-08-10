# tuonelang v0 — Lexical Grammar

> **Normative.** This document defines the **lexical** layer of tuonelang v0:
> how a byte stream becomes a token stream. It is the prose companion to the
> `(A)` section of [`grammar.ebnf`](grammar.ebnf) and is cross-referenced to
> [`CONSTITUTION.md`](CONSTITUTION.md) by `§N`.
>
> The lexical grammar is **regular** and runs to completion *before* the
> syntactic (concrete) grammar in [`syntax.md`](syntax.md) sees anything. The
> lexer **never** consults parser state: a keyword is always a keyword, a number
> is always a number, regardless of syntactic position (§4). This is the first
> guarantee that makes tuonelang deterministic to parse.

## Three layers (where this document sits)

tuonelang's grammar is split into three layers; this document is layer 1.

1. **Lexical grammar** *(this document)* — characters → **tokens**. Regular,
   context-free of the parser, no significant whitespace (§5), no preprocessor.
2. **Concrete grammar** — tokens → **parse tree**. See [`syntax.md`](syntax.md).
3. **Semantic restrictions** — everything the context-free grammar deliberately
   does *not* enforce (types, resolution, exhaustiveness, borrowing, …). These
   are **not** lexical or syntactic; they are enumerated in
   [`syntax.md`](syntax.md#semantic-restrictions). **We do not pretend type
   checking is part of the grammar.**

## 1. Source-level preconditions (§1)

Before tokenization, the raw bytes must satisfy:

| Rule | Requirement | On violation |
|------|-------------|--------------|
| Encoding | Valid **UTF-8** | Lexical error (not recoverable per token) |
| BOM | **No** byte-order mark | Lexical error |
| Newlines | **LF (`U+000A`) only** | A lone `\r` or `\r\n` is a **lexical error**, never silently normalized |
| Tabs | **No `U+0009`** outside string/char literals | Lexical error *(§1: Experimental — may relax)* |
| Extension | File is `*.tuo` | Enforced by the driver, not the lexer |

These are byte-level checks; keeping the input byte-exact is what makes build
and spec hashing reproducible (§28).

## 2. Trivia: whitespace and comments (§3, §5)

**Whitespace** is any run of spaces (`U+0020`) and LF. Whitespace separates
tokens and carries **no** syntactic meaning — tuonelang has **no significant
whitespace** (§5). It is discarded after tokenization.

**Comments** come in exactly two forms (§3); there are **no block comments**:

| Form | Syntax | Treatment |
|------|--------|-----------|
| Line comment | `// … <newline>` | **Trivia** — discarded |
| Doc comment | `/// … <newline>` | **Captured** — attached to the following item; fed to docs/agent tooling |

A `///` line is lexed as a distinct `DOC_COMMENT` token so the parser can attach
it to the next `item`, `field`, or `variant`. A `//` line is discarded entirely.
Because there is no `/* */` form, the “accidentally commented-out half the file”
failure mode cannot occur (§3).

## 3. Identifiers (§2)

```
IDENT = xid_start xid_continue*     — in NFC, not a keyword, not a lone "_"
```

- `xid_start` = a character with the Unicode **`XID_Start`** property, **or**
  `_`. `xid_continue` = a character with **`XID_Continue`** (UAX-31).
- Identifiers are compared and stored in **NFC** (§2). A source identifier that
  is **not already in NFC is a lexical error** — the lexer does not silently
  normalize, so two byte-different spellings can never both appear.
- A lone `_` is **not** an identifier: it is the reserved **wildcard** token
  `UNDERSCORE` (§2), usable only where a pattern/wildcard is allowed.
- Casing conventions (`snake_case`, `UpperCamelCase`, `SCREAMING_SNAKE_CASE`)
  are a **lint**, not a lexical rule (§2): the lexer accepts any casing.

## 4. Keywords (§4)

The reserved set is **fixed, closed, and fully reserved** — every word is a
keyword in *every* position, so there are **no contextual keywords** (§4). The
lexer recognizes an identifier-shaped lexeme and, if it matches the set below,
emits the corresponding keyword token instead of `IDENT`.

```
module  import  pub  fn  let  var  const
struct  enum  interface  impl  where
if  else  match  for  in  while  loop  break  continue  return
take  mut  move  Box  Shared  Weak
spec  given  when  then  assert  unsafe  as  true  false
self  Self
```

This list is normative and **must equal** `CONSTITUTION.md` §4 (plus `assert`,
`self`, `Self` — see the notes below). Consequences:

- A generator never has to decide whether a word is a keyword *here*; it always
  is. This removes an entire class of context-sensitive lexing bugs.
- `true`/`false` are keywords but surface to the concrete grammar as
  `BOOL_LITERAL` (§10).
- `assert` is reserved for the **spec** assertion form (`grammar.ebnf` §I). Its
  spec surface syntax is **Experimental** (§27, Q-0001); reserving the word now
  avoids a breaking change later.
- `self` (value receiver) and `Self` (the implementing type) are reserved for
  interfaces/impls (§19).

**Reserved type names.** The primitive type names (§10) — `I8 … Usize`, `F32`,
`F64`, `Bool`, `Char`, `String`, `Str`, `Int`, `Float` — are lexed as ordinary
`IDENT`s. That they cannot be used as *binding* names is a **resolution** rule,
not a lexical one.

## 5. Literals (§10, §11)

### Integer literals

```
INT_LITERAL = ( dec | 0x hex | 0o oct | 0b bin ) int_suffix?
```

- Underscores `_` may separate digits for readability (`1_000_000`).
- An optional suffix pins the type: `i8 i16 i32 i64 isize u8 u16 u32 u64 usize`
  (e.g. `255u8`, `0xFFi32`). Without a suffix the type is inferred, defaulting to
  `Int` (= `I64`) when unconstrained (§11).
- Whether a literal **fits** its type is a **semantic** check (§24), not lexical:
  `256u8` lexes fine and is rejected later.

### Float literals

```
FLOAT_LITERAL = dec "." dec exp? float_suffix?
              | dec exp  float_suffix?
exp           = (e|E) (+|-)? dec
```

- Optional suffix `f32` / `f64`; unsuffixed floats default to `Float` (= `F64`,
  IEEE-754 binary64, §11).
- **`1.` and `.5` are *not* float literals.** A trailing/leading dot is reserved
  for the `.` member/index operator so `x.0`, `1.foo()`, and `0 .. 5` stay
  unambiguous with minimal lookahead. Write `1.0` and `0.5`.

### Char and string literals

```
CHAR_LITERAL   = ' ( char | escape ) '
STRING_LITERAL = " ( char | escape )* "
escape         = \" | \' | \\ | \n | \r | \t | \0 | \u{HEX}
```

- `CHAR_LITERAL` is a single Unicode scalar value → `Char` (§10).
- `STRING_LITERAL` is UTF-8 text → `String`/`Str` (§10).
- v0 has **one** string form: **no** raw strings, **no** multi-line string
  literals, **no** interpolation. One canonical spelling keeps generation
  predictable; the omissions are deliberate (see [`syntax.md`](syntax.md)).
- A newline inside a char/string literal is a lexical error (use `\n`).

## 6. Punctuation and operators

Multi-character tokens are matched by **maximal munch** (longest match wins), so
`->`, `::`, `..`, `==`, `!=`, `<=`, `>=`, `&&`, `||`, `=>` each lex as one token
before their single-character prefixes are considered.

| Group | Tokens |
|-------|--------|
| Arrows / paths | `->` `=>` `::` |
| Range | `..` |
| Comparison | `==` `!=` `<` `>` `<=` `>=` |
| Logical | `&&` `\|\|` `!` |
| Pattern alternative | `\|` (or-patterns `A \| B` and interface bounds use it/`+`; **not** a bitwise operator) |
| Arithmetic | `+` `-` `*` `/` `%` |
| Assignment | `=` |
| Propagation | `?` |
| Member | `.` |
| Delimiters | `(` `)` `{` `}` `[` `]` |
| Separators | `,` `;` `:` |

**No angle-bracket generics.** `<` and `>` are *only* comparison operators;
generics use square brackets `[T]` (§18). This is why the parser needs no
special handling for the `<`/`>` “is this a generic or a comparison?” ambiguity
that complicates C++/Rust-family parsers.

**Fixed-capacity arrays add no tokens** (ADR-0004 Stage 2): the `[T; N]` type
and the `[a, b, c]` / `[x; N]` literals are built entirely from the existing
`[` `]` delimiters, the `;` separator, the `,` separator, and `INT_LITERAL`.
The `;`-inside-brackets disambiguation (`;` after the first expression selects
the repeat form) is a parser concern, not a lexical one.

**No compound-assignment, no bitwise-operator tokens in v0.** `+=`, `|`, `&`,
`^`, `<<`, `>>` are intentionally absent from the token set; the `|` glyph exists
only inside patterns (`A | B`) and bounds (`T + U` uses `+`). These omissions are
deliberate and revisited only via a future edition (§30).

## 7. The `label` sigil (Experimental, §16)

Labeled loops (`'label: loop { … break 'label; }`) use a `LIFETIME_LABEL`
token: an apostrophe `'` followed by an identifier. This spelling is
**Experimental** (§16): the sigil may change (it is the one token whose form the
Constitution has not frozen). It is included so the v0 parser can accept labeled
multi-level breaks (§16). Note the apostrophe here is *not* a char-literal start
because it is **not** followed by a single character then a closing `'`.

## 8. Token classes exported to the parser

The lexer emits this closed set of token classes (the terminals of
[`syntax.md`](syntax.md)):

- `IDENT`, `UNDERSCORE`
- one token per **keyword** (§4)
- `INT_LITERAL`, `FLOAT_LITERAL`, `BOOL_LITERAL`, `CHAR_LITERAL`,
  `STRING_LITERAL`
- one token per **punctuation/operator** (§6)
- `DOC_COMMENT` (captured), `LIFETIME_LABEL` (Experimental)
- `EOF`

Whitespace and line comments are consumed as trivia and are **not** in the
stream the parser consumes (doc comments are).

## 9. Lexical error recovery

The lexer is designed to **keep going** after an error so the parser still
receives a usable stream (diagnostics accumulate rather than aborting at the
first problem — the machine-readable feedback TDG relies on):

- On an invalid byte/encoding, a stray `\r`, a tab, a non-NFC identifier, an
  unterminated string/char, or an unknown character, the lexer emits a
  **diagnostic** and a synthetic error token, then resynchronizes at the next
  whitespace or delimiter.
- Because whitespace and delimiters are unambiguous and mandatory (§5), the
  lexer always has a reliable resync point — it never needs to guess based on
  indentation.

## 10. What the lexical grammar does *not* do

The lexer decides **token identity only**. It does **not**:

- check that an integer literal fits its type (§24 — semantic);
- check that an identifier resolves to anything (resolution — semantic);
- know whether `Int` is a valid type in scope (semantic);
- attach any meaning to operators beyond their token identity.

Everything past “what tokens are these?” is the concrete grammar
([`syntax.md`](syntax.md)) and then the semantic layer. Keeping the lexer this
thin is what makes it regular, fast, and deterministic.
