#!/usr/bin/env python3
"""Generate the positive (valid-syntax) tuonelang parser fixtures.

Each fixture is a hand-authored .tuo snippet that MUST parse under the v0
concrete grammar (grammar.ebnf). They are grouped by grammar feature; the number
prefix keeps them ordered and stable. A manifest (valid.manifest.json) lists
every fixture with a one-line description of the construct it exercises.
"""
import json, pathlib

# This script lives at tests/parser/fixtures/tools/; fixtures go one level up.
OUT = pathlib.Path(__file__).resolve().parent.parent / "valid"

# (slug, description, source). slug becomes NNN-slug.tuo in emission order.
V = []
def add(slug, desc, src):
    V.append((slug, desc, src.strip() + "\n"))

# ---- modules & imports (§6, §7) ------------------------------------------
add("module-decl", "optional self-naming module declaration",
    "module math::linalg;\n\nfn zero() -> Int { 0 }")
add("import-simple", "single import binding a name",
    "import std::io;")
add("import-nested-path", "deep path import",
    "import std::collections::Map;")
add("import-group", "grouped import leaves",
    "import std::io::{read, write};")
add("import-group-trailing-comma", "grouped import with trailing comma",
    "import std::io::{read, write,};")
add("import-rename", "import with `as` rename",
    "import std::io::read as fs_read;")
add("import-group-rename", "grouped import with a renamed leaf",
    "import std::collections::{Map, Set as HashSet};")
add("import-pub", "re-exported public import",
    "pub import std::io::read;")
add("empty-file", "an empty compilation unit",
    "")
add("only-comments", "a file of comments and whitespace only",
    "// a lonely comment\n// another one\n")

# ---- functions (§8) -------------------------------------------------------
add("fn-empty", "function with empty body returning unit",
    "fn main() {}")
add("fn-unit-explicit", "explicit unit return type",
    "fn noop() -> () {}")
add("fn-one-param", "single typed parameter",
    "fn square(in n: Int) -> Int { n * n }")
add("fn-two-params", "two parameters",
    "fn add(in a: Int, in b: Int) -> Int { a + b }")
add("fn-trailing-comma-params", "trailing comma in parameter list",
    "fn add(in a: Int, in b: Int,) -> Int { a + b }")
add("fn-return-stmt", "explicit early return",
    "fn pick(in c: Bool) -> Int { if c { return 1; } return 0; }")
add("fn-tail-expr", "block tail expression as value",
    "fn five() -> Int { let x = 5; x }")
add("fn-pub", "public function",
    "pub fn api() -> Int { 0 }")
add("fn-doc-comment", "doc comment attached to a function",
    "/// Returns the answer.\nfn answer() -> Int { 42 }")
add("fn-multi-doc-comment", "multiple doc-comment lines",
    "/// Line one.\n/// Line two.\nfn documented() {}")
add("fn-mode-mut", "mutable-borrow parameter mode",
    "fn bump(mut counter: Int) { counter = counter + 1; }")
add("fn-mode-take", "ownership-taking parameter mode",
    "fn consume(take value: String) {}")
add("fn-mixed-modes", "in/mut/take modes together",
    "fn f(in a: Int, mut b: Int, take c: String) {}")
add("fn-nested-call", "nested function calls",
    "fn go() -> Int { add(mul(2, 3), 4) }")
add("fn-where-clause", "generic function with a where clause",
    "fn largest[T](in xs: List[T]) -> T where T: Ord { first(xs) }")

# ---- generics (§18) -------------------------------------------------------
add("fn-generic-one", "one type parameter",
    "fn id[T](take x: T) -> T { x }")
add("fn-generic-bound", "bounded type parameter",
    "fn max[T: Ord](in a: T, in b: T) -> T { a }")
add("fn-generic-multi-bound", "multiple bounds joined by +",
    "fn show[T: Display + Clone](in x: T) {}")
add("fn-generic-two-params", "two type parameters",
    "fn pair[A, B](take a: A, take b: B) -> Pair[A, B] { Pair { first: a, second: b } }")
add("struct-generic", "generic struct declaration",
    "struct Pair[A, B] { first: A, second: B }")
add("enum-generic", "generic enum declaration",
    "enum Option[T] { Some { value: T }, None }")
add("type-nested-generic", "nested generic type argument",
    "fn f(in m: Map[Str, List[Int]]) {}")
add("turbofish", "explicit type arguments on a value path",
    "fn f() { let v = empty::[Int](); }")

# ---- structs (§12) --------------------------------------------------------
add("struct-empty", "struct with no fields",
    "struct Unit {}")
add("struct-one-field", "single field",
    "struct Wrapper { value: Int }")
