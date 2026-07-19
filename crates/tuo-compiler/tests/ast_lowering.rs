//! AST lowering tests: parse source through the facade, view the CST through
//! the typed AST accessors, and verify the views expose the right structure —
//! including error islands on malformed input. The CST/AST layer separation
//! is exercised throughout: every check goes CST → typed view.

use tuo_compiler::ast::{
    self, Ast, ElseBranch, Expr, Item, Pattern, SpecStatement, Statement, TypeRef,
};
use tuo_compiler::parser::{ParseResult, parse};
use tuo_compiler::source::SourceMap;

fn parsed(text: &str) -> (ParseResult, String) {
    let mut map = SourceMap::new();
    let file = map.intern_file("lowering.tuo");
    let source = map.add_source(file, text).expect("test source fits");
    (parse(map.source(source)), text.to_owned())
}

/// Run `check` on the typed AST of `text`.
fn with_ast(text: &str, check: impl FnOnce(Ast<'_>)) {
    let (result, owned) = parsed(text);
    check(Ast::new(&result.tree, &owned));
}

#[test]
fn functions_lower_with_names_params_and_return_types() {
    let text = "/// Doc line.\npub fn scale[T: Ord + Clone](in self, mut factor: F64, take rest: Box[T]) -> List[T] where T: Display {\n    factor\n}\n";
    with_ast(text, |ast| {
        let items: Vec<Item<'_>> = ast.file().items().collect();
        assert_eq!(items.len(), 1);
        let Item::Fn(func) = items[0] else {
            panic!("expected a function item");
        };
        assert_eq!(func.name(), Some("scale"));
        assert!(func.is_pub());
        assert!(!func.is_signature());
        assert_eq!(func.docs().collect::<Vec<_>>(), ["/// Doc line."]);

        let generics = func.generics().expect("has generics");
        let params: Vec<_> = generics.params().collect();
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].name(), Some("T"));
        let bounds: Vec<String> = params[0].bounds().map(|b| b.text().to_owned()).collect();
        assert_eq!(bounds, ["Ord", "Clone"]);

        let params: Vec<_> = func.params().collect();
        assert_eq!(params.len(), 3);
        assert!(params[0].is_receiver());
        assert_eq!(params[0].mode(), Some("in"));
        assert_eq!(params[0].name(), Some("self"));
        assert_eq!(params[1].mode(), Some("mut"));
        assert_eq!(params[1].name(), Some("factor"));
        assert_eq!(params[1].ty().map(TypeRef::text), Some("F64"));
        assert_eq!(params[2].mode(), Some("take"));
        assert_eq!(params[2].ty().map(TypeRef::text), Some("Box[T]"));

        assert_eq!(func.return_type().map(TypeRef::text), Some("List[T]"));
        assert!(func.where_clause().is_some());

        let body = func.body().expect("has body");
        assert_eq!(body.statements().count(), 0);
        assert!(matches!(body.tail(), Some(Expr::Path(_))));
    });
}

#[test]
fn structs_and_enums_lower_with_fields_and_variants() {
    let text = "pub struct Point[T] {\n    /// The x coordinate.\n    pub x: T,\n    y: T,\n}\n\nenum Shape {\n    Circle { radius: F64 },\n    Empty,\n}\n";
    with_ast(text, |ast| {
        let items: Vec<Item<'_>> = ast.file().items().collect();
        let Item::Struct(point) = items[0] else {
            panic!("expected a struct");
        };
        assert_eq!(point.name(), Some("Point"));
        assert!(point.is_pub());
        let fields: Vec<_> = point.fields().collect();
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].name(), Some("x"));
        assert!(fields[0].is_pub());
        assert_eq!(fields[0].docs().count(), 1);
        assert_eq!(fields[0].ty().map(TypeRef::text), Some("T"));
        assert!(!fields[1].is_pub());

        let Item::Enum(shape) = items[1] else {
            panic!("expected an enum");
        };
        assert_eq!(shape.name(), Some("Shape"));
        let variants: Vec<_> = shape.variants().collect();
        assert_eq!(variants.len(), 2);
        assert_eq!(variants[0].name(), Some("Circle"));
        assert_eq!(variants[0].fields().count(), 1);
        assert_eq!(variants[1].name(), Some("Empty"));
        assert_eq!(variants[1].fields().count(), 0);
    });
}

