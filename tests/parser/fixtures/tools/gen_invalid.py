#!/usr/bin/env python3
"""Generate the intentionally-invalid tuonelang parser fixtures.

Every fixture here MUST be rejected at the LEXICAL or SYNTACTIC (concrete-
grammar) level — NOT the semantic level. A fixture that is merely ill-typed but
grammatically well-formed does not belong here (it would parse fine); those
belong under tests/types. Each entry records the reason it must fail and a
`stage` of "lexical" or "syntax".

The manifest (invalid.manifest.json) pairs every fixture with its failure reason
so the future parser's diagnostics can be asserted against it.
"""
import json, pathlib

# This script lives at tests/parser/fixtures/tools/; fixtures go one level up.
OUT = pathlib.Path(__file__).resolve().parent.parent / "invalid"

# (slug, stage, reason, source)
I = []
def add(slug, stage, reason, src):
    I.append((slug, stage, reason, src.strip("\n") + "\n"))

# ===== LEXICAL errors (§1, §2, §3, §10) ===================================
add("tab-indentation", "lexical", "tab character used for indentation (§1 forbids tabs outside literals)",
    "fn f() {\n\tlet x = 1;\n}")
add("crlf-line-ending", "lexical", "CRLF line ending; only LF is permitted (§1)",
    "fn f() {\r\n    let x = 1;\r\n}")
add("block-comment", "lexical", "block comment /* */ does not exist in tuonelang (§3)",
    "/* block comment */\nfn f() {}")
add("nested-block-comment", "lexical", "nested block comment; block comments do not exist (§3)",
    "fn f() { /* outer /* inner */ */ }")
add("doc-block-comment", "lexical", "/** */ doc form does not exist; only /// (§3)",
    "/** doc */\nfn f() {}")
add("unterminated-string", "lexical", "string literal not closed before end of line (§10)",
    "fn f() -> Str { \"unterminated }")
add("unterminated-char", "lexical", "char literal not closed (§10)",
    "fn f() -> Char { 'a }")
add("multi-char-char-literal", "lexical", "char literal holds more than one scalar (§10)",
    "fn f() -> Char { 'ab' }")
add("newline-in-string", "lexical", "raw newline inside a string literal; use \\n (§10)",
    "fn f() -> Str { \"a\nb\" }")
add("bad-escape", "lexical", "unknown escape sequence in a string (§10)",
    "fn f() -> Str { \"\\q\" }")
add("trailing-dot-float", "lexical", "`1.` is not a float literal; write 1.0 (lexical rule)",
    "fn f() -> Float { 1. }")
add("leading-dot-float", "lexical", "`.5` is not a float literal; write 0.5 (lexical rule)",
    "fn f() -> Float { .5 }")
add("bad-numeric-suffix", "lexical", "unknown integer suffix (§10 suffix set is closed)",
    "fn f() { let x = 10i7; }")
add("stray-backslash", "lexical", "stray backslash outside a literal is not a token",
    "fn f() { let x = \\ ; }")
add("stray-at-sign", "lexical", "`@` is not a token in v0",
    "fn f() { let x = @y; }")
add("stray-dollar", "lexical", "`$` is not a token in v0",
    "fn f() { let $x = 1; }")
add("hex-no-digits", "lexical", "`0x` prefix with no hex digits (§10)",
    "fn f() { let x = 0x; }")
add("keyword-as-ident", "lexical", "reserved keyword `match` used as a binding name (§4)",
    "fn f() { let match = 1; }")
add("keyword-fn-as-ident", "lexical", "reserved keyword `fn` used as a variable (§4)",
    "fn f() { let fn = 1; }")
add("bom-present", "lexical", "byte-order mark at start of file is forbidden (§1)",
    "﻿fn f() {}")

# ===== SYNTAX errors — items, functions ==================================
add("fn-missing-name", "syntax", "function has no name",
    "fn () {}")
add("fn-missing-parens", "syntax", "function signature missing parameter parentheses",
    "fn f { }")
add("fn-missing-body", "syntax", "function declared without a body or signature terminator",
    "fn f()")
