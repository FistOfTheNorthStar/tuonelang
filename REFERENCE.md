# The tuonelang Programming Reference (v0)

A complete programmer's guide to writing tuonelang today, against **grammar 0.1 /
edition 2024** and the **v0 runnable core** (ADR-0004 landed: aggregates, fixed
arrays, and bounded iteration compile natively). Everything here reflects what the
compiler actually accepts and executes — nothing aspirational.

> **Two tiers to keep in mind while reading.** The *front end* (`tuo check`)
> accepts the full v0 language. The *runnable core* — what `tuo spec`, `tuo run`,
> and `tuo build` execute — is scalars, structs, enums,
> `Option`/`Result`, fixed `[T; N]` arrays, `for`/`while`/`match`, calls and
> recursion, floats, borrow-mode (`in`/`mut`) calls, `Str` values with the
> `std::str` byte operations, the **owned heap types** — an owned `String` and
> a growable `Array[T]` that allocate and free real memory (`std::string`/
> `std::array`; since ADR-0009, element-generic since ADR-0012, with `Float`
> elements and the in-place `set` since ADR-0016) — the `std::rt` host effects
> (`write`/`read_byte`/`write_string`/`exit`, the ADR-0007 `par_map`
> fork-join, — since ADR-0013 — `now_nanos`/`arg_count`/`arg_byte`/
> `open`/`close`/`remove_file`, the clock, argv, and file boundary, — since
> ADR-0014 — `listen`/`bound_port`/`accept`/`connect`, the loopback TCP
> sockets, — since ADR-0015 — the channel and mutex primitives
> `chan_new`/`chan_send`/`chan_recv`/`chan_close` and
> `mutex_new`/`mutex_lock`/`mutex_unlock`, and — since ADR-0017 — the
> bounded waits `accept_timeout`/`connect_timeout`/`read_byte_timeout`, the
> IPv6 pair `listen6`/`peer_family`, and the UDP tier
> `udp_bind`/`udp_send`/`udp_recv`/`udp_byte_at`/`udp_peer_port` — native
> only; the spec sandbox
> stays pure, but heap ops *are* pure, so a spec may build strings and arrays) —
> and, since ADR-0008 Tier 1, **first-class (non-capturing) function values**: a
> bare top-level `fn` name is a `Copy` code pointer of type `fn(mode T, …) -> R`,
> called indirectly, which powers the generic higher-order `std::collections`
> combinators. **Capturing closures** (Tier 2) remain a tracked ADR and are
> not in v0.
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
A natively-compiled program can also write real output:
`std::rt::write(1, "hello\n")` prints to stdout (ADR-0006). Specs stay pure —
inside the spec sandbox the exit value and verdicts are how a program
communicates.

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
| Float | `1.0`, `0.5`, `2.0e3`, `1.5f32` | `1.` and `.5` are **not** floats. |
| Bool | `true`, `false` | |
| Char | `'a'`, `'\n'`, `'\u{1F600}'` | Escapes: `\" \' \\ \n \r \t \0 \u{HEX}` |
| String | `"hello"` | One canonical form. No raw strings, no interpolation. (A literal is a `Str`; runs everywhere, natively as a `{ptr, len}` view of static data.) |
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
| `String` | Owned, growable, heap-backed byte buffer (`std::string`; runnable since ADR-0009) |
| `Array[T]` | Owned, growable, heap-backed sequence (`std::array`; runnable since ADR-0009, element-generic since ADR-0012). Supported elements: `Int`, `Float` (since ADR-0016), `Bool`, `Str`, `String`, and structs/enums built from them |
| `Map[K, V]` | Owned, heap-backed hash map (`std::map`; since ADR-0011). The v0 operation surface is `Map[Int, Int]` and `Map[Str, Int]` |
| `Box[T]`, `Shared[T]`, `Weak[T]` | Heap-wrapper **values** (declared; construction not in the runnable core) |
| `()` | Unit |

