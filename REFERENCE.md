# The tuonelang Programming Reference (v0)

A complete programmer's guide to writing tuonelang today, against **grammar 0.1 /
edition 2024** and the **v0 runnable core** (ADR-0004 landed: aggregates, fixed
arrays, and bounded iteration compile natively). Everything here reflects what the
compiler actually accepts and executes — nothing aspirational.

> **Two tiers to keep in mind while reading.** The *front end* (`tuo check`)
> accepts the full v0 language. The *runnable core* — what `tuo spec`, `tuo run`,
> and `tuo build` execute — is pure integer computation: scalars, structs, enums,
> `Option`/`Result`, fixed `[T; N]` arrays, `for`/`while`/`match`, calls and
> recursion. Strings and floats run in the *interpreter* (`spec`/`verify`) but
> are not yet compiled *natively*; I/O, concurrency, and first-class functions
> are tracked ADRs (0006/0007/0008) and are not in v0.
> Anything outside the core is **refused with a clear error, never mis-compiled**.

---

## 1. Getting started

```bash
cargo build -p tuo-cli                       # build the `tuo` binary
cargo run -p tuo-cli -- check   hello.tuo    # parse → resolve → type-check → ownership-check
cargo run -p tuo-cli -- spec    hello.tuo    # run the colocated specs (reference interpreter)
cargo run -p tuo-cli -- verify  hello.tuo    # all static checks AND the specs
cargo run -p tuo-cli -- run     hello.tuo    # compile natively (Cranelift) and run
cargo run -p tuo-cli -- run --release hello.tuo  # optimizing LLVM backend
cargo run -p tuo-cli -- fmt     hello.tuo    # canonical formatter (zero config)
```

A minimal complete program:

```tuo
module hello;

/// Add two integers.
fn add(take a: Int, take b: Int) -> Int {
    a + b
}

fn main() -> Int {
    add(40, 2)
}

spec add {
    then add(2, 3) == 5;
    then add(-1, 1) == 0;
}
```

### `main` and the exit status

The entry point is `fn main() -> Int` — **nullary, returning an integer**. Its
return value becomes the process exit status, truncated to a byte (`& 0xff`).
There is no `println` at runtime in v0 (effects await ADR-0006), so the exit
code and spec verdicts are how a program communicates.

---

## 2. Lexical basics

- Source is **UTF-8, LF line endings, no tabs** (a tab is a lexical error).
- Comments: `// line comment` and `/// doc comment` (attaches to the next item).
  **There are no block comments.**
- Identifiers are Unicode (UAX-31), NFC. `_` alone is the wildcard pattern, not
  a name.

### Keywords (never usable as identifiers)

```
module import pub fn let var const
struct enum interface impl where
if else match for in while loop
break continue return
take mut move Box Shared Weak
spec given when then assert
unsafe as true false
self Self
```

Primitive type names (`Int`, `Bool`, `I64`, `Usize`, `Str`, …) are reserved as
type names and unusable as binding names.

### Literals

| Kind | Forms | Notes |
|------|-------|-------|
| Integer | `42`, `0xFF`, `0o77`, `0b1010`, `2_000_000`, `42i32` | Suffixes `i8`…`usize`. **No negative literal** — `-` is a unary operator, so `-5` is negation applied to `5` (fine in expressions, but e.g. `[x; -1]` is a parse error). |
| Float | `1.0`, `0.5`, `2.0e3`, `1.5f32` | `1.` and `.5` are **not** floats. (Run in specs/the interpreter; not yet compiled natively.) |
| Bool | `true`, `false` | |
| Char | `'a'`, `'\n'`, `'\u{1F600}'` | Escapes: `\" \' \\ \n \r \t \0 \u{HEX}` |
| String | `"hello"` | One canonical form. No raw strings, no interpolation. (Runs in specs/the interpreter; not yet compiled natively.) |
| Unit | `()` | |

---

## 3. Bindings: `let`, `var`, `const`

```tuo
let x = 1;                 // immutable; type inferred (Int by default)
let xs: [Int; 4] = [1, 2, 3, 4];   // optional annotation
var total = 0;             // mutable
total = total + 5;         // plain assignment (there is NO += / -= etc.)
const LIMIT: Int = 100;    // constant; type annotation REQUIRED
```