add("struct-fields", "several fields",
    "struct Point { x: F64, y: F64 }")
add("struct-trailing-comma", "trailing comma after last field",
    "struct Point { x: F64, y: F64, }")
add("struct-pub-field", "public field",
    "struct Config { pub name: String, timeout: Int }")
add("struct-doc-fields", "doc comments on fields",
    "struct Point {\n    /// horizontal\n    x: F64,\n    /// vertical\n    y: F64,\n}")
add("struct-generic-where", "generic struct with where clause",
    "struct Sorted[T] where T: Ord { items: List[T] }")
add("struct-wrapper-type-field", "field of an ownership-wrapper type",
    "struct Node { next: Box[Node], value: Int }")

# ---- enums (§13) ----------------------------------------------------------
add("enum-c-like", "all payload-less variants",
    "enum Direction { North, South, East, West }")
add("enum-payload", "variant with a named-field payload",
    "enum Shape { Circle { radius: F64 }, Empty }")
add("enum-mixed", "mixed bare and payload variants",
    "enum Shape {\n    Circle { radius: F64 },\n    Rectangle { width: F64, height: F64 },\n    Empty,\n}")
add("enum-result", "the Result shape",
    "enum Result[T, E] { Ok { value: T }, Err { error: E } }")
add("enum-doc-variant", "doc comment on a variant",
    "enum Level {\n    /// lowest\n    Low,\n    High,\n}")
add("enum-trailing-comma", "trailing comma after last variant",
    "enum Bit { Zero, One, }")

# ---- interfaces & impls (§19) --------------------------------------------
add("interface-one-method", "interface with one required method",
    "interface Ord {\n    fn compare(in self, in other: Self) -> Ordering;\n}")
add("interface-default-body", "interface method with a default body",
    "interface Greet {\n    fn hello(in self) -> Str { \"hi\" }\n}")
add("interface-multi", "interface with several methods",
    "interface Collection {\n    fn len(in self) -> Int;\n    fn is_empty(in self) -> Bool;\n}")
add("interface-generic", "generic interface",
    "interface Into[T] {\n    fn into(take self) -> T;\n}")
add("impl-for", "impl of an interface for a type",
    "impl Ord for Point {\n    fn compare(in self, in other: Point) -> Ordering { Ordering::Equal }\n}")
add("impl-generic", "generic impl",
    "impl[T] Container for Vec[T] {\n    fn size(in self) -> Int { 0 }\n}")
add("impl-receiver-mut", "impl method with a mut receiver",
    "impl Counter for Cell {\n    fn incr(mut self) { self.n = self.n + 1; }\n}")
add("impl-receiver-take", "impl method with a take receiver",
    "impl Close for File {\n    fn close(take self) {}\n}")

# ---- const & bindings (§9) -----------------------------------------------
add("const-item", "module-level constant",
    "const MAX: Int = 100;")
add("const-expr", "constant initialized from an expression",
    "const AREA: Int = 10 * 20;")
add("let-typed", "let with type annotation",
    "fn f() { let x: Int = 1; }")
add("let-inferred", "let without annotation",
    "fn f() { let x = 1; }")
add("var-binding", "mutable var binding",
    "fn f() { var count: Int = 0; count = count + 1; }")
add("let-shadowing", "shadowing within a scope",
    "fn f() { let x = 1; let x = x + 1; }")
add("let-no-init", "let with no initializer (definite-assignment is semantic)",
    "fn f() { let x: Int; x = 3; }")
add("block-scoped-const", "const inside a block",
    "fn f() { const K: Int = 3; let y = K; }")

# ---- control flow (§16) ---------------------------------------------------
add("if-expr", "if/else expression",
    "fn f(in c: Bool) -> Int { if c { 1 } else { 0 } }")
add("if-else-if", "else-if chain",
    "fn sign(in n: Int) -> Int { if n > 0 { 1 } else if n < 0 { 0 - 1 } else { 0 } }")
add("if-statement", "if as a statement (no trailing semicolon)",
    "fn f(in c: Bool) { if c { do_a(); } }")
add("while-loop", "while statement",
    "fn f() { var i = 0; while i < 10 { i = i + 1; } }")
add("loop-break-value", "loop yielding a value via break",
    "fn f() -> Int { loop { break 42; } }")
add("for-range", "for over a range",
    "fn f() { for i in 0 .. 10 { use(i); } }")
add("for-iterable", "for over an iterable path",
    "fn f(in xs: List[Int]) { for x in xs { use(x); } }")
add("break-continue", "break and continue in a loop",
    "fn f() { loop { if done() { break; } continue; } }")
add("labeled-loop", "labeled loop with labeled break (experimental sigil)",
    "fn f() { 'outer: loop { loop { break 'outer; } } }")