There is **no reference type** (`&T` does not exist). Borrowing happens only
through parameter modes (`in` / `mut`, §7).

### Copy vs move

A type is `Copy` iff bitwise duplication is sound — this is compiler-derived,
never hand-written:

- Copy: all integer/float scalars, `Bool`, `Char`, `()`, `Str`, and `[T; N]`
  when `T` is Copy.
- **Not** Copy: any struct/enum with a non-Copy field, `Box`/`Shared`/`Weak`,
  the owned `String`, the growable `Array[T]`, the `Map[K, V]`, and generic `T`
  inside a generic body.
- In practice in v0: **structs, enums you define, `String`, `Array[T]`, and
  `Map[K, V]` are moved**; scalars and `[Int; N]` are copied. A moved-out owned
  heap value is freed exactly once (drop glue frees at scope end; a move transfers ownership).

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

Float arithmetic is IEEE-754 and **never traps** (`0.0 / 0.0` is NaN). An
`as` cast from float to integer truncates toward zero and **saturates** to the
target range; NaN casts to 0 — also never a trap.

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
  but you cannot assign `xs[i] = v` or move out of an index in v0. (For a
  growable `Array[T]`, the in-place element write is the `std::array::set`
  builtin — ADR-0016.)
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
- **A struct or enum may not reach itself** through its own fields/payloads —
  through any chain of by-value fields, array/map elements, `Option`/`Result`
  payloads, or tuple/fixed-array components. That is a type error at the
  declaration (`T0016`, since ADR-0016), *unless* every cycle passes through a
  `Box`/`Shared`/`Weak` indirection. Recursive data in v0 uses the index-arena
  pattern instead (the way `std::json` does).

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

A fixed `[T; N]` never allocates — it lives inline wherever the value lives.
When you need a sequence whose size isn't known at compile time, reach for the
owned, heap-backed `Array[T]` in the next section.

---

## 11a. Owned `String` and growable `Array[T]`

Since ADR-0009, tuonelang has two heap-backed, owned, growable types that
allocate and free real memory — and they run in all three engines (the
interpreter and both native backends), with the compiler generating the
`free` at the end of each value's scope. Both are **non-Copy** (they own a
buffer), so passing one by value moves it.

They are exposed as builtin free functions — there is no literal syntax and no
`.method` call; you write `std::string::…` / `std::array::…`. Building one needs
a `var` binding, because the mutators take the buffer by `mut` borrow.

### Owned `String` (`std::string`)

```tuo
var s = std::string::empty();        // {ptr, len: 0, cap: 0}
std::string::push_byte(s, 72);       // 'H'  — a byte outside 0..=255 traps (InvalidByte)
std::string::append(s, "i there");   // append the bytes of a Str
let n = std::string::len(s);         // 8
let c = std::string::byte_at(s, 0);  // 72 — checked, traps IndexOutOfBounds
let joined = std::string::concat("ab", "cd");   // a fresh owned String "abcd"
let part = std::string::slice(s, 0, 2);         // a fresh owned String (a copy, not a view)
let view = std::string::as_str(s);              // borrow the bytes as a Str view (zero-copy; ADR-0010)
```

`String == String` is byte-wise content equality (like `Str`); ordering is not
defined. A `String` is written to a file descriptor with
`std::rt::write_string(fd, s)` (an `in` borrow — no view type needed).

### Growable `Array[T]` (`std::array`)

```tuo
var xs = std::array::empty();
std::array::push(xs, 10);
std::array::push(xs, 20);
let len = std::array::len(xs);            // 2
let first = std::array::get(xs, 0);       // 10 — checked, traps IndexOutOfBounds
std::array::set(xs, 0, 15);               // in-place indexed write — checked, traps IndexOutOfBounds (ADR-0016)
let last = std::array::pop(xs);           // Option[Int]: Some { value: 20 }
```

