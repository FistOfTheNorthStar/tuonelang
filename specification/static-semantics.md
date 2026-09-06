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
| `R0002` | Undefined name. When the name is one a **builtin** module owns (tuonelang is free functions only, so `len` is spelled `std::array::len` / `std::map::len` / `std::str::len` / `std::string::len`), the diagnostic lists the owning modules; a name owned by exactly one module carries a machine-applicable edit that qualifies the call. |
| `R0003` | Unresolved import. |
| `R0004` | Ambiguous name. |
| `R0005` | Visibility violation. |
| `R0006` | Non-function spec target (a `spec` naming something that is not a function). |
| `R0007` | Spec reaches an effectful function (§3.6) — specs execute only the pure core. |

`R0007` belongs to the spec-semantics code group that `R0006` opened. It is
*computed* by the type-checking stage (the effect discipline is part of the type
system, §3.6), but its number lives here because diagnostic codes are stable
data grouped by rule family, and both codes pin what a `spec` block may name or
reach.

### 2.4 Builtin functions (`[EXPERIMENTAL]`, ADR-0006/ADR-0009 Stage A)

The language provides **builtin functions** that resolve without any
declaration, exactly as the prelude's `Option`/`Some`/`None`/`Result`/`Ok`/`Err`
resolve without one. They live in four real, always-present modules —
`std::rt` (the effect builtins, ADR-0006 + ADR-0009), `std::str` (the pure
`Str` builtins, ADR-0006), and `std::string` / `std::array` (the pure
allocator-core builtins, ADR-0009) — and are reached by ordinary path
resolution (`std::rt::write(1, "x")`, `std::string::concat("a", "b")`); they
have **no tuonelang bodies** and are not loadable source (the stdlib's `.tuo`
modules are a separate, host-loaded mechanism).

Because `std` and its four submodules are real modules in every resolution:

- a file declaring `module std::rt;` (or `std::str`, `std::string`,
  `std::array`) shares that module, and declaring a function with a builtin's
  name there collides with it — an ordinary `R0001` duplicate definition. The
  builtins are not shadowable *at their own paths*.
- a local binding or module-level declaration named `std` shadows the module in
  that scope, exactly as any name shadows any other (§2.2); the builtins are
  then unreachable from that scope, never silently rebound.

