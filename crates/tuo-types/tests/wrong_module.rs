//! The wrong-module fix: tuonelang is free functions only, so an operation
//! other languages spell `xs.len()` is written `std::array::len(xs)` — and
//! the same short name is owned by four builtin modules (`len` by
//! `std::array`, `std::map`, `std::str`, `std::string`). Choosing the wrong
//! one is a type error whose real cause is the *module*, not the argument.
//!
//! The compiler already knows the argument's type and every sibling's
//! signature, so it can name the module that would have worked. These tests
//! pin that it does — and, just as importantly, that it stays quiet when it
//! has nothing true to say: a suggestion pointing somewhere equally wrong is
//! worse than none, because it costs a round trip to discover.

use tuo_ast::Ast;
use tuo_source::SourceMap;

/// Type-check one program and return its rendered diagnostics (`code`, plus
/// every help line).
fn check(source: &str) -> Vec<(String, Vec<String>)> {
    let mut map = SourceMap::new();
    let file = map.intern_file("wrong_module.tuo");
    let id = map.add_source(file, source).expect("test source fits");
    let parse = tuo_parser::parse(map.source(id));
    let asts = [Ast::new(&parse.tree, source)];
    let resolution = tuo_resolve::resolve(&asts);
    let types = tuo_types::check(&asts, &resolution);
    resolution
        .diagnostics()
        .iter()
        .chain(types.diagnostics())
        .map(|diagnostic| {
            (
                diagnostic.code.to_string(),
                diagnostic.help.iter().cloned().collect(),
            )
        })
        .collect()
}

/// The help text of the first diagnostic with `code`, or `None`.
fn help_for(source: &str, code: &str) -> Option<String> {
    check(source)
        .into_iter()
        .find(|(found, _)| found == code)
        .map(|(_, help)| help.join(" "))
}

#[test]
fn a_str_passed_to_the_string_module_names_the_str_module() {
    let help = help_for(
        "fn main() -> Int {\n    let s: Str = \"hi\";\n    std::string::len(s)\n}\n",
        "T0001",
    )
    .expect("the mismatch is reported");
    assert!(
        help.contains("std::str::len"),
        "the hint must name the module that accepts `Str`, got: {help}"
    );
}

#[test]
fn an_array_passed_to_the_map_module_names_the_array_module() {
    let help = help_for(
        "fn main() -> Int {\n\
         \x20   let xs: Array[Int] = std::array::empty();\n\
         \x20   std::map::len(xs)\n\
         }\n",
        "T0001",
    )
    .expect("the mismatch is reported");
    assert!(
        help.contains("std::array::len"),
        "the hint must name the module that accepts `Array`, got: {help}"
    );
}

/// The honesty rule. An ordinary mismatch on a user-defined function has no
/// "other module" to point at, and inventing one would be noise on every
/// type error in the language.
#[test]
fn an_ordinary_mismatch_suggests_no_module() {
    let (_, help) = check(
        "fn takes_int(take n: Int) -> Int {\n    n\n}\n\n\
         fn main() -> Int {\n\
         \x20   let s: Str = \"hi\";\n\
         \x20   takes_int(s)\n\
         }\n",
    )
    .into_iter()
    .find(|(code, _)| code == "T0001")
    .expect("the mismatch is reported");
    assert!(
        help.iter().all(|line| !line.contains("different module")),
        "a user-function mismatch must not blame a module: {help:?}"
    );
}

/// A bare unqualified name is not an invented symbol — the caller picked the
/// right name and omitted the module. `R0002` should say which modules
/// define it rather than only that it was not found.
#[test]
fn a_bare_ambiguous_name_lists_every_owning_module() {
    let help = help_for(
        "fn main() -> Int {\n    let s: Str = \"hi\";\n    len(s)\n}\n",
        "R0002",
    )
    .expect("the undefined name is reported");
    for module in ["std::str::len", "std::string::len", "std::array::len"] {
        assert!(
            help.contains(module),
            "`{module}` must be listed among the candidates, got: {help}"
        );
    }
}

/// When exactly one module owns the name there is no ambiguity to resolve,
/// so the fix is machine-applicable rather than a list to choose from.
#[test]
fn a_bare_unique_name_is_machine_applicable() {
    let mut map = SourceMap::new();
    let file = map.intern_file("unique.tuo");
    let source = "fn main() -> Int {\n    let s: String = std::string::empty();\n    push_byte(s, 65)\n}\n";
    let id = map.add_source(file, source).expect("fits");
    let parse = tuo_parser::parse(map.source(id));
    let asts = [Ast::new(&parse.tree, source)];
    let resolution = tuo_resolve::resolve(&asts);

    let undefined = resolution
        .diagnostics()
        .iter()
        .find(|diagnostic| diagnostic.code.to_string() == "R0002")
        .expect("the undefined name is reported");
    let suggestion = undefined
        .suggestions
        .first()
        .expect("a single owner yields a suggestion");
    assert_eq!(
        suggestion.confidence,
        tuo_diagnostics::Confidence::MachineApplicable,
        "an unambiguous qualification is machine-applicable"
    );
    assert_eq!(
        suggestion.edits.first().expect("one edit").replacement,
        "std::string::push_byte"
    );
}