The operation surface is **element-generic** (since ADR-0012):
`push`/`pop`/`len`/`get`/`set` operate on `Array[T]` for any supported
element — `Int`, `Float` (since ADR-0016), `Bool`, `Str`, `String`, and
structs/enums whose fields are themselves supported (any other element type
is an ordinary `T0001`). A `get` of a heap-owning element is a recursive
**deep copy**, and drop is recursive per-element glue; `set` drops the old
element in place before the new value moves in. Growth uses an amortized
doubling strategy internally; only length, contents, and `pop`'s `Option` are
observable, so all three engines agree.

Because these operations are **pure** (allocation is deterministic, not an
effect), specs may build strings and arrays inside the sandbox — bounded by the
interpreter's memory budget:

```tuo
spec build { then std::string::len(std::string::concat("ab", "cd")) == 4; }
```

Still on the heap roadmap: the `Box`/`Shared`/`Weak` wrapper *values* (declared,
still refused natively), **capturing closures** (ADR-0008 Tier 2), and
**recursive nominal types** — a struct/enum that reaches itself is refused at
the declaration (`T0016`) until a successor ADR gives the backends
runtime-recursive clone/drop glue.

---

## 11b. The hash map `Map[K, V]`

Since ADR-0011, tuonelang has a builtin hash map. Like `String` and `Array[T]`
it is heap-backed, **non-Copy** (passing one by value moves it), exposed as
builtin free functions under `std::map::…` — no literal syntax, no `.method` —
and it runs in all three engines.

The **type** `Map[K, V]` is generic, but the v0 **operation surface is
monomorphic** over two instantiations: **`Map[Int, Int]`** and
**`Map[Str, Int]`**. A call with any other key or value type is an ordinary
`T0001`. Key and value types are witnessed by context, so an `empty()` whose
pair never gets determined is `T0011`.

```tuo
var m = std::map::empty();
let old = std::map::insert(m, 7, 1);      // Option[Int]: None — 7 was absent
let prev = std::map::insert(m, 7, 5);     // Some { value: 1 } — the PREVIOUS value
let hit = std::map::get(m, 7);            // Some { value: 5 }
let miss = std::map::get(m, 4);           // None — never traps
let has = std::map::contains_key(m, 7);   // true
let gone = std::map::remove(m, 7);        // Some { value: 5 }
let n = std::map::len(m);                 // 0
let ks = std::map::keys(m);               // a NEW Array[K]
```

| Function | Signature |
|----------|-----------|
| `empty` | `fn empty() -> Map[K, V]` |
| `insert` | `fn insert(mut m: Map[K, V], take k: K, take v: V) -> Option[V]` |
| `get` | `fn get(in m: Map[K, V], take k: K) -> Option[V]` |
| `contains_key` | `fn contains_key(in m: Map[K, V], take k: K) -> Bool` |
| `remove` | `fn remove(mut m: Map[K, V], take k: K) -> Option[V]` |
| `len` | `fn len(in m: Map[K, V]) -> Int` |
| `keys` | `fn keys(in m: Map[K, V]) -> Array[K]` |

Building a map needs a `var` binding, because `insert`/`remove` take the map by
`mut` borrow. `insert` **returns the previous value**, so the common
count-or-zero update reads:

```tuo
let seen = match std::map::get(freq, x) {
    Some { value } => value,
    None => 0,
};
let _ = std::map::insert(freq, x, seen + 1);   // bind the returned Option to `_`
```

**No map operation ever traps** — a missing key is `None`, not an abort.

**`keys` is insertion order.** The map's internal open-addressing index is
hidden; what is observable is a dense, insertion-ordered entry list, and
`remove` preserves the relative order of the entries that remain. That order is
part of the contract (all three engines agree on it), so iterating `keys` is
deterministic:

