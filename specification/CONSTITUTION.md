# The tuonelang Language Constitution (v0)

> **Purpose.** This document freezes the foundational v0 design decisions for
> **tuonelang** *before* the compiler is implemented, so that the lexer, parser,
> type checker, ownership checker, MIR, and backends all target one agreed
> language rather than discovering it accidentally.
>
> **tuonelang** is the language. It is designed around **TDG — Test Driven
> Generation**: development driven by colocated, executable specifications. TDG
> is the paradigm; tuonelang is the language built for it.

## How to read this document

Every decision below is recorded in a fixed shape:

- **Design** — the chosen rule.
- **Rationale** — why, with tuonelang's goals in mind (LLM-reliable, human-easy,
  memory-safe, deterministic).
- **Rejected** — alternatives considered and why they lost.
- **Status** — **Frozen** (a v0 commitment; changing it is a breaking language
  change requiring an ADR) or **Experimental** (deliberately open; may change
  without an edition bump while v0 stabilizes).

Design guidance that shapes the whole document: prefer **one obvious way** to
express a thing. Redundant syntax is a reliability tax on both humans and code
generators, so where a choice is arbitrary, tuonelang picks one form and forbids
the rest.

A note on examples: the snippets below are **illustrative of the frozen
semantics**, not a promise that the exact surface syntax is final where marked
Experimental. The authoritative surface grammar is tracked in `grammar.ebnf`.

---

## 1. Source encoding

- **Design.** Source files are **UTF-8**, no byte-order mark permitted. The
  canonical newline is `\n` (LF); a lone `\r` or `\r\n` is a lexical error, not
  silently normalized. Files use the extension **`.tuo`**. Tab characters are
  forbidden in source outside string/char literals; indentation uses spaces.
- **Rationale.** A single canonical encoding removes an entire class of "works
  on my machine" and invisible-character bugs that confuse both humans and
  generators. Rejecting a BOM and CRLF outright (rather than normalizing) keeps
  the byte-for-byte input deterministic, which matters for reproducible builds
  and for spec hashing (§27). Forbidding tabs eliminates mixed-indentation
  ambiguity.
- **Rejected.** *Permit UTF-16 / other encodings*: multiplies tooling
  complexity for no benefit. *Silently normalize CRLF→LF*: hides the fact that
  two byte-different files are being treated as identical, which undermines
  deterministic hashing. *Allow tabs*: reintroduces alignment ambiguity.
- **Status.** **Frozen** (encoding, LF, `.tuo`). Tab prohibition:
  **Experimental** (may relax to "tabs allowed only for indentation" if
  formatter experience argues for it).

## 2. Identifiers

- **Design.** Identifiers match `XID_Start XID_Continue*` (Unicode UAX-31) or a
  leading `_` followed by `XID_Continue*`. Identifiers are compared using
  **NFC normalization**; two identifiers that are not identical after NFC
  normalization are distinct, and a non-NFC identifier in source is a lexical
  error. A single `_` is the reserved **wildcard** and is not a binding name.
  Naming convention (enforced by the formatter/linter, not the grammar):
  `snake_case` for values, functions, modules, and fields; `UpperCamelCase` for
  types, enum variants, and interfaces; `SCREAMING_SNAKE_CASE` for constants.
- **Rationale.** UAX-31 is the industry-standard identifier definition. Forcing
  NFC prevents "look-alike but byte-different" identifiers — a subtle bug source
  and a security concern, and one that trips up generators that emit composed
  vs. decomposed forms inconsistently. Convention enforcement gives generators a
  single predictable casing to target.
- **Rejected.** *ASCII-only identifiers*: excludes non-English contributors.
  *Case-insensitive identifiers*: surprising and error-prone. *No normalization*:
  admits confusable identifiers.
- **Status.** **Frozen** (UAX-31 + NFC + `_` wildcard). Casing convention:
  **Frozen as a lint**, i.e. required by tooling but not a parse error.

## 3. Comments

- **Design.** Two forms only:
  - Line comments: `// …` to end of line.
  - Doc comments: `/// …` (attached to the following item).

  There are **no block comments** (`/* … */`). Doc comments are Markdown and are
  the only comments the compiler attributes semantic meaning to (they feed
  documentation tooling and the agent protocol).
- **Rationale.** Block comments nest inconsistently across languages and are a
  frequent source of "accidentally commented out half the file" errors; omitting
  them removes that failure mode. A single documentation form keeps generated
  docs uniform.
- **Rejected.** *Nestable block comments* (Rust-style): convenient for commenting
  out code but rewards leaving dead code in place, which TDG discourages in favor
  of version control. *Multiple doc-comment styles* (`/** */` and `///`): two
  ways to do one thing.
- **Status.** **Frozen.**

## 4. Keywords

- **Design.** The reserved keyword set for v0 is fixed and closed:

  ```
  module  import  pub  fn  let  var  const
  struct  enum  interface  impl  where
  if  else  match  for  in  while  loop  break  continue  return
  take  mut  move  Box  Shared  Weak
  spec  given  when  then  unsafe  as  true  false
  ```

  Additionally, `_` is reserved (wildcard) and the primitive type names in §10
  are reserved. Contextual keywords are **not** used in v0: every word above is
  reserved everywhere, so it can never also be an identifier.
- **Rationale.** A closed, fully-reserved set means a generator never has to
  reason about whether a word is a keyword *in this position* — it is always a
  keyword. That eliminates a notorious class of context-sensitive parse bugs.
  Reserving a little extra now (e.g. `interface`, `spec`) avoids breaking changes
  later.
