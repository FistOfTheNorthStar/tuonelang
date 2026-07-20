//! Integration tests for name resolution: multi-file programs are parsed
//! for real (via `tuo-parser`) and resolved, then symbols, references,
//! spec attachments, and diagnostics are asserted.

use tuo_ast::Ast;
use tuo_resolve::{Resolution, SymbolId, SymbolKind, resolve};
use tuo_source::{SourceMap, Span};
use tuo_syntax::SyntaxTree;

/// A parsed multi-file program plus the machinery to resolve and inspect it.
struct Program {
    map: SourceMap,
    files: Vec<(SyntaxTree, String)>,
}

impl Program {
    fn parse(sources: &[&str]) -> Self {
        let mut map = SourceMap::new();
        let files = sources
            .iter()
            .enumerate()
            .map(|(index, text)| {
                let file = map.intern_file(&format!("file{index}.tuo"));
                let id = map.add_source(file, *text).expect("test source fits");
                let result = tuo_parser::parse(map.source(id));
                (result.tree, (*text).to_owned())
            })
            .collect();
        Self { map, files }
    }

    fn resolve(&self) -> Resolution {
        let asts: Vec<Ast<'_>> = self
            .files
            .iter()
            .map(|(tree, text)| Ast::new(tree, text))
            .collect();
        resolve(&asts)
    }

    /// The exact source text a span covers.
    fn slice(&self, span: Span) -> &str {
        let text = self.map.source(span.source()).text();
        &text[span.range().start().as_usize()..span.range().end().as_usize()]
    }
}

fn resolve_one(source: &str) -> Resolution {
    Program::parse(&[source]).resolve()
}

fn codes(resolution: &Resolution) -> Vec<String> {
    resolution
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.code.to_string())
        .collect()
}

fn find(resolution: &Resolution, name: &str, kind: SymbolKind) -> SymbolId {
    resolution
        .symbols()
        .find(|(_, symbol)| symbol.name == name && symbol.kind == kind)
        .map(|(id, _)| id)
        .unwrap_or_else(|| panic!("no {kind:?} symbol named `{name}`"))
}

fn assert_clean(resolution: &Resolution) {
    assert_eq!(
        resolution.diagnostics(),
        &[],
        "expected no resolution diagnostics"
    );
}

// ----------------------------------------------------------------------
// Forward references
// ----------------------------------------------------------------------

#[test]
fn forward_references_work_across_items_and_files() {
    let program = Program::parse(&[
        "module app;\n\
         fn first() -> Int { second() + LATER }\n",
        "module app;\n\
         fn second() -> Int { 1 }\n\
         const LATER: Int = 2;\n\
         fn third() -> Int { first() }\n",
    ]);
    let resolution = program.resolve();
    assert_clean(&resolution);

    let first = find(&resolution, "first", SymbolKind::Function);
    let second = find(&resolution, "second", SymbolKind::Function);
    let later = find(&resolution, "LATER", SymbolKind::Const);
    assert_eq!(resolution.references_to(first).count(), 1);
    assert_eq!(resolution.references_to(second).count(), 1);
    assert_eq!(resolution.references_to(later).count(), 1);
}

#[test]
fn a_struct_field_may_name_a_type_declared_later() {
    let resolution = resolve_one(
        "struct Holder { item: Payload }\n\
         struct Payload { size: Int }\n",
    );
    assert_clean(&resolution);
    let payload = find(&resolution, "Payload", SymbolKind::Struct);
    assert_eq!(resolution.references_to(payload).count(), 1);
}

// ----------------------------------------------------------------------
// Shadowing
// ----------------------------------------------------------------------