```tuo
var m = std::map::empty();
let _ = std::map::insert(m, 5, 0);
let _ = std::map::insert(m, 2, 0);
let ks = std::map::keys(m);
// std::array::get(ks, 0) == 5, std::array::get(ks, 1) == 2 — first-seen order
```

`Map[Str, Int]` is the identical surface with `K = Str`, which composes with
`std::string::as_str` (ADR-0010) to key a map by an owned `String`'s bytes.

Map operations are **pure** (allocation is deterministic, not an effect), so
specs may build maps in the sandbox. `std::collections::counts(in xs:
Array[Int]) -> Map[Int, Int]` is the stdlib's frequency table over this surface:

```tuo
fn distinct_count() -> Int {
    std::map::len(std::collections::counts(std::collections::of3(2, 7, 2)))
}

spec distinct_count {
    then distinct_count() == 2;
}
```

Deferred, honestly: map **values** beyond `Int` (`Map[Str, String]` and friends
are an additive later increment — the *type* admits any `V`, the v0 *ops* are
`V = Int`), and a `Set[K]` (a set is a `Map[K, Unit]`; the map ships first).

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

Twelve modules — `std::core`, `std::collections`, `std::math`, `std::str`,
`std::json`, `std::io`, `std::fs`, `std::net`, `std::time`, `std::process`,
`std::sync`, `std::test` — free functions only, one obvious API per task.
Three tiers: **executable** (pure computation — runs today, has specs),
**`EFFECT:`** (real implementations over the `std::rt` effect primitives —
they run natively, but an effectful spec is impossible by `R0007`, so each is
pinned by a named native CLI test instead), and **`CONTRACT:`** (exact
signature + documented contract for an effect whose primitive does not exist
yet — no executable spec, so nothing pretends to run). Since ADR-0015
discharged the last `lock`/`unlock` stubs, **the contract tier is empty** —
and a CLI test (`the_contract_tier_is_empty`) pins it so nothing re-enters
silently.

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

**Error chains** (context wrapping; v0 has no traits, so the honest concrete
model is a chain of messages — an `Array[String]`, root cause first):

```tuo
pub fn error_new(in msg: Str) -> Array[String]          // a chain with one root
pub fn error_wrap(take chain: Array[String], in context: Str) -> Array[String]
                                                        // fmt.Errorf("%w") analog
pub fn error_is(in chain: Array[String], in target: Str) -> Bool
                                                        // errors.Is analog (sentinel message)
pub fn error_root(in chain: Array[String]) -> String    // the innermost cause
pub fn error_message(in chain: Array[String]) -> String // "outer: inner: root"
```

### `std::collections`

Executable: `Pair[A, B]` with `pair`, `first`, `second`, `swap`; range helpers
`range_len`, `range_sum`, `range_contains(start, end, value)`; and — real over
the growable `Array[Int]` since ADR-0009 — `sum`, `max_of` (→ `Option[Int]`),
`contains`, `index_of` (→ `Option[Int]`), and `reversed` (→ a new `Array[Int]`).
Since ADR-0008 Tier 1, the **generic higher-order combinators** over a
first-class function value (see "First-class functions" at the end of this
section):

| Combinator | Signature | Result |
|------------|-----------|--------|
| `fold` | `fold(in xs: Array[Int], take init: Int, take step: fn(take Int, take Int) -> Int) -> Int` | left fold: `acc = step(acc, xs[i])`, `init` for empty |
| `map_into` | `map_into(in xs: Array[Int], take f: fn(take Int) -> Int) -> Array[Int]` | a new array of `f(x)` per element |
| `filter_into` | `filter_into(in xs: Array[Int], take pred: fn(take Int) -> Bool) -> Array[Int]` | a new array of the elements where `pred` holds |
| `any` / `all` | `any(in xs, take pred: fn(take Int) -> Bool) -> Bool` | does `pred` hold for any / every element |

One `fold` replaces N specialized folds: `sum(xs)` is `fold(xs, 0, add)`. The
step/predicate element modes are `take` (the `Int`/`Bool` scalars are `Copy`).
The array primitives themselves live in the `std::array` builtin module (§11a).