- **Rejected.** *Contextual keywords* (like Rust's `union`/`dyn` history, C#'s
  `async`): smaller reserved set but context-dependent lexing, which is exactly
  the ambiguity TDG wants to avoid. *Open/soft keyword set*: unpredictable.
- **Status.** **Frozen** for the words listed. The set **may grow** in a future
  edition (§30); growth is the reason some words are reserved before use.

## 5. Braces, blocks, and semicolons

- **Design.** Blocks are delimited by **braces** `{ … }`. Statements are
  **terminated by semicolons** `;`. Every non-block statement requires its
  terminating `;`; there is no automatic semicolon insertion. A block is an
  expression whose value is that of its final *expression without* a trailing
  `;`; a block whose last item ends in `;` (or is empty) has the unit value
  `()`. Braces are **mandatory** for all `if`/`else`/`for`/`while`/`loop`/`match`
  arms — there are no brace-less single-statement bodies.
- **Rationale.** Explicit braces + explicit semicolons are the most
  generation-robust choice: there is no whitespace-sensitivity and no ASI
  edge-cases (the source of real-world JavaScript bugs). Mandatory braces
  eliminate the "dangling else" and "goto fail" families of bugs. Block-as-
  expression is a clean, composable rule that keeps the language small.
- **Rejected.** *Significant indentation* (Python-style): whitespace errors are
  invisible and hard for generators to get right under editing. *Optional
  braces*: dangling-else hazard. *ASI* (JS/Go-style): subtle, position-dependent.
- **Status.** **Frozen.**

## 6. Modules

- **Design.** Each `.tuo` file is a **module**; the module's name is the file
  stem. A directory with a `mod.tuo` file forms a parent module whose submodules
  are the other files/directories beside it. Item visibility is **private by
  default**; `pub` makes an item visible to the module's parent and beyond.
  There is no separate `mod { … }` block form in v0 — module structure mirrors
  the filesystem exactly, one module per file.
- **Rationale.** A 1:1 file↔module mapping is the most predictable structure for
  a generator: to find `foo::bar` you look in `foo/bar.tuo`. Private-by-default
  is the memory-safety- and encapsulation-friendly default. Removing inline
  `mod` blocks removes a second, redundant way to declare modules.
- **Rejected.** *Inline `mod` blocks* (Rust): a second module mechanism.
  *Public-by-default*: leaks implementation detail and invites accidental API.
  *Header/impl split* (C/C++): duplicate declarations drift.
- **Status.** **Frozen** (file = module, private default). Directory/`mod.tuo`
  convention: **Experimental** (the exact parent-module filename may change).

## 7. Imports

- **Design.** `import` brings names into scope:

  ```
  import std::io;                 // binds `io`
  import std::collections::Map;   // binds `Map`
  import std::io::{read, write};  // binds `read`, `write`
  import std::io::read as fs_read;// renamed binding
  ```

  Imports are **explicit and non-glob**: there is no `import foo::*`. Imports
  are not re-exported unless declared `pub import`. All paths are absolute from a
  package root or the current package; there are no relative `super`/`self`
  path prefixes in v0 (a module refers to a sibling by its package-qualified
  path).
- **Rationale.** Explicit, glob-free imports mean the origin of every name is
  determinable by reading the file top — critical for both human review and for
  a generator that must not guess where `Map` came from. Banning glob imports
  removes ambiguity about which names are in scope.
- **Rejected.** *Glob imports*: make name resolution context-dependent and can
  break on upstream additions. *Relative paths* (`super::`, `self::`): convenient
  but make a snippet's meaning depend on its location; absolute paths are
  position-independent.
- **Status.** **Frozen** (explicit, non-glob). Absolute-only paths:
  **Experimental** (relative paths may be added if ergonomics demand).

## 8. Functions

- **Design.**

  ```
  fn name(param: Type, other: Type) -> ReturnType { … }
  ```

  Parameters are always typed; there is no parameter type inference. The return
  type is required **except** that `-> ()` may be omitted (a function with no
  `->` returns unit). Parameters are immutable bindings by default (§9, §22).
  There are no default parameter values, no variadic parameters, and no function
  overloading in v0. `return expr;` returns early; a function may also yield its
  value as the block's final expression (§5). All functions are declared with
  `fn`; there are no separate method/free-function syntaxes — methods are `fn`
  items inside an `impl` (§17) whose first parameter is a receiver.
- **Rationale.** Mandatory parameter types make every function signature a
  complete, checkable contract — a generator never has to infer a caller's
  expectations. No overloading means a call site resolves to exactly one
  function by name, removing overload-resolution ambiguity (a common source of
  surprising behavior and of generation errors). No defaults/variadics keeps the
  call ABI and the mental model simple; the same effect is achieved with explicit
  arguments or a config struct.
- **Rejected.** *Overloading*: overload resolution is subtle and
  generation-hostile. *Default arguments*: introduce call-site-dependent
  behavior and complicate the ABI. *Inferred parameter types*: signatures stop
  being self-describing.
- **Status.** **Frozen** (typed params, no overloading, no defaults/variadics).

## 9. Variable declarations, immutable and mutable bindings

- **Design.** Three binding forms:
  - `let name: Type = expr;` — an **immutable** binding (the default).
  - `var name: Type = expr;` — a **mutable** binding (may be reassigned).
  - `const NAME: Type = expr;` — a compile-time constant (module or block scope).

  The `: Type` annotation is **optional for `let`/`var`** when the initializer's
  type is unambiguous (local inference, §11), and **required for `const`** and
  for any binding without an initializer. There is no uninitialized binding that
  can be read before assignment: a binding is either initialized at declaration
  or definitely-assigned before first use (checked). Shadowing within a scope is
  **allowed** (`let x = …; let x = f(x);`).