#[test]
fn shadowing_resolves_each_use_to_the_nearest_binding() {
    let resolution = resolve_one(
        "fn f(in x: Int) -> Int {\n\
             let doubled = x + x;\n\
             let x = doubled;\n\
             let result = x;\n\
             result\n\
         }\n",
    );
    assert_clean(&resolution);

    let param = find(&resolution, "x", SymbolKind::Param);
    let local_x = find(&resolution, "x", SymbolKind::Local);
    assert_ne!(param, local_x, "the shadowing local is a distinct symbol");
    assert_eq!(
        resolution.references_to(param).count(),
        2,
        "only the uses before the shadowing `let` see the parameter"
    );
    assert_eq!(
        resolution.references_to(local_x).count(),
        1,
        "uses after the shadowing `let` see the local"
    );
}

#[test]
fn a_shadowing_initializer_still_sees_the_outer_binding() {
    let resolution = resolve_one(
        "fn f() -> Int {\n\
             let x = 1;\n\
             let x = x + 1;\n\
             x\n\
         }\n",
    );
    assert_clean(&resolution);
    let (outer, _) = resolution
        .symbols()
        .find(|(_, symbol)| symbol.name == "x" && symbol.kind == SymbolKind::Local)
        .expect("first local `x` exists");
    assert_eq!(
        resolution.references_to(outer).count(),
        1,
        "the second initializer's `x` refers to the first binding"
    );
}

#[test]
fn an_inner_block_shadow_ends_with_its_block() {
    let resolution = resolve_one(
        "fn f() -> Int {\n\
             let x = 1;\n\
             let y = { let x = 2; x };\n\
             x + y\n\
         }\n",
    );
    assert_clean(&resolution);
    let locals: Vec<SymbolId> = resolution
        .symbols()
        .filter(|(_, symbol)| symbol.name == "x")
        .map(|(id, _)| id)
        .collect();
    let [outer, inner] = locals[..] else {
        panic!("expected exactly two `x` bindings");
    };
    assert_eq!(
        resolution.references_to(inner).count(),
        1,
        "the block tail sees the inner `x`"
    );
    assert_eq!(
        resolution.references_to(outer).count(),
        1,
        "after the block, `x` is the outer binding again"
    );
}

// ----------------------------------------------------------------------
// Duplicate names
// ----------------------------------------------------------------------

#[test]
fn duplicate_module_level_names_are_rejected_even_across_kinds() {
    let resolution = resolve_one("fn thing() {}\nstruct thing { x: Int }\n");
    assert_eq!(codes(&resolution), ["R0001"]);
    let diagnostic = &resolution.diagnostics()[0];
    assert!(diagnostic.message.contains("`thing`"));
    assert_eq!(
        diagnostic.secondary_labels.len(),
        1,
        "points back at the first definition"
    );
}

#[test]
fn duplicate_definitions_are_detected_across_files_of_one_module() {
    let program = Program::parse(&["module app;\nfn go() {}\n", "module app;\nfn go() {}\n"]);
    assert_eq!(codes(&program.resolve()), ["R0001"]);
}

#[test]
fn duplicate_parameters_generics_and_variants_are_rejected() {
    let resolution = resolve_one(
        "fn f(in a: Int, in a: Int) {}\n\
         fn g[T, T]() {}\n\
         enum E { One, One }\n",
    );
    assert_eq!(codes(&resolution), ["R0001", "R0001", "R0001"]);
}

#[test]
fn binding_a_name_twice_in_one_pattern_is_rejected() {
    let resolution = resolve_one(
        "struct Point { x: Int, y: Int }\n\
         fn f(in p: Point) -> Int {\n\
             match p {\n\
                 Point { x, x } => x,\n\
                 _ => 0,\n\
             }\n\
         }\n",
    );
    assert_eq!(codes(&resolution), ["R0001"]);
}

#[test]
fn or_pattern_alternatives_may_rebind_the_same_name() {
    let resolution = resolve_one(
        "fn f(in n: Int) -> Int {\n\
             match n {\n\
                 (x | x) => x,\n\
                 _ => 0,\n\
             }\n\
         }\n",
    );
    assert_clean(&resolution);
}

// ----------------------------------------------------------------------
// Undefined names
// ----------------------------------------------------------------------