### `std::test` — all executable (predicates for `then`)

`eq`, `ne`, `near(value, target, tolerance)`, `in_range(value, low, high)`,
`is_true`, `is_false`, `str_eq`.

### `std::json` — all executable (ADR-0016)

JSON parsing and rendering, written in tuonelang, **entirely in the executable
tier** (parsing is pure computation, so the whole module runs in the spec
sandbox — and natively). A parsed document is a `Json` **index arena** — flat
parallel arrays in DFS pre-order — navigated by node index:

```tuo
pub fn parse(in text: Str) -> Result[Json, String]  // Err is "invalid JSON at byte N"
pub fn render(in d: Json) -> String                 // canonical re-render of what parse produced
pub fn root() -> Int                                // the root node's index (always 0)
pub fn node_count(in d: Json) -> Int
pub fn kind_of(in d: Json, take node: Int) -> Int   // compare against the kind_* tags:
                                                    // kind_null/kind_false/kind_true/kind_number/
                                                    // kind_string/kind_array/kind_object
pub fn num_of(in d: Json, take node: Int) -> Float
pub fn text_of(in d: Json, take node: Int) -> String
pub fn key_of(in d: Json, take node: Int) -> String
pub fn first_child(in d: Json, take node: Int) -> Int
pub fn next_sibling(in d: Json, take node: Int) -> Int
pub fn child_count(in d: Json, take node: Int) -> Int
pub fn member(in d: Json, take node: Int, in key: Str) -> Int
```

Documented limits (honest, in-module): numbers are IEEE `Float`; `\uXXXX`
escapes are a **parse error** naming the gap; `render` re-emits what `parse`
produced (building a document from scratch is deferred); objects preserve
member order (the arena is insertion-ordered).

### `std::io`, `std::fs`, `std::net`, `std::process`, `std::sync`, `std::time`

Each pairs an executable pure core with an effect tier (where the `std::rt`
primitive exists) and a contract tier (where it does not — empty since
ADR-0015):

| Module | Executable today | `EFFECT:` (runs natively, no spec) | `CONTRACT:` (not yet runnable) |
|--------|------------------|------------------------------------|-------------------------------|
| `std::io` | `IoError` enum, `error_code`, `is_eof` | `print`, `println` (over `std::rt::write`), `read_line` → `Result[String, IoError]` (reads bytes via `std::rt::read_byte` into an owned `String`) | — |
| `std::fs` | `FsError` enum, `error_code`, `is_not_found`, path predicates | `read` → `Result[String, FsError]`, `write`, `exists`, `remove` (over the ADR-0013 `open`/`close`/`remove_file` primitives + the descriptor seam) | — |
| `std::net` | `is_descriptor`, `is_timeout`, `is_ipv6` | `listen`, `bound_port`, `accept`, `connect`, `close` (over the ADR-0014 socket primitives — TCP, loopback listen, numeric hosts; data moves through the existing `write`/`read_byte` descriptor seam); since ADR-0017: `accept_timeout`, `connect_timeout`, `read_byte_timeout` (bounded waits, `-3` on timeout), `listen6`, `peer_family` (IPv6 — `connect` infers the family from the address), and `udp_bind`, `udp_send`, `udp_recv`, `udp_byte_at`, `udp_peer_port` (datagrams — a receive reports the message boundary and stages the payload) | — |
| `std::process` | `ExitStatus`, `success`, `failure`, `code`, `is_success` | `exit` (over `std::rt::exit`); `arg_count`, `arg(i)` → `String` (over the ADR-0013 argv primitives) | — |
| `std::sync` | `Once`/`LockState` pure state models | `par_map` (structured fork-join over `std::rt::par_map`, ADR-0007); `channel`, `send`, `recv`, `close`, `mutex`, `lock`, `unlock` (over the ADR-0015 channel/mutex primitives — process-lived `Int` handles; channels carry non-negative `Int`s by copy, `recv` returns `-1` once closed and drained; error-checked mutexes) | — |
| `std::time` | `Duration` arithmetic (`from_nanos/millis/secs`, `add`, `lt`, …), `render` → `String`, `instant_at`, `elapsed` | `now` (over `std::rt::now_nanos`, ADR-0013) | — |