- **Rationale.** Immutable-by-default (`let`) nudges toward the easier-to-reason,
  easier-to-parallelize code and pairs naturally with the ownership model (§21).
  Distinct `let`/`var` keywords make mutability visible at a glance — no `mut`
  qualifier buried in a pattern — which is both human- and generator-friendly.
  Definite-assignment checking removes the "read of uninitialized memory" bug
  class entirely. Shadowing supports the common "refine a value through a
  pipeline" pattern without inventing new names.
- **Rejected.** *`let mut`* (Rust): mutability as a modifier is easy to miss and
  easy for a generator to drop. *Mutable-by-default*: contrary to the safety
  goal. *No shadowing*: forces awkward naming (`x2`, `x_final`).
- **Status.** **Frozen** (`let`/`var`/`const`, immutable default,
  definite-assignment, shadowing allowed).

## 10. Primitive types

- **Design.** The v0 primitive types are a **fixed set with explicit widths**:

  | Category | Types |
  |----------|-------|
  | Signed integers | `I8`, `I16`, `I32`, `I64`, `Isize` |
  | Unsigned integers | `U8`, `U16`, `U32`, `U64`, `Usize` |
  | Floating point | `F32`, `F64` |
  | Boolean | `Bool` (`true` / `false`) |
  | Text | `Char` (a Unicode scalar value), `String` (owned, UTF-8), `Str` (borrowed UTF-8 slice) |
  | Unit | `()` (the unit type, one value `()`) |

  `Int` is a **type alias for `I64`** and `Float` an alias for `F64` (§11).
  Numeric literals require an explicit type when ambiguous; there is **no
  implicit numeric conversion** between any two types — conversions are written
  `x as U32` (§10 conversions) and only between numeric types. `Isize`/`Usize`
  are pointer-width and are the index/length type.
- **Rationale.** Explicit-width integers by name eliminate the portability
  ambiguity of C's `int`/`long`. No implicit conversions removes the silent
  truncation/sign-change bug class and makes every widening/narrowing visible and
  auditable — a major reliability win. Providing `Int`/`Float` aliases gives a
  friendly default for prose and examples while keeping the underlying width
  explicit and fixed.
- **Rejected.** *Machine-dependent `int`*: non-portable. *Implicit numeric
  coercions* (C/JS): silent data loss. *Arbitrary-precision default integer*:
  hides allocation/performance and complicates the native ABI (may be offered as
  a library type later).
- **Status.** **Frozen** (the width set, `Bool`/`Char`/`String`/`Str`/`()`, no
  implicit conversion). `Int=I64`/`Float=F64` aliases: **Frozen** (see §11).

## 11. Fixed semantics of `Int` and `Float`

- **Design.**
  - **`Int` is exactly `I64`**: a two's-complement signed 64-bit integer.
    `Int`/`I64` arithmetic obeys the overflow policy in §24.
  - **`Float` is exactly `F64`**: **IEEE-754 binary64**, round-to-nearest-even,
    with the standard subnormals, ±0, ±∞, and NaN. Float operations are *not*
    reordered or contracted by the compiler in ways that change results (no
    implicit fused-multiply-add, no `-ffast-math`-style reassociation); floating
    results are **bit-reproducible** across tuonelang's backends for the same
    program and inputs.
  - Integer/float literals are typed by inference or annotation; `Int` and
    `Float` are the inference defaults when a literal is otherwise unconstrained.
- **Rationale.** Pinning `Int`/`Float` to concrete, standard representations —
  and forbidding value-changing float optimizations — is what makes tuonelang
  **deterministic**: the same program yields the same bits on every backend and
  machine. This is essential for TDG, where an executable spec must produce the
  same observable result under the interpreter and under native codegen (§27).
- **Rejected.** *`Int` as platform word size*: non-deterministic across targets.
  *Allow fast-math reassociation*: better performance but destroys
  bit-reproducibility and would make specs backend-dependent. *`Float` as
  extended/80-bit on some targets*: non-portable results.
- **Status.** **Frozen.** This is one of the most load-bearing determinism
  guarantees in v0.

## 12. Structs

- **Design.** Nominal record types with named fields:

  ```
  struct Point { x: F64, y: F64 }
  ```

  Fields are **private by default**; `pub` on a field exposes it. Struct
  construction is by named fields: `Point { x: 1.0, y: 2.0 }` (positional/tuple
  structs are **not** in v0). Structs have **value semantics** (§21): assignment
  or passing moves or copies per the ownership rules. A struct with all-`Copy`
  fields is itself `Copy` (§21). There is no field-order-dependent layout
  guarantee (the compiler may reorder for packing) unless a future `repr`
  attribute requests one.
- **Rationale.** Named-field-only construction makes every struct literal
  self-documenting and order-independent — a generator cannot silently swap two
  same-typed positional fields. Private-by-default fields preserve invariants and
  make the memory model tractable.
- **Rejected.** *Tuple/positional structs*: terse but order-dependent and
  error-prone under generation; the same role is filled by a small named struct.
  *Public-by-default fields*: breaks encapsulation and invariant maintenance.
- **Status.** **Frozen** (named fields, private default, value semantics).
  Tuple structs: **deliberately excluded from v0** (may be reconsidered).

## 13. Enums

