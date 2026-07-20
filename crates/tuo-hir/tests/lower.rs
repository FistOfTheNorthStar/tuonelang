//! Lowering tests: canonically equivalent syntax produces equivalent HIR,
//! spans and semantic IDs are retained, and irrelevant forms are erased.
//!
//! Equivalence is asserted on the development pretty-print, which shows
//! every semantic fact (resolved symbols, structure, literals) and omits
//! spans — exactly the equality "same program, different spelling" should
//! have.

use tuo_ast::Ast;
use tuo_hir::{ExprKind, Hir, Item, Res, StmtKind};
use tuo_resolve::Resolution;
use tuo_source::SourceMap;

/// Parse (cleanly), resolve (cleanly), and lower one program snapshot.
fn build(sources: &[&str]) -> (Hir, Resolution) {
    let mut map = SourceMap::new();
    let mut parses = Vec::new();
    for (index, text) in sources.iter().enumerate() {
        let file = map.intern_file(&format!("file{index}.tuo"));
        let id = map.add_source(file, *text).expect("fixture fits");
        let parse = tuo_parser::parse(map.source(id));
        assert_eq!(parse.diagnostics, vec![], "fixtures must parse cleanly");
        parses.push(parse);
    }
    let asts: Vec<Ast<'_>> = parses
        .iter()
        .zip(sources)
        .map(|(parse, text)| Ast::new(&parse.tree, text))
        .collect();
    let resolution = tuo_resolve::resolve(&asts);
    assert_eq!(
        resolution.diagnostics(),
        &[],
        "fixtures must resolve cleanly"
    );
    let hir = tuo_hir::lower(&asts, &resolution);
    (hir, resolution)
}

/// The span-free development dump of a program.
fn dump(sources: &[&str]) -> String {
    let (hir, resolution) = build(sources);
    tuo_hir::render(&hir, &resolution)
}

/// Both spellings must lower to *equal* HIR.
fn assert_equivalent(a: &[&str], b: &[&str]) {
    let left = dump(a);
    let right = dump(b);
    assert_eq!(
        left, right,
        "canonically equivalent programs must produce equivalent HIR"
    );
}

/// The two spellings mean different things and must *not* collapse.
fn assert_distinct(a: &[&str], b: &[&str]) {
    let left = dump(a);
    let right = dump(b);
    assert_ne!(
        left, right,
        "semantically different programs must not lower to the same HIR"
    );
}

// ----------------------------------------------------------------------
// Canonically equivalent spellings
// ----------------------------------------------------------------------

#[test]
fn parentheses_are_erased() {
    assert_equivalent(
        &["fn f(in a: Int, in b: Int, in c: Int) -> Int {\n    (a + b) * c\n}\n"],
        &["fn f(in a: Int, in b: Int, in c: Int) -> Int {\n    (((a + b)) * (c))\n}\n"],
    );
}

#[test]
fn parentheses_that_change_precedence_still_matter() {
    assert_distinct(
        &["fn f(in a: Int, in b: Int, in c: Int) -> Int {\n    (a + b) * c\n}\n"],
        &["fn f(in a: Int, in b: Int, in c: Int) -> Int {\n    a + b * c\n}\n"],
    );
}

#[test]
fn trailing_commas_are_erased() {
    assert_equivalent(
        &[concat!(
            "struct Point { x: Int, y: Int }\n",
            "fn f(in a: Int, in b: Int) -> Point {\n",
            "    let p = Point { x: g(a, b), y: b };\n    p\n}\n",
            "fn g(in a: Int, in b: Int) -> Int { a + b }\n",
        )],
        &[concat!(
            "struct Point { x: Int, y: Int, }\n",
            "fn f(in a: Int, in b: Int,) -> Point {\n",
            "    let p = Point { x: g(a, b,), y: b, };\n    p\n}\n",
            "fn g(in a: Int, in b: Int) -> Int { a + b }\n",
        )],
    );
}

#[test]
fn comments_and_whitespace_are_erased() {
    assert_equivalent(
        &["fn f(in a: Int) -> Int {\n    a + 1\n}\n"],
        &[
            "// leading comment\nfn f( in a : Int )   -> Int {\n    // inline note\n    a + 1 // done\n}\n",
        ],
    );
}

#[test]
fn omitted_unit_return_type_is_explicit() {
    assert_equivalent(
        &["fn f() {\n    g();\n}\nfn g() {}\n"],
        &["fn f() -> () {\n    g();\n}\nfn g() -> () {}\n"],
    );
}

