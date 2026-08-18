//! The ADR-0006 Stage A effect discipline: builtin signatures, the
//! transitive purity computation (`TypeckResult::is_effectful`), and the
//! spec-purity gate (`R0007`).

use tuo_ast::Ast;
use tuo_resolve::{Builtin, Resolution, SymbolId, SymbolKind};
use tuo_types::TypeckResult;

fn check(source: &str) -> (Resolution, TypeckResult) {
    let mut map = tuo_source::SourceMap::new();
    let file = map.intern_file("test.tuo");
    let id = map.add_source(file, source).expect("test source fits");
    let parse = tuo_parser::parse(map.source(id));
    assert_eq!(parse.diagnostics, vec![], "test programs must parse");
    let asts = [Ast::new(&parse.tree, source)];
    let resolution = tuo_resolve::resolve(&asts);
    assert_eq!(
        resolution.diagnostics(),
        &[],
        "test programs must resolve cleanly"
    );
    let types = tuo_types::check(&asts, &resolution);
    (resolution, types)
}

fn function(resolution: &Resolution, name: &str) -> SymbolId {
    resolution
        .symbols()
        .find(|(id, symbol)| {
            symbol.name == name
                && symbol.kind == SymbolKind::Function
                && resolution.builtin(*id).is_none()
        })
        .map(|(id, _)| id)
        .unwrap_or_else(|| panic!("no declared function named `{name}`"))
}

#[test]
fn builtin_calls_type_check_like_ordinary_calls() {
    let (_, types) = check(
        "fn f() -> Int { std::rt::write(1, \"x\") }\n\
         fn g(in s: Str) -> Str { std::str::slice(s, 0, 1) }\n",
    );
    assert_eq!(
        types.diagnostics(),
        &[],
        "well-typed builtin calls are clean"
    );
}

#[test]
fn effect_builtins_are_effectful_and_string_builtins_are_pure() {
    let (resolution, types) = check("fn main() -> Int { 0 }\n");
    for builtin in Builtin::ALL {
        let symbol = resolution.builtin_symbol(builtin).expect("has a symbol");
        assert_eq!(
            types.is_effectful(symbol),
            builtin.is_effect(),
            "{} purity",
            builtin.qualified_name()
        );
    }
}

#[test]
fn effectfulness_propagates_transitively_and_through_cycles() {
    let (resolution, types) = check(
        "fn leaf(take fd: Int) -> Int { std::rt::write(fd, \"x\") }\n\
         fn mid() -> Int { leaf(1) }\n\
         fn top() -> Int { mid() }\n\
         fn pure_top() -> Int { pure_leaf(\"abc\") }\n\
         fn pure_leaf(in s: Str) -> Int { std::str::len(s) }\n\
         fn ping(take n: Int) -> Int { if n == 0 { std::rt::exit(0) } else { pong(n - 1) } }\n\
         fn pong(take n: Int) -> Int { ping(n) }\n",
    );
    for name in ["leaf", "mid", "top", "ping", "pong"] {
        assert!(
            types.is_effectful(function(&resolution, name)),
            "`{name}` reaches an effect"
        );
    }
    for name in ["pure_top", "pure_leaf"] {
        assert!(
            !types.is_effectful(function(&resolution, name)),
            "`{name}` is pure (std::str is not an effect)"
        );
    }
}

#[test]
fn referencing_an_effectful_function_as_a_value_taints_conservatively() {
    let (resolution, types) = check(
        "fn emit() -> Int { std::rt::write(1, \"x\") }\n\
         fn names_it() -> Int { let f = emit; 0 }\n",
    );
    assert!(types.is_effectful(function(&resolution, "names_it")));
}

#[test]
fn a_spec_reaching_an_effect_is_r0007_and_a_pure_spec_is_not() {
    let (_, types) = check(
        "fn emit() -> Int { std::rt::write(1, \"x\") }\n\
         fn measure(in s: Str) -> Int { std::str::len(s) }\n\
         spec emit { then emit() == 1; }\n\
         spec measure { then measure(\"abc\") == 3; }\n",
    );
    let codes: Vec<String> = types
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.code.to_string())
        .collect();
    assert_eq!(
        codes,
        vec!["R0007"],
        "exactly the effectful spec is refused"
    );
    let message = &types.diagnostics()[0].message;
    assert!(
        message.contains("spec `emit`") && message.contains("effectful function `emit`"),
        "the diagnostic names the spec and the function: {message}"
    );
    assert!(
        message.contains("pure core"),
        "the diagnostic explains the sandbox rule: {message}"
    );
}

#[test]
fn a_spec_reaching_an_effect_directly_names_the_builtin() {
    let (_, types) = check("spec \"direct\" { then std::rt::read_byte(0) == -1; }\n");
    let codes: Vec<String> = types
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.code.to_string())
        .collect();
    assert_eq!(codes, vec!["R0007"]);
    assert!(
        types.diagnostics()[0]
            .message
            .contains("`std::rt::read_byte`"),
        "a direct effect use names the qualified builtin: {}",
        types.diagnostics()[0].message
    );
}

#[test]
fn main_may_be_effectful_without_any_diagnostic() {
    let (resolution, types) = check("fn main() -> Int { std::rt::exit(0) }\n");
    assert_eq!(types.diagnostics(), &[]);
    assert!(types.is_effectful(function(&resolution, "main")));
    assert_eq!(
        types.effectful_functions().count(),
        4,
        "the three effect builtins plus `main`"
    );
}
