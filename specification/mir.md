# The tuonelang MIR semantics (v0)

> **Status.** Normative. This document specifies the meaning of **MIR** — the
> single executable semantic representation of tuonelang. Every instruction's
> semantics are defined here on its type; there is no undefined behavior in MIR.
> This document **consolidates** the semantics already defined on the MIR types
> in [`tuo-mir`](../crates/tuo-mir) (`mir.rs`), the well-formedness invariants of
> the mandatory verifier ([`tuo-mir`'s `verify.rs`](../crates/tuo-mir/src/verify.rs)),
> and the execution and abort model of the reference interpreter
> ([`tuo-mir-interp`](../crates/tuo-mir-interp)). It introduces **no new
> semantics**: where this prose and the crate documentation disagree, the crate
> — and the tests that pin it — win, and this file is corrected to match. It is a
> peer of [`ownership.md`](ownership.md) and [`abi.md`](abi.md).

MIR is the point at which the language's meaning is fixed **once**. The reference
interpreter and every native backend (Cranelift, LLVM) consume the same MIR, so
the semantics are defined by the instruction documentation below and never
re-derived per backend. The interpreter is the reference implementation: the
meaning of a tuonelang program is, by definition, what the interpreter computes
for its MIR. The native backends must agree with the interpreter instruction for
instruction; **where they diverge, the backend is wrong.**

Two invariants frame everything below and are load-bearing:

- **Verification is mandatory.** Every consumer — the interpreter and both
  backends — must reject MIR that does not verify (§7). A consumer is entitled to
  *assume* verified MIR and is forbidden from consuming unverified MIR.
- **The interpreter is the reference on _unoptimized_ MIR.** Optimization is not
  part of the reference semantics: the interpreter executes unoptimized MIR, and
  every optimization pass is a meaning-preserving rewrite that leaves the
  program's observable behavior — its return value and which/whether it traps —
  unchanged.

---

## 1. Structure of a program

A **program** ([`Program`]) is a list of **functions** in program order, plus a
list of **skipped** bodies. A skipped body is one the front end lowered no
further because it uses a construct outside the v0 runnable core; it is recorded
with its name, its span, and the documented v0 limit that caused it — a skipped
body is *absent*, never silently mis-lowered.

A **function** ([`Function`]) is:

- a **symbol** identity and a **name**;
- a list of **parameter modes** (`params`), one per parameter in declaration
  order (§2);
- a list of typed **locals** (`locals`) — parameters first, then user locals and
  compiler temporaries in creation order. Locals `0 .. params.len()` are the
  parameters. Each local (`LocalDecl`) is a typed storage location live for the
  whole call; a temporary has no name;
- a graph of **basic blocks** (`blocks`), block `0` being the entry;
- a return type (`ret`).

A **basic block** ([`BasicBlock`]) is a straight-line sequence of **statements**
(§4) closed by exactly one **terminator** (§6). A `LocalId` indexes
`Function::locals`; a `BlockId` indexes `Function::blocks`.

### Execution model

Execution starts at block `0` with the parameter locals holding the arguments and
every other local **uninitialized**. Reading an uninitialized or moved local is
**statically impossible** in lowered MIR — the ownership checker ran first, and
the verifier re-proves it (§7) — so no instruction below defines a meaning for
such a read. Within a block, **evaluation order is the textual order of
statements**. Everything observable is defined here; there is no undefined
behavior to leave unspecified.

Traps are **deterministic aborts** (Constitution §24): abort with no unwinding,
and destructors do not run. A trap is the same observable event however it
arises — a trapping arithmetic statement (§4, §5.3), a failed `Assert`, or a
`Trap` terminator (§6).

---

## 2. Parameter modes (`PassMode`)

Borrow modes are the only borrow formers of the language: a borrow exists exactly
for the duration of the call, and the callee never drops it (Constitution §9,
§22). Each parameter carries one mode:

| Mode | Source spelling | Meaning |
|------|-----------------|---------|
| **`Value`** | `take` | The argument is moved (or copied, if `Copy`) into the parameter local; the callee owns it like any other local. |
| **`Borrow`** | `in` | The parameter *aliases* the argument place, read-only: reads read through it. |
| **`BorrowMut`** | `mut` | The parameter aliases the argument place exclusively; writes through it are visible to the caller after the call. |

A `Borrow`/`BorrowMut` local aliases the caller's place for the call's duration;
the callee never drops it. A `Value` parameter is owned by the callee.

---

## 3. Naming memory: places and operands

### Places

A **place** ([`Place`]) is a `local` plus a list of **projections** applied left
to right. **Places are the only way MIR names memory.** There is no address-of,
no pointer arithmetic, and no dereference operator; aliasing exists **solely**
through borrow-mode parameters (§2).

Projections ([`Projection`]):

- **`Field(n)`** — the `n`-th field (declaration order) of a struct, tuple, or
  range value. A range has field `0` = start, field `1` = end.
- **`VariantField { variant, field }`** — the `field`-th payload field (declaration
  order) of enum variant `variant`. Defined **only while the discriminant is
  `variant`**; lowering guards every use with a discriminant test, so backends may
  assume it.
- **`Index(i)`** — the element of the **array (growable `Array[T]` or fixed
  `[T; N]`)** at the `Usize` value held by local `i`. Defined **only for
  in-bounds values**; lowering emits an explicit bounds-check
  [`Assert`](#6-terminators) before every use.

### Operands

An **operand** ([`Operand`]) reads a value:

- **`Copy(place)`** — read the place's value, leaving it initialized. Emitted
  for `Copy` types (the copy is bitwise-equivalent and independent) — and, since
  ADR-0009, as the documented **borrow-read** of a `String` operand of an
  `Eq`/`Ne` binary (§5.3): the read leaves the place initialized and untouched,
  and observably copies nothing. Everywhere else a non-`Copy` value is read
  through a *place* (`Len`, `Discriminant`, `HeapOp` subjects, borrow
  arguments) or consumed by `Move`.
- **`Move(place)`** — read the place's value and **end its initialization**: the
  place's contents must not be read or dropped again until reassigned. A move is a
  transfer, never a deep copy.
- **`Const(c)`** — a compile-time constant (§3.1).

### 3.1 Constants (`Const`)

| Constant | Meaning |
|----------|---------|
| `Unit` | `()`. |
| `Bool(b)` | a boolean. |
| `Int(v, kind)` | an integer; `v` is always in the kind's range (lowering rejects out-of-range literals). |
| `Float(v, kind)` | a float; an `F32` is stored at `f64` precision and rounds to nearest even when materialized. |
| `Char(c)` | a Unicode scalar value. |
| `Str(s)` | an immutable view of static UTF-8 text. |
| `Fn(sym)` (`[EXPERIMENTAL]`, ADR-0008 Tier 1) | a **function value**: a compile-time-known code pointer naming the top-level function `sym`. Its type is `sym`'s signature-as-function-type (`Ty::Fn`). `Copy`, non-heap, never traps. See §4.4. |

---

## 4. Statements

A statement never branches. Statements in a block execute in textual order; a
statement that traps (arithmetic, §5.3) aborts the program deterministically.

- **`Assign { place, rvalue }`** — evaluate `rvalue` (§5) and store the result into
  `place`, which must be uninitialized (or `Copy`). Drops of a previously held
  value are always explicit, earlier `Drop` statements — an assign never silently
  overwrites a live value.
- **`Call { dest, callee, args }`** — call `callee` with `args` (§4.1) and store its
  return value into `dest`. Arguments are evaluated before the call; the call
  returns normally or aborts — **there is no unwinding** (Constitution §24). The
  callee is a **`Callee`** (`[EXPERIMENTAL]`, ADR-0008 Tier 1, §4.4):
  `Direct(sym)` names a function symbol (a v0 builtin never reaches here — those
  lower to `Effect`/`StrOp`/`HeapOp`), and `Indirect(operand)` calls through a
  function-value operand of function type. The two forms share **every** other
  mechanism — argument passing, borrows, the destination, the no-unwind rule —
  only the target differs.
- **`Drop { place }`** — destroy the value held by `place`, which must be
  initialized; afterwards the place is uninitialized. Drop glue is
  compiler-generated only — there are **no user destructors in v0** (ADR-0003):
  - a `Box` frees its allocation after dropping the contents;
  - a `Shared` decrements its count and destroys the contents (and both counts'
    storage) when it reaches zero;
  - a `Weak` releases its handle;
  - an `Array` drops elements front to back, then the storage;
  - a `[T; N]` drops elements front to back; its storage is inline and is not
    freed;
  - a `String` frees its buffer;
  - a struct or enum drops its (active variant's) fields in declaration order;
  - every `Copy` value's drop is a **no-op**.

- **`Effect { op, args, dest }`** (`[EXPERIMENTAL]`, ADR-0006 Stage A) — one
  host effect (§4.2). This is the **only** MIR construct whose meaning involves
  the host; everything else in this document is pure computation. Since
  ADR-0009 its arguments are call-style `Arg`s (§4.1), so an effect can borrow
  a non-`Copy` value (`WriteString`'s `String`) exactly as a call does.
- **`HeapMutate { op, target, args, dest }`** (`[EXPERIMENTAL]`, ADR-0009
  Stage A) — mutate the `String`/`Array[I64]` at `target` in place (§4.3),
  through what is semantically a `mut` borrow: `target` must be initialized
  before and remains initialized after. Pure, deterministic computation — an
  allocation is not an effect.

### 4.1 Call arguments (`Arg`)

Each argument names its passing semantics and must agree with the callee's
`PassMode` (§2):

- **`Value(operand)`** — pass by value (a `take` parameter, or any `Copy` argument).
- **`Borrow(place)`** — lend `place` read-only for the duration of the call.
- **`BorrowMut(place)`** — lend `place` exclusively for the call; the callee may
  write through it.

### 4.2 Effects (`EffectOp`) — `[EXPERIMENTAL]`, ADR-0006 Stage A

`Statement::Effect { op, args, dest }` performs one host effect and stores its
`I64` result into `dest`. Calls to the `std::rt` builtins
(`static-semantics.md` §3.6) lower to it; there is no other way to construct
one. Its arguments are call-style `Arg`s (§4.1); argument counts, kinds, and
types are fixed per op and verified (§7.1, `M0011`):

| Op | Arguments | Result stored in `dest` |
|----|-----------|-------------------------|
| `Write` | `Value fd: I64`, `Value text: Str` | Bytes written, or a negative value on host error. Never traps. |
| `ReadByte` | `Value fd: I64` | The byte read (`0..=255`), `-1` on end of input, or another negative value on host error. Never traps. |
| `Exit` | `Value code: I64` | Nothing — the process terminates with `code & 0xff` as its exit status; `dest` is never observably written because control never continues. |
| `WriteString` (ADR-0009) | `Value fd: I64`, `Borrow s: String` | Bytes written, or a negative value on host error. Never traps. The `String` is **lent read-only** for the statement — a `Borrow` place, exactly as an `in` call argument — so the effect consumes nothing and the caller drops the string where it always would. |

`Exit` is a **statement, not a terminator**, by choice: the surface builtin is
declared `-> Int`, so the statement form (an effect with an `I64` destination)
composes in expression position exactly like the other two effects and like any
call, and lowering stays uniform. A terminator encoding would force lowering to
fabricate an unreachable continuation block and special-case expression
position for no observable difference: natively (Stage B) `tuo_rt_exit` does
not return, so the statements after an `Exit` are simply never executed, and in
the reference interpreter no effect executes at all (§8). Verified-MIR totality
is unaffected — the statement types like the others and the dataflow treats
`dest` as initialized after it.

**The sandbox rule.** The reference interpreter **never performs an effect**.
The spec-purity gate (`R0007`, `static-semantics.md` §3.6) statically refuses
any spec whose closure could reach one, so under the spec runner an `Effect`
statement is unreachable by construction; an interpreter that is nevertheless
asked to execute one reports it as an **internal error**
(`TrapKind::Internal`, §8) — the same family as its other impossible-state
signals — never a silent no-op and never a real effect. Effectful programs run
**natively** (`tuo run`): since ADR-0006 Stage B both backends lower an
`Effect` to a direct call to the matching runtime symbol (`tuo_rt_write`/
`tuo_rt_read_byte`/`tuo_rt_exit`, `abi.md` "Effect symbols"). `WriteString`
lands natively with **ADR-0009 Stage B**; until then both backends refuse it
(they never mis-compile it).

### 4.3 Heap mutations (`HeapMutOp`) — `[EXPERIMENTAL]`, ADR-0009 Stage A

`Statement::HeapMutate { op, target, args, dest }` mutates the heap value at
the place `target` **in place** and stores the op's result into `dest`. Calls
to the mutating `std::string`/`std::array` builtins (`static-semantics.md`
§3.7) lower to it; there is no other way to construct one. `target` is
semantically a **`mut` borrow** for the statement's duration: it must be
initialized before the statement and remains initialized after (the dataflow
treats it like a `BorrowMut` call argument). Argument counts and types are
fixed per op and verified (§7.1, `M0013`):

| Op | `target` | Operands | Result stored in `dest` | Semantics |
|----|----------|----------|-------------------------|-----------|
| `PushByte` | `String` | `b: I64` | `()` | Append the byte `b`. **Traps `InvalidByte`** when `b < 0` or `b > 255` — the byte range is enforced, never silently masked. |
| `Append` | `String` | `t: Str` | `()` | Append `t`'s bytes. Never traps. |
| `Push` | `Array[I64]` | `v: I64` | `()` | Append the element `v`. Never traps. |
| `Pop` | `Array[I64]` | — | `Option[I64]` | Remove and return the last element (`Some`), or `None` when empty. Never traps. |

`Pop` both mutates and produces a value, which is exactly why it is a
statement-level mutator and not an rvalue: an rvalue must not mutate a place.
The traps are *statement-level* deterministic aborts, the same source as
trapping arithmetic (§5.3) and the `StrOp`/`HeapOp` traps — not an `Assert`.
A `HeapMutate` is **pure computation** (growth is allocation, not I/O): the
reference interpreter executes it, specs may reach it freely, and its memory
growth is counted against the sandbox's live-value budget (§8.1). Optimization
passes must never eliminate one (`target`'s mutation is observable, and
`PushByte` can trap). Native lowering is **ADR-0009 Stage B**; until then both
backends refuse the statement (they never mis-compile it).

### 4.4 Function values and indirect calls — `[EXPERIMENTAL]`, ADR-0008 Tier 1

A **function value** is `Operand::Const(Const::Fn(sym))` — a compile-time-known
code pointer naming the top-level function `sym`. It is produced only by using a
bare `fn` name in value position (`static-semantics.md` §3.8); there is no other
way to construct one in Tier 1, so **every** function value in MIR names a
concrete top-level symbol. Its type is `sym`'s signature-as-function-type
(`Ty::Fn`, modes included); it is `Copy`, non-heap, and never traps or drops.

`Statement::Call`'s callee is a **`Callee`**:

- **`Direct(sym)`** — the ordinary direct call; `sym` is a user function symbol.
- **`Indirect(operand)`** — call through `operand`, which must be of function
  type. Argument passing, borrows, the destination place, and the no-unwind rule
  are **identical** to a direct call; only the target is looked up at run time
  from the operand's function value.

The **verifier** (§7.1, `M0014`) checks a `Const::Fn(sym)` names a function
symbol whose signature-as-function-type is well-formed, and checks an
`Indirect` call's callee operand is of function type. (Deep argument-vs-signature
type equality is not attempted here, exactly as for a `Direct` call — see §7.)

**Purity.** An indirect call through a known `Const::Fn(sym)` has exactly `sym`'s
effectfulness; a call through an opaque operand is conservatively possibly
effectful. This does not weaken the R0007 spec-gate: the type checker already
taints any function that references an effectful function *as a value*
(`static-semantics.md` §3.6, §3.8), and in Tier 1 a function value can only
originate from a named top-level `fn`, so a spec can never reach an effect
through a function value undetected.

**Optimization.** A `Const::Fn` is a constant like any other: copy-propagatable,
never folded into anything. An indirect `Call` is **never eliminable** (a call
may trap or diverge, and its target is opaque to the passes). Native lowering of
the indirect call is **ADR-0008 Stage B**; until then both backends refuse a
`Const::Fn` operand and an `Indirect` call cleanly (they never mis-compile one).

---

## 5. Rvalues

An **rvalue** ([`Rvalue`]) is evaluated left to right and produces exactly one
value of a statically known type.

### 5.1 Non-arithmetic rvalues

- **`Use(operand)`** — the operand's value, unchanged.
- **`Cast { kind, operand, to }`** — a numeric conversion (§5.4). **Never traps.**
- **`Aggregate { kind, fields }`** — construct an aggregate from `fields` in
  declaration order (§5.5).
- **`Discriminant(place)`** — the discriminant of the enum (or `Option`/`Result`)
  value at `place`, as a `Usize`: the declaration-order index of the active
  variant (`Some` = 0, `None` = 1; `Ok` = 0, `Err` = 1).
- **`Len(place)`** — the length of the array at `place`, as a `Usize`.
  **`Len` applies only to the growable `Array[T]`.** A `[T; N]`'s length is a
  compile-time constant; lowering emits `Const N : Usize` and the verifier
  rejects `Len` of a fixed-array place.
- **`HeapOp { op, subject, args }`** (`[EXPERIMENTAL]`, ADR-0009 Stage A) — a
  pure, value-producing heap operation (§5.7): construct, measure, or read an
  owned `String`/`Array[I64]`. The borrowed heap value, when the op has one,
  is the **place** `subject` — a non-consuming borrow-read, the same
  discipline as `Len`/`Discriminant` — never an operand. `ByteAt`, `Slice`,
  and `ArrayGet` **trap `IndexOutOfBounds`** exactly like the `StrOp` traps
  below; the constructors and length ops never trap.
- **`StrOp { op, args }`** (`[EXPERIMENTAL]`, ADR-0006 Stage A) — a pure
  byte-level string operation on `Str` operands (§5.6). `Len` never traps;
  `ByteAt` and `Slice` **trap `IndexOutOfBounds`** on an out-of-range argument,
  as a *statement-level* trap — the same source of abort as trapping integer
  arithmetic (§5.3), **not** an explicit `Assert`. Array indexing is bounded by
  an explicit `Assert` because its bound (`Len`/a constant) is already a MIR
  value lowering compares against; a `Str`'s byte length is a runtime property
  of the operand itself, so the bound lives in the operation — spelling it as
  asserts would cost a `StrOp::Len` temp and two compares per use for the same
  observable behavior (a deterministic `IndexOutOfBounds` abort). The
  interpreter and the verifier agree on this encoding: the verifier types the
  operation (§7), the interpreter traps it (§8).

### 5.2 Unary operations (`UnOp`)

- **`Neg`** — arithmetic negation of a signed integer or float. Integer negation
  **traps** on the minimum value (two's complement, §24); float negation flips the
  sign bit (IEEE 754, never traps).
- **`Not`** — boolean negation.

### 5.3 Binary operations (`BinOp`)

Both operands of a binary operation have the same type. Integer arithmetic is
two's complement with **trapping overflow** (Constitution §24): a trap is a
deterministic abort, exactly as if a failed `Assert` were reached. Float
arithmetic is IEEE 754 (round to nearest even) and **never traps**.

| Op | Integer | Float |
|----|---------|-------|
| `Add` | traps on overflow | IEEE 754 |
| `Sub` | traps on overflow | IEEE 754 |
| `Mul` | traps on overflow | IEEE 754 |
| `Div` | truncates toward zero; **traps on a zero divisor and on overflow (`MIN / -1`)** | IEEE 754 (`x / 0.0` is an infinity or NaN, no trap) |
| `Rem` | sign of the dividend (truncated division); **traps on a zero divisor and on `MIN % -1`** | IEEE 754 remainder with the sign of the dividend (as `fmod`) |
| `Eq` | equality | IEEE: `NaN == x` is `false` |
| `Ne` | negation of `Eq` | so `NaN != x` is `true` |
| `Lt` | signed/unsigned by value | ordered comparison (`false` if either is NaN) |
| `Le` | as `Lt` | as `Lt` |
| `Gt` | as `Lt` | as `Lt` |
| `Ge` | as `Lt` | as `Lt` |

`Eq`/`Ne` are also defined on `Bool`, `Char` (scalar values), `Str` (byte-wise
content equality) and — since ADR-0009 — `String` (the same byte-wise content
equality, over the same byte buffer). `Lt`/`Le`/`Gt`/`Ge` are also defined on
`Char` by scalar value. A `String` operand of an `Eq`/`Ne` is a
**borrow-read** (§3): the comparison reads the bytes and consumes neither
side, so `Operand::Copy` of a `String` place is legal exactly there.

### 5.4 Casts (`CastKind`) — all total, never trap

- **`IntToInt`** — wrap to the target width: truncate the two's-complement
  representation; sign- or zero-extend (from the source's signedness) when
  widening.
- **`IntToFloat`** — round to nearest even.
- **`FloatToInt`** — truncate toward zero, then **saturate** to the target's range;
  NaN converts to `0`.
- **`FloatToFloat`** — IEEE 754 conversion (round to nearest even when narrowing).

### 5.5 Aggregate kinds (`AggregateKind`)

- **`Adt { ty, variant }`** — a struct, enum-variant, `Option`, or `Result` value
  of type `ty` with active variant `variant` (declaration-order index; `0` for a
  struct). The operands are the variant's payload fields in declaration order.
- **`Range`** — a range value from two operands, start then end.
- **`Array { element, len }`** — a fixed-size array from `len` operands in index
  order; the destination's type is `[element; len]`. Carrying both `element` and
  `len` in the kind keeps verification local and the rendered MIR
  self-describing (ADR-0004 Stage 2).

### 5.6 String operations (`StrOp`) — `[EXPERIMENTAL]`, ADR-0006 Stage A

`Rvalue::StrOp { op, args }` carries one of three pure operations over the
UTF-8 **byte buffer** of a `Str`. Let `len(s)` be the byte length of `s`.
Operand counts and types are fixed per op and verified (§7.1, `M0010`):

| Op | Operands | Result | Semantics |
|----|----------|--------|-----------|
| `Len` | `s: Str` | `I64` | `len(s)`. Never traps. |
| `ByteAt` | `s: Str`, `index: I64` | `I64` | The byte value (`0..=255`) at `index`. **Traps `IndexOutOfBounds`** when `index < 0` or `index >= len(s)`. |
| `Slice` | `s: Str`, `start: I64`, `end: I64` | `Str` | The byte range `[start, end)` of `s`. **Traps `IndexOutOfBounds`** unless `0 <= start <= end <= len(s)`. |

These are **byte** operations: a `Slice` may split a multi-byte code point,
and the resulting `Str` is then not valid UTF-8 on its own — this is the
documented v0 contract (ADR-0006 amendments). `Str` equality (`Eq`/`Ne`, §5.3)
remains byte-wise, `Len` counts bytes, and a sliced value slices bytes, so the
three operations and equality are mutually consistent on any value they can
produce. The corresponding surface builtins are `std::str::len`, `byte_at`,
and `slice` (`static-semantics.md` §3.6); since ADR-0006 Stage B both
backends lower a `StrOp` natively over the `{ptr, len}` fat pointer
(`abi.md` "Slices"), with the same deterministic traps, pinned by the
`str_*.tuo` fixtures in the differential suites.

### 5.7 Heap operations (`HeapOp`) — `[EXPERIMENTAL]`, ADR-0009 Stage A

`Rvalue::HeapOp { op, subject, args }` carries one pure, value-producing
operation over an owned `String` or growable `Array[I64]`. Calls to the
non-mutating `std::string`/`std::array` builtins (`static-semantics.md` §3.7)
lower to it. When the op reads an existing heap value, that value is the
**place** `subject` — a non-consuming borrow-read (the value stays initialized
and untouched), the same discipline `Len` and `Discriminant` use — while the
`Str` and integer inputs are ordinary operands in `args`. Subject presence and
type, operand counts and types, and the destination type are fixed per op and
verified (§7.1, `M0012`). Let `len(x)` be the byte/element length:

| Op | `subject` | Operands | Result | Semantics |
|----|-----------|----------|--------|-----------|
| `StringEmpty` | — | — | `String` | The empty string. Never traps. |
| `StringFromStr` | — | `s: Str` | `String` | `s`'s bytes, copied into a new owned buffer. Never traps. |
| `StringConcat` | — | `a: Str`, `b: Str` | `String` | `a`'s bytes then `b`'s, in a new owned buffer. Never traps. |
| `StringLen` | `String` | — | `I64` | `len(subject)`. Never traps. |
| `StringByteAt` | `String` | `i: I64` | `I64` | The byte (`0..=255`) at `i`. **Traps `IndexOutOfBounds`** when `i < 0` or `i >= len(subject)`. |
| `StringSlice` | `String` | `a: I64`, `b: I64` | `String` | The byte range `[a, b)` **copied out** as a new owned `String` — never an aliasing view. **Traps `IndexOutOfBounds`** unless `0 <= a <= b <= len(subject)`. |
| `StringAsStr` | `String` | — | `Str` | (ADR-0010) A borrowed `Str` **view** of the whole subject — the `{ptr, len}` prefix of its header, no copy. The result aliases the subject; the ownership checker keeps the subject shared-borrowed for the view's lifetime (`O0011`), so no defensive copy is needed. Never traps. |
| `ArrayEmpty` | — | — | `Array[I64]` | The empty array. Never traps. |
| `ArrayLen` | `Array[I64]` | — | `I64` | `len(subject)`. Never traps. |
| `ArrayGet` | `Array[I64]` | `i: I64` | `I64` | The element at `i`. **Traps `IndexOutOfBounds`** when `i < 0` or `i >= len(subject)`. |

Like `StrOp`, the traps are *statement-level* deterministic aborts (§5.6's
rationale applies unchanged), and like `String` itself the operations are
**byte-level**: a `StringSlice` may split a multi-byte code point, so a
`String`'s bytes are UTF-8 *by convention*, not by invariant. A `HeapOp` is
pure — specs may execute it, and the interpreter counts the produced value
against the live-value budget (§8.1). Optimization passes must **not**
constant-fold a `HeapOp` (the trapping ops for the usual reason; the
constructors because folding would have to invent a heap value the constant
domain does not model) and must not eliminate a trapping one whose result is
unread. Distinctions worth pinning: `ArrayLen`/`StringLen` produce `I64` (the
surface `-> Int` contract), while the pre-existing `Rvalue::Len` produces
`Usize` for the indexing path — both remain, with different types and
different jobs. Native lowering is **ADR-0009 Stage B**; until then both
backends refuse every `HeapOp` (they never mis-compile one).

---

## 6. Terminators

Each block closes with exactly one terminator ([`Terminator`]):

- **`Return(operand)`** — return the operand to the caller.
- **`Goto(block)`** — jump unconditionally.
- **`Branch { cond, then_block, else_block }`** — two-way branch on a `Bool`
  operand: `then_block` when `true`, `else_block` when `false`.
- **`Switch { discr, arms, otherwise }`** — multi-way branch on an integer-classed
  operand (an integer, `Char` as its scalar value, or a discriminant). Exactly one
  edge is taken: the first arm whose value equals the operand, else `otherwise`.
  Arm values are pairwise distinct.
- **`Assert { cond, trap, target }`** — an explicit runtime check: if `cond` is
  `true`, continue at `target`; otherwise **trap** — abort deterministically with
  `trap` (§6.1), without unwinding (§24).
- **`Trap(trap)`** — abort deterministically with `trap`. Emitted only where
  control can arrive solely through a compiler bug (e.g. after a `match` whose arms
  the front end proved exhaustive), never for reachable user-visible behavior.

### 6.1 MIR-level traps (`Trap`)

The MIR-level `Trap` enum names the two causes a terminator can carry:

- **`IndexOutOfBounds`** — an array index was out of bounds.
- **`Unreachable`** — control reached a point the front end proved unreachable.

The trapping *arithmetic* causes — integer overflow, division by zero, `MIN / -1`,
`MIN % -1`, negation of the minimum — are **not** carried as a terminator payload.
They arise directly from evaluating a `Neg` unary or an `Add`/`Sub`/`Mul`/`Div`/
`Rem` binary statement (§5.2, §5.3); the same is true of the op-carried
`StrOp`/`HeapOp` `IndexOutOfBounds` traps (§5.6, §5.7) and `HeapMutate`'s
`InvalidByte` (§4.3). The interpreter unifies both sources — plus the sandbox
and internal aborts — into one runtime taxonomy (§8).

---

## 7. Well-formedness: the mandatory verifier

MIR's well-formedness is a load-bearing invariant: the interpreter and every
backend are entitled to *assume* verified MIR and are **forbidden** from consuming
MIR that has not been verified. `tuo_mir::verify` is that gate. The verifier is
**total**: it never panics, never indexes out of bounds, and never trusts a field
it has not first validated — a malformed program is reported as data (a list of
`Mxxxx` diagnostics), not by aborting. An empty list means well-formed.

Per function, in one pass, the verifier proves:

- **Function signatures** — the parameter mode list and the locals agree
  (`params.len() <= locals.len()`) and there is at least one block (the entry,
  `bb0`).
- **Block targets** — every `BlockId` a terminator names exists.
- **Instruction operands** — every `LocalId` a place/operand/argument/rvalue names
  exists, and every projection is legal for the type it projects (a field index in
  range for a struct/enum-variant, an `Index` projection's index local is a
  `Usize`, and so on).
- **No undefined use** — a data-flow pass over the CFG proves that no read
  (`Copy`/`Move`, a `Drop`, a borrow argument, a discriminant or length) can
  observe a local that is not initialized on *every* path reaching it, and that no
  local is read after it was moved out. Parameters begin initialized; ordinary
  locals begin uninitialized; an `Assign`/`Call` destination and a `BorrowMut`
  initialize; a `Move` (of the whole local) and a `Drop` de-initialize; a
  `HeapMutate` `target` reads and re-initializes (a `mut` borrow).
  Unreachable blocks are excluded (dead code is not an undefined-use bug).
  Because a `Drop` is itself a read that de-initializes, this same pass makes
  a **double drop structurally impossible in verified MIR**: a second `Drop`
  of the same place (or a use after a drop) is exactly an `M0004`/`M0005`
  undefined-use error — the drop-glue invariant ADR-0009 Stage B's native
  free relies on.
- **Type consistency** — the two operands of a `Binary` share a type, a
  `Discriminant`/`Len` projects an enum-like / array place, a `Branch`/`Assert`
  condition is `Bool`, and a call/return destination's arity is right. An
  `Aggregate` of kind `Array { element, len }` supplies exactly `len` operands,
  each of the element type, into a destination of exactly `[element; len]`; and
  **`Len` is growable-`Array`-only** — `Len` of a `[T; N]` place is a verifier
  error, because fixed-array lengths are lowered as constants (a backend may
  rely on never seeing one). A `StrOp` supplies exactly its op's operands with
  their fixed types into a destination of the op's result type (`M0010`,
  §5.6); an `Effect` supplies exactly its op's arguments with their fixed
  kinds and types into an `I64` destination (`M0011`, §4.2); a `HeapOp`
  supplies exactly its op's subject and operands with their fixed types into a
  destination of the op's result type (`M0012`, §5.7); a `HeapMutate`'s
  target, operands, and destination match its op (`M0013`, §4.3); a
  `Const::Fn(sym)` names a function whose signature-as-function-type is
  well-formed, and an `Indirect` call's callee operand is of function type
  (`M0014`, §4.4).
- **Terminators** — a `Switch`'s arm values are pairwise distinct.
- **Ownership invariants that must survive lowering** — a borrow argument is never
  paired against a by-value read of the same call; a `Copy`-typed place is never
  consumed by `Move`; a moved-from local is re-initialized before the next read.

Deep, fully-instantiated type equality of every operand against its expected slot
is intentionally **not** attempted here — the checks above are the structural and
flow invariants a backend actually relies on.

### 7.1 Verifier diagnostics (`Mxxxx`)

Codes are in the `Mir` namespace; once shipped, a number is never reused.

| Code | Meaning |
|------|---------|
| `M0001` | A terminator names a block that does not exist. |
| `M0002` | A place, operand, or argument names a local that does not exist. |
| `M0003` | A place projection is illegal for the type it projects. |
| `M0004` | A read observes a local that is not initialized on every path. |
| `M0005` | A read observes a local after it was moved out. |
| `M0006` | Two operands that must share a type do not. |
| `M0007` | A function signature is malformed (params/locals/blocks disagree). |
| `M0008` | A terminator is malformed (e.g. duplicated switch arm values). |
| `M0009` | An ownership invariant that must survive lowering is violated. |
| `M0010` | A `StrOp` rvalue is malformed: wrong operand count for its op, a non-`Str` string operand, or a non-`I64` index operand (§5.6). |
| `M0011` | An `Effect` statement is malformed: wrong argument count for its op, a mistyped or wrong-kind argument (`Value` where `Borrow` is required, or vice versa), or a non-`I64` destination (§4.2). |
| `M0012` | A `HeapOp` rvalue is malformed: a missing/extra/mistyped `subject` place, wrong operand count or types for its op, or a destination that is not the op's result type (§5.7). |
| `M0013` | A `HeapMutate` statement is malformed: a mistyped `target` place, wrong operand count or types for its op, or a destination that is not the op's result type (§4.3). |
| `M0014` | A function value or indirect call is malformed: a `Const::Fn(sym)` whose `sym` is not a function or whose signature-as-function-type is ill-formed, or a `Call` whose `Indirect` callee operand is not of function type (§4.4). |

### 7.2 Re-verification after every pass

The verifier runs unconditionally after lowering; it must **also** run after every
optimization pass. In debug and test builds the driver re-verifies after each pass
and panics on any problem — a pass that produces malformed MIR is a compiler bug,
never user-facing; in release builds this is a no-op.

---

## 8. Execution and the abort taxonomy (`TrapKind`)

The reference interpreter executes **verified** MIR by walking the CFG of one
function, evaluating each statement and terminator exactly as documented above. It
is deterministic and total: under the same limits (§8.1), the same program yields
the same result, the same trap, and the same trace every run; it never panics on
any input it accepts. It is a **sandbox** — no host filesystem, network, process,
clock, or randomness. The one MIR construct that names a host effect,
`Statement::Effect` (§4.2), is **never performed** by the interpreter: the
spec-purity gate keeps it out of everything the spec runner executes, and
actually reaching one is reported as `TrapKind::Internal` — an
interpreter/compiler bug, never an I/O path.

A run either returns a value or aborts with a structured runtime error carrying a
`TrapKind`, a source span, and a call-stack backtrace (innermost frame first). An
abort is never a panic and never a compile diagnostic; it mirrors the language's
trap model (§24): a deterministic abort with no unwinding and no destructor
execution. The `TrapKind` variants fall into three groups:

**Language traps** — defined by MIR semantics (§5, §6), part of the language:

| Trap | Meaning |
|------|---------|
| `IntegerOverflow` | Integer arithmetic overflowed its type (add/sub/mul, negation of the minimum, or `MIN / -1`). |
| `DivisionByZero` | Integer division or remainder by zero. |
| `IndexOutOfBounds` | An array, `Str`, or `String` index was out of bounds (a failed bounds-check assert, an out-of-range `Index` projection, or an out-of-range `StrOp`/`HeapOp` argument). |
| `InvalidByte` | (ADR-0009) A byte-valued argument was outside `0..=255` (`HeapMutate::PushByte`, §4.3). Appended to the taxonomy together with the native `TrapCode` — the taxonomy is append-only. |
| `Unreachable` | Control reached a point the front end proved unreachable. |

**Resource aborts** — the sandbox ceilings (§8.1); not part of the language, but
deterministic given the same limits:

| Trap | Meaning |
|------|---------|
| `OutOfFuel` | The instruction-fuel budget was exhausted. |
| `RecursionLimit` | The call stack exceeded the configured recursion depth. |
| `MemoryBudget` | The live-value budget (a proxy for memory) was exceeded. |

**Internal error** — one variant, `Internal`, guards the interpreter against MIR
that should never have reached it (a verifier bug or unverified MIR): the
interpreter was handed a construct outside the deterministic v0 subset it
supports. This is an **interpreter/compiler bug**, surfaced structurally rather
than as a panic — never a language outcome. It is the interpreter's own
impossible-state signal, emitted precisely at points the verifier (§7) guarantees
cannot occur.

### 8.1 The deterministic sandbox (`Limits`)

Three ceilings make every run terminate and bound its resources. Exceeding any of
them, like any language trap, produces a structured runtime error rather than a
panic:

| Limit | Guarantees | Trap on breach |
|-------|-----------|----------------|
| **fuel** — max instructions (statements + terminators) executed | termination of any run | `OutOfFuel` |
| **max call depth** — max nested calls, including the entry | bounded stack | `RecursionLimit` |
| **max live values** — max live values across all frames (a backend-independent memory proxy) | bounded memory | `MemoryBudget` |

---

## 9. Relation to specs and to the backends

- **Specs.** Executable specs (Constitution §27, ADR-0002) are run by lowering
  them to **ordinary** MIR and executing it in the interpreter: each `then`/`assert`
  becomes one synthetic condition function, and an `lhs == rhs` assertion adds two
  synthetic functions returning the *actual* (`lhs`) and *expected* (`rhs`) values.
  Everything is a normal function call and a normal return — **there is no new
  instruction**. Specs do not ship in release binaries; codegen for release
  artifacts excludes spec bodies entirely.
- **Backends.** Cranelift and LLVM consume the same verified (and, on the native
  build path, optimized) MIR and must agree with the interpreter instruction for
  instruction. The interpreter is the reference on *unoptimized* MIR; any
  divergence is a bug in the backend or in an optimization pass, never in the
  interpreter. This agreement is a mandatory CI gate — see the differential suites
  under `tests/differential/` and `crates/tuo-cli/tests/`.

---

## Cross-references

- **Constitution §9, §22** — parameter modes / borrow modes (§2 above).
- **Constitution §24** — the trap model: deterministic aborts, two's-complement
  trapping integer overflow, no unwinding, destructors do not run.
- **[ADR-0002](adr/ADR-0002-spec-semantics.md)** — spec execution is the
  interpreter's job; specs lower to ordinary/synthetic MIR functions.
- **[ADR-0003](adr/ADR-0003-ownership-model.md)** — no user destructors in v0; drop
  glue is compiler-generated.
- Peer normative documents: [`ownership.md`](ownership.md), [`abi.md`](abi.md).