#[test]
fn undefined_names_are_reported_with_their_exact_span() {
    let program = Program::parse(&["fn f() -> Int { missing() }\n"]);
    let resolution = program.resolve();
    assert_eq!(codes(&resolution), ["R0002"]);
    let span = resolution.diagnostics()[0].primary_span;
    assert_eq!(program.slice(span), "missing");
}

#[test]
fn a_local_is_not_visible_before_its_declaration() {
    let resolution = resolve_one(
        "fn f() -> Int {\n\
             let early = late;\n\
             let late = 1;\n\
             late + early\n\
         }\n",
    );
    assert_eq!(codes(&resolution), ["R0002"]);
}

#[test]
fn builtin_type_names_resolve_without_declarations() {
    let resolution = resolve_one(
        "struct P { x: Int, y: F64, s: String, b: Bool, c: Char }\n\
         fn f(in p: P) -> Bool { true }\n",
    );
    assert_clean(&resolution);
}

// ----------------------------------------------------------------------
// Imports
// ----------------------------------------------------------------------

#[test]
fn imports_bind_items_modules_and_aliases() {
    let program = Program::parse(&[
        "module std::mem;\npub fn swap() {}\n",
        "module std::collections;\npub struct Map { size: Int }\npub struct Set { size: Int }\n",
        "module app;\n\
         import std::mem as memory;\n\
         import std::collections::{Map as Dict, Set};\n\
         fn f(in d: Dict, in s: Set) { memory::swap() }\n",
    ]);
    let resolution = program.resolve();
    assert_clean(&resolution);

    let swap = find(&resolution, "swap", SymbolKind::Function);
    let map = find(&resolution, "Map", SymbolKind::Struct);
    let set = find(&resolution, "Set", SymbolKind::Struct);
    // `swap`: once at the use (the import path names the module, not the fn).
    assert_eq!(resolution.references_to(swap).count(), 1);
    // `Map`: once at the import leaf, once via `Dict` in the signature.
    assert_eq!(resolution.references_to(map).count(), 2);
    assert_eq!(resolution.references_to(set).count(), 2);
}

#[test]
fn unresolved_imports_are_r0003() {
    let program = Program::parse(&[
        "module geometry;\npub fn area() {}\n",
        "module app;\n\
         import nowhere;\n\
         import geometry::missing;\n\
         import geometry::{also_missing};\n",
    ]);
    assert_eq!(codes(&program.resolve()), ["R0003", "R0003", "R0003"]);
}

#[test]
fn imports_are_not_reexported() {
    let program = Program::parse(&[
        "module a;\npub fn f() {}\n",
        "module b;\nimport a::f;\n",
        "module c;\nimport b::f;\n",
    ]);
    let resolution = program.resolve();
    assert_eq!(codes(&resolution), ["R0003"]);
    assert!(
        resolution.diagnostics()[0]
            .notes
            .iter()
            .any(|note| note.contains("not re-exported"))
    );
}

#[test]
fn an_import_colliding_with_a_declaration_is_r0001() {
    let program = Program::parse(&[
        "module util;\npub fn helper() {}\n",
        "module app;\nimport util::helper;\nfn helper() {}\n",
    ]);
    assert_eq!(codes(&program.resolve()), ["R0001"]);
}

// ----------------------------------------------------------------------
// Ambiguous names
// ----------------------------------------------------------------------

#[test]
fn conflicting_imports_make_uses_ambiguous() {
    let program = Program::parse(&[
        "module alpha;\npub fn helper() {}\n",
        "module beta;\npub fn helper() {}\n",
        "module app;\n\
         import alpha::helper;\n\
         import beta::helper;\n\
         fn go() { helper() }\n",
    ]);
    let resolution = program.resolve();
    assert_eq!(codes(&resolution), ["R0004"]);
    let diagnostic = &resolution.diagnostics()[0];
    assert_eq!(
        diagnostic.secondary_labels.len(),
        2,
        "both candidate imports are labelled"
    );
}