- **Design.** Nominal **sum types** (algebraic data types) with optional payloads
  per variant:

  ```
  enum Shape {
      Circle { radius: F64 },
      Rectangle { width: F64, height: F64 },
      Empty,
  }
  ```

  Variant payloads use named fields (consistent with §12); a payload-less variant
  is written bare. Enums are exhaustively matched (§16). Enums are **not** open
  and have no implicit integer representation in v0 (no C-style `enum = int`);
  an explicit discriminant/representation is a future `repr` concern.
- **Rationale.** Sum types + exhaustive matching are the backbone of tuonelang's
  error handling (`Option`, `Result`) and of making illegal states
  unrepresentable — a large, well-established reliability win that also gives
  generators a precise, checkable modeling tool. Named payload fields keep
  construction and matching unambiguous.
- **Rejected.** *C-style integer enums*: conflate a tag with a number and invite
  invalid values. *Open/extensible enums*: break exhaustiveness. *Positional
  payloads*: order-dependent (see §12).
- **Status.** **Frozen** (closed ADT enums, named payloads, exhaustive).

## 14. `Option`

- **Design.** `Option[T]` is a standard-library enum, **not** a language
  primitive:

  ```
  enum Option[T] { Some { value: T }, None }
  ```

  It is the *only* way to express "a value that may be absent." There is no
  `null` (§23). Ergonomic support (the `?`-style propagation, pattern matching)
  is provided uniformly with `Result` (§16, §20).
- **Rationale.** Absence-as-a-type, checked by exhaustive matching, eliminates
  null-dereference bugs at the type level. Defining it in the stdlib rather than
  the language keeps the core small while still making it canonical.
- **Rejected.** *Nullable references / `null`*: the "billion-dollar mistake";
  reintroduces the exact bug class ownership+types are meant to remove.
  *Language-builtin option type*: no benefit over an enum.
- **Status.** **Frozen** (the type and its role). Its exact method surface:
  **Experimental** (stdlib API, §26).

## 15. `Result`

- **Design.** `Result[T, E]` is a standard-library enum:

  ```
  enum Result[T, E] { Ok { value: T }, Err { error: E } }
  ```

  It is the canonical representation of a fallible computation's outcome and the
  substrate for tuonelang's error handling (§19). `E` is an ordinary type; there
  is no privileged exception hierarchy.
- **Rationale.** Errors-as-values make every fallible call visible in its type
  and impossible to ignore silently, which is both safer and far easier for a
  generator to reason about than invisible control-flow exceptions.
- **Rejected.** *Exceptions*: invisible control flow, non-local, and hostile to
  the "one obvious path" principle. *Error codes via out-params*: unsafe and
  easy to ignore.
- **Status.** **Frozen** (type and role). Method surface: **Experimental**.

## 16. Control flow

- **Design.** v0 control-flow constructs, all brace-mandatory (§5), all
  expressions where sensible:
  - `if cond { … } else { … }` — an expression; both arms must yield compatible
    types when used for its value. `else if` chains are permitted.
  - `match expr { pattern => arm, … }` — an expression; **exhaustive** (§17).
  - `while cond { … }` — a statement.
  - `loop { … }` — an infinite loop; `break value;` yields a value from it,
    making `loop` usable as an expression.
  - `for name in iterable { … }` — iterates an iterator/range; a statement.
  - `break` / `continue` operate on the innermost loop; **labeled** loops
    (`'label: loop { … break 'label; }`) are supported for multi-level exits.
  There is **no `goto`** and no fallthrough in `match`.
- **Rationale.** `if`/`match`-as-expressions reduce the need for mutable
  temporaries and make value flow explicit. Exhaustive `match` with no
  fallthrough removes the C `switch` fallthrough bug class. Labeled `break`
  covers legitimate multi-level exits without `goto`.
- **Rejected.** *`goto`*: unstructured control flow. *`switch` fallthrough*: a
  perennial bug source. *Statement-only `if`*: forces temporaries.
- **Status.** **Frozen** (the construct set, exhaustiveness, no goto/fallthrough).
  Labeled-loop syntax: **Experimental** (the `'label` sigil may change).

## 17. Pattern matching

- **Design.** Patterns appear in `match` arms, `let`/`var` destructuring, and
  `for` bindings. v0 patterns:
  - literals (`0`, `true`, `'a'`, string literals);
  - the wildcard `_`;
  - binding patterns (`name`, binding by move/copy per ownership);
  - struct patterns (`Point { x, y }`, with `..` to ignore the rest);
  - enum-variant patterns (`Some { value }`, `Ok { value }`, `Circle { radius }`);
  - `or`-patterns (`A | B`);
  - guards (`pattern if cond`).

  `match` must be **exhaustive**: the compiler statically proves every possible
  value is covered, and reports the specific missing cases as a diagnostic.
  Range patterns and reference/`ref` patterns are **not** in v0.
- **Rationale.** Compiler-proven exhaustiveness is a cornerstone reliability
  feature: adding an enum variant turns every incomplete `match` into a compile
  error with the missing case named — precisely the machine-readable feedback
  TDG and agents rely on. Struct/enum destructuring makes data access total and
  explicit.
- **Rejected.** *Non-exhaustive match with implicit default*: silently swallows
  new cases. *Regex/active patterns*: too much surface for v0.
- **Status.** **Frozen** (the pattern set, mandatory exhaustiveness). Range and
  `ref` patterns: **deliberately deferred**, tracked in open questions.

## 18. Generics