A real program printing through the stdlib (compile the `std::io` module
source alongside your own — the stdlib is consumed as input):

```tuo
module app;

import std::io;

fn main() -> Int {
    match std::io::println("hello, world") {
        Ok { value } => 0,       // "hello, world\n" is on stdout
        Err { error } => std::io::error_code(error),
    }
}
```

An effectful function like `main` here cannot appear in a `spec` (`R0007`);
observable output is tested from outside the process, the way
`crates/tuo-cli/tests/stdlib.rs` pins `println` itself.

### First-class functions (ADR-0008 Tier 1)

A **function type** is written `fn(P1, P2, …) -> R`, where each parameter is a
**mode** (`take`/`in`/`mut`) followed by a type — modes are **mandatory** in the
type, and the return is always spelled (write `-> ()` for unit). Two function
types are equal iff arity, parameter types, modes, and return type all match
exactly: `fn(take Int) -> Int` is a *different* type from `fn(in Int) -> Int`.

A bare **top-level `fn` name in value position** (not immediately called) is a
**function value** of that function's signature-as-type — a `Copy` code pointer,
no heap. Calling an expression of function type is an **indirect call**, checked
and lowered with the identical ABI as a direct call. This is what the generic
combinators consume:

```tuo
fn add(take a: Int, take b: Int) -> Int { a + b }

fn main() -> Int {
    var xs = std::array::empty();
    std::array::push(xs, 1);
    std::array::push(xs, 2);
    std::array::push(xs, 3);
    std::collections::fold(xs, 0, add)   // add passed as a VALUE → 6
}
```

Only ordinary top-level user functions are first-class in Tier 1: a **builtin**
or a **generic** used as a value is `T0015`. **Tier 1 has no closures** — a
function value cannot capture locals; capturing closures (with a heap-allocated
captured environment) are Tier 2, deferred to a future ADR.

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
| `IndexOutOfBounds` | array/string index or slice out of range |
| `InvalidByte` | `std::string::push_byte` with a byte outside `0..=255` |

Float operations (where they run at all) follow IEEE-754 and never trap.

---

## 16. What runs where (the honesty map)