#[test]
fn an_alias_disambiguates_conflicting_imports() {
    let program = Program::parse(&[
        "module alpha;\npub fn helper() {}\n",
        "module beta;\npub fn helper() {}\n",
        "module app;\n\
         import alpha::helper;\n\
         import beta::helper as beta_helper;\n\
         fn go() { helper(); beta_helper() }\n",
    ]);
    assert_clean(&program.resolve());
}

// ----------------------------------------------------------------------
// Visibility
// ----------------------------------------------------------------------

#[test]
fn private_items_are_not_visible_outside_their_module() {
    let program = Program::parse(&[
        "module geo;\nfn hidden() {}\npub fn open() {}\n",
        "module app;\n\
         import geo::hidden;\n\
         fn f() { geo::hidden() }\n",
    ]);
    let resolution = program.resolve();
    assert_eq!(codes(&resolution), ["R0005", "R0005"]);
    assert!(resolution.diagnostics()[0].message.contains("private"));
}

#[test]
fn pub_items_and_same_module_private_items_are_visible() {
    let program = Program::parse(&[
        "module geo;\nfn hidden() {}\npub fn open() { hidden() }\n",
        "module app;\nimport geo::open;\nfn f() { open() }\n",
    ]);
    assert_clean(&program.resolve());
}

// ----------------------------------------------------------------------
// Enum variants and patterns
// ----------------------------------------------------------------------

#[test]
fn enum_variant_paths_resolve_to_variant_symbols() {
    let resolution = resolve_one(
        "enum Shape { Circle { radius: Int }, Dot }\n\
         fn f(in s: Shape) -> Int {\n\
             match s {\n\
                 Shape::Circle { radius } => radius,\n\
                 Shape::Dot => 0,\n\
             }\n\
         }\n",
    );
    assert_clean(&resolution);
    let circle = find(&resolution, "Circle", SymbolKind::Variant);
    let shape = find(&resolution, "Shape", SymbolKind::Enum);
    assert_eq!(resolution.references_to(circle).count(), 1);
    // `Shape` appears twice in the signature/patterns paths plus the type.
    assert_eq!(resolution.references_to(shape).count(), 3);
}

#[test]
fn unknown_enum_variants_are_r0002() {
    let resolution = resolve_one(
        "enum Shape { Dot }\n\
         fn f() -> Shape { Shape::Square }\n",
    );
    assert_eq!(codes(&resolution), ["R0002"]);
    assert!(resolution.diagnostics()[0].message.contains("variant"));
}

// ----------------------------------------------------------------------
// Spec attachments
// ----------------------------------------------------------------------

#[test]
fn a_spec_attaches_to_the_same_function_symbol_calls_resolve_to() {
    let program = Program::parse(&["fn fibonacci(in n: Int) -> Int {\n\
             if n < 2 { n } else { fibonacci(n - 1) + fibonacci(n - 2) }\n\
         }\n\
         fn caller() -> Int { fibonacci(10) }\n\
         spec fibonacci {\n\
             assert fibonacci(1) == 1;\n\
         }\n"]);
    let resolution = program.resolve();
    assert_clean(&resolution);

    let fibonacci = find(&resolution, "fibonacci", SymbolKind::Function);
    let spec = find(&resolution, "fibonacci", SymbolKind::Spec);
    assert_eq!(resolution.spec_targets().len(), 1);
    assert_eq!(resolution.spec_targets()[0].spec, spec);
    assert_eq!(
        resolution.spec_targets()[0].target,
        fibonacci,
        "the spec targets the very symbol calls resolve to"
    );
    // 2 recursive calls + 1 caller + the spec's target name + 1 in `assert`.
    assert_eq!(resolution.references_to(fibonacci).count(), 5);
}

#[test]
fn string_named_specs_are_free_standing() {
    let resolution = resolve_one("spec \"holds anyway\" { assert 1 == 1; }\n");
    assert_clean(&resolution);
    assert_eq!(resolution.spec_targets(), &[]);
}