- **Design.** Parametric polymorphism with **bracketed** type parameters and
  **interface bounds**:

  ```
  fn max[T: Ord](a: T, b: T) -> T { … }
  struct Pair[A, B] { first: A, second: B }
  enum Option[T] { Some { value: T }, None }
  ```

  Bounds are written `T: Interface` or in a `where` clause for complex cases.
  Generics are **monomorphized** (each concrete instantiation generates
  specialized code). There is **no** lifetime-parameter syntax in v0 (ownership
  is checked without user-written lifetimes, §21); there are no const generics,
  no variance annotations, and no higher-kinded types in v0.
- **Rationale.** Bracketed `[T]` (rather than `<T>`) removes the classic
  `<`/`>` vs. comparison/shift parsing ambiguity — a real parser-complexity and
  generation-ambiguity win. Interface-bounded generics give precise, checkable
  contracts. Monomorphization keeps runtime cost predictable and native-friendly.
  Omitting user lifetimes keeps generic signatures readable for humans and
  generators while the ownership checker still guarantees safety.
- **Rejected.** *Angle brackets `<T>`*: parsing ambiguity that has cost every
  C++/Rust-family parser real complexity. *User-written lifetimes in v0*: a large
  cognitive and generation burden; deferred until inference is shown
  insufficient. *HKT/const-generics*: powerful but heavy; out of scope for v0.
- **Status.** **Frozen** (bracketed params, interface bounds, monomorphization,
  no user lifetimes in v0). Const generics / HKT: **excluded from v0**.

## 19. Interfaces

- **Design.** `interface` declares a set of required methods (and associated
  functions); `impl` provides them for a type:

  ```
  interface Ord {
      fn compare(in self, other: in Self) -> Ordering;
  }

  impl Ord for Point {
      fn compare(in self, other: in Point) -> Ordering { … }
  }
  ```

  Interfaces are tuonelang's **only** abstraction mechanism over behavior: no
  class inheritance, no implementation inheritance. Static dispatch is the
  default (via generics, §18); dynamic dispatch is available through an explicit
  boxed interface object (`Box[dyn Interface]`-style; exact spelling
  **Experimental**). A type may implement many interfaces. **Coherence** is
  required: at most one `impl` of a given interface for a given type across a
  program (orphan rules restrict where impls may live).
- **Rationale.** Interfaces + coherence give a single, unambiguous answer to
  "what does this type do and which implementation runs" — no inheritance
  diamonds, no method-resolution-order surprises. This is both a human-clarity
  and a generation-reliability win. Static-by-default keeps performance
  predictable; explicit dynamic dispatch makes the (rarer) erased case visible.
- **Rejected.** *Class/implementation inheritance*: fragile base class, MRO
  ambiguity, and it fights value semantics. *Incoherent/duplicate impls*:
  behavior depends on imports — unacceptable for determinism.
- **Status.** **Frozen** (interfaces-only, coherence, static default). Dynamic-
  dispatch spelling: **Experimental**. Associated types/constants:
  **Experimental** (likely needed but not yet frozen).

## 20. Function errors (fallibility)

- **Design.** A function that can fail returns `Result[T, E]` (§15). Propagation
  uses the postfix **`?`** operator: `let x = may_fail()?;` returns early with
  the `Err` if the callee failed, otherwise binds the `Ok` value. `?` is
  permitted only in functions whose return type is a compatible `Result` (or
  `Option`, for `None`-propagation). There are **no exceptions**, no `throw`, no
  stack unwinding for ordinary errors. A genuinely unrecoverable condition uses
  an explicit `panic`-style abort (see §24) which is **not** catchable as an
  ordinary error and terminates deterministically.
- **Rationale.** One error path — values + `?` — means every fallible call is
  visible in the source and in the type, and error handling composes locally.
  This is dramatically easier to generate correctly than exception-based code,
  where the invisible control flow is a frequent source of leaks and missed
  cases. Separating recoverable errors (`Result`) from bugs (panic) keeps the two
  from being conflated.
- **Rejected.** *Exceptions / `try`-`catch`*: invisible non-local control flow.
  *Checked exceptions* (Java): the ergonomic problems without the value-level
  clarity. *Implicit error propagation*: hides the failure path.
- **Status.** **Frozen** (Result + `?`, no exceptions). The exact `?`
  error-conversion rule (how `E` widens across `?`): **Experimental** (an
  interface-based conversion is intended).

## 21. Ownership terminology

- **Design.** tuonelang is **memory-safe without a garbage collector**, via
  compile-time ownership. Core terms, fixed for the whole project:
  - **Value semantics** is the default: each value has a single **owner**.
  - **Move**: transferring ownership; the source is thereafter invalid (use-
    after-move is a compile error). Written implicitly on assignment/pass, or
    explicitly with the `move` keyword where clarity is wanted.
  - **Copy**: a type may be `Copy` (bitwise-duplicated on assignment/pass instead
    of moved) iff all its fields are `Copy`; the primitive scalars (§10) are
    `Copy`. `Copy` is a property the compiler derives, not a hand-written impl in
    v0.
  - **Borrow**: temporary access to a value without taking ownership, expressed
    through the parameter modes `in` / `mut` / `take` (§22).
  - **Drop**: when an owner goes out of scope its value is destroyed
    deterministically (RAII); destruction order is reverse of declaration.
  There are **no user-written lifetime parameters** in v0 (§18); the checker
  infers all region information.
- **Rationale.** Single-owner value semantics + moves + scoped borrows is a
  proven route to memory safety without a GC, giving native performance and
  deterministic destruction (important for both performance and predictable
  resource release). Deriving `Copy` rather than hand-writing it removes a
  footgun. Inferring lifetimes keeps the surface language approachable.
- **Rejected.** *Garbage collection*: non-deterministic pauses, runtime weight,
  and it weakens the determinism story. *Reference-counting-by-default*: hidden
  cost and cycle leaks (RC is available explicitly via `Shared`, §25).
  *User-written lifetimes in v0*: cognitive/generation burden.