- `let` bindings cannot be reassigned; `var` bindings can.
- Both may be **shadowed**; the initializer sees the outer name.
- Local types are inferred by unification where unambiguous (`let x = 1` is
  `Int`). If inference can't decide (e.g. an empty `[]`), you get `T0011`
  "type annotation needed".
- **Function signatures are never inferred** — every parameter and return type
  is written out.

---

## 4. Types

### Primitives

- Signed: `I8 I16 I32 I64 Isize` — **`Int` is an alias for `I64`** (two's-complement).
- Unsigned: `U8 U16 U32 U64 Usize`.
- `F32 F64` (**`Float` = `F64`**), `Bool`, `Char`, `Str`/`String`, unit `()`.

**No implicit numeric conversion.** All conversions are explicit with `as`
(numeric-only; any other cast is `T0008`):

```tuo
let wide: I64 = small as I64;
```

### Compound types

| Syntax | Meaning |
|--------|---------|
| `Point`, `std::io::IoError` | Named / path types |
| `Option[Int]`, `Result[Int, Str]`, `Pair[A, B]` | Generics use **square brackets**, never `<>` |
| `[T; N]` | Fixed-capacity inline array, length is part of the type |
| `Box[T]`, `Shared[T]`, `Weak[T]` | Heap wrappers (declared; heap allocation is not in the runnable core) |
| `()` | Unit |

There is **no reference type** (`&T` does not exist). Borrowing happens only
through parameter modes (`in` / `mut`, §7).

### Copy vs move

A type is `Copy` iff bitwise duplication is sound — this is compiler-derived,
never hand-written:

- Copy: all integer/float scalars, `Bool`, `Char`, `()`, `Str`, and `[T; N]`
  when `T` is Copy.
- **Not** Copy: any struct/enum with a non-Copy field, `Box`/`Shared`/`Weak`,
  the growable `Array[T]`, and generic `T` inside a generic body.
- In practice in v0: **structs and enums you define are moved**, scalars and
  `[Int; N]` are copied.

---

## 5. Operators and precedence

Lowest to highest:

```
=                     assignment (right-assoc; the ONLY assignment operator)
||                    boolean or
&&                    boolean and
==  !=  <  >  <=  >=  comparison (NON-associative: a < b < c is a parse error)
..                    half-open range (a..b)
+  -                  add, subtract
*  /  %               multiply, divide, remainder
-  !  move            prefix: negate, boolean not, explicit move
?  .field  f(args)  a[i]  x as T    postfix
```

Things that deliberately **do not exist** in v0: bitwise operators and shifts
(`| & ^ << >>`), compound assignment (`+=`), the `<>` generic syntax.

Integer arithmetic **traps** (deterministic abort — no wraparound, no
unwinding) on: overflow of `+ - *`, negation of `MIN`, `MIN / -1`, and division
or remainder by zero. `/` truncates toward zero; `%` takes the dividend's sign.

---

## 6. Expressions and control flow

**Blocks are expressions.** A block's value is its trailing expression written
*without* a semicolon; otherwise it's `()`. Semicolons are mandatory on
statements (no ASI); braces are mandatory on all control flow.

### `if` (an expression)

```tuo
fn abs(take n: Int) -> Int {
    if n < 0 { 0 - n } else { n }
}
```

`else if` chains work. The condition may not be a bare struct literal
(parenthesize if you need one there).

### `match` (an expression)

```tuo
match shape {
    Shape::Circle { r } => r * r * 3,
    Shape::Rect { w, h } => w * h,
}

match opt {
    Some { value } => value,
    None => fallback,
}
```

- Arms: `pattern [if guard] => expression,` — exhaustiveness is enforced
  (`T0007`).
- Patterns: literals (`0`, `true`, `'a'`, `"s"`), wildcard `_`, bindings,
  or-patterns `a | b`, struct patterns `Point { x, .. }` (with `..` to ignore
  the rest), enum patterns `Variant { field }`. No range patterns in v0.

### Loops

```tuo
for x in xs { total = total + x; }     // over a fixed array (element must be Copy)
for i in 0..n { acc = acc + i; }       // half-open range
while i < end { i = i + 1; }
loop { break; }                        // `break [value]` gives the loop a value
```

`break` and `continue` are expressions; using them outside a loop is `T0013`.
Bounded iteration is a design guarantee: array lengths are compile-time
constants, so every loop over data is provably finite.

### `?` and `move`

- `expr?` propagates the `Err` of a `Result` out of a `Result`-returning
  function (`T0009` elsewhere).
- `move place` makes a move explicit; applying it to a Copy value is `O0007`.

---

## 7. Functions and parameter modes

Every parameter has an explicit **mode** and type:

```tuo
pub fn clamp(take value: Int, take low: Int, take high: Int) -> Int { ... }
pub fn is_some(in opt: Option[Int]) -> Bool { ... }
fn bump(mut counter: Int) { counter = counter + 1; }
```

| Mode | Callee gets | Callee may | Callee may not |
|------|-------------|------------|----------------|
| `take` | **Ownership** (value moved in) | read, mutate, move out, return, drop | — |
| `in` | Shared read-only borrow for the call | read, pass on as `in` | mutate, move out |
| `mut` | Exclusive mutable borrow for the call | read, assign, pass on | move out (and must leave a value) |

**Borrow rule (frozen):** for any one place in a single argument list — any
number of `in`, *or* exactly one `mut`, never both (`O0005`). Disjoint field
paths never conflict: `f(mut p.x, mut p.y)` is legal. Since there are no
reference types, borrows live only for the duration of a call.

Conventions from the stdlib: `take` for consumed scalars, `in` for reading
aggregates. No default arguments, no variadics, no overloading — a call
resolves to exactly one function by name. Omitting `-> T` means the function
returns `()`.

---

## 8. Ownership in practice

- Using a non-Copy value (struct, enum) *by value* **moves** it; using it again
  afterward is `O0001` "use of moved value". Moves are tracked per field path:
  moving `s.f` leaves `s.g` usable.
- Copy values (scalars, `[Int; N]`) are copied and stay usable.
- **Index expressions `xs[i]` are not places**: you can read a Copy element out,
  but you cannot assign `xs[i] = v` or move out of an index in v0.
- No user destructors in v0; drop glue is compiler-generated.

---

## 9. Structs and enums

```tuo
pub struct Point { x: Int, y: Int }

pub struct Pair[A, B] { first: A, second: B }

enum Color { Red, Green, Blue }

pub enum Shape {
    Circle { r: Int },
    Rect { w: Int, h: Int },
}
```

- Fields are **named only** (no positional/tuple structs) and **private by
  default**; mark the struct/field `pub` to export. Enum variants inherit the
  enum's visibility.
- Construction uses named fields, with shorthand:

```tuo
let p = Point { x: 10, y: 20 };
let s = ExitStatus { code };          // shorthand for { code: code }
let c = Color::Red;
let sh = Shape::Circle { r: 4 };
```

- Field access is `p.x`. **Method syntax is not lowered in v0** — `impl` blocks
  and interfaces parse, but you write free functions (`area(s)`, not
  `s.area()`). The stdlib follows the same rule.

---

## 10. `Option` and `Result`

Built-in prelude enums with **named-field payloads** (not positional — there is
no `Some(x)`):

```tuo
let a: Option[Int] = Some { value: 1 };
let b: Option[Int] = None;

let ok: Result[Int, Str] = Ok { value: 0 };
let err: Result[Int, Str] = Err { error: "boom" };

match r {
    Ok { value } => value,
    Err { error } => fallback,
}
```

Errors are values; **there is no exception channel**. Use `?` to propagate, or
the `std::core` helpers (`is_some`, `unwrap_or`, `map_add`, `is_ok`, `ok_or`,
`ok` — see §14).

---

## 11. Fixed arrays `[T; N]`

The v0 collection type — inline, compile-time length (ADR-0004 Stage 2):

```tuo
let xs: [Int; 4] = [10, 20, 30, 40];   // list literal (trailing comma OK)
let sevens = [7; 5];                   // repeat literal: five 7s
let first = xs[0];                     // checked: out-of-bounds TRAPS at runtime
let seed3 = [seed, seed * 2, seed * 3];

var total = 0;
for x in xs { total = total + x; }     // bounded iteration
```

Rules:

- The length is part of the type: `[Int; 3]` and `[Int; 4]` are different types
  (`T0001` on mismatch). No length inference.
- Maximum length **65 536** (`T0014` beyond that). `[T; 0]` is legal.
- Indexing is bounds-checked — an out-of-range index is a deterministic
  `IndexOutOfBounds` trap, never undefined behavior.
- `xs[i]` is a *read*, not a place: **no `xs[i] = v`** in v0, and the element
  must be Copy to take it by value.
- The repeat form `[x; N]` requires a Copy element (`O0010`); `for` over an
  array requires Copy elements too (each iteration binds a copy).
- Arrays of Copy elements are themselves Copy; they pass to and return from
  functions by value and compile natively.

The growable, heap-backed `Array[T]` is *declared* (it appears in stdlib
contract signatures) but has no constructor and is not in the runnable core —
it's a later ADR on the allocator seam.

---

## 12. Specs — executable, colocated tests

A `spec` is an item that names a function (or carries a description string) and
asserts boolean expressions about it. Specs run in the reference interpreter's
deterministic sandbox (`tuo spec` / `tuo verify` / `tuo test`), are checked but
not executed by `tuo check`, and are excluded from release binaries.

The idiomatic compact form:

```tuo
spec min {
    then min(3, 7) == 3;
    then min(7, 3) == 3;
    then min(4, 4) == 4;
}

spec "the pipeline totals a category" {
    then total_for(2) == 400;
}
```

The full form also exists (`given` inputs, `when` steps, `assert`):

```tuo
spec "add is commutative" {
    given a: Int, b: Int;
    when let s1 = add(a, b);
    when let s2 = add(b, a);
    then s1 == s2;
}
```

- A spec named by identifier must resolve to a function (`R0006`); it may
  reference functions declared later in the file.
- `==` in v0 compares **scalars** (`Int`, `Bool`, `Str`) — to check an ADT,
  extract a scalar first (e.g. `unwrap_or(map_add(Some { value: 1 }, 4), 0) == 5`),
  or use the `std::test` predicates (`eq`, `near`, `in_range`, …) in a `then`.
- The sandbox is hermetic (no filesystem/network/clock/randomness) with fuel,
  recursion, and memory ceilings — a runaway spec aborts deterministically
  (`OutOfFuel` / `RecursionLimit` / `MemoryBudget`).

---

## 13. Modules, imports, and packages

### Modules

One `.tuo` file = one module, optionally self-named by a header:

```tuo
module geometry;
```

Module-level declarations are collected before use, so **forward references
always work**. Items are private unless `pub`.

### Imports

The keyword is **`import`** (not `use`); paths are absolute; no globs:

```tuo
import geometry::Point;
import geometry::{point, manhattan};
import numeric::max as maximum;
```

After importing, use names unqualified — or skip the import and write the full
path at the call site: `std::core::min(3, 7)`.

### Packages (`tdg.toml`)

```toml
[package]
name = "geometry"          # [a-z][a-z0-9_]* — also the module path root
version = "0.1.0"
edition = "2024"

[modules]
root = "src"               # the directory of .tuo modules

[dependencies]
numeric = { path = "../numeric" }   # v0: local path deps only
```

- Binaries put their entry in `src/main.tuo`; libraries in `src/lib.tuo`.
- A `tdg.lock` pins every resolved package's checksum; a *dependency* whose
  bytes drift from the lock is refused (the root package you're editing is
  exempt).
- Lifecycle: `tuo new <name>`, `tuo add <name> --path <p>`, `tuo remove <name>`,
  and file-less `tuo check` / `tuo build` / `tuo verify` / `tuo test` operate on
  the package in the current directory. `tuo --message-format=json package symbols`
  reports a package's real exported symbols (no guessing).

---

## 14. Standard library

Free functions only, one obvious API per task. Two tiers: **executable**
(runs today, has specs) and **`CONTRACT:`** (exact signature + documented
contract for an effect v0 cannot perform — no executable spec, so nothing
pretends to run).

### `std::core` — all executable

```tuo
pub fn min(take a: Int, take b: Int) -> Int
pub fn max(take a: Int, take b: Int) -> Int
pub fn abs(take n: Int) -> Int              // abs(Int::MIN) traps
pub fn clamp(take value: Int, take low: Int, take high: Int) -> Int
pub fn signum(take n: Int) -> Int           // -1, 0, or 1
pub fn xor(take a: Bool, take b: Bool) -> Bool
pub fn is_some(in opt: Option[Int]) -> Bool
pub fn is_none(in opt: Option[Int]) -> Bool
pub fn unwrap_or(in opt: Option[Int], take fallback: Int) -> Int
pub fn map_add(in opt: Option[Int], take add: Int) -> Option[Int]
pub fn is_ok(in res: Result[Int, Str]) -> Bool
pub fn is_err(in res: Result[Int, Str]) -> Bool
pub fn ok_or(in res: Result[Int, Str], take fallback: Int) -> Int
pub fn ok(in res: Result[Int, Str]) -> Option[Int]
```

### `std::collections`

Executable: `Pair[A, B]` with `pair`, `first`, `second`, `swap`; range helpers
`range_len`, `range_sum`, `range_contains(start, end, value)`.
Contract (await growable arrays): `get`, `sum`, `contains` over `Array[Int]`.

### `std::test` — all executable (predicates for `then`)

`eq`, `ne`, `near(value, target, tolerance)`, `in_range(value, low, high)`,
`is_true`, `is_false`, `str_eq`.

### `std::io`, `std::fs`, `std::process`, `std::sync`, `std::time`

Each pairs an executable pure core with a contract tier:

| Module | Executable today | `CONTRACT:` (not yet runnable) |
|--------|------------------|-------------------------------|
| `std::io` | `IoError` enum, `error_code`, `is_eof` | `print`, `println`, `read_line` |
| `std::fs` | `FsError` enum, `error_code`, `is_not_found`, path predicates | `read`, `write`, `exists`, `remove` |
| `std::process` | `ExitStatus`, `success`, `failure`, `code`, `is_success` | `exit`, `arg_count` |
| `std::sync` | `Once`/`LockState` pure state models | `lock`, `unlock`, `spawn` |
| `std::time` | `Duration` arithmetic (`from_nanos/millis/secs`, `add`, `lt`, …) | `Instant`, `now`, `elapsed` |

---

## 15. Runtime model: traps and determinism

All three execution engines — the reference MIR interpreter, the Cranelift
debug backend, and the LLVM release backend — agree instruction for
instruction; a divergence is a release-blocking compiler bug, never your
program's problem.

A trap is a **deterministic abort**: a stable message on stderr, a fixed exit
status, no unwinding, no destructors. Language traps:

| Trap | Cause |
|------|-------|
| `IntegerOverflow` | `+ - *` overflow, `-MIN`, `MIN / -1` |
| `DivisionByZero` | integer `/` or `%` by zero |
| `IndexOutOfBounds` | array index out of range |

Float operations (where they run at all) follow IEEE-754 and never trap.

---

## 16. What runs where (the honesty map)

| Construct | `tuo check` | Interpreter (`spec`/`verify`) | Native (`run`/`build`) |
|-----------|:-----------:|:--------------:|:------:|
| Integers, `Bool`, `Char`, arithmetic, comparison | ✅ | ✅ | ✅ |
| `if` / `match` / `while` / `for` / `loop`, calls, recursion | ✅ | ✅ | ✅ |
| Structs, enums, `Option`/`Result` (scalar fields, nested) | ✅ | ✅ | ✅ |
| `[T; N]` arrays, literals, checked indexing, `for` over arrays | ✅ | ✅ | ✅ |
| Borrow-mode (`in`/`mut`) calls | ✅ | ✅ | scalars ✅ / aggregate borrows refused |
| `Str` / `String` values (literals, `==`, `if`/`match` over them) | ✅ | ✅ | ❌ refused |
| Floats (`F32`/`F64`) arithmetic and comparison | ✅ | ✅ | ❌ refused |
| Growable `Array[T]`, `Box`/`Shared`/`Weak` heap values | declared | ❌ | ❌ refused |
| Method calls, `impl` bodies | parse | not lowered | not lowered |
| I/O, filesystem, clock, processes, threads | contract sigs only | ❌ (sandbox) | ❌ |

"Refused" means a clear `Unsupported` error naming the construct, pointing you
back to the interpreter as the reference — never silent mis-compilation.

---

## 17. Diagnostics quick reference

**Resolution** — `R0001` duplicate definition · `R0002` undefined name ·
`R0003` unresolved import · `R0004` ambiguous name · `R0005` visibility
violation · `R0006` spec target is not a function.

**Types** — `T0001` mismatched types · `T0002` wrong argument count · `T0003`
call of non-function · `T0004` unknown field · `T0005` struct field-set error ·
`T0006` operator unsupported for type · `T0007` non-exhaustive match · `T0008`
invalid cast · `T0009` invalid `?` · `T0010` wrong type-argument count ·
`T0011` type annotation needed · `T0012` expected value/constructor · `T0013`
break/continue outside loop · `T0014` invalid fixed-array length.

**Ownership** — `O0001` use of moved value · `O0003` move out of borrowed
param · `O0005` conflicting borrows in a call · `O0006` move of borrowed value
in a call · `O0007` invalid explicit `move` · `O0010` repeat literal needs a
Copy element.

---

## 18. Gotchas cheat sheet

1. **`var`, not `let mut`** — and assignment is plain `=` (no `+=`).
2. **`import`, not `use`**; generics are `Option[Int]`, never `Option<Int>`.
3. **`Some { value: x }`, not `Some(x)`** — Option/Result payloads are named
   fields, matched as `Some { value } => …`.
4. **No negative literals**: `-5` works in expressions (unary minus), but
   contexts requiring a literal (array lengths, suffixed forms) need a
   non-negative INT_LITERAL.
5. **Comparisons don't chain**: `a < b < c` is a parse error.
6. **No bitwise operators** — `|` appears only in or-patterns.
7. **The last expression of a block (no semicolon) is its value**; adding a `;`
   turns it into a `()`-valued statement (usually a `T0001` at the return).
8. **`xs[i]` is not assignable** — build a new array or restructure with locals.
9. **Every parameter needs a mode** (`take`/`in`/`mut`) and a type.
10. **Free functions only** — `map(x, f)`-style, never `x.map(f)`.
11. **Overflow traps.** Don't rely on wraparound; it doesn't exist.
12. **Specs compare scalars** — extract an `Int`/`Bool` before `==` on ADTs.

---

## 19. A complete worked example

The shape of a real v0 program (condensed from `examples/cli-stats` and
`examples/data-pipeline`, which run natively today):

```tuo
module stats;

/// The dataset, as a fixed array.
fn data() -> [Int; 7] {
    [12, 7, 23, 4, 16, 9, 19]
}

fn sum(take xs: [Int; 7]) -> Int {
    var total = 0;
    for x in xs {
        total = total + x;
    }
    total
}

fn largest(take xs: [Int; 7]) -> Int {
    var best = xs[0];
    for x in xs {
        if x > best {
            best = x;
        }
    }
    best
}

fn mean(take xs: [Int; 7]) -> Int {
    sum(xs) / 7
}

fn main() -> Int {
    mean(data()) + largest(data()) / 5
}

spec sum {
    then sum(data()) == 90;
    then sum([0; 7]) == 0;
}

spec largest {
    then largest(data()) == 23;
}

spec mean {
    then mean(data()) == 12;
}
```

```bash
tuo verify stats.tuo   # static checks + all specs green
tuo run stats.tuo      # native; exit status = main()
```

For larger, multi-package layouts see [`examples/workspace/`](examples/workspace/)
(`app → geometry → numeric` over path dependencies) and the other programs in
[`examples/`](examples/).