Calls to the builtins are checked as ordinary calls (arity `T0002`, argument
types `T0001`, modes by the ownership checker — a `let`-bound value passed to
a `mut` parameter such as `std::string::push_byte`'s is `O0004`); their
signatures are fixed in §3.6 and §3.7.

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
| `T0001` | Mismatched types (carries structured expected/actual). On an argument to a **builtin** whose short name several modules share, the diagnostic also names the sibling whose first parameter accepts the type actually passed — the wrong-module case, where the mismatch's real cause is the module rather than the argument. The hint appears only when such a sibling exists; it is never guessed. |
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
| `T0015` | Not first-class: a builtin or generic function used as a value (ADR-0008 Tier 1, §3.8). Only ordinary top-level user `fn`s become function values. |
| `T0016` | Recursive type without a heap-wrapper indirection (ADR-0016, §3.7): a struct/enum reaches itself through its own fields/payloads. Only `Box`/`Shared`/`Weak` break a cycle. |
| `T0017` | Data-dependent control flow in a `#[constant_time]` function (ADR-0020 Stage C, §3.10): `if`, `match`, `while`, `loop`, or `for`. |
| `T0018` | Array indexing in a `#[constant_time]` function (§3.10): the bounds check is a conditional branch on the index. |
| `T0019` | Trapping arithmetic (`+ - * / %`) in a `#[constant_time]` function (§3.10): the overflow check is a conditional branch on the operands. |
| `T0020` | A `#[constant_time]` function calls a function that is not marked (§3.10), so the callee carries no checked guarantee. |
| `T0021` | Unknown attribute (§3.10). v0 defines exactly one, `#[constant_time]`; an unrecognized name is an error, never a silently-ignored annotation. |
| `T0022` | **Warning, not an error** — the runnable-core advisory. A heap-wrapper (`Box`/`Shared`/`Weak`) **value** in a parameter, return, or `let`/`var` position is accepted here and executes on the reference interpreter, but no native backend lowers it, so `tuo build`/`tuo run` refuse it. Wrapper **declarations** (struct fields, enum payloads) are not reported — they lower fine, and `T0016` recommends exactly that indirection. |

`T0022` is the table's only warning: it reports a program the static
semantics **accepts**. It exists because `tuo check` deliberately accepts a
larger language than the native backends lower, and that gap should be
visible at the offending type rather than surfacing later as a spanless
build failure. Because it is a warning it never changes whether a program is
accepted, and the set of programs this document defines as well-typed is
exactly what it was before the advisory existed.

Ownership-flavored array rules live in `specification/ownership.md`: indexing
reads an element out of `Array[T]` **and** `[T; N]` alike (only `Copy`
elements by value, `O0007`), and the repeat literal `[x; N]` requires a `Copy`
element (`O0010`).

### 3.6 The effect system (`[EXPERIMENTAL]`, ADR-0006 Stage A)

v0 gains one narrow, explicit effect seam: the **effect builtins** in
`std::rt`, plus three **pure string builtins** in `std::str` (no effect). All
are ordinary functions to the checker — fixed, non-generic signatures,
normal call checking — resolved per §2.4.

**Effect builtins (`std::rt`):**

| Signature | Meaning |
|-----------|---------|
| `fn write(take fd: Int, in text: Str) -> Int` | Writes `text`'s bytes to file descriptor `fd`. Returns the number of bytes written, or a negative value on host error. **Never traps.** |
| `fn read_byte(take fd: Int) -> Int` | Reads one byte from `fd`. Returns `0..=255`, or `-1` on end of input, or another negative value on host error. **Never traps.** |
| `fn exit(take code: Int) -> Int` | Terminates the process with `code & 0xff` as the exit status. Declared as returning `Int` so it composes in expression position, but it **never returns**. |
| `fn write_string(take fd: Int, in s: String) -> Int` | (ADR-0009) Writes the owned `String`'s bytes to `fd` — the `String` is lent read-only for the call, so no `String`→`Str` view is needed (Q-0012 stays deferred). Same contract as `write`: bytes written or a negative host-error value. **Never traps.** |
| `fn par_map(take f: fn(take Int) -> Int, in tasks: Array[Int], take workers: Int) -> Array[Int]` | (ADR-0007) Applies the non-capturing function value `f` to every task on `workers` OS threads (round-robin: task `i` on thread `i % workers`; below 1 behaves as 1), joining every thread before returning, and yields the results **in task order**. Structured fork-join: nothing outlives the call, the tasks array is lent read-only, and only `Copy` values and disjoint result slots cross a thread boundary — no data race is expressible through it. Deterministic in its result whenever `f` is pure. **Never traps.** Spawning is an effect: a spec that could reach it is `R0007`. |
| `fn now_nanos() -> Int` | (ADR-0013) The monotonic clock, in nanoseconds since an arbitrary process-local epoch; only differences are meaningful. Effectful because non-deterministic. **Never traps.** |
| `fn arg_count() -> Int` | (ADR-0013) The number of process arguments, **including** the program name (argv[0]). **Never traps.** |
| `fn arg_byte(take i: Int, take j: Int) -> Int` | (ADR-0013) Byte `j` (`0..=255`) of process argument `i`, or `-1` when `i` is out of range or `j` is past that argument's end — the `read_byte` EOF convention over argv. **Never traps.** |
| `fn open(in path: Str, take mode: Int) -> Int` | (ADR-0013) Opens `path`; returns a file descriptor (`>= 0`), `-2` when the path does not exist, or another negative value on host error (an unknown `mode` included). Modes: `0` read, `1` write (create + truncate), `2` append (create). The returned descriptor is exactly what `write`/`read_byte`/`write_string` accept — file I/O is the composition of `open` with the descriptor seam. **Never traps.** |
| `fn close(take fd: Int) -> Int` | (ADR-0013) Closes `fd`; `0` on success, negative on host error. **Never traps.** |
| `fn remove_file(in path: Str) -> Int` | (ADR-0013) Removes the file at `path`; `0` on success, `-2` when it does not exist, another negative value on other host errors. **Never traps.** |
| `fn listen(take port: Int) -> Int` | (ADR-0014) Creates an IPv4 TCP socket bound to `127.0.0.1:port` and listening; returns the listening descriptor (`>= 0`) or a negative value on host error. Port `0` requests an ephemeral port — pair with `bound_port`. **Never traps.** |
| `fn bound_port(take fd: Int) -> Int` | (ADR-0014) The local port a listening descriptor is bound to, or a negative value on host error. **Never traps.** |
| `fn accept(take fd: Int) -> Int` | (ADR-0014) Accepts one pending connection; the connected descriptor (`>= 0`) or a negative value on host error. Blocks. The descriptor is exactly what `write`/`read_byte`/`close` accept — socket I/O is the composition of the producers with the descriptor seam. **Never traps.** |
| `fn connect(in host: Str, take port: Int) -> Int` | (ADR-0014) Opens a TCP connection to the numeric IPv4 `host:port` (no name resolution); the connected descriptor (`>= 0`) or a negative value on host error. **Never traps.** |
| `fn chan_new() -> Int` | (ADR-0015) Creates an unbounded FIFO channel of **non-negative** `Int` values; a process-lived handle (`>= 0`) or `-1` on registry exhaustion. **Never traps.** |
| `fn chan_send(take ch: Int, take v: Int) -> Int` | (ADR-0015) Enqueues `v`; `0` on success, `-1` on an invalid handle, a closed channel, or a negative `v` (refused so `chan_recv`'s `-1` stays unambiguous). **Never traps.** |
| `fn chan_recv(take ch: Int) -> Int` | (ADR-0015) Dequeues the oldest value, **blocking** until one is available; the value, or `-1` once the channel is closed and drained (or the handle is invalid). **Never traps.** |
| `fn chan_close(take ch: Int) -> Int` | (ADR-0015) Closes the channel — sends refused, every blocked or future receive returns `-1` after the queue drains. `0` on success (idempotent), `-1` on an invalid handle. **Never traps.** |
| `fn mutex_new() -> Int` | (ADR-0015) Creates a mutex; a process-lived handle (`>= 0`) or `-1` on registry exhaustion. **Never traps.** |
| `fn mutex_lock(take m: Int) -> Int` | (ADR-0015) Acquires, **blocking** until available; `0` on success, `-1` on an invalid handle or an error-checked relock by the holding thread — a mistake is a code, never undefined behavior. **Never traps.** |
| `fn mutex_unlock(take m: Int) -> Int` | (ADR-0015) Releases; `0` on success, `-1` on an invalid handle or when the caller does not hold it. **Never traps.** |

**Pure string builtins (`std::str`)** — byte-level operations on the UTF-8
buffer of a `Str` (a `slice` may split a multi-byte code point; that is the v0
contract, see ADR-0006's amendments):

| Signature | Meaning |
|-----------|---------|
| `fn len(in s: Str) -> Int` | The byte length of `s`. Never traps. |
| `fn byte_at(in s: Str, take index: Int) -> Int` | The byte at `index` (`0..=255`). **Traps `IndexOutOfBounds`** when `index < 0` or `index >= len(s)`. |
| `fn slice(in s: Str, take start: Int, take end: Int) -> Str` | The byte range `[start, end)` of `s`. **Traps `IndexOutOfBounds`** unless `0 <= start <= end <= len(s)`. |

**Purity is computed per function, transitively.** A function is **effectful**
iff its body contains a call to (or any reference to) an effect builtin, or
transitively calls an effectful function. The computation is a fixed point over
the program's call graph (cycles converge: a recursive cluster is effectful iff
some member reaches an effect), performed by the type-checking stage over the
resolved call edges of every checked body; it is deliberately **conservative** —
merely naming an effectful function as a value taints the referrer, which can
never wrongly accept. The result is queryable from the check pipeline's
`TypeckResult` (`is_effectful`), so the spec runner, the CLI, and the agent
read one computation. `std::str`'s builtins are **pure**; `main` and ordinary
functions **may** be effectful — no diagnostic attaches to them.

**Specs execute only pure code.** A `spec` whose executed closure — its
resolved dependencies (§2, ADR-0002) transitively closed over the call graph —
would include an effectful function is rejected **statically** with `R0007`,
before the interpreter is ever involved: "spec `X` reaches the effectful
function `Y`; specs execute only the pure core in the deterministic sandbox."
This turns the spec sandbox's honesty split (the interpreter performs no I/O,
ever — [`mir.md`](mir.md) §8) into a *checked* property: `tuo check`, `tuo
spec`, and `tuo verify` all refuse such a spec as a front-end error. The rule
applies to direct calls, transitive calls, and `std::rt::exit` alike —
and, since ADR-0009, to `std::rt::write_string`.

### 3.7 The allocator-core builtins (`[EXPERIMENTAL]`, ADR-0009 Stage A)

Two further builtin modules provide the owned, growable heap values — checked
exactly like the §3.6 builtins (fixed, non-generic signatures; arity `T0002`,
argument types `T0001`; modes by the ownership checker). **Every
`std::string`/`std::array` builtin is pure**: allocation is deterministic
computation, not I/O, so specs may build strings and arrays freely (bounded by
the interpreter sandbox's memory budget — [`mir.md`](mir.md) §8.1). Only the
`std::rt` builtins are effectful.

**Owned-string builtins (`std::string`)** — `String` is a UTF-8-by-convention
**byte buffer**; like `std::str`, these are byte-level operations, and a
`slice` may split a multi-byte code point:

| Signature | Meaning |
|-----------|---------|
| `fn empty() -> String` | The empty string. Never traps. |
| `fn from_str(in s: Str) -> String` | Copies `s`'s bytes into a new owned buffer. Never traps. |
| `fn push_byte(mut s: String, take b: Int)` | Appends one byte. **Traps `InvalidByte`** when `b < 0` or `b > 255` — never silently masked. |
| `fn append(mut s: String, in t: Str)` | Appends `t`'s bytes. Never traps. |
| `fn concat(in a: Str, in b: Str) -> String` | A new owned buffer holding `a`'s bytes then `b`'s. Never traps. |
| `fn len(in s: String) -> Int` | The byte length of `s`. Never traps. |
| `fn byte_at(in s: String, take i: Int) -> Int` | The byte (`0..=255`) at `i`. **Traps `IndexOutOfBounds`** when `i < 0` or `i >= len(s)`. |
| `fn slice(in s: String, take a: Int, take b: Int) -> String` | The byte range `[a, b)` **copied out** as a new owned `String` (no aliasing view). **Traps `IndexOutOfBounds`** unless `0 <= a <= b <= len(s)`. |
| `fn as_str(in s: String) -> Str` | (ADR-0010) A borrowed `Str` **view** of the whole `String` — the `{ptr, len}` prefix of its header, no copy. The result is a **shared borrow** of `s`: while it is live, `s` may not be moved, mutated, dropped, or overwritten, and it may not escape its frame (`O0011`; see `ownership.md` §13). Never traps. |

**Growable-array builtins (`std::array`)** — **element-generic** over `Array[T]`
since ADR-0012 (they were `Array[Int]`-monomorphic in ADR-0009). `T` is
**witnessed by the receiver**: `push`/`pop`/`get`/`len` take `Array[T]` and the
value/result follow that `T`, unified from the first argument; `empty()` gives
`Array[T]` for a `T` solved from the use site (an annotation or a later `push`).
These are not user-written generics — the operations stay language-provided
builtins whose element type is resolved from the call, not user type parameters:

| Signature | Meaning |
|-----------|---------|
| `fn empty() -> Array[T]` | The empty array. `T` is inferred from context; an undetermined `T` is `T0011` ("type annotation needed"), never silently `Int`. Never traps. |
| `fn push(mut xs: Array[T], take v: T)` | Appends `v`. Never traps. |
| `fn pop(mut xs: Array[T]) -> Option[T]` | Removes and returns the last element; `None` when empty. Never traps. |
| `fn len(in xs: Array[T]) -> Int` | The element count. Never traps. |
| `fn get(in xs: Array[T], take i: Int) -> T` | The element at `i`. **Traps `IndexOutOfBounds`** when `i < 0` or `i >= len(xs)`. |
| `fn set(mut xs: Array[T], take i: Int, take v: T)` | (ADR-0016) Overwrites the element at `i` with `v` in place — the previous element is dropped. **Traps `IndexOutOfBounds`** on `get`'s bounds; `set` never grows the array. The one in-place element write, mirroring `get` as the one read. |

The **v0-supported element set** is the scalars `Int`/`Float`/`Bool`/`Str`/
`String` (`Float` since ADR-0016) and user structs/enums whose fields are
themselves supported; an element outside it (a nested `Array`/`Map`, a
`Box`/`Shared`/`Weak` wrapper) is an ordinary `T0001`. The reference
interpreter **and both native backends** run every supported element type:
since the ADR-0012 owned-element increment, a heap-owning element (`String`,
or a struct/enum carrying one) gets a native deep copy on `get` and recursive
per-element drop glue matching the interpreter's clone/drop semantics
(`abi.md` §Arrays). Only an element containing a heap wrapper would be
refused natively — and the wrapper is already outside the type-level set, so
the refusal is unreachable from checked code.

**The recursion boundary (`T0016`, ADR-0016).** A struct or enum that reaches
itself through its own fields or variant payloads — by value, or through any
chain of `Array`/`Map` elements, `Option`/`Result` payloads, tuple or
fixed-array components, or generic type arguments — is a **type error at the
declaration**. v0 cannot represent such a type: a by-value cycle has infinite
size, and a container cycle would recurse the native backends' inline
clone/drop glue emission without bound. The one legal indirection is a heap
wrapper (`Box`/`Shared`/`Weak`): its layout is a bare pointer, so a
wrapper-broken cycle (the `Weak` back-edge shapes) declares fine. The
diagnostic points at the wrapper escape hatch and at the index-arena pattern
(`std::json`'s tree). A successor ADR that gives the backends
runtime-recursive glue functions relaxes this boundary.

**String equality.** `==`/`!=` on two `String` operands is **byte-wise content
equality**, the same contract as `Str == Str` (§3.4); the comparison consumes
neither operand. Ordering operators on either string type remain unsupported
(`T0009`-family "operator not supported"), and `String` never compares equal
to `Str` at the type level — type equality is exact, so mixing them is
`T0001`.

`String` and `Array[T]` remain **non-`Copy`** ([`ownership.md`](ownership.md)
§2): a `take` argument moves them, and a `mut` argument requires a mutable
place (`O0004` otherwise).

**Hash-map builtins (`std::map`)** — the builtin `Map[K, V]` (ADR-0011). The
type is generic and non-`Copy` (it owns a heap table); the v0 **operation
surface** is `Map[Int, Int]` and `Map[Str, Int]` — the scalar keys whose
equality and hash the language owns, with `Int` values. `K`/`V` are witnessed
by the receiver exactly as the array element is; any other *determined* pair
is a `T0001` naming the pair, and an undetermined `empty()` is `T0011`:

| Signature | Meaning |
|-----------|---------|
| `fn empty() -> Map[K, V]` | The empty map. `K`/`V` inferred from context. Never traps. |
| `fn insert(mut m: Map[K, V], take k: K, take v: V) -> Option[V]` | Insert/overwrite `k → v`; the **previous** value comes back (`Some`) or `None` when `k` was absent. A fresh key appends to the insertion order; an overwrite keeps the key's position. Never traps. |
| `fn get(in m: Map[K, V], take k: K) -> Option[V]` | The value for `k`, or `None`. Never traps. |
| `fn contains_key(in m: Map[K, V], take k: K) -> Bool` | Is `k` present? Never traps. |
| `fn remove(mut m: Map[K, V], take k: K) -> Option[V]` | Remove `k`, returning its value (`Some`) or `None`. The remaining entries keep their relative insertion order. Never traps. |
| `fn len(in m: Map[K, V]) -> Int` | The entry count. Never traps. |
| `fn keys(in m: Map[K, V]) -> Array[K]` | A **new** array of the keys in **insertion order** — deterministic and identical across runs and engines (the ADR-0011 rule; a program that needs sorted keys composes `std::collections::sorted_ascending`). Never traps. |

Key equality is the scalar `==` (`Int` by value, `Str` by byte content); the
bucket-placing hash is language-owned, fixed, unseeded, and **unobservable**
(`abi.md` §Maps). **`Map == Map` is deferred**: structural equality over an
unordered container needs a canonical comparison the v0 core does not define,
so `==`/`!=` on map operands is a `T0006` "operator not supported" — specs
compare observations (`get`/`len`/`keys`), the established heap-spec idiom.

### 3.8 First-class function values (`[EXPERIMENTAL]`, ADR-0008 Tier 1)

v0 gains **non-capturing function values**: a bare `fn` name, used in value
position, is a value of a **function type**.

**The function type.** `fn(P1, …, Pn) -> R` where each `Pi` is a **mode**
(`in`/`mut`/`take`) followed by a type, and `-> R` is always written (there is
no unit default in the *type* syntax — a unit-returning function value has type
`fn(…) -> ()`). Modes are **mandatory**: the ownership vocabulary is part of a
function's calling contract, and a function value is a code pointer with a known
ABI, so it must carry the modes the indirect-call site drives its borrows from.

**Type equality is exact, modes included.** Two function types are the same type
iff they have equal arity, pairwise-equal parameter types, **pairwise-equal
modes**, and equal return types. `fn(take Int) -> Int` and `fn(in Int) -> Int`
are *distinct* types (exactly as `I32` never equals `I64`). A function type is
**`Copy`** and **non-heap** (a single code pointer); it is not `Drop`.

**Producing a function value.** A path resolving to an **ordinary top-level user
`fn`**, used in value position (not immediately called), has that function's
signature-as-function-type. Given `fn add(take a: Int, take b: Int) -> Int`, the
expression `add` has type `fn(take Int, take Int) -> Int`.

Only ordinary top-level user functions are first-class in Tier 1:

- A **builtin** (`std::rt`/`std::str`/`std::string`/`std::array`) used as a
  value is `T0015` — builtins lower to dedicated MIR ops, not a callable code
  pointer, so there is no function value to take.
- A **generic** function used as a value is `T0015` — a function value is a
  single monomorphic code pointer; monomorphizing function values is deferred.
- A **spec** name is not a value (`T0012`, "expected a value"), and method calls
  are already v0 no-ops — neither needs a new rule.

**Calling a function value.** `f(args)` where `f` is an expression of function
type (not a direct function name) is an **indirect call**. The checker verifies
arity (`T0002`) and argument types (`T0001`) against the function type; the
**ownership checker** applies the per-argument borrow rules driven by the
function type's modes (the same rules as a direct call — [`ownership.md`](ownership.md);
`O0004`/`O0005`/`O0006`). Calling a value that is not of function type is
`T0003`. A direct call of a named function keeps its dedicated fast path
(sharper diagnostics; generics); only an indirect callee goes through the
function-type detour.

**Purity (the R0007 spec-gate).** The transitive effect computation of §3.6
already records a call-graph edge for a **value reference** to a function, not
only for a direct call — a reference *taints conservatively*. In Tier 1 the only
way to obtain a function value is a compile-time-known top-level `fn` name, so
this conservative taint is **complete**: any function that names an effectful
function as a value (to pass it, store it, or call it indirectly) is itself in
the effectful closure, and a spec whose execution could reach that value is
refused by `R0007` before the interpreter runs. An indirect call through a
known constant `fn`-value has exactly its target's effectfulness; an indirect
call through a value that flowed through a variable or parameter is treated
conservatively as possibly effectful — but such a value still originates from a
named top-level `fn` whose taint was already recorded, so no spec reaches an
effect through a function value undetected. (Tier 2's captured environments will
revisit this.)

Tier 1 has **no closures, no capture, and no heap**; capturing closures are a
later ADR increment.

### 3.9 Bitwise operators (`[EXPERIMENTAL]`, ADR-0019 Stage A)

The six bitwise operators — `&`, `|`, `^`, `~`, `<<`, `>>` — are checked by
three rules, all of which produce `T0006` ("operation not supported for a
type") when violated.

1. **Integers only.** Every bitwise operator requires integer operands. Floats
   are rejected: a bit pattern is not defined on an IEEE-754 value in v0, so
   `a & b` on `Float` is a type error rather than a reinterpreted operation.
   `Bool` is rejected too — `&&`/`||`/`!` are its operators, and keeping the
   two families apart is what makes `!` (logical) and `~` (bitwise) distinct.
2. **`&`, `|`, `^` unify their operands** exactly as arithmetic does, and the
   expression's type is that shared operand type.
3. **Shifts do *not* unify their operands.** `<<` and `>>` take an integer
   value and an independent integer *amount* — a bit count, not a value of the
   shifted type — so `x << 3` needs no cast on the literal even when `x` is
   `U32`. The expression's type is the **left** operand's type. Both sides
   must still be integers.

Two further facts are *dynamic* rather than static, and so are enforced by a
trap rather than the checker (see [`mir.md`](mir.md) §5.3): `>>` is arithmetic
on a signed type and logical on an unsigned one, and a shift amount outside
`0..width` traps `InvalidShift`.

The surface grammar's `|` is one terminal serving two roles — pattern
alternation and bitwise-or. That is a *syntactic* disambiguation, decided by
grammatical context and owned by the parser (see §4), not a type rule.

### 3.10 Constant-time checking (`[EXPERIMENTAL]`, ADR-0020 Stage C)

A function may carry the attribute `#[constant_time]`, which asks the compiler
to verify that **neither its control flow nor its running time can depend on
the values it is given**. This is the only attribute v0 defines; an
unrecognized one is `T0021` rather than a decoration the compiler ignores,
because a misspelled `#[constant_tim]` that compiled clean would leave an
author believing in a guarantee that was never checked.

Inside a marked function the checker rejects:

| Construct | Code | Why |
|-----------|------|-----|
| `if`, `match`, `while`, `loop`, `for` | `T0017` | control flow branches on data |
| `xs[i]` | `T0018` | the bounds check is a branch on the index |
| `+`, `-`, `*`, `/`, `%` (binary or unary) | `T0019` | the overflow check is a branch on the operands |
| a call to an unmarked function | `T0020` | the callee could contain any of the above |

What remains is the branchless subset: shifts, the bitwise operators
(§3.9), comparisons used as values, `let` bindings, literals, and calls to
other marked functions. `std::ct` is written in exactly this subset.

**The rule is sufficient, not necessary, and that asymmetry is deliberate.**
Some rejected code is genuinely constant time — a subtraction of halved
operands provably cannot overflow, so its trap is unreachable — but the checker
cannot follow that argument, and accepting on an argument it cannot verify is
precisely the unbacked claim this discipline exists to prevent. The intended
response to a rejection is to restructure (as `std::ct::lt` was, to need no
arithmetic at all) or to drop the marking and state plainly that the function
carries no machine-checked guarantee.

**What the attribute does *not* promise.** It constrains the *source*. Whether
the emitted machine code is branch-free is a separate question, checked
separately by disassembling real binaries on both backends
(`crates/tuo-cli/tests/constant_time.rs`), because an optimizer may rewrite a
branchless idiom into a conditional. And whether branch-free instructions
execute in data-independent time is a *hardware* property beyond any compiler's
reach. The attribute is one of three links, and none of the three alone is the
guarantee.

There is deliberately **no escape hatch** — no `#[allow]`, no override. A
function that cannot satisfy the checker simply goes unmarked, which is visible
in the source: `std::ct`'s array scans carry no attribute, because a scan needs
a loop (`T0017`) and indexing (`T0018`) by its nature.

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
| Spec purity: a spec's closure is effect-free (`R0007`, §3.6) | type checker | §27, ADR-0006 |
| Spec determinism; runs on shared MIR | spec runner | §27 |
| Receiver (`self`) only as first parameter of an `impl` method | resolver | §19 |
| Struct-literal-in-condition restriction | parser | `syntax.md` |

The last row is the **only** restriction the parser itself enforces beyond pure
shape; every other row is resolution, typing, ownership, or a runtime stage.

---

## 5. What pins these rules

The static semantics are executable: `tests/types/` is the fixture corpus that
holds this document honest.

- `tests/types/fixtures/ok/` — programs that must resolve **and** type-check with
  **zero diagnostics** (`adts`, `arrays`, `casts`, `control_flow`, `effects`,
  `functions`, `impls`, `inference`, `option_result`, `specs`).
- `tests/types/fixtures/err/` — programs that resolve cleanly but contain type
  errors; their diagnostics (code, span, message, structured expected/actual) are
  snapshotted under `tests/types/snapshots/`. The err stems map onto the code
  groups above: `mismatch` → `T0001`, `arity` → `T0002`, `constructors` →
  `T0005`/`T0010`, `fields` → `T0004`, `operators` → `T0006`, `exhaustive` →
  `T0007`, `casts` → `T0008`, `fallibility` → `T0009`, `annotations` → `T0011`,
  `loops` → `T0013`, `effects` → builtin-call typing plus the `R0007`
  spec-purity gate (§3.6).

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