#[test]
fn numeric_separators_are_erased() {
    assert_equivalent(
        &["fn f() -> Int {\n    1_000_000\n}\n"],
        &["fn f() -> Int {\n    1000000\n}\n"],
    );
    assert_distinct(
        &["fn f() -> Int {\n    1000000\n}\n"],
        &["fn f() -> Int {\n    100000\n}\n"],
    );
}

#[test]
fn struct_literal_shorthand_is_expanded() {
    let longhand = concat!(
        "struct Point { x: Int, y: Int }\n",
        "fn f(in x: Int, in y: Int) -> Point {\n",
        "    Point { x: x, y: y }\n}\n",
    );
    let shorthand = concat!(
        "struct Point { x: Int, y: Int }\n",
        "fn f(in x: Int, in y: Int) -> Point {\n",
        "    Point { x, y }\n}\n",
    );
    assert_equivalent(&[longhand], &[shorthand]);
}

#[test]
fn field_pattern_shorthand_is_expanded() {
    let longhand = concat!(
        "fn f(in o: Option[Int]) -> Int {\n",
        "    match o {\n",
        "        Some { value: value } => value,\n",
        "        None => 0,\n",
        "    }\n}\n",
    );
    let shorthand = concat!(
        "fn f(in o: Option[Int]) -> Int {\n",
        "    match o {\n",
        "        Some { value } => value,\n",
        "        None => 0,\n",
        "    }\n}\n",
    );
    assert_equivalent(&[longhand], &[shorthand]);
}

#[test]
fn where_clause_bounds_merge_into_inline_bounds() {
    let inline = concat!(
        "interface Ord {\n    fn less(in self) -> Bool;\n}\n",
        "fn largest[T: Ord](in a: T) -> T {\n    a\n}\n",
    );
    let where_clause = concat!(
        "interface Ord {\n    fn less(in self) -> Bool;\n}\n",
        "fn largest[T](in a: T) -> T where T: Ord {\n    a\n}\n",
    );
    assert_equivalent(&[inline], &[where_clause]);
}

#[test]
fn qualified_paths_imports_and_aliases_all_resolve_to_the_same_symbol() {
    let util = "module util;\npub fn helper() -> Int {\n    7\n}\n";
    let qualified = "module app;\nfn go() -> Int {\n    util::helper()\n}\n";
    let imported = "module app;\nimport util::helper;\nfn go() -> Int {\n    helper()\n}\n";
    let aliased = "module app;\nimport util::helper as h;\nfn go() -> Int {\n    h()\n}\n";
    assert_equivalent(&[util, qualified], &[util, imported]);
    assert_equivalent(&[util, imported], &[util, aliased]);
}

#[test]
fn block_form_tail_statement_is_the_tail() {
    // A block-form expression standing last without `;` is grammatically a
    // statement, but semantically the block's tail value.
    let (hir, _resolution) = build(&[concat!(
        "fn f(in flag: Bool) -> Int {\n",
        "    if flag {\n        1\n    } else {\n        2\n    }\n",
        "}\n",
    )]);
    let Item::Fn(function) = &hir.items[0] else {
        panic!("expected a function");
    };
    let body = function.body.as_ref().expect("has body");
    assert_eq!(body.stmts.len(), 0, "the `if` must not remain a statement");
    let tail = body.tail.as_ref().expect("the `if` is the tail");
    assert!(matches!(tail.kind, ExprKind::If { .. }));
}

// ----------------------------------------------------------------------
// Meaningful differences must survive
// ----------------------------------------------------------------------

#[test]
fn let_and_var_stay_distinct() {
    assert_distinct(
        &["fn f() -> Int {\n    let x = 1;\n    x\n}\n"],
        &["fn f() -> Int {\n    var x = 1;\n    x\n}\n"],
    );
}

#[test]
fn parameter_modes_stay_distinct() {
    assert_distinct(
        &["fn f(in a: Int) -> Int {\n    a\n}\n"],
        &["fn f(mut a: Int) -> Int {\n    a\n}\n"],
    );
}

// ----------------------------------------------------------------------
// Semantic IDs and source mappings
// ----------------------------------------------------------------------