add("fn-unclosed-body", "syntax", "function body brace never closed",
    "fn f() {")
add("fn-param-no-type", "syntax", "parameter missing its type annotation (§8)",
    "fn f(in x) {}")
add("fn-param-no-mode", "syntax", "parameter missing its in/mut/take mode (§22)",
    "fn f(x: Int) {}")
add("fn-param-no-colon", "syntax", "parameter missing the colon before its type",
    "fn f(in x Int) {}")
add("fn-double-arrow", "syntax", "malformed return arrow",
    "fn f() => Int { 0 }")
add("fn-return-no-type", "syntax", "arrow with no return type",
    "fn f() -> { 0 }")
add("fn-trailing-comma-only", "syntax", "parameter list is a lone comma",
    "fn f(,) {}")
add("fn-missing-param-separator", "syntax", "two parameters without a separating comma",
    "fn f(in a: Int in b: Int) {}")

# ===== SYNTAX errors — statements, blocks ================================
add("missing-semicolon", "syntax", "statement not terminated by a semicolon (§5, no ASI)",
    "fn f() { let x = 1 let y = 2; }")
add("let-missing-pattern", "syntax", "let with no binding pattern before `=`",
    "fn f() { let = 1; }")
add("let-missing-eq", "syntax", "let with an initializer but no `=`",
    "fn f() { let x 5; }")
add("var-no-name", "syntax", "var binding without a name",
    "fn f() { var : Int = 0; }")
add("unbalanced-open-brace", "syntax", "extra opening brace",
    "fn f() { { }")
add("unbalanced-close-brace", "syntax", "extra closing brace",
    "fn f() { } }")
add("unclosed-paren", "syntax", "unclosed parenthesis in an expression",
    "fn f() -> Int { (1 + 2 }")
add("unclosed-bracket", "syntax", "unclosed index bracket",
    "fn f(in xs: List[Int]) -> Int { xs[0 }")
add("stray-semicolon-in-params", "syntax", "semicolon where a comma is required",
    "fn f(in a: Int; in b: Int) {}")
add("assign-in-condition-missing-brace", "syntax", "if condition not followed by a block",
    "fn f(in c: Bool) { if c do(); }")
add("braceless-if-body", "syntax", "brace-less if body (§5 mandatory braces)",
    "fn f(in c: Bool) { if c return 1; }")
add("braceless-while-body", "syntax", "brace-less while body (§5)",
    "fn f() { while cond do(); }")
add("braceless-for-body", "syntax", "brace-less for body (§5)",
    "fn f() { for x in xs use(x); }")

# ===== SYNTAX errors — structs, enums ===================================
add("struct-missing-name", "syntax", "struct with no name",
    "struct { x: Int }")
add("struct-positional-field", "syntax", "positional/tuple field; only named fields (§12)",
    "struct Point(F64, F64);")
add("struct-field-no-type", "syntax", "struct field missing its type",
    "struct P { x }")
add("struct-field-no-colon", "syntax", "struct field missing the colon",
    "struct P { x F64 }")
add("struct-semicolon-separator", "syntax", "fields separated by `;` instead of `,`",
    "struct P { x: F64; y: F64 }")
add("struct-unclosed", "syntax", "struct body never closed",
    "struct P { x: F64")
add("enum-missing-name", "syntax", "enum with no name",
    "enum { A, B }")
add("enum-positional-payload", "syntax", "positional variant payload; only named (§13)",
    "enum E { V(Int) }")
add("enum-eq-discriminant", "syntax", "C-style integer discriminant (§13 forbids)",
    "enum E { A = 0, B = 1 }")
add("enum-unclosed", "syntax", "enum body never closed",
    "enum E { A, B")

# ===== SYNTAX errors — generics, types ==================================
add("generic-angle-brackets", "syntax", "angle-bracket generics; v0 uses [T] (§18)",
    "fn id<T>(take x: T) -> T { x }")
add("type-angle-brackets", "syntax", "angle brackets on a type argument (§18)",
    "fn f(in m: Map<Str, Int>) {}")
add("generic-empty-brackets", "syntax", "empty generic parameter list",
    "fn f[]() {}")