#[test]
fn a_spec_targeting_a_non_function_is_r0006() {
    let resolution = resolve_one("struct widget { x: Int }\nspec widget { assert 1 == 1; }\n");
    assert_eq!(codes(&resolution), ["R0006"]);
    assert!(resolution.diagnostics()[0].message.contains("struct"));
}

#[test]
fn a_spec_with_no_matching_target_is_r0002() {
    let resolution = resolve_one("spec vanished { assert 1 == 1; }\n");
    assert_eq!(codes(&resolution), ["R0002"]);
}

#[test]
fn spec_bodies_resolve_given_and_when_bindings() {
    let resolution = resolve_one(
        "struct Point { x: Int, y: Int }\n\
         fn classify(in p: Point, in n: Int) -> Bool { true }\n\
         spec classify {\n\
             given p: Point = Point { x: 1, y: 2 }, n: Int;\n\
             when let first = classify(p, n);\n\
             then first == classify(p, n);\n\
         }\n",
    );
    assert_clean(&resolution);
    let classify = find(&resolution, "classify", SymbolKind::Function);
    // Target name + the call in `when` + the call in `then`.
    assert_eq!(resolution.references_to(classify).count(), 3);
}

// ----------------------------------------------------------------------
// Rename-relevant references
// ----------------------------------------------------------------------

#[test]
fn rename_spans_cover_declaration_calls_imports_and_spec_targets() {
    let program = Program::parse(&[
        "module math;\n\
         pub fn fibonacci(in n: Int) -> Int { fibonacci(n) }\n",
        "module app;\n\
         import math::fibonacci;\n\
         fn f() -> Int { fibonacci(5) }\n\
         spec fibonacci { assert fibonacci(0) == 0; }\n",
    ]);
    let resolution = program.resolve();
    assert_clean(&resolution);

    let fibonacci = find(&resolution, "fibonacci", SymbolKind::Function);
    let spans = resolution.rename_spans(fibonacci);
    // Declaration + recursive call + import leaf + call + spec target +
    // spec-body call.
    assert_eq!(spans.len(), 6);
    for span in &spans {
        assert_eq!(
            program.slice(*span),
            "fibonacci",
            "every rename site covers exactly the name token"
        );
    }
    assert_eq!(
        resolution.symbol(fibonacci).declaration,
        Some(spans[0]),
        "the declaration leads the rename list"
    );

    // The reference map answers position queries (rename's entry point).
    for span in &spans[1..] {
        assert_eq!(resolution.resolved_at(*span), Some(fibonacci));
    }
}

// ----------------------------------------------------------------------
// Malformed input
// ----------------------------------------------------------------------

#[test]
fn malformed_files_resolve_without_panicking() {
    let root =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/parser/fixtures");
    for name in [
        "err/broken_items.tuo",
        "err/broken_statements.tuo",
        "err/unclosed.tuo",
    ] {
        let text = std::fs::read_to_string(root.join(name)).expect("fixture is readable");
        let program = Program::parse(&[&text]);
        // No panic is the property; diagnostics are free to appear.
        let _ = program.resolve();
    }
}

#[test]
fn resolution_is_deterministic() {
    let sources = [
        "module geo;\npub enum Shape { Dot }\npub fn area(in s: Shape) -> Int { 0 }\n",
        "module app;\nimport geo::{Shape, area};\nfn f() -> Int { area(Shape::Dot) }\n",
    ];
    let first = Program::parse(&sources).resolve();
    let second = Program::parse(&sources).resolve();
    let names = |resolution: &Resolution| {
        resolution
            .symbols()
            .map(|(id, symbol)| (id, symbol.name.clone(), symbol.kind))
            .collect::<Vec<_>>()
    };
    assert_eq!(names(&first), names(&second));
    assert_eq!(first.references(), second.references());
    assert_eq!(codes(&first), codes(&second));
}