- **Status.** **Frozen** (single-owner, move/copy/borrow/drop, RAII, no user
  lifetimes in v0). This section and §22/§25 are designed to be mutually
  consistent with the type system (see the consistency note at the end).

## 22. Parameter modes: `in`, `mut`, `take`

- **Design.** A function parameter (and method receiver) declares how it uses its
  argument via one of three **modes**:
  - **`in x: T`** — a **shared, read-only borrow**. The callee may read but not
    mutate `x` and does not own it. Multiple `in` borrows may coexist. This is
    the default for receivers written `in self`.
  - **`mut x: T`** — an **exclusive, mutable borrow**. The callee may mutate `x`;
    while the borrow is live no other borrow (shared or exclusive) of `x` exists.
  - **`take x: T`** — the callee **takes ownership** (a move). The caller's value
    is consumed; the callee may mutate or drop it freely.

  The **borrow rule** (frozen): at any point a value has *either* any number of
  `in` borrows *or* exactly one `mut` borrow, never both. This is checked
  statically. These three modes are the complete set for v0.
- **Rationale.** Naming the three modes with distinct keywords (rather than
  sigils like `&`/`&mut`) makes the caller/callee ownership contract explicit and
  legible — a generator picks one of three named intents rather than composing
  reference/mutability sigils correctly. The shared-XOR-exclusive borrow rule is
  the classic guarantee that eliminates data races and iterator-invalidation-
  style aliasing bugs at compile time.
- **Rejected.** *`&` / `&mut` sigils* (Rust): terse but easy to misplace and a
  common generation error; the named modes carry the same guarantee more
  legibly. *Only `in`/`take` (no `mut`)*: would force ownership transfer for all
  mutation, hurting ergonomics and performance. *Unrestricted aliasing*:
  reintroduces data races.
- **Status.** **Frozen** (the three modes and the borrow rule). Whether `mut`
  borrows may be re-borrowed/split: **Experimental**.

## 23. Null policy

- **Design.** **There is no `null`, `nil`, or nullable reference in tuonelang.**
  Absence is expressed only as `Option[T]` (§14). Every binding of type `T`
  holds a valid `T`. References/borrows (§22) are always valid for their
  duration — there is no null borrow.
- **Rationale.** Eliminating null at the type level removes null-dereference
  crashes as a category and makes "can this be missing?" a visible, checked part
  of every type. This is one of the highest-leverage reliability decisions for
  both humans and generators.
- **Rejected.** *Nullable references*: the canonical unsafe default.
  *Null with static null-checking* (Kotlin-style `T?`): better than C, but
  `Option[T]` already provides this uniformly with the enum machinery and without
  a second, special-cased mechanism.
- **Status.** **Frozen.** Non-negotiable in v0.

## 24. Integer overflow policy

- **Design.** Signed and unsigned integer arithmetic that **overflows is a
  defined error, never silent wraparound**:
  - In **checked builds** (the default, incl. debug and the reference
    interpreter), overflow **panics** deterministically (aborts with a
    diagnostic; §20 distinguishes this from recoverable errors).
  - In **release builds**, overflow still traps by default; a program may opt
    specific operations into explicit **wrapping**, **saturating**, or
    **checked** (`-> Option`) arithmetic via named operations
    (e.g. `a.wrapping_add(b)`, `a.checked_add(b)`), which are the *only* way to
    get non-trapping behavior.
  Division by zero and remainder by zero likewise trap deterministically.
- **Rationale.** Silent two's-complement wraparound is a classic source of
  security and correctness bugs; making overflow a loud, deterministic failure
  (and forcing the programmer to *name* their intent when they want wrapping)
  turns a silent hazard into a visible, testable decision. Deterministic trapping
  keeps interpreter and native behavior identical, which TDG specs depend on.
- **Rejected.** *Silent wraparound default* (C unsigned, release-mode default in
  some languages): invisible bugs. *Undefined behavior on signed overflow* (C):
  the worst option — unpredictable and unsafe. *Always-wrapping*: surprising for
  arithmetic that "should" be exact.
- **Status.** **Frozen** (overflow traps by default; opt-in wrapping/saturating/
  checked). The exact spelling of the named operations: **Experimental** (§26).

## 25. `Box`, `Shared`, `Weak`

- **Design.** Three library-provided ownership wrappers, reserved as keywords
  (§4) for clarity and future syntax:
  - **`Box[T]`** — a uniquely-owned heap allocation. Single owner, moved like any
    value, dropped deterministically. Used for recursive types and for putting a
    large/dynamically-sized value on the heap.
  - **`Shared[T]`** — **shared ownership** via atomic reference counting. Cloning
    a `Shared` increments the count; the value is dropped when the last `Shared`
    goes away. Enables multiple owners when a single owner is genuinely
    insufficient. `Shared` grants **shared read** access; interior mutation
    requires an explicit synchronized cell type (stdlib, §26).
  - **`Weak[T]`** — a **non-owning** reference to a `Shared[T]` that does not keep
    it alive and must be **upgraded** to an `Option[Shared[T]]` before use
    (yielding `None` if the value is gone). `Weak` is how ownership **cycles are
    broken**.
- **Rationale.** Making `Box`/`Shared`/`Weak` distinct, named types keeps the
  common case (unique ownership, no wrapper) free of cost and reserves reference
  counting for where it is explicitly chosen — no hidden RC. `Weak` gives a
  principled answer to reference cycles (which pure RC leaks). Distinct names
  make the ownership/cost model legible to humans and generators.
