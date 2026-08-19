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
]