#[test]
fn interfaces_and_impls_lower_with_members() {
    let text = "interface Area {\n    fn area(in self) -> F64;\n    fn doubled(in self) -> F64 {\n        self.area() * 2.0\n    }\n}\n\nimpl Area for Shape {\n    fn area(in self) -> F64 {\n        0.0\n    }\n}\n";
    with_ast(text, |ast| {
        let items: Vec<Item<'_>> = ast.file().items().collect();
        let Item::Interface(interface) = items[0] else {
            panic!("expected an interface");
        };
        assert_eq!(interface.name(), Some("Area"));
        let members: Vec<_> = interface.members().collect();
        assert_eq!(members.len(), 2);
        assert!(members[0].is_signature());
        assert!(members[0].body().is_none());
        assert!(!members[1].is_signature());
        assert!(members[1].body().is_some());

        let Item::Impl(implementation) = items[1] else {
            panic!("expected an impl");
        };
        assert_eq!(implementation.interface().map(|p| p.text()), Some("Area"));
        assert_eq!(implementation.target().map(TypeRef::text), Some("Shape"));
        assert_eq!(implementation.functions().count(), 1);
    });
}

#[test]
fn imports_module_and_consts_lower() {
    let text = "module app::core;\n\nimport std::collections::{Map as Dict, Set};\nimport std::fmt as formatting;\n\nconst LIMIT: Int = 64 * 2;\n";
    with_ast(text, |ast| {
        let file = ast.file();
        let module = file.module().expect("has module decl");
        let segments: Vec<&str> = module.path().expect("has path").segments().collect();
        assert_eq!(segments, ["app", "core"]);

        let items: Vec<Item<'_>> = file.items().collect();
        let Item::Import(grouped) = items[0] else {
            panic!("expected an import");
        };
        let path: Vec<&str> = grouped.path().expect("has path").segments().collect();
        assert_eq!(path, ["std", "collections"]);
        let leaves: Vec<(Option<&str>, Option<&str>)> =
            grouped.leaves().map(|l| (l.name(), l.alias())).collect();
        assert_eq!(leaves, [(Some("Map"), Some("Dict")), (Some("Set"), None)]);

        let Item::Import(aliased) = items[1] else {
            panic!("expected an import");
        };
        assert_eq!(aliased.alias(), Some("formatting"));

        let Item::Const(limit) = items[2] else {
            panic!("expected a const");
        };
        assert_eq!(limit.name(), Some("LIMIT"));
        assert_eq!(limit.ty().map(TypeRef::text), Some("Int"));
        assert!(matches!(limit.value(), Some(Expr::Binary(_))));
    });
}