add("generic-missing-close", "syntax", "generic parameter list not closed",
    "fn f[T(take x: T) {}")
add("bound-missing-interface", "syntax", "bound colon with no interface named",
    "fn f[T: ]() {}")
add("where-missing-bound", "syntax", "where predicate with no bound",
    "fn f[T]() where T: {}")
add("type-double-colon-trailing", "syntax", "path ends with a trailing `::`",
    "fn f(in x: std::io::) {}")

# ===== SYNTAX errors — interfaces, impls ================================
add("interface-missing-name", "syntax", "interface with no name",
    "interface { fn m(in self); }")
add("interface-method-no-semicolon", "syntax", "required method signature missing `;` or body",
    "interface I { fn m(in self) -> Int }")
add("impl-missing-for", "syntax", "impl of an interface without `for T` (inherent impl not in v0)",
    "impl Ord { fn compare(in self) {} }")
add("impl-missing-type", "syntax", "impl ... for with no target type",
    "impl Ord for { }")
add("impl-unclosed", "syntax", "impl body never closed",
    "impl Ord for Point {")

# ===== SYNTAX errors — imports ==========================================
add("import-glob", "syntax", "glob import is forbidden (§7)",
    "import std::io::*;")
add("import-no-semicolon", "syntax", "import not terminated by a semicolon",
    "import std::io")
add("import-empty-group", "syntax", "empty import group",
    "import std::io::{};")
add("import-group-no-close", "syntax", "import group brace not closed",
    "import std::io::{read, write;")
add("import-double-colon-start", "syntax", "import path starting with `::`",
    "import ::std::io;")
add("import-as-no-name", "syntax", "`as` with no rename target",
    "import std::io as ;")

# ===== SYNTAX errors — match, patterns ==================================
add("match-missing-scrutinee", "syntax", "match with no scrutinee expression",
    "fn f() { match { _ => 0 } }")
add("match-missing-arrow", "syntax", "match arm missing `=>`",
    "fn f(in n: Int) -> Int { match n { 0 1, _ => 0 } }")
add("match-arm-comma-missing", "syntax", "two arms without a separating comma",
    "fn f(in n: Int) -> Int { match n { 0 => 0 1 => 1, _ => 2 } }")
add("match-unclosed", "syntax", "match body never closed",
    "fn f(in n: Int) -> Int { match n { _ => 0 ")
add("match-fat-arrow-reversed", "syntax", "reversed arm arrow `<=` instead of `=>`",
    "fn f(in n: Int) -> Int { match n { _ <= 0 } }")
add("pattern-double-pipe", "syntax", "or-pattern with a doubled `||`",
    "fn f(in n: Int) -> Int { match n { 0 || 1 => 0, _ => 1 } }")
add("range-pattern", "syntax", "range pattern is not in v0 (§17)",
    "fn f(in n: Int) -> Int { match n { 0 .. 9 => 0, _ => 1 } }")
add("ref-pattern", "syntax", "ref pattern is not in v0 (§17)",
    "fn f(in o: Option[Int]) -> Int { match o { Some { ref value } => 0, None => 1 } }")

# ===== SYNTAX errors — expressions, operators ===========================
add("chained-comparison", "syntax", "non-associative comparison chained (a < b < c)",
    "fn f(in a: Int, in b: Int, in c: Int) -> Bool { a < b < c }")
add("compound-assignment", "syntax", "`+=` compound assignment not in v0",
    "fn f() { var x = 0; x += 1; }")
add("bitwise-or", "syntax", "bitwise `|` operator not in v0",
    "fn f(in a: Int, in b: Int) -> Int { a | b }")
add("shift-operator", "syntax", "shift `<<` operator not in v0",
    "fn f(in a: Int) -> Int { a << 2 }")
add("increment-operator", "syntax", "`++` operator does not exist",
    "fn f() { var x = 0; x++; }")
add("ternary-operator", "syntax", "C-style ternary `?:` does not exist (use if-expr)",
    "fn f(in c: Bool) -> Int { c ? 1 : 0 }")
add("double-dot-dot", "syntax", "malformed range `...`",
    "fn f() -> Range { 0 ... 9 }")