| Construct | `tuo check` | Interpreter (`spec`/`verify`) | Native (`run`/`build`) |
|-----------|:-----------:|:--------------:|:------:|
| Integers, `Bool`, `Char`, arithmetic, comparison | ✅ | ✅ | ✅ |
| `if` / `match` / `while` / `for` / `loop`, calls, recursion | ✅ | ✅ | ✅ |
| Structs, enums, `Option`/`Result` (scalar fields, nested) | ✅ | ✅ | ✅ |
| `[T; N]` arrays, literals, checked indexing, `for` over arrays | ✅ | ✅ | ✅ |
| Borrow-mode (`in`/`mut`) calls | ✅ | ✅ | ✅ |
| Floats (`F32`/`F64`) arithmetic, comparison, casts | ✅ | ✅ | ✅ |
| `Str` values (literals, `==`, `std::str::len`/`byte_at`/`slice`) | ✅ | ✅ | ✅ |
| Owned `String`, growable `Array[T]` (`std::string`/`std::array` incl. `set`; elements `Int`/`Float`/`Bool`/`Str`/`String` + supported structs/enums) | ✅ | ✅ | ✅ |
| `Map[K, V]` hash map (`std::map`; surface `Map[Int, Int]` / `Map[Str, Int]`), insertion-ordered `keys` | ✅ | ✅ | ✅ |
| `std::json` (`parse`/`render`, arena accessors — all pure) | ✅ | ✅ | ✅ |
| First-class (non-capturing) function values `fn(mode T, …) -> R`, indirect calls | ✅ | ✅ | ✅ |
| `std::rt::write`/`read_byte`/`write_string`/`exit` host effects | ✅ | ❌ (sandbox; specs gated by `R0007`) | ✅ |
| `std::rt::now_nanos`/`arg_count`/`arg_byte`/`open`/`close`/`remove_file` (ADR-0013: clock, argv, files) | ✅ | ❌ (sandbox; specs gated by `R0007`) | ✅ |
| `std::rt::listen`/`bound_port`/`accept`/`connect` (ADR-0014: loopback TCP sockets over the descriptor seam) | ✅ | ❌ (sandbox; specs gated by `R0007`) | ✅ |
| `std::rt::chan_new`/`chan_send`/`chan_recv`/`chan_close`, `mutex_new`/`mutex_lock`/`mutex_unlock` (ADR-0015: channels, mutexes) | ✅ | ❌ (sandbox; specs gated by `R0007`) | ✅ |
| `std::rt::accept_timeout`/`connect_timeout`/`read_byte_timeout` (ADR-0017: bounded waits, `-3` timeout sentinel) | ✅ | ❌ (sandbox; specs gated by `R0007`) | ✅ |
| `std::rt::listen6`/`peer_family` (ADR-0017: IPv6; `connect` infers the family from a numeric address) | ✅ | ❌ (sandbox; specs gated by `R0007`) | ✅ |
| `std::rt::udp_bind`/`udp_send`/`udp_recv`/`udp_byte_at`/`udp_peer_port` (ADR-0017: UDP datagrams) | ✅ | ❌ (sandbox; specs gated by `R0007`) | ✅ |
| Stdlib effect tier: `std::io::print`/`println`/`read_line`, `std::process::exit`/`arg_count`/`arg`, `std::time::now`, `std::fs::read`/`write`/`exists`/`remove`, `std::net::listen`/`bound_port`/`accept`/`connect`/`close`/`accept_timeout`/`connect_timeout`/`read_byte_timeout`/`listen6`/`peer_family`/`udp_bind`/`udp_send`/`udp_recv`/`udp_byte_at`/`udp_peer_port`, `std::sync::par_map`/`channel`/`send`/`recv`/`close`/`mutex`/`lock`/`unlock` | ✅ | ❌ (sandbox; specs gated by `R0007`) | ✅ |
| Capturing closures (Tier 2), `Box`/`Shared`/`Weak` heap-wrapper values | declared / refused | ❌ | ❌ refused |
| Method calls, `impl` bodies | parse | not lowered | not lowered |
| Recursive nominal types (a struct/enum reaching itself without a `Box`/`Shared`/`Weak` indirection) | ❌ refused (`T0016`) | ❌ | ❌ |

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
break/continue outside loop · `T0014` invalid fixed-array length · `T0015` a
builtin or generic function is not first-class (cannot be used as a value) ·
`T0016` recursive struct/enum without a `Box`/`Shared`/`Weak` indirection.

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
8. **`xs[i]` is not assignable** — on a growable `Array[T]` use
   `std::array::set`; on a fixed `[T; N]` build a new array or restructure
   with locals.
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

---

## 20. Teaching a model to write tuonelang

To fine-tune a language model into a professional tuonelang programmer, see
[`training/`](training/): a compiler-validated dataset generator that turns the
concepts in this reference into supervised-fine-tuning examples (task → correct
program), multi-turn **repair transcripts** (buggy attempt → the compiler's real
diagnostic → corrected program — the TDG feedback loop), and a held-out eval set
scored by the real compiler ([`training/README.md`](training/README.md)). Every
example is compiled through `tuo` before it is emitted, so the material can never
drift from what the language actually accepts.
