"""Seed library for tuonelang fine-tuning data.

Every seed is a (concept, task, solution) triple written by hand against the v0
runnable core described in ``REFERENCE.md``. Nothing here is trusted on
assertion: ``generate.py`` compiles every solution through the real ``tuo``
compiler (``check`` + ``spec``) and refuses to emit any example whose solution
does not pass. A seed that stops compiling is a loud failure, never a silent
hallucination.

A seed's ``solution`` is a complete, self-contained ``.tuo`` program. Snippets
need no ``module`` header and no ``main`` to ``check`` and ``spec`` — the
compiler accepts a bare set of functions with colocated specs — so most seeds
are just the function(s) under test plus their spec block. Seeds that show a
runnable ``main`` set ``runnable=True`` and a ``run_exit`` byte, and the
generator additionally compiles-and-runs them.

Concept tags let the guide and eval slice coverage by language feature.
"""

from dataclasses import dataclass


@dataclass(frozen=True)
class Seed:
    """One verified teaching example."""

    concept: str
    """Kebab-case feature tag (e.g. ``fixed-arrays``, ``option-result``)."""

    task: str
    """The natural-language instruction a model must satisfy."""

    solution: str
    """A complete, compiler-valid tuonelang program answering ``task``."""

    runnable: bool = False
    """True if the program has a nullary ``main`` and should be run natively."""

    run_exit: int = 0
    """Expected process exit byte when ``runnable`` (``main() & 0xff``)."""

    notes: str = ""
    """Optional teaching note surfaced in the SFT ``assistant`` rationale."""

    stdlib_deps: tuple[str, ...] = ()
    """Stdlib module names (e.g. ``"core"``, ``"test"``) whose ``.tuo`` source
    must be compiled alongside this program. tuonelang's standard library is
    *consumed as input*: modules like ``std::core`` are ordinary source files,
    not linked-in builtins, so a program importing them must be compiled
    together with their source. (The ``std::array`` / ``std::string`` heap ops
    and the ``std::rt`` effects are compiler builtins and need no dep here.)"""


# ---------------------------------------------------------------------------
# The seed library. Grouped by concept; each block is authored to compile.
# ---------------------------------------------------------------------------