#[test]
fn names_are_resolved_to_stable_symbol_ids() {
    let text = concat!(
        "fn fibonacci(in n: Int) -> Int {\n    n\n}\n",
        "fn caller() -> Int {\n    fibonacci(3)\n}\n",
        "spec fibonacci {\n    assert fibonacci(1) == 1;\n}\n",
    );
    let (hir, resolution) = build(&[text]);
    let Item::Fn(fibonacci) = &hir.items[0] else {
        panic!("expected fn fibonacci");
    };
    let Res::Symbol(fibonacci_id) = fibonacci.symbol else {
        panic!("declaration must carry its symbol");
    };
    assert_eq!(resolution.symbol(fibonacci_id).name, "fibonacci");

    // The call in `caller` resolves to that same symbol …
    let Item::Fn(caller) = &hir.items[1] else {
        panic!("expected fn caller");
    };
    let tail = caller
        .body
        .as_ref()
        .and_then(|b| b.tail.as_ref())
        .expect("tail");
    let ExprKind::Call { callee, .. } = &tail.kind else {
        panic!("expected a call");
    };
    assert_eq!(
        callee.kind,
        ExprKind::Path {
            res: Res::Symbol(fibonacci_id),
            args: Vec::new()
        }
    );

    // … and the spec attaches to it too.
    let Item::Spec(spec) = &hir.items[2] else {
        panic!("expected the spec");
    };
    assert_eq!(spec.target, Some(fibonacci_id));
    assert_eq!(spec.name, "fibonacci");
}

#[test]
fn spans_map_back_to_the_source() {
    let text = "fn f(in a: Int) -> Int {\n    a + 41\n}\n";
    let (hir, _resolution) = build(&[text]);
    let Item::Fn(function) = &hir.items[0] else {
        panic!("expected a function");
    };
    let tail = function
        .body
        .as_ref()
        .and_then(|b| b.tail.as_ref())
        .expect("tail");
    let range = tail.span.range();
    assert_eq!(
        &text[range.start().as_usize()..range.end().as_usize()],
        "a + 41"
    );
    let ExprKind::Binary { rhs, .. } = &tail.kind else {
        panic!("expected a binary expression");
    };
    let range = rhs.span.range();
    assert_eq!(
        &text[range.start().as_usize()..range.end().as_usize()],
        "41"
    );
}

#[test]
fn imports_and_module_declarations_are_resolved_away() {
    let util = "module util;\npub fn helper() -> Int {\n    7\n}\n";
    let app = "module app;\nimport util::helper;\nfn go() -> Int {\n    helper()\n}\n";
    let (hir, resolution) = build(&[util, app]);
    // Two functions, no import items.
    assert_eq!(hir.items.len(), 2);
    assert!(hir.items.iter().all(|item| matches!(item, Item::Fn(_))));
    let rendered = tuo_hir::render(&hir, &resolution);
    assert!(
        !rendered.contains("import"),
        "imports must not appear:\n{rendered}"
    );
}

#[test]
fn lowering_is_deterministic() {
    let text = concat!(
        "struct Point { x: Int, y: Int }\n",
        "fn f(in p: Point) -> Int {\n    p.x + p.y\n}\n",
    );
    assert_equivalent(&[text], &[text]);
    let (first, resolution) = build(&[text]);
    let (second, _) = build(&[text]);
    assert_eq!(first, second);
    let _ = resolution;
}

#[test]
fn lowering_malformed_input_does_not_panic() {
    // Malformed source: lowering must be total (poison, not panic).
    let text = "fn broken(in a: Int) -> Int {\n    a +\n}\nfn ok() {}\n";
    let mut map = SourceMap::new();
    let file = map.intern_file("broken.tuo");
    let id = map.add_source(file, text).expect("fixture fits");
    let parse = tuo_parser::parse(map.source(id));
    assert!(
        !parse.diagnostics.is_empty(),
        "fixture is deliberately malformed"
    );
    let asts = [Ast::new(&parse.tree, text)];
    let resolution = tuo_resolve::resolve(&asts);
    let hir = tuo_hir::lower(&asts, &resolution);
    let rendered = tuo_hir::render(&hir, &resolution);
    assert!(rendered.contains("fn ok"), "recovery keeps later items");
}

#[test]
fn local_consts_lower_inside_blocks() {
    let (hir, _resolution) = build(&["fn f() -> Int {\n    const LIMIT: Int = 3;\n    LIMIT\n}\n"]);
    let Item::Fn(function) = &hir.items[0] else {
        panic!("expected a function");
    };
    let body = function.body.as_ref().expect("has body");
    assert!(matches!(body.stmts[0].kind, StmtKind::Const(_)));
}