#[test]
fn statements_and_expressions_lower_structurally() {
    let text = "fn f() -> Int {\n    let total: Int = 1 + 2 * 3;\n    var kept = registry::get::[Int](total)?.value;\n    ;\n    kept = kept + 1;\n    if kept > 0 {\n        kept\n    } else if total > 0 {\n        total\n    } else {\n        0\n    }\n}\n";
    with_ast(text, |ast| {
        let items: Vec<Item<'_>> = ast.file().items().collect();
        let Item::Fn(func) = items[0] else {
            panic!("expected a function");
        };
        let body = func.body().expect("has body");
        let statements: Vec<Statement<'_>> = body.statements().collect();
        // Block-form expressions standing alone are statements per the
        // grammar (never tail expressions), so the trailing if-chain is the
        // fifth statement and the block has no tail.
        assert_eq!(statements.len(), 5);

        let Statement::Let(let_stmt) = statements[0] else {
            panic!("expected let");
        };
        assert!(!let_stmt.is_var());
        assert!(matches!(let_stmt.pattern(), Some(Pattern::Binding(_))));
        assert_eq!(let_stmt.ty().map(TypeRef::text), Some("Int"));
        // `1 + 2 * 3` — precedence surfaces through the views.
        let Some(Expr::Binary(add)) = let_stmt.initializer() else {
            panic!("expected binary initializer");
        };
        assert_eq!(add.op(), Some("+"));
        assert!(matches!(add.rhs(), Some(Expr::Binary(_))));

        let Statement::Var(var_stmt) = statements[1] else {
            panic!("expected var");
        };
        // `registry::get::[Int](total)?.value` — postfix chain.
        let Some(Expr::Field(field)) = var_stmt.initializer() else {
            panic!("expected field access");
        };
        assert_eq!(field.name(), Some("value"));
        let Some(Expr::Try(try_expr)) = field.receiver() else {
            panic!("expected try");
        };
        let Some(Expr::Call(call)) = try_expr.inner() else {
            panic!("expected call");
        };
        let Some(Expr::Path(path)) = call.callee() else {
            panic!("expected path callee");
        };
        assert_eq!(path.segments().collect::<Vec<_>>(), ["registry", "get"]);
        assert!(path.turbofish().is_some());
        assert_eq!(call.args().count(), 1);

        assert!(matches!(statements[2], Statement::Empty(_)));

        let Statement::Expr(assign_stmt) = statements[3] else {
            panic!("expected expression statement");
        };
        assert!(matches!(assign_stmt.expr(), Some(Expr::Assign(_))));

        // The trailing if/else-if/else chain (a block-form statement).
        assert!(body.tail().is_none());
        let Statement::Expr(if_stmt) = statements[4] else {
            panic!("expected if statement");
        };
        let Some(Expr::If(if_expr)) = if_stmt.expr() else {
            panic!("expected if expr");
        };
        assert!(matches!(if_expr.condition(), Some(Expr::Binary(_))));
        assert!(if_expr.then_block().is_some());
        let Some(ElseBranch::If(nested)) = if_expr.else_branch() else {
            panic!("expected else-if");
        };
        assert!(matches!(nested.else_branch(), Some(ElseBranch::Block(_))));
    });
}

#[test]
fn match_loops_and_patterns_lower() {
    let text = "fn f(in xs: List[Int]) -> Int {\n    for x in xs {\n        continue;\n    }\n    'outer: loop {\n        break 'outer 1;\n    }\n    match point {\n        Point { x, .. } => x,\n        1 | 2 => 0,\n        _ => 9,\n    }\n}\n";
    with_ast(text, |ast| {
        let Item::Fn(func) = ast.file().items().next().expect("one item") else {
            panic!("expected a function");
        };
        let body = func.body().expect("has body");
        let statements: Vec<Statement<'_>> = body.statements().collect();

        let Statement::Expr(for_stmt) = statements[0] else {
            panic!("expected for statement");
        };
        let Some(Expr::For(for_expr)) = for_stmt.expr() else {
            panic!("expected for expr");
        };
        assert_eq!(for_expr.pattern().map(Pattern::text), Some("x"));
        assert!(matches!(for_expr.iterable(), Some(Expr::Path(_))));

        let Statement::Expr(loop_stmt) = statements[1] else {
            panic!("expected loop statement");
        };
        let Some(Expr::Loop(loop_expr)) = loop_stmt.expr() else {
            panic!("expected loop expr");
        };
        assert_eq!(loop_expr.label(), Some("'outer"));
        let loop_body = loop_expr.body().expect("has body");
        let Statement::Expr(break_stmt) = loop_body.statements().next().expect("one stmt") else {
            panic!("expected break statement");
        };
        let Some(Expr::Break(break_expr)) = break_stmt.expr() else {
            panic!("expected break");
        };
        assert_eq!(break_expr.label(), Some("'outer"));
        assert!(break_expr.value().is_some());

        // Block-form `match` standing alone is a statement, not a tail.
        let Statement::Expr(match_stmt) = statements[2] else {
            panic!("expected match statement");
        };
        let Some(Expr::Match(match_expr)) = match_stmt.expr() else {
            panic!("expected match expr");
        };
        let arms: Vec<_> = match_expr.arms().collect();
        assert_eq!(arms.len(), 3);
        let Some(Pattern::Path(path_pat)) = arms[0].pattern() else {
            panic!("expected path pattern");
        };
        assert!(path_pat.has_rest());
        assert_eq!(path_pat.fields().count(), 1);
        assert!(matches!(arms[1].pattern(), Some(Pattern::Or(_))));
        assert!(matches!(arms[2].pattern(), Some(Pattern::Wildcard(_))));
    });
}