add("nested-blocks", "nested bare blocks",
    "fn f() -> Int { { { 1 } } }")

# ---- match & patterns (§17) ----------------------------------------------
add("match-literal", "match on integer literals",
    "fn f(in n: Int) -> Int { match n { 0 => 0, 1 => 1, _ => 2 } }")
add("match-enum", "match on enum variants",
    "fn area(in s: Shape) -> F64 { match s { Circle { radius } => radius, Empty => 0.0 } }")
add("match-binding", "binding pattern in a match arm",
    "fn f(in o: Option[Int]) -> Int { match o { Some { value } => value, None => 0 } }")
add("match-or-pattern", "or-pattern in an arm",
    "fn f(in n: Int) -> Bool { match n { 0 | 1 | 2 => true, _ => false } }")
add("match-guard", "guard on a match arm",
    "fn f(in n: Int) -> Str { match n { x if x > 0 => \"pos\", _ => \"nonpos\" } }")
add("match-struct-rest", "struct pattern with `..` rest",
    "fn f(in p: Point) -> F64 { match p { Point { x, .. } => x } }")
add("match-block-arm", "block-bodied match arm",
    "fn f(in n: Int) -> Int { match n { 0 => { let z = 0; z }, _ => 1 } }")
add("match-wildcard-only", "single wildcard arm",
    "fn f(in n: Int) -> Int { match n { _ => 0 } }")
add("let-destructure-struct", "struct destructuring in let",
    "fn f(in p: Point) -> F64 { let Point { x, y } = p; x + y }")
add("match-nested-pattern", "nested field pattern",
    "fn f(in r: Result[Int, Str]) -> Int { match r { Ok { value } => value, Err { error } => 0 } }")

# ---- expressions & operators (§16, §20, §24) ------------------------------
add("arith-precedence", "arithmetic precedence",
    "fn f() -> Int { 1 + 2 * 3 - 4 / 2 % 3 }")
add("comparison", "comparison operators",
    "fn f(in a: Int, in b: Int) -> Bool { a <= b }")
add("logical-ops", "logical and/or/not",
    "fn f(in a: Bool, in b: Bool) -> Bool { !a && (a || b) }")
add("unary-neg", "unary negation",
    "fn f(in n: Int) -> Int { -n }")
add("grouping", "parenthesized grouping",
    "fn f() -> Int { (1 + 2) * 3 }")
add("question-operator", "postfix ? error propagation",
    "fn f() -> Result[Int, Err] { let x = may_fail()?; Ok { value: x } }")
add("method-chain", "chained method calls",
    "fn f(in s: String) -> Int { s.trim().len() }")
add("field-access", "field access",
    "fn f(in p: Point) -> F64 { p.x }")
add("index-expr", "indexing",
    "fn f(in xs: List[Int]) -> Int { xs[0] }")
add("as-cast", "numeric conversion with as",
    "fn f(in n: Int) -> U32 { n as U32 }")
add("call-trailing-comma", "trailing comma in call args",
    "fn f() { g(1, 2, 3,); }")
add("assignment-expr", "assignment to a var",
    "fn f() { var x = 0; x = 5; }")
add("move-expr", "explicit move keyword",
    "fn f(take s: String) { let t = move s; }")
add("range-expr", "range expression outside for",
    "fn f() -> Range { 0 .. 100 }")
add("bool-literals", "boolean literals",
    "fn f() -> Bool { true }")
add("char-literal", "char literal",
    "fn f() -> Char { 'a' }")
add("char-escape", "escaped char literal",
    "fn f() -> Char { '\\n' }")
add("string-literal", "string literal",
    "fn f() -> Str { \"tuonelang\" }")
add("string-escapes", "string with escapes",
    "fn f() -> Str { \"line\\n\\ttab\\\"quote\" }")
add("unicode-escape", "unicode scalar escape",
    "fn f() -> Char { '\\u{1F600}' }")
add("int-literals", "integer literals in several bases",
    "fn f() { let a = 42; let b = 0xFF; let c = 0o17; let d = 0b1010; let e = 1_000_000; }")
add("suffixed-literals", "type-suffixed numeric literals",
    "fn f() { let a = 255u8; let b = 1i64; let c = 3.14f64; }")
add("float-literals", "float literals with exponents",
    "fn f() { let a = 1.0; let b = 0.5; let c = 6.022e23; let d = 1.6E-19; }")
add("unit-value", "the unit value ()",
    "fn f() -> () { () }")
add("struct-literal", "struct literal construction",
    "fn f() -> Point { Point { x: 1.0, y: 2.0 } }")