SEEDS: list[Seed] = [
    # --- basics: functions, integer arithmetic, specs --------------------
    Seed(
        concept="functions-arithmetic",
        task="Write a function `add(a, b)` that returns the sum of two Ints, "
        "with a spec covering a positive case and a zero case.",
        solution="""fn add(take a: Int, take b: Int) -> Int {
    a + b
}

spec add {
    then add(40, 2) == 42;
    then add(-1, 1) == 0;
}
""",
    ),
    Seed(
        concept="functions-arithmetic",
        task="Write `double(x)` returning x + x, specced on 3 and 0.",
        solution="""fn double(take x: Int) -> Int {
    x + x
}

spec double {
    then double(3) == 6;
    then double(0) == 0;
}
""",
    ),
    Seed(
        concept="functions-arithmetic",
        task="Write `square(n)` returning n * n. Spec it on a few values.",
        solution="""fn square(take n: Int) -> Int {
    n * n
}

spec square {
    then square(0) == 0;
    then square(5) == 25;
    then square(-4) == 16;
}
""",
    ),
    # --- if / else -------------------------------------------------------
    Seed(
        concept="if-else",
        task="Write `abs(n)` using an if/else expression (remember: there are "
        "no negative literals, use unary minus). Spec both branches.",
        solution="""fn abs(take n: Int) -> Int {
    if n < 0 {
        0 - n
    } else {
        n
    }
}

spec abs {
    then abs(5) == 5;
    then abs(-5) == 5;
    then abs(0) == 0;
}
""",
        notes="`if` is an expression; each branch is a block whose trailing "
        "expression (no semicolon) is its value.",
    ),
    Seed(
        concept="if-else",
        task="Write `sign(n)` returning -1, 0, or 1 using an else-if chain.",
        solution="""fn sign(take n: Int) -> Int {
    if n < 0 {
        0 - 1
    } else if n > 0 {
        1
    } else {
        0
    }
}

spec sign {
    then sign(-8) == 0 - 1;
    then sign(0) == 0;
    then sign(99) == 1;
}
""",
    ),
    Seed(
        concept="if-else",
        task="Write `max2(a, b)` and `min2(a, b)` over two Ints, each specced.",
        solution="""fn max2(take a: Int, take b: Int) -> Int {
    if a > b { a } else { b }
}

fn min2(take a: Int, take b: Int) -> Int {
    if a < b { a } else { b }
}

spec max2 {
    then max2(3, 7) == 7;
    then max2(7, 3) == 7;
}

spec min2 {
    then min2(3, 7) == 3;
    then min2(4, 4) == 4;
}
""",
    ),
    # --- recursion -------------------------------------------------------
    Seed(
        concept="recursion",
        task="Write a recursive `factorial(n)` for small n, with a spec.",
        solution="""fn factorial(take n: Int) -> Int {
    if n <= 1 {
        1
    } else {
        n * factorial(n - 1)
    }
}

spec factorial {
    then factorial(0) == 1;
    then factorial(1) == 1;
    then factorial(5) == 120;
}
""",
    ),
    Seed(
        concept="recursion",
        task="Write a recursive `gcd(a, b)` using Euclid's algorithm (a, b "
        "non-negative). Spec it.",
        solution="""fn gcd(take a: Int, take b: Int) -> Int {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

spec gcd {
    then gcd(12, 8) == 4;
    then gcd(17, 5) == 1;
    then gcd(9, 0) == 9;
}
""",
    ),
    Seed(
        concept="recursion",
        task="Write a recursive `fib(n)` (n-th Fibonacci, fib(0)=0, fib(1)=1).",
        solution="""fn fib(take n: Int) -> Int {
    if n < 2 {
        n
    } else {
        fib(n - 1) + fib(n - 2)
    }
}

spec fib {
    then fib(0) == 0;
    then fib(1) == 1;
    then fib(10) == 55;
}
""",
    ),
    # --- loops and var mutation -----------------------------------------
    Seed(
        concept="loops",
        task="Write `sum_to(n)` that sums 0..n with a while loop and a `var` "
        "accumulator (remember: assignment is plain `=`, no `+=`).",
        solution="""fn sum_to(take n: Int) -> Int {
    var total = 0;
    var i = 0;
    while i < n {
        total = total + i;
        i = i + 1;
    }
    total
}

spec sum_to {
    then sum_to(0) == 0;
    then sum_to(5) == 10;
    then sum_to(10) == 45;
}
""",
        notes="Mutable bindings are `var`, not `let mut`; reassignment uses "
        "plain `=`.",
    ),
    Seed(
        concept="loops",
        task="Write `count_up(n)` that sums the range 0..n using a `for i in "
        "0..n` loop.",
        solution="""fn count_up(take n: Int) -> Int {
    var acc = 0;
    for i in 0..n {
        acc = acc + i;
    }
    acc
}

spec count_up {
    then count_up(0) == 0;
    then count_up(4) == 6;
}
""",
    ),
    Seed(
        concept="loops",
        task="Write `power(base, exp)` computing base^exp for exp >= 0 with a "
        "loop.",
        solution="""fn power(take base: Int, take exp: Int) -> Int {
    var result = 1;
    var i = 0;
    while i < exp {
        result = result * base;
        i = i + 1;
    }
    result
}

spec power {
    then power(2, 0) == 1;
    then power(2, 10) == 1024;
    then power(5, 3) == 125;
}
""",
    ),
    # --- fixed arrays ----------------------------------------------------
    Seed(
        concept="fixed-arrays",
        task="Write `sum(xs)` over a fixed `[Int; 5]` array using a `for` loop.",
        solution="""fn sum(take xs: [Int; 5]) -> Int {
    var total = 0;
    for x in xs {
        total = total + x;
    }
    total
}

spec sum {
    then sum([1, 2, 3, 4, 5]) == 15;
    then sum([0; 5]) == 0;
}
""",
        notes="`[0; 5]` is the repeat literal (five zeros). Array length is "
        "part of the type: `[Int; 5]` and `[Int; 4]` are different types.",
    ),
    Seed(
        concept="fixed-arrays",
        task="Write `largest(xs)` returning the maximum of a `[Int; 4]` array.",
        solution="""fn largest(take xs: [Int; 4]) -> Int {
    var best = xs[0];
    for x in xs {
        if x > best {
            best = x;
        }
    }
    best
}

spec largest {
    then largest([3, 9, 2, 7]) == 9;
    then largest([5, 5, 5, 5]) == 5;
}
""",
    ),
    Seed(
        concept="fixed-arrays",
        task="Write `dot(a, b)` computing the dot product of two `[Int; 3]` "
        "arrays.",
        solution="""fn dot(take a: [Int; 3], take b: [Int; 3]) -> Int {
    var acc = 0;
    for i in 0..3 {
        acc = acc + a[i] * b[i];
    }
    acc
}

spec dot {
    then dot([1, 2, 3], [4, 5, 6]) == 32;
    then dot([0, 0, 0], [1, 2, 3]) == 0;
}
""",
    ),
    # --- structs ---------------------------------------------------------
    Seed(
        concept="structs",
        task="Define a `Point { x, y }` struct and a `manhattan(p)` function "
        "returning |x| + |y| (assume non-negative for the spec).",
        solution="""struct Point {
    x: Int,
    y: Int,
}

fn manhattan(in p: Point) -> Int {
    p.x + p.y
}

spec manhattan {
    then manhattan(Point { x: 3, y: 4 }) == 7;
    then manhattan(Point { x: 0, y: 0 }) == 0;
}
""",
        notes="Aggregates are read with the `in` borrow mode. Fields are "
        "named-only and private by default.",
    ),
    Seed(
        concept="structs",
        task="Define `Rect { w, h }` and `area(r)`. Use field shorthand in a "
        "constructor helper `square_rect(side)`.",
        solution="""struct Rect {
    w: Int,
    h: Int,
}

fn area(in r: Rect) -> Int {
    r.w * r.h
}

fn square_rect(take side: Int) -> Rect {
    Rect { w: side, h: side }
}

spec area {
    then area(Rect { w: 3, h: 4 }) == 12;
    then area(square_rect(5)) == 25;
}
""",
    ),
    # --- enums and match -------------------------------------------------
    Seed(
        concept="enums-match",
        task="Define an enum `Dir { North, South, East, West }` and a function "
        "`turn_right(d)` mapping each direction to the next clockwise one. "
        "Return an Int code (0..3) so the spec can compare scalars.",
        solution="""enum Dir {
    North,
    South,
    East,
    West,
}

fn code(take d: Dir) -> Int {
    match d {
        Dir::North => 0,
        Dir::East => 1,
        Dir::South => 2,
        Dir::West => 3,
    }
}

fn turn_right(take d: Dir) -> Dir {
    match d {
        Dir::North => Dir::East,
        Dir::East => Dir::South,
        Dir::South => Dir::West,
        Dir::West => Dir::North,
    }
}

spec turn_right {
    then code(turn_right(Dir::North)) == 1;
    then code(turn_right(Dir::West)) == 0;
}
""",
        notes="`match` is exhaustive (T0007 otherwise). Specs compare scalars, "
        "so map the enum to an Int before `==`.",
    ),
    Seed(
        concept="enums-match",
        task="Define `enum Shape { Circle { r }, Rect { w, h } }` and "
        "`area_approx(s)` using pi≈3.",
        solution="""enum Shape {
    Circle { r: Int },
    Rect { w: Int, h: Int },
}

fn area_approx(take s: Shape) -> Int {
    match s {
        Shape::Circle { r } => r * r * 3,
        Shape::Rect { w, h } => w * h,
    }
}

spec area_approx {
    then area_approx(Shape::Circle { r: 2 }) == 12;
    then area_approx(Shape::Rect { w: 3, h: 5 }) == 15;
}
""",
    ),
    # --- option / result -------------------------------------------------
    Seed(
        concept="option-result",
        task="Write `safe_div(a, b)` returning `Option[Int]` — `None` on divide "
        "by zero, else `Some { value }`. Provide a helper `or_zero(o)` that "
        "extracts the value or 0 so the spec can compare scalars.",
        solution="""fn safe_div(take a: Int, take b: Int) -> Option[Int] {
    if b == 0 {
        None
    } else {
        Some { value: a / b }
    }
}

fn or_zero(in o: Option[Int]) -> Int {
    match o {
        Some { value } => value,
        None => 0,
    }
}

spec safe_div {
    then or_zero(safe_div(10, 2)) == 5;
    then or_zero(safe_div(10, 0)) == 0;
}
""",
        notes="Option/Result payloads are named fields: `Some { value: x }`, "
        "matched as `Some { value } => ...`. There is no `Some(x)`.",
    ),
    Seed(
        concept="option-result",
        task="Write `checked_sub(a, b)` returning `Result[Int, Str]` — "
        "`Err { error: \"underflow\" }` when b > a, else `Ok { value }`.",
        solution="""fn checked_sub(take a: Int, take b: Int) -> Result[Int, Str] {
    if b > a {
        Err { error: "underflow" }
    } else {
        Ok { value: a - b }
    }
}

fn or_neg1(in r: Result[Int, Str]) -> Int {
    match r {
        Ok { value } => value,
        Err { error } => 0 - 1,
    }
}

spec checked_sub {
    then or_neg1(checked_sub(10, 3)) == 7;
    then or_neg1(checked_sub(3, 10)) == 0 - 1;
}
""",
    ),
    Seed(
        concept="option-result",
        task="Write `first_even(a, b)` returning `Option[Int]` — `Some` of the "
        "first even argument, else `None`. Add an `or_zero` extractor for the "
        "spec.",
        solution="""fn first_even(take a: Int, take b: Int) -> Option[Int] {
    if a % 2 == 0 {
        Some { value: a }
    } else if b % 2 == 0 {
        Some { value: b }
    } else {
        None
    }
}

fn or_zero(in o: Option[Int]) -> Int {
    match o {
        Some { value } => value,
        None => 0,
    }
}

spec first_even {
    then or_zero(first_even(3, 4)) == 4;
    then or_zero(first_even(2, 7)) == 2;
    then or_zero(first_even(1, 3)) == 0;
}
""",
        notes="Construct Option payloads with named fields: `Some { value: a }`, "
        "never `Some(a)`.",
    ),
    Seed(
        concept="option-result",
        task="Write `parse_flag(n)` returning `Result[Int, Str]` — `Ok` of n "
        "when n is 0 or 1, else `Err { error: \"not a flag\" }`.",
        solution="""fn parse_flag(take n: Int) -> Result[Int, Str] {
    if n == 0 {
        Ok { value: 0 }
    } else if n == 1 {
        Ok { value: 1 }
    } else {
        Err { error: "not a flag" }
    }
}

fn or_neg1(in r: Result[Int, Str]) -> Int {
    match r {
        Ok { value } => value,
        Err { error } => 0 - 1,
    }
}

spec parse_flag {
    then or_neg1(parse_flag(1)) == 1;
    then or_neg1(parse_flag(9)) == 0 - 1;
}
""",
        notes="`Ok { value: x }` / `Err { error: e }` — Result payloads are "
        "named fields.",
    ),
    Seed(
        concept="constants",
        task="Define a `const LIMIT: Int = 100` and a function `capped(n)` that "
        "returns the smaller of n and LIMIT.",
        solution="""const LIMIT: Int = 100;

fn capped(take n: Int) -> Int {
    if n < LIMIT {
        n
    } else {
        LIMIT
    }
}

spec capped {
    then capped(30) == 30;
    then capped(250) == 100;
}
""",
        notes="`const` requires an explicit type annotation.",
    ),
    # --- parameter modes / borrowing ------------------------------------
    Seed(
        concept="borrow-modes",
        task="Write `is_origin(p)` taking a Point by `in` borrow and returning "
        "whether both coordinates are zero.",
        solution="""struct Point {
    x: Int,
    y: Int,
}

fn is_origin(in p: Point) -> Bool {
    p.x == 0 && p.y == 0
}

spec is_origin {
    then is_origin(Point { x: 0, y: 0 });
    then is_origin(Point { x: 1, y: 0 }) == false;
}
""",
        notes="`in` is a shared read-only borrow for the duration of the call. "
        "There are no reference types; borrowing is only through modes.",
    ),
    # --- floats ----------------------------------------------------------
    Seed(
        concept="floats",
        task="Write `average3(a, b, c)` over three F64 values returning their "
        "mean, and a `Bool` helper `is_two(x)` checking x == 2.0 so the spec "
        "can assert a scalar.",
        solution="""fn average3(take a: F64, take b: F64, take c: F64) -> F64 {
    (a + b + c) / 3.0
}

fn is_two(take x: F64) -> Bool {
    x == 2.0
}

spec average3 {
    then is_two(average3(1.0, 2.0, 3.0));
    then average3(0.0, 0.0, 0.0) == 0.0;
}
""",
        notes="Floats are IEEE-754 and never trap. `1.` and `.5` are not valid "
        "float literals (write `1.0`, `0.5`). Float `==` is exact; there is no "
        "float tolerance helper in v0 (`std::test::near` operates on `Int`), so "
        "compare against exactly-representable values or fold to an Int first.",
    ),
    # --- std::core -------------------------------------------------------
    Seed(
        concept="stdlib-core",
        task="Using `std::core`, write `clamp_unit(n)` that clamps n into "
        "0..=100 by calling `std::core::clamp`.",
        solution="""import std::core;

fn clamp_unit(take n: Int) -> Int {
    std::core::clamp(n, 0, 100)
}

spec clamp_unit {
    then clamp_unit(-5) == 0;
    then clamp_unit(50) == 50;
    then clamp_unit(250) == 100;
}
""",
        stdlib_deps=("core",),
        notes="Standard library is free functions only, consumed as input: "
        "`std::core` is an ordinary source module, so compile its `.tuo` source "
        "alongside your program. One obvious API per task; call it as "
        "`std::core::clamp(...)`.",
    ),
    # --- growable Array[Int] (heap) -------------------------------------
    Seed(
        concept="growable-array",
        task="Using `std::array`, build a growable `Array[Int]`, push three "
        "values, and return its length via `std::array::len`.",
        solution="""fn build_len() -> Int {
    var xs = std::array::empty();
    std::array::push(xs, 10);
    std::array::push(xs, 20);
    std::array::push(xs, 30);
    std::array::len(xs)
}

spec build_len {
    then build_len() == 3;
}
""",
        notes="Owned `Array[Int]` allocates real memory; the mutators take the "
        "buffer by `mut` borrow, so it must be a `var`. Heap ops are pure, so "
        "specs may build arrays.",
    ),
    Seed(
        concept="growable-array",
        task="Using `std::array`, write `first_or_zero()` that pushes one "
        "element then reads index 0 with `std::array::get`.",
        solution="""fn first_or_zero() -> Int {
    var xs = std::array::empty();
    std::array::push(xs, 42);
    std::array::get(xs, 0)
}

spec first_or_zero {
    then first_or_zero() == 42;
}
""",
    ),
    # --- owned String (heap) --------------------------------------------
    Seed(
        concept="owned-string",
        task="Using `std::string`, concatenate two string literals and return "
        "the length of the result.",
        solution="""fn joined_len() -> Int {
    std::string::len(std::string::concat("ab", "cde"))
}

spec joined_len {
    then joined_len() == 5;
}
""",
        notes="`std::string::concat` returns a fresh owned `String`. Heap ops "
        "are pure, so this is spec-checkable.",
    ),
    # --- first-class functions (ADR-0008 Tier 1) ------------------------
    Seed(
        concept="first-class-functions",
        task="Write a top-level `add(a, b)` and use it as a first-class value "
        "passed to `std::collections::fold` over a growable array to sum it.",
        solution="""fn add(take a: Int, take b: Int) -> Int {
    a + b
}

fn sum_all() -> Int {
    var xs = std::array::empty();
    std::array::push(xs, 1);
    std::array::push(xs, 2);
    std::array::push(xs, 3);
    std::collections::fold(xs, 0, add)
}

spec sum_all {
    then sum_all() == 6;
}
""",
        stdlib_deps=("collections",),
        notes="A bare top-level `fn` name in value position is a Copy code "
        "pointer of type `fn(take Int, take Int) -> Int`. Only ordinary "
        "top-level user functions are first-class (builtins/generics are "
        "T0015). No closures in Tier 1. `std::collections` is a source module, "
        "compiled alongside; `std::array` is a builtin needing no dep.",
    ),
    # --- generics --------------------------------------------------------
    Seed(
        concept="generics",
        task="Define a generic `Pair[A, B] { first, second }` struct and a "
        "`fst` that reads the first field of a `Pair[Int, Int]`.",
        solution="""struct Pair[A, B] {
    first: A,
    second: B,
}

fn fst(in p: Pair[Int, Int]) -> Int {
    p.first
}

spec fst {
    then fst(Pair { first: 7, second: 9 }) == 7;
}
""",
        notes="Generics use square brackets: `Pair[A, B]`, never `<>`.",
    ),
    # --- runnable programs (main + native run) --------------------------
    Seed(
        concept="runnable-main",
        task="Write a complete program with `main() -> Int` that returns 42 via "
        "a helper. Include a spec on the helper.",
        solution="""module answer;

fn compute() -> Int {
    6 * 7
}

fn main() -> Int {
    compute()
}

spec compute {
    then compute() == 42;
}
""",
        runnable=True,
        run_exit=42,
        notes="`main` is nullary returning Int; its value becomes the process "
        "exit status truncated to a byte.",
    ),
    Seed(
        concept="runnable-main",
        task="Write a runnable program whose `main` sums a fixed `[Int; 4]` "
        "array and returns the total (which fits in a byte).",
        solution="""module total;

fn sum(take xs: [Int; 4]) -> Int {
    var acc = 0;
    for x in xs {
        acc = acc + x;
    }
    acc
}

fn main() -> Int {
    sum([10, 20, 30, 40])
}

spec sum {
    then sum([1, 2, 3, 4]) == 10;
}
""",
        runnable=True,
        run_exit=100,
    ),
    # --- hash maps (ADR-0011) -------------------------------------------
    Seed(
        concept="hash-map",
        task="Write `count_of(xs, needle)` that builds a frequency table of an "
        "`Array[Int]` in a `Map[Int, Int]` and returns how many times `needle` "
        "occurs (0 if absent). Spec it.",
        solution="""fn build() -> Array[Int] {
    var xs = std::array::empty();
    std::array::push(xs, 4);
    std::array::push(xs, 9);
    std::array::push(xs, 4);
    xs
}

fn count_of(in xs: Array[Int], take needle: Int) -> Int {
    var freq = std::map::empty();
    var i = 0;
    while i < std::array::len(xs) {
        let x = std::array::get(xs, i);
        let seen = match std::map::get(freq, x) {
            Some { value } => value,
            None => 0,
        };
        let _ = std::map::insert(freq, x, seen + 1);
        i = i + 1;
    }
    match std::map::get(freq, needle) {
        Some { value } => value,
        None => 0,
    }
}

spec count_of {
    then count_of(build(), 4) == 2;
    then count_of(build(), 9) == 1;
    then count_of(build(), 7) == 0;
}
""",
        notes="`std::map::get` returns `Option[V]` and never traps; "
        "`std::map::insert` returns the PREVIOUS value, so bind it to `_`. "
        "A map is built through a `var` binding because the mutators take it "
        "by `mut` borrow.",
    ),
    Seed(
        concept="hash-map",
        task="Using a `Map[Str, Int]`, write `score_of(key)` that looks up a "
        "score for a string key, returning 0 when the key is absent.",
        solution="""fn score_of(in key: Str) -> Int {
    var m = std::map::empty();
    let _ = std::map::insert(m, "alpha", 10);
    let _ = std::map::insert(m, "beta", 20);
    match std::map::get(m, key) {
        Some { value } => value,
        None => 0,
    }
}

spec score_of {
    then score_of("alpha") == 10;
    then score_of("beta") == 20;
    then score_of("gamma") == 0;
}
""",
        notes="The v0 map operation surface is `Map[Int, Int]` and "
        "`Map[Str, Int]`; any other key/value pair is a `T0001`.",
    ),
    Seed(
        concept="hash-map",
        task="Write `distinct(xs)` returning how many distinct values an "
        "`Array[Int]` holds, using a map's keys.",
        solution="""fn sample() -> Array[Int] {
    var xs = std::array::empty();
    std::array::push(xs, 3);
    std::array::push(xs, 3);
    std::array::push(xs, 8);
    xs
}

fn distinct(in xs: Array[Int]) -> Int {
    var seen = std::map::empty();
    var i = 0;
    while i < std::array::len(xs) {
        let _ = std::map::insert(seen, std::array::get(xs, i), 1);
        i = i + 1;
    }
    std::array::len(std::map::keys(seen))
}

spec distinct {
    then distinct(sample()) == 2;
    then distinct(std::array::empty()) == 0;
}
""",
        notes="`std::map::keys` returns a NEW `Array[K]` in insertion "
        "(first-seen) order — deterministic across all three engines.",
    ),
    Seed(
        concept="hash-map",
        task="Key a `Map[Str, Int]` with a view borrowed from an owned "
        "`String` via `std::string::as_str`, and look entries up by Str.",
        solution="""fn tally_for(in name: Str) -> Int {
    var owned = std::string::empty();
    std::string::append(owned, "beta");
    var m = std::map::empty();
    let _ = std::map::insert(m, "alpha", 1);
    let _ = std::map::insert(m, std::string::as_str(owned), 2);
    match std::map::get(m, name) {
        Some { value } => value,
        None => 0,
    }
}

spec tally_for {
    then tally_for("alpha") == 1;
    then tally_for("beta") == 2;
    then tally_for("gamma") == 0;
}
""",
        notes="`std::string::as_str` is a zero-copy `Str` view into a live "
        "`String` (ADR-0010); it composes with `Map[Str, Int]` keys.",
    ),
    # --- the data increment (ADR-0016) ----------------------------------
    Seed(
        concept="array-set",
        task="Write `zero_first(xs)` that overwrites element 0 of a growable "
        "`Array[Int]` in place with 0 (leaving an empty array alone) and "
        "returns it.",
        solution="""fn two() -> Array[Int] {
    var xs = std::array::empty();
    std::array::push(xs, 5);
    std::array::push(xs, 6);
    xs
}

fn head(in xs: Array[Int]) -> Int {
    if std::array::len(xs) > 0 { std::array::get(xs, 0) } else { -1 }
}

fn zero_first(take xs: Array[Int]) -> Array[Int] {
    var ys = xs;
    if std::array::len(ys) > 0 {
        std::array::set(ys, 0, 0);
    }
    ys
}

spec zero_first {
    then head(zero_first(two())) == 0;
    then std::array::len(zero_first(two())) == 2;
}
""",
        notes="`std::array::set` is an in-place indexed write (ADR-0016). It "
        "never grows the array and traps IndexOutOfBounds off the end; the "
        "old element is dropped in place.",
    ),
    Seed(
        concept="float-array",
        task="Write `mean(xs)` over an `Array[Float]`, returning 0.0 for an "
        "empty array. Spec it.",
        solution="""fn sample() -> Array[Float] {
    var xs = std::array::empty();
    std::array::push(xs, 1.0);
    std::array::push(xs, 2.0);
    std::array::push(xs, 3.0);
    xs
}

fn mean(in xs: Array[Float]) -> Float {
    if std::array::len(xs) == 0 {
        return 0.0;
    }
    var total = 0.0;
    var i = 0;
    while i < std::array::len(xs) {
        total = total + std::array::get(xs, i);
        i = i + 1;
    }
    total / (std::array::len(xs) as Float)
}

spec mean {
    then mean(sample()) == 2.0;
    then mean(std::array::empty()) == 0.0;
}
""",
        notes="`Float` array elements landed with ADR-0016. `as Float` is a "
        "saturating cast; float literals need the decimal point (`0.0`).",
    ),
    Seed(
        concept="json",
        task="Using `std::json`, write `port_of(text)` that parses a JSON "
        "object and returns its integer `port` member, or 0 when the input is "
        "not valid JSON or has no such member.",
        solution="""module app;

import std::json;

fn port_of(in text: Str) -> Int {
    match std::json::parse(text) {
        Ok { value } => {
            let node = std::json::member(value, std::json::root(), "port");
            if node < 0 { 0 } else { std::json::num_of(value, node) as Int }
        },
        Err { error } => 0,
    }
}

spec port_of {
    then port_of("{\\"port\\": 8080}") == 8080;
    then port_of("{\\"host\\": \\"x\\"}") == 0;
    then port_of("not json") == 0;
}
""",
        notes="A parsed `Json` is an index arena navigated by node index; "
        "`member` returns a negative index when the key is absent, and JSON "
        "numbers are `Float` (cast with `as Int`). std::json is entirely pure, "
        "so it runs in the spec sandbox.",
        stdlib_deps=("json",),
    ),
    # --- concurrency and effects (ADR-0007, ADR-0013/14/15) -------------
    Seed(
        concept="concurrency",
        task="Write a runnable program whose `main` squares four tasks in "
        "parallel with `std::rt::par_map` over two workers and returns the sum "
        "of the results.",
        solution="""module app;

fn square(take n: Int) -> Int {
    n * n
}

fn main() -> Int {
    var tasks = std::array::empty();
    std::array::push(tasks, 1);
    std::array::push(tasks, 2);
    std::array::push(tasks, 3);
    std::array::push(tasks, 4);
    let results = std::rt::par_map(square, tasks, 2);
    var total = 0;
    var i = 0;
    while i < std::array::len(results) {
        total = total + std::array::get(results, i);
        i = i + 1;
    }
    total
}
""",
        runnable=True,
        run_exit=30,
        notes="`par_map` is structured fork-join: it takes a NON-capturing "
        "function value, runs the tasks across N OS threads, and joins before "
        "returning — so no data race is expressible. It is an EFFECT, so it "
        "cannot appear in a spec (R0007); this program is verified by running.",
    ),
    Seed(
        concept="channels",
        task="Write a runnable program that sends five doubled values into a "
        "channel, closes it, then drains it and returns the total.",
        solution="""module app;

fn main() -> Int {
    let ch = std::rt::chan_new();
    var i = 1;
    while i <= 5 {
        let _ = std::rt::chan_send(ch, i * 2);
        i = i + 1;
    }
    let _ = std::rt::chan_close(ch);
    var total = 0;
    var v = std::rt::chan_recv(ch);
    while v >= 0 {
        total = total + v;
        v = std::rt::chan_recv(ch);
    }
    total
}
""",
        runnable=True,
        run_exit=30,
        notes="Channels (ADR-0015) carry non-negative Ints by copy through a "
        "process-lived Int handle; `chan_recv` returns -1 once the channel is "
        "closed AND drained, which is the drain loop's sentinel. An EFFECT, so "
        "no spec (R0007).",
    ),
    Seed(
        concept="filesystem",
        task="Write a runnable program that writes a short file with "
        "`std::fs`, reads it back, removes it, and returns the byte count it "
        "read.",
        solution="""module app;

import std::fs;

fn main() -> Int {
    let path = "tuo_seed_scratch.txt";
    match std::fs::write(path, "hello") {
        Ok { value } => {
            let n = match std::fs::read(path) {
                Ok { value } => std::string::len(value),
                Err { error } => 0,
            };
            match std::fs::remove(path) {
                Ok { value } => n,
                Err { error } => 0,
            }
        },
        Err { error } => 0,
    }
}
""",
        runnable=True,
        run_exit=5,
        notes="`std::fs` returns `Result[_, FsError]` over the ADR-0013 file "
        "primitives. The whole tier is an EFFECT (no spec, R0007) — correctness "
        "is proven by really running it.",
        stdlib_deps=("fs",),
    ),
    # --- bounded waits, IPv6, UDP (ADR-0017) ----------------------------
    Seed(
        concept="net-outcomes",
        task="Write `classify(outcome)` that turns a `std::net` return value "
        "into 0 for success, 1 for a timeout, and 2 for a host error. Spec "
        "every branch.",
        solution="""fn classify(take outcome: Int) -> Int {
    if outcome >= 0 {
        0
    } else {
        if outcome == -3 { 1 } else { 2 }
    }
}

spec classify {
    then classify(7) == 0;
    then classify(0) == 0;
    then classify(-3) == 1;
    then classify(-1) == 2;
    then classify(-2) == 2;
}
""",
        notes="The socket seam never traps: it reports through negative return "
        "values. A descriptor/length is `>= 0`, `-3` is the ADR-0017 timeout "
        "sentinel NET_TIMEOUT, and `-1`/`-2` are host errors. The stdlib spells "
        "these once as `std::net::is_descriptor` / `is_timeout`. Classifying "
        "is pure, so unlike the socket calls themselves it can carry a spec.",
    ),
    Seed(
        concept="timeouts",
        task="Write a runnable program that binds a UDP socket, waits at most "
        "50ms for a datagram that never arrives, and returns 42 when the wait "
        "times out instead of hanging.",
        solution="""module app;

import std::net;

fn main() -> Int {
    let sock = std::net::udp_bind(0);
    if sock < 0 { return 1; }
    let outcome = std::net::udp_recv(sock, 50);
    let _ = std::net::close(sock);
    if std::net::is_timeout(outcome) { 42 } else { 0 }
}
""",
        runnable=True,
        run_exit=42,
        notes="ADR-0017 added bounded waits so a program can RECOVER from a "
        "silent peer instead of blocking forever. A timeout is a value you "
        "branch on (`is_timeout`), not a trap. `ms` of 0 is a valid poll; a "
        "negative `ms` is a host error, never an unbounded wait. An EFFECT, so "
        "no spec (R0007) — proven by running.",
        stdlib_deps=("net",),
    ),
    Seed(
        concept="ipv6",
        task="Write a runnable program that listens on an ephemeral IPv6 "
        "loopback port, connects to it over `::1`, accepts the connection, and "
        "returns 6 when the peer family really is IPv6.",
        solution="""module app;

import std::net;

fn main() -> Int {
    let server = std::net::listen6(0);
    if server < 0 { return 1; }
    let port = std::net::bound_port(server);
    if port < 0 { return 2; }
    let client = std::net::connect("::1", port);
    if client < 0 { return 3; }
    let session = std::net::accept(server);
    if session < 0 { return 4; }
    let family = std::net::peer_family(session);
    let _ = std::net::close(session);
    let _ = std::net::close(client);
    let _ = std::net::close(server);
    if std::net::is_ipv6(family) { 6 } else { 4 }
}
""",
        runnable=True,
        run_exit=6,
        notes="A server cannot infer an address family from a port, so IPv6 "
        "listening is the explicit `listen6`. The CLIENT side needs no new "
        "spelling: `connect` infers the family from the numeric address, so "
        "`connect(\"::1\", port)` just works. `bound_port` covers both "
        "families. An EFFECT, so no spec (R0007).",
        stdlib_deps=("net",),
    ),
    Seed(
        concept="udp",
        task="Write a runnable program that sends the datagram \"ping\" from "
        "one UDP socket to another on loopback, reads the payload back byte by "
        "byte, and returns the byte sum modulo 100.",
        solution="""module app;

fn main() -> Int {
    let server = std::rt::udp_bind(0);
    if server < 0 { return 1; }
    let port = std::rt::bound_port(server);
    if port < 0 { return 2; }
    let client = std::rt::udp_bind(0);
    if client < 0 { return 3; }

    let sent = std::rt::udp_send(client, "127.0.0.1", port, "ping");
    if sent < 0 { return 4; }

    let n = std::rt::udp_recv(server, 2000);
    if n < 0 { return 5; }

    var sum = 0;
    var i = 0;
    while i < n {
        sum = sum + std::rt::udp_byte_at(server, i);
        i = i + 1;
    }
    let _ = std::rt::close(server);
    let _ = std::rt::close(client);
    sum % 100
}
""",
        runnable=True,
        run_exit=30,
        notes="A datagram is a MESSAGE, not a stream: `udp_recv` returns the "
        "message LENGTH and stages the payload, which `udp_byte_at` then "
        "indexes — `read_byte` (the stream spelling) cannot see it. "
        "`udp_peer_port` names the sender so a server can reply. An EFFECT, so "
        "no spec (R0007).",
    ),
]
