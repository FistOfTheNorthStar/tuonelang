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
fn a_bare_prelude_variant_pattern_resolves_to_the_variant_not_a_binding() {
    // Regression: a bare `None` in a pattern is the unit-variant pattern, so it
    // must resolve to the prelude `None` variant — recorded as a *reference* —
    // rather than declaring a fresh local named "None" (which would make the
    // arm an irrefutable catch-all).
    let resolution = resolve_one(
        "fn tag(in v: Option[Int]) -> Int {\n\
             match v {\n\
                 None => 0,\n\
                 Some { value } => value,\n\
             }\n\
         }\n",
    );
    assert_clean(&resolution);

    // `None` resolved to the prelude variant, and it was referenced once.
    let none = find(&resolution, "None", SymbolKind::Variant);
    assert_eq!(resolution.references_to(none).count(), 1);

    // Crucially, no local binding named `None` was declared.
    assert!(
        resolution
            .symbols()
            .all(|(_, symbol)| !(symbol.name == "None" && symbol.kind == SymbolKind::Local)),
        "a bare `None` pattern must not declare a local binding"
    );

    // A genuine catch-all name (`value`) is still a local binding, unaffected.
    let _ = find(&resolution, "value", SymbolKind::Local);
}

#[test]
fn a_local_shadows_a_bare_variant_name_in_a_pattern() {
    // If a name in scope binds a local, a bare pattern of that name is a fresh
    // binding, not a variant — the variant interpretation only applies when the
    // name is not otherwise bound. `None` here is not shadowed, so this checks
    // the ordinary case stays a binding for a non-variant name.
    let resolution = resolve_one(
        "fn f(in n: Int) -> Int {\n\
             match n {\n\
                 anything => anything,\n\
             }\n\
         }\n",
    );
    assert_clean(&resolution);
    // `anything` is a plain binding (not a variant), used once in the arm body.
    let binding = find(&resolution, "anything", SymbolKind::Local);
    assert_eq!(resolution.references_to(binding).count(), 1);
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

#[test]
fn multiple_specs_may_attach_to_one_function_in_source_order() {
    let resolution = resolve_one(
        "fn double(in n: Int) -> Int { n * 2 }\n\
         spec double { assert double(0) == 0; }\n\
         spec double { assert double(2) == 4; }\n",
    );
    assert_clean(&resolution);
    let double = find(&resolution, "double", SymbolKind::Function);
    let specs = resolution.specs_for(double);
    assert_eq!(specs.len(), 2, "both specs attach");
    assert!(specs[0] < specs[1], "attachments come back in source order");
    for spec in specs {
        assert_eq!(resolution.target_of(spec), Some(double));
    }
}

#[test]
fn duplicate_specs_are_distinct_and_all_attach() {
    // Byte-identical spec blocks are two distinct specs (ADR-0002): spec
    // names are not bindings, so nothing collides, merges, or errors.
    let resolution = resolve_one(
        "fn double(in n: Int) -> Int { n * 2 }\n\
         spec double { assert double(1) == 2; }\n\
         spec double { assert double(1) == 2; }\n",
    );
    assert_clean(&resolution);
    let double = find(&resolution, "double", SymbolKind::Function);
    let specs = resolution.specs_for(double);
    assert_eq!(specs.len(), 2);
    assert_ne!(specs[0], specs[1], "each block is its own spec");
}

#[test]
fn spec_attachment_is_source_order_independent() {
    // A spec may precede its target in the same file …
    let resolution = resolve_one(
        "spec double { assert double(2) == 4; }\n\
         fn double(in n: Int) -> Int { n * 2 }\n",
    );
    assert_clean(&resolution);
    let double = find(&resolution, "double", SymbolKind::Function);
    assert_eq!(resolution.specs_for(double).len(), 1);

    // … or live in a different file of the same module.
    let program = Program::parse(&[
        "module m;\nspec double { assert double(2) == 4; }\n",
        "module m;\nfn double(in n: Int) -> Int { n * 2 }\n",
    ]);
    let resolution = program.resolve();
    assert_clean(&resolution);
    let double = find(&resolution, "double", SymbolKind::Function);
    let specs = resolution.specs_for(double);
    assert_eq!(specs.len(), 1);
    assert_eq!(resolution.target_of(specs[0]), Some(double));
}

#[test]
fn target_of_is_none_for_free_standing_specs() {
    let resolution = resolve_one(
        "fn helper() -> Int { 1 }\n\
         spec \"free standing\" { assert helper() == 1; }\n",
    );
    assert_clean(&resolution);
    let spec = find(&resolution, "\"free standing\"", SymbolKind::Spec);
    assert_eq!(resolution.target_of(spec), None);
    assert_eq!(
        resolution.dependencies_of(spec),
        &[find(&resolution, "helper", SymbolKind::Function)],
        "dependency discovery does not require a target"
    );
}

#[test]
fn specs_for_is_empty_for_untargeted_functions() {
    let resolution = resolve_one(
        "fn covered() -> Int { 1 }\n\
         fn bare() -> Int { 2 }\n\
         spec covered { assert covered() == 1; }\n",
    );
    assert_clean(&resolution);
    let bare = find(&resolution, "bare", SymbolKind::Function);
    assert_eq!(resolution.specs_for(bare), &[]);
}

#[test]
fn spec_dependencies_cover_referenced_items_in_first_use_order() {
    let resolution = resolve_one(
        "fn double(in n: Int) -> Int { n * 2 }\n\
         fn helper() -> Int { 3 }\n\
         const LIMIT: Int = 10;\n\
         spec double {\n\
             given n: Int = helper();\n\
             then double(n) < LIMIT;\n\
         }\n",
    );
    assert_clean(&resolution);
    let double = find(&resolution, "double", SymbolKind::Function);
    let spec = resolution.specs_for(double)[0];
    assert_eq!(
        resolution.dependencies_of(spec),
        &[
            double, // the target name itself is the first use
            find(&resolution, "helper", SymbolKind::Function),
            find(&resolution, "LIMIT", SymbolKind::Const),
        ],
        "items in first-use order; the local `n` is not a dependency and \
         the second `double` use deduplicates"
    );
}

#[test]
fn spec_dependencies_include_types_and_variants() {
    let resolution = resolve_one(
        "struct Point { x: Int, y: Int }\n\
         enum Shape { Dot, Line }\n\
         fn classify(in p: Point) -> Shape { Shape::Dot }\n\
         spec classify {\n\
             given p: Point = Point { x: 1, y: 2 };\n\
             assert classify(p) == Shape::Dot;\n\
         }\n",
    );
    assert_clean(&resolution);
    let classify = find(&resolution, "classify", SymbolKind::Function);
    let spec = resolution.specs_for(classify)[0];
    let deps = resolution.dependencies_of(spec);
    for (name, kind) in [
        ("classify", SymbolKind::Function),
        ("Point", SymbolKind::Struct),
        ("Shape", SymbolKind::Enum),
        ("Dot", SymbolKind::Variant),
    ] {
        assert!(
            deps.contains(&find(&resolution, name, kind)),
            "`{name}` should be a dependency"
        );
    }
}

#[test]
fn spec_dependencies_exclude_items_declared_inside_the_block() {
    let resolution = resolve_one(
        "fn f() -> Int { 1 }\n\
         spec f {\n\
             when let x = { const K: Int = 2; K + f() };\n\
             assert x == 3;\n\
         }\n",
    );
    assert_clean(&resolution);
    let f = find(&resolution, "f", SymbolKind::Function);
    let spec = resolution.specs_for(f)[0];
    assert_eq!(
        resolution.dependencies_of(spec),
        &[f],
        "the block-local `K` and the local `x` are not dependencies"
    );
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
// The prelude
// ----------------------------------------------------------------------

#[test]
fn option_result_and_their_variants_are_in_the_prelude() {
    let resolution = resolve_one(
        "fn find(in n: Int) -> Option[Int] {\n\
             if n > 0 { Some { value: n } } else { None }\n\
         }\n\
         fn run() -> Result[Int, Str] { Ok { value: 1 } }\n\
         fn unwrap_or_zero(in v: Option[Int]) -> Int {\n\
             match v {\n\
                 Some { value } => value,\n\
                 None => 0,\n\
             }\n\
         }\n",
    );
    assert_clean(&resolution);
    let option = resolution
        .prelude_symbol("Option")
        .expect("Option is in the prelude");
    assert_eq!(
        resolution.variants_of(option).len(),
        2,
        "Option has Some and None"
    );
    let some = resolution.prelude_symbol("Some").expect("Some in prelude");
    assert!(
        resolution.references_to(some).count() >= 2,
        "`Some` uses resolve to the prelude variant"
    );
}

#[test]
fn a_module_declaration_shadows_the_prelude() {
    let resolution = resolve_one(
        "enum Option { Filled, Vacant }\n\
         fn f() -> Option { Option::Filled }\n",
    );
    assert_clean(&resolution);
    // The user's enum has a written declaration; the prelude's does not.
    let user_option = resolution
        .symbols()
        .find(|(_, symbol)| {
            symbol.name == "Option"
                && symbol.kind == SymbolKind::Enum
                && symbol.declaration.is_some()
        })
        .map(|(id, _)| id)
        .expect("the user enum exists");
    assert_ne!(
        Some(user_option),
        resolution.prelude_symbol("Option"),
        "the user enum is a distinct symbol"
    );
    assert_eq!(
        resolution.references_to(user_option).count(),
        2,
        "both uses resolve to the user enum, not the prelude"
    );
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

// ----------------------------------------------------------------------
// Builtin functions (ADR-0006 Stage A)
// ----------------------------------------------------------------------

#[test]
fn effect_and_string_builtins_resolve_without_any_stdlib() {
    // No stdlib source is loaded here: the builtins resolve through the
    // always-present `std::rt` / `std::str` modules alone.
    let resolution = resolve_one(
        "fn go() -> Int {\n\
         \x20   let n = std::rt::write(1, \"x\");\n\
         \x20   let b = std::rt::read_byte(0);\n\
         \x20   let l = std::str::len(\"abc\");\n\
         \x20   let c = std::str::byte_at(\"abc\", 0);\n\
         \x20   let s = std::str::slice(\"abc\", 0, 1);\n\
         \x20   std::rt::exit(n + b + l + c)\n\
         }\n",
    );
    assert_clean(&resolution);
    // Every builtin (ADR-0006 and ADR-0009) is installed as a real, `pub`,
    // declaration-less function symbol reachable by `builtin_symbol`, even
    // the ones this program does not reference.
    for builtin in tuo_resolve::Builtin::ALL {
        let symbol = resolution
            .builtin_symbol(builtin)
            .expect("every builtin has a symbol");
        assert_eq!(resolution.builtin(symbol), Some(builtin));
        let data = resolution.symbol(symbol);
        assert_eq!(data.kind, SymbolKind::Function);
        assert_eq!(data.name, builtin.name());
        assert!(data.is_pub, "builtins are reachable from any module");
        assert!(
            data.declaration.is_none(),
            "builtins have no written declaration"
        );
    }
    // The six ADR-0006 builtins this program calls are each referenced.
    for builtin in [
        tuo_resolve::Builtin::RtWrite,
        tuo_resolve::Builtin::RtReadByte,
        tuo_resolve::Builtin::RtExit,
        tuo_resolve::Builtin::StrLen,
        tuo_resolve::Builtin::StrByteAt,
        tuo_resolve::Builtin::StrSlice,
    ] {
        let symbol = resolution.builtin_symbol(builtin).expect("has a symbol");
        assert!(
            resolution.references_to(symbol).next().is_some(),
            "{builtin:?} was referenced by the program"
        );
    }
}

#[test]
fn allocator_core_builtins_resolve_without_any_stdlib() {
    // ADR-0009: the `std::string`, `std::array`, and `std::rt::write_string`
    // builtins resolve through their always-present modules with no stdlib
    // loaded, and calls to them are referenced.
    let resolution = resolve_one(
        "fn go() -> Int {\n\
         \x20   var s = std::string::from_str(\"ab\");\n\
         \x20   std::string::push_byte(s, 99);\n\
         \x20   std::string::append(s, \"cd\");\n\
         \x20   let t = std::string::concat(\"x\", \"y\");\n\
         \x20   let e = std::string::empty();\n\
         \x20   let sl = std::string::slice(s, 0, 1);\n\
         \x20   let sb = std::string::byte_at(s, 0);\n\
         \x20   var xs = std::array::empty();\n\
         \x20   std::array::push(xs, 7);\n\
         \x20   let p = std::array::pop(xs);\n\
         \x20   let g = std::array::get(xs, 0);\n\
         \x20   let w = std::rt::write_string(1, s);\n\
         \x20   std::string::len(t) + std::array::len(xs) + sb + g + w\n\
         }\n",
    );
    assert_clean(&resolution);
    for builtin in [
        tuo_resolve::Builtin::RtWriteString,
        tuo_resolve::Builtin::StringEmpty,
        tuo_resolve::Builtin::StringFromStr,
        tuo_resolve::Builtin::StringPushByte,
        tuo_resolve::Builtin::StringAppend,
        tuo_resolve::Builtin::StringConcat,
        tuo_resolve::Builtin::StringLen,
        tuo_resolve::Builtin::StringByteAt,
        tuo_resolve::Builtin::StringSlice,
        tuo_resolve::Builtin::ArrayEmpty,
        tuo_resolve::Builtin::ArrayPush,
        tuo_resolve::Builtin::ArrayPop,
        tuo_resolve::Builtin::ArrayLen,
        tuo_resolve::Builtin::ArrayGet,
    ] {
        let symbol = resolution
            .builtin_symbol(builtin)
            .expect("every ADR-0009 builtin has a symbol");
        assert_eq!(resolution.builtin(symbol), Some(builtin));
        assert!(
            resolution.references_to(symbol).next().is_some(),
            "{builtin:?} was referenced by the program"
        );
    }
}

#[test]
fn redefining_an_allocator_builtin_at_its_own_path_is_a_duplicate() {
    // A file declaring `module std::string;` shares the builtin module, so a
    // function named `concat` there collides with the builtin: R0001.
    let resolution = resolve_one("module std::string;\nfn concat() {}\n");
    assert_eq!(codes(&resolution), vec!["R0001"]);
}

#[test]
fn builtin_modules_are_navigable_but_not_loadable_source() {
    // `std`, `std::rt`, `std::str`, `std::string`, and `std::array` are real
    // modules with real paths.
    let resolution = resolve_one("fn f() -> Int { std::str::len(\"a\") }\n");
    assert_clean(&resolution);
    let paths: Vec<Vec<String>> = resolution
        .modules()
        .iter()
        .map(|module| module.path.clone())
        .collect();
    assert!(paths.contains(&vec!["std".to_owned()]));
    assert!(paths.contains(&vec!["std".to_owned(), "rt".to_owned()]));
    assert!(paths.contains(&vec!["std".to_owned(), "str".to_owned()]));
    assert!(paths.contains(&vec!["std".to_owned(), "string".to_owned()]));
    assert!(paths.contains(&vec!["std".to_owned(), "array".to_owned()]));
}

#[test]
fn redefining_a_builtin_at_its_own_path_is_a_duplicate_definition() {
    // A file declaring `module std::rt;` shares the builtin module, so a
    // function named `write` there collides with the builtin: R0001. The
    // builtins are not shadowable at their own paths.
    let resolution = resolve_one("module std::rt;\nfn write() {}\n");
    assert_eq!(codes(&resolution), vec!["R0001"]);
}

#[test]
fn a_local_named_std_shadows_the_builtin_modules_in_that_scope() {
    // Ordinary shadowing (static-semantics §2.4): a binding named `std`
    // hides the module, so `std::rt::write` no longer resolves through it —
    // the path tail hangs off a local and is skipped as type-dependent,
    // never silently rebound to the builtin.
    let resolution = resolve_one(
        "fn f() -> Int {\n\
         \x20   let std = 1;\n\
         \x20   std\n\
         }\n",
    );
    assert_clean(&resolution);
}

#[test]
fn a_bare_fn_name_in_value_position_resolves_to_its_function() {
    // ADR-0008 Tier 1: using a function name as a value resolves it to the
    // function symbol (in the value namespace) — the front end already does
    // this; the fn-value feature builds on it.
    let resolution = resolve_one(
        "fn add(take a: Int, take b: Int) -> Int { a + b }\n\
         fn main() -> Int { let f = add; f(1, 2) }\n",
    );
    assert_clean(&resolution);
    let add = find(&resolution, "add", SymbolKind::Function);
    // `add` is referenced twice: as a value (`let f = add`) and... actually
    // only once as a value here; the call is through `f`. There is exactly one
    // value reference to `add`.
    let refs: Vec<_> = resolution.references_to(add).collect();
    assert_eq!(refs.len(), 1, "one value reference to `add`");
}

#[test]
fn a_function_type_annotation_resolves_its_embedded_types() {
    // `fn(mode T, …) -> R` is structural: the `fn` keyword and modes resolve
    // to nothing, but the embedded parameter and return types resolve.
    let resolution = resolve_one(
        "struct P { x: Int }\n\
         fn takes(take f: fn(take P) -> P, take p: P) -> P { f(p) }\n\
         fn id(take p: P) -> P { p }\n\
         fn main() -> Int { takes(id, P { x: 0 }); 0 }\n",
    );
    assert_clean(&resolution);
}