add("struct-literal-shorthand", "field shorthand in a struct literal",
    "fn f(in x: F64, in y: F64) -> Point { Point { x, y } }")
add("struct-literal-generic", "generic struct literal",
    "fn f() -> Pair[Int, Str] { Pair { first: 1, second: \"a\" } }")
add("struct-literal-paren-cond", "parenthesized struct literal in if condition",
    "fn f() -> Bool { if (Point { x: 0.0, y: 0.0 }).is_origin() { true } else { false } }")
add("enum-variant-path", "enum variant via path",
    "fn f() -> Ordering { Ordering::Equal }")
add("nested-generic-call", "call with turbofish type args",
    "fn f() { let v = collect::[List[Int]](); }")

# ---- unsafe (§26) ---------------------------------------------------------
add("unsafe-block", "unsafe block expression",
    "fn f() { unsafe { raw_write(); } }")
add("unsafe-in-expr", "unsafe block yielding a value",
    "fn f() -> Int { unsafe { raw_read() } }")

# ---- specs (§27) — experimental surface ----------------------------------
add("spec-assert", "spec with a compact assert (Prompt-003 example)",
    "spec fibonacci {\n    assert fibonacci(0) == 0;\n}\n\nfn fibonacci(in n: Int) -> Int { n }")
add("spec-given-when-then", "spec with given/when/then",
    "spec \"add is commutative\" {\n    given a: Int, b: Int;\n    when let s1 = add(a, b);\n    when let s2 = add(b, a);\n    then s1 == s2;\n}")
add("spec-forward-reference", "spec references a function declared later",
    "spec double {\n    assert twice(2) == 4;\n}\n\nfn twice(in n: Int) -> Int { n * 2 }")
add("spec-string-name", "spec named by a description string",
    "spec \"empty list has length zero\" {\n    assert len(empty()) == 0;\n}")
add("spec-multiple-asserts", "spec with several assertions",
    "spec bounds {\n    assert min(1, 2) == 1;\n    assert max(1, 2) == 2;\n}")
add("spec-given-default", "given binding with a concrete value",
    "spec sample {\n    given n: Int = 5;\n    then square(n) == 25;\n}")
add("spec-when-expr", "when clause with an expression step",
    "spec effectful {\n    given xs: List[Int];\n    when push(xs, 1);\n    then len(xs) == 1;\n}")
add("spec-local-let", "spec with a local let statement",
    "spec local {\n    let expected = 10;\n    assert compute() == expected;\n}")

# ---- larger integration snippets -----------------------------------------
add("program-fibonacci", "a complete small program with a spec",
    "spec fibonacci {\n    assert fibonacci(0) == 0;\n    assert fibonacci(1) == 1;\n}\n\nfn fibonacci(in n: Int) -> Int {\n    if n < 2 {\n        n\n    } else {\n        fibonacci(n - 1) + fibonacci(n - 2)\n    }\n}")
add("program-shape-area", "enum + match + interface program",
    "enum Shape {\n    Circle { radius: F64 },\n    Rectangle { width: F64, height: F64 },\n}\n\nfn area(in s: Shape) -> F64 {\n    match s {\n        Circle { radius } => 3.14159 * radius * radius,\n        Rectangle { width, height } => width * height,\n    }\n}")
add("program-generic-stack", "generic struct with methods via impl",
    "struct Stack[T] { items: List[T] }\n\nimpl[T] Container for Stack[T] {\n    fn push(mut self, take item: T) { append(self.items, item); }\n    fn pop(mut self) -> Option[T] { remove_last(self.items) }\n}")
add("program-imports-and-const", "imports, const, and a function together",
    "import std::io::{read, write};\n\nconst BUFFER: Int = 4096;\n\nfn copy() -> Result[Int, IoErr] {\n    let data = read(BUFFER)?;\n    let n = write(data)?;\n    Ok { value: n }\n}")
add("program-nested-match", "nested match and option handling",
    "fn classify(in o: Option[Result[Int, Str]]) -> Int {\n    match o {\n        Some { value } => match value {\n            Ok { value } => value,\n            Err { error } => 0,\n        },\n        None => 0 - 1,\n    }\n}")

def main():
    OUT.mkdir(parents=True, exist_ok=True)
    manifest = []
    for i, (slug, desc, src) in enumerate(V, start=1):
        name = f"{i:03d}-{slug}.tuo"
        (OUT / name).write_text(src, encoding="utf-8")
        manifest.append({"file": name, "construct": desc})
    (OUT.parent / "valid.manifest.json").write_text(
        json.dumps({"schema_version": 1, "kind": "valid", "count": len(manifest),
                    "fixtures": manifest}, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {len(manifest)} valid fixtures")

if __name__ == "__main__":
    main()