#[test]
fn specs_lower_with_clauses() {
    let text = "spec area_is_positive {\n    given radius: F64 = 1.5, name: Str;\n    when let shape = Circle { radius };\n    then shape.area() > 0.0;\n    assert radius > 0.0;\n}\n";
    with_ast(text, |ast| {
        let Item::Spec(spec) = ast.file().items().next().expect("one item") else {
            panic!("expected a spec");
        };
        assert_eq!(spec.name(), Some("area_is_positive"));
        let statements: Vec<SpecStatement<'_>> = spec.statements().collect();
        assert_eq!(statements.len(), 4);

        let SpecStatement::Given(given) = statements[0] else {
            panic!("expected given");
        };
        let bindings: Vec<_> = given.bindings().collect();
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[0].name(), Some("radius"));
        assert_eq!(bindings[0].ty().map(TypeRef::text), Some("F64"));
        assert!(bindings[0].initializer().is_some());
        assert!(bindings[1].initializer().is_none());

        let SpecStatement::When(when) = statements[1] else {
            panic!("expected when");
        };
        let binding = when.binding().expect("when-let form");
        assert!(matches!(
            binding.initializer(),
            Some(Expr::StructLiteral(_))
        ));

        assert!(matches!(statements[2], SpecStatement::Then(_)));
        assert!(matches!(statements[3], SpecStatement::Assert(_)));
    });
}

#[test]
fn malformed_input_surfaces_error_islands_not_panics() {
    let text = "fn ok() -> Int { 1 }\n\nfn broken( {\n\nstruct Point { x: F64 }\n\nfn also_ok() -> Int {\n    let = bad;\n    2\n}\n";
    with_ast(text, |ast| {
        let items: Vec<Item<'_>> = ast.file().items().collect();
        // ok fn, error island, struct, also_ok fn — in source order.
        assert_eq!(items.len(), 4);
        assert!(matches!(items[0], Item::Fn(_)));
        let Item::Error(island) = items[1] else {
            panic!("expected an error island");
        };
        assert!(island.text().starts_with("fn broken"));
        assert!(island.span().is_some());
        assert!(matches!(items[2], Item::Struct(_)));

        let Item::Fn(also_ok) = items[3] else {
            panic!("expected a function");
        };
        let body = also_ok.body().expect("has body");
        let statements: Vec<Statement<'_>> = body.statements().collect();
        assert_eq!(statements.len(), 1);
        let Statement::Error(error) = statements[0] else {
            panic!("expected error statement");
        };
        assert_eq!(error.text(), "let = bad;");
        assert!(matches!(body.tail(), Some(Expr::Literal(_))));
    });
}

#[test]
fn views_never_panic_on_arbitrary_soup() {
    // Walk the whole typed AST (via the renderer, which touches every
    // accessor) over random fragment soup.
    let fragments = [
        "fn f",
        "() {",
        "}",
        "let x",
        "= 1;",
        "struct S {",
        "x: Int,",
        "spec s {",
        "assert ",
        "match e {",
        "=> 2,",
        "if a",
        "else",
        "loop",
        "'l:",
        "break",
        "|",
        "..",
        "::",
        "->",
        "impl",
        "for",
        "()",
        "pub",
        "0x2",
        ";",
    ];
    let mut seed = 0xDEAD_BEEF_u64;
    for _ in 0..300 {
        let mut text = String::new();
        for _ in 0..(seed % 14) {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            text.push_str(fragments[(seed as usize) % fragments.len()]);
        }
        let (result, owned) = parsed(&text);
        let _ = ast::render(Ast::new(&result.tree, &owned));
    }
}