- **Rejected.** *Implicit/pervasive reference counting*: hidden cost and cycle
  leaks. *GC to handle cycles*: rejected in §21. *A single `Rc` that is sometimes
  atomic*: making atomicity explicit avoids accidental cross-thread misuse.
- **Status.** **Frozen** (the three wrappers and their ownership meaning).
  Whether `Shared` is always atomic or has a non-atomic sibling:
  **Experimental** (tracked in open questions).

## 26. The `unsafe` boundary

- **Design.** tuonelang is memory-safe by construction; the small set of
  operations that cannot be statically proven safe (raw pointer dereference, FFI
  calls, and a few stdlib intrinsics) are confined to explicitly marked
  **`unsafe { … }`** blocks and **`unsafe fn`** declarations. Outside `unsafe`,
  the language guarantees memory and type safety. `unsafe` **does not** disable
  the type checker, the borrow rule for safe operations, or overflow checks — it
  only unlocks the specific extra operations. The safe standard library is
  responsible for exposing sound abstractions over its internal `unsafe` use;
  application code is expected to need `unsafe` rarely or never.
- **Rationale.** A minimal, clearly delimited `unsafe` boundary is the pragmatic
  way to build a safe language that can still do FFI and low-level work: it
  concentrates the audit surface into greppable regions while keeping the vast
  majority of code provably safe. Making `unsafe` narrow (not a blanket "turn off
  all checks") preserves as many guarantees as possible even inside it.
- **Rejected.** *No escape hatch at all*: cannot do FFI or implement the stdlib's
  low-level pieces. *Broad `unsafe` that disables all checking*: too large an
  audit surface and too easy to misuse.
- **Status.** **Frozen** (existence and delimitation of `unsafe`; that it does
  not disable orthogonal checks). The exact intrinsic set: **Experimental**.

## 27. Spec semantics (Test Driven Generation)

- **Design.** **Specifications are a first-class, colocated language construct**,
  not an external test framework. A `spec` block states executable expectations
  about the code it accompanies:

  ```
  spec "add is commutative" {
      given a: Int, b: Int;
      when let s1 = add(a, b);
      when let s2 = add(b, a);
      then s1 == s2;
  }
  ```

  Frozen semantics:
  - A `spec` is **executable**: it runs on the **MIR interpreter** (§ARCHITECTURE)
    over the *same* verified MIR the native backends compile, so a spec's verdict
    is backend-independent.
  - `given` introduces inputs; `when` performs steps/bindings; `then` states
    conditions that must hold. A spec **passes** iff every `then` holds for the
    covered inputs and no `then` triggers a deterministic panic (§24).
  - Specs are **pure with respect to program state**: a spec may not mutate
    global state observable outside it, so running specs is deterministic and
    order-independent.
  - Specs are addressable/hashable so tooling and agents can reference a specific
    spec and detect when its meaning changed (the deterministic-build hash, §28).
- **Rationale.** Colocating executable specs with code is the defining feature of
  the TDG paradigm tuonelang is built for: it gives generators and agents an
  in-language, machine-checkable statement of intent, run on the canonical
  semantics (the interpreter over shared MIR) so "the spec passes" means the same
  thing everywhere. Purity + determinism make specs a reliable signal rather than
  a flaky one.
- **Rejected.** *External test framework only*: specs would live outside the
  language, could drift from the code, and would not share the single-MIR
  semantics. *Non-deterministic/impure specs*: flaky signals that agents can't
  trust.
- **Status.** **Frozen** (specs are in-language, executable on shared MIR,
  pure/deterministic). The **surface syntax** of `given/when/then` is
  **Experimental** and tracked in `grammar.ebnf`/open questions; the *semantic
  contract* above is frozen.

## 28. Deterministic build requirements

- **Design.** A tuonelang build is **reproducible**: compiling the same source
  package set with the same compiler version and the same declared configuration
  produces **bit-identical** artifacts and **bit-identical** spec/interpreter
  results. This requires and freezes:
  - deterministic source input (§1) and identifier normalization (§2);
  - fixed numeric semantics with no value-changing float optimization (§11);
  - deterministic overflow/panic behavior (§24);
  - no dependence on wall-clock time, environment ordering, hash-map iteration
    order, or address values in any observable compiler or program-under-spec
    behavior;
  - a stable, content-addressed representation of MIR so that spec identities and
    build outputs hash reproducibly.
- **Rationale.** Determinism is a stated project goal and a precondition for TDG:
  agents and CI must be able to trust that a green spec run reproduces. It also
  enables caching and verifiable builds.
- **Rejected.** *Best-effort/"mostly reproducible"*: undermines the trust that
  makes automated generation-and-verify loops useful. *Allowing nondeterministic
  fast-math or platform-dependent ints*: rejected in §11/§10.
- **Status.** **Frozen** as a **requirement** (the guarantee). The concrete
  mechanisms (hashing scheme, config surface): **Experimental** (§30, ADRs).

## 29. Package naming

- **Design.** A **package** is the unit of distribution and the root of an
  absolute module path (§7). Package names are **lowercase**, must match
  `[a-z][a-z0-9_]*`, are globally unique within a given registry namespace, and
  map 1:1 to a directory containing a manifest. A package declares its name and
  its **edition** (§30) in its manifest. The package name is also the default
  root module name.
- **Rationale.** A single, restricted naming scheme (lowercase, underscore)
  removes case-sensitivity and confusable-name issues across filesystems and
  registries, and gives generators one predictable form. Tying package = path
  root keeps §7's absolute paths unambiguous.
- **Rejected.** *Mixed-case or dotted package names*: filesystem case-folding
  hazards and path ambiguity. *Multiple packages per directory*: breaks the
  file/module/package mapping.
- **Status.** **Frozen** (naming charset and package = path root/distribution
  unit). Registry/namespace details: **Experimental** (part of the package tool,
  not the language core).

## 30. Language edition / version rules

- **Design.** tuonelang evolves via **editions**. This document defines
  **edition `v0`**. Rules:
  - Every package declares the edition it is written for; the compiler applies
    that edition's rules to that package. Packages of different editions can
    interoperate through a stable ABI/type interface.
  - Within an edition, changes must be **backward compatible**: v0 will accept
    additive, non-breaking changes (e.g. new stdlib items, new reserved words
    already reserved in §4) without breaking existing v0 programs.
  - **Breaking** changes (new keywords not previously reserved, changed
    semantics of a frozen decision) require a **new edition** and an ADR, and
    old-edition code keeps compiling under its own edition.
  - v0 is explicitly a **pre-stability** edition: items marked *Experimental*
    above may change **within** v0 without an edition bump; items marked *Frozen*
    may not.
- **Rationale.** Editions let the language improve without a flag-day break,
  which matters for a corpus of machine-generated code that cannot all be
  hand-migrated. Distinguishing Frozen from Experimental *inside* v0 gives a
  precise, honest contract about what is safe to depend on now.
- **Rejected.** *A single ever-mutating language with no versioning*: silently
  breaks existing code and corpora. *Freezing everything immediately at v0*:
  premature; the Experimental areas genuinely need implementation experience.
- **Status.** **Frozen** (the edition mechanism and the Frozen/Experimental
  contract). The set of editions beyond v0: not yet defined.

---

## Consistency note: type system ↔ memory model

The type system (§8–§20) and the memory model (§21–§26) are designed to be
**mutually consistent**, with no contradictions:

- **Value semantics is the single foundation.** Structs (§12) and enums (§13)
  have value semantics; `Copy` is derived precisely when all fields are `Copy`
  (§21). Generic code (§18) is monomorphized, so ownership behavior is resolved
  per concrete type — a generic `T` behaves exactly as its instantiation's
  ownership rules dictate.
- **Borrowing and the type system agree on aliasing.** The `in`/`mut`/`take`
  modes (§22) and the shared-XOR-exclusive borrow rule are the *only* aliasing
  mechanism; there is no separate reference type in the type system that could
  bypass them. Interface receivers (§19) are expressed in the same three modes,
  so method calls obey the same borrow rule as free functions.
- **No null, no uninitialized reads.** §23 (no null) and §9 (definite
  assignment) jointly guarantee every value of type `T` is a valid, initialized
  `T`; `Option[T]` (§14) is the *only* representation of absence and is an
  ordinary enum, so it participates in exhaustiveness (§17) like any other type.
- **Ownership wrappers are types, checked like types.** `Box`/`Shared`/`Weak`
  (§25) are library types with ordinary ownership behavior; `Weak`'s required
  upgrade-to-`Option` (§25) is enforced by the type system, so cycle-breaking
  cannot silently produce a dangling access.
- **Errors and panics do not smuggle control flow past ownership.** `Result` +
  `?` (§20) are ordinary values and expressions; propagation via `?` is a normal
  early return that runs destructors (§21) correctly. Panics (§24) unwind-or-
  abort deterministically and are not catchable as values, so they cannot be
  used to violate the borrow rule.
- **Determinism is preserved end to end.** Fixed numeric semantics (§11),
  trapping overflow (§24), and the no-nondeterminism rule (§28) hold in both the
  MIR interpreter and native backends, so a spec (§27) has one meaning
  everywhere.

No decision in the type system requires a capability the memory model forbids,
and no memory-model rule depends on a type-system feature deferred out of v0.

---

## Index of v0 status

**Frozen (v0 commitments):** source encoding & `.tuo` (§1); identifiers + NFC
(§2); comment forms (§3); reserved keyword set (§4); braces + semicolons + block-
expression (§5); file = module, private default (§6); explicit non-glob imports
(§7); typed params, no overloading (§8); `let`/`var`/`const`, immutable default,
definite assignment (§9); primitive width set, no implicit conversion (§10);
`Int=I64`/`Float=F64` + IEEE-754 no-fast-math (§11); named-field structs (§12);
ADT enums (§13); `Option` (§14); `Result` (§15); control-flow set + exhaustive
match (§16, §17); bracketed generics + monomorphization, no user lifetimes v0
(§18); interfaces-only + coherence (§19); Result+`?`, no exceptions (§20);
ownership terminology (§21); `in`/`mut`/`take` + borrow rule (§22); no null
(§23); trapping overflow (§24); `Box`/`Shared`/`Weak` (§25); `unsafe` boundary
(§26); spec semantic contract (§27); deterministic-build guarantee (§28);
package naming (§29); edition mechanism (§30).

**Experimental (open within v0):** tab prohibition (§1); directory/`mod.tuo`
convention & relative paths (§6, §7); dynamic-dispatch spelling & associated
types (§19); `?` error-conversion rule (§20); `mut` re-borrowing (§22); named
overflow-op spellings (§24); `Shared` atomicity (§25); intrinsic set (§26);
`given/when/then` surface syntax (§27); reproducibility mechanisms (§28);
registry details (§29). Deferred entirely from v0: tuple structs (§12), range /
`ref` patterns (§17), const generics / HKT (§18).

See `open-questions.md` for the tracked unresolved questions behind the
Experimental items.