add("empty-call-comma", "syntax", "call args are a lone comma",
    "fn f() { g(,); }")
add("struct-literal-in-if-cond", "syntax", "unparenthesized struct literal in if condition",
    "fn f() -> Bool { if Point { x: 0.0, y: 0.0 }.is_origin() { true } else { false } }")
add("dangling-operator", "syntax", "binary operator with no right operand",
    "fn f() -> Int { 1 + }")
add("dangling-dot", "syntax", "member access with no member name",
    "fn f(in p: Point) -> F64 { p. }")
add("as-no-type", "syntax", "`as` conversion with no target type",
    "fn f(in n: Int) { let x = n as; }")
add("double-question", "syntax", "malformed use of `?` as a prefix",
    "fn f() -> Int { ?x }")

# ===== SYNTAX errors — const, misc items ================================
add("const-no-type", "syntax", "const without a type annotation (§9 requires it)",
    "const K = 3;")
add("const-no-value", "syntax", "const without an initializer",
    "const K: Int;")
add("const-no-semicolon", "syntax", "const not terminated by a semicolon",
    "const K: Int = 3")
add("pub-nothing", "syntax", "`pub` with no item following",
    "pub ;")
add("stray-keyword", "syntax", "a keyword where an item is expected",
    "return 0;")
add("top-level-expression", "syntax", "a bare expression at module top level",
    "1 + 2;")
add("module-no-semicolon", "syntax", "module declaration missing its semicolon",
    "module math")

# ===== SYNTAX errors — specs (experimental surface) =====================
add("spec-missing-name", "syntax", "spec with no name",
    "spec { assert x == 1; }")
add("spec-missing-body", "syntax", "spec with no body",
    "spec fibonacci")
add("spec-unclosed", "syntax", "spec body never closed",
    "spec s { assert x == 1;")
add("spec-assert-no-semicolon", "syntax", "assert clause missing its semicolon",
    "spec s { assert x == 1 }")
add("spec-given-no-type", "syntax", "given binding missing its type",
    "spec s { given a; then a == a; }")
add("spec-then-no-expr", "syntax", "then clause with no condition",
    "spec s { then ; }")
add("spec-given-no-semicolon", "syntax", "given clause missing its semicolon",
    "spec s { given a: Int then a == a; }")

# ===== a few more to comfortably exceed 100 =============================
add("fn-body-missing-open", "syntax", "function body missing its opening brace",
    "fn f() -> Int 0 }")
add("nested-unclosed", "syntax", "nested block left unclosed",
    "fn f() { if c { g(); }")
add("comma-before-arrow", "syntax", "stray comma before the return arrow",
    "fn f(in a: Int,) , -> Int { a }")
add("two-return-types", "syntax", "two return arrows",
    "fn f() -> Int -> Int { 0 }")
add("let-two-eq", "syntax", "let with a doubled `=`",
    "fn f() { let x == 1; }")
add("misplaced-else", "syntax", "else with no preceding if",
    "fn f() { else { g(); } }")
add("keyword-self-as-binding", "syntax", "reserved word `Self` used as a binding pattern; a pattern wants an IDENT (§4)",
    "fn f() { let Self = 1; }")

def main():
    OUT.mkdir(parents=True, exist_ok=True)
    manifest = []
    seen = set()
    for i, (slug, stage, reason, src) in enumerate(I, start=1):
        assert slug not in seen, f"duplicate slug {slug}"
        seen.add(slug)
        name = f"{i:03d}-{slug}.tuo"
        (OUT / name).write_text(src, encoding="utf-8")
        manifest.append({"file": name, "stage": stage, "reason": reason})
    (OUT.parent / "invalid.manifest.json").write_text(
        json.dumps({"schema_version": 1, "kind": "invalid", "count": len(manifest),
                    "fixtures": manifest}, indent=2) + "\n", encoding="utf-8")
    lex = sum(1 for m in manifest if m["stage"] == "lexical")
    print(f"wrote {len(manifest)} invalid fixtures ({lex} lexical, {len(manifest)-lex} syntax)")

if __name__ == "__main__":
    main()
