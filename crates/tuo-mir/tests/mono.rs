//! ADR-0027 Stage B: `tuo_mir::monomorphize`, the native path's
//! clone-and-substitute pass over generic functions.
//!
//! The end-to-end agreement (interpreter on generic MIR == both backends on
//! the instances) is pinned by `tuo-cli/tests/codegen_three_way.rs`; this
//! file pins the pass's own contract: which instances exist, what they are
//! called, that they verify and carry no type parameter, and that a growing
//! chain is refused without touching the program.

use tuo_ast::Ast;
use tuo_mir::Program;
use tuo_resolve::Resolution;
use tuo_source::SourceMap;
use tuo_types::{Ty, TypeckResult};

fn lower(text: &str) -> (Program, TypeckResult, Resolution) {
    let mut map = SourceMap::new();
    let file = map.intern_file("mono.tuo");
    let id = map.add_source(file, text).expect("source fits");
    let parse = tuo_parser::parse(map.source(id));
    assert_eq!(parse.diagnostics, vec![], "parse errors");
    let asts = [Ast::new(&parse.tree, text)];
    let resolution = tuo_resolve::resolve(&asts);
    assert_eq!(resolution.diagnostics(), &[], "resolution errors");
    let types = tuo_types::check(&asts, &resolution);
    assert_eq!(types.diagnostics(), &[], "type errors");
    let hir = tuo_hir::lower(&asts, &resolution);
    let program = tuo_mir::lower(&hir, &resolution, &types);
    assert!(
        tuo_mir::verify(&program, &types).is_empty(),
        "raw MIR verifies"
    );
    (program, types, resolution)
}

fn names(program: &Program) -> Vec<&str> {
    let mut names: Vec<&str> = program.functions.iter().map(|f| f.name.as_str()).collect();
    names.sort_unstable();
    names
}

fn has_param(ty: &Ty) -> bool {
    match ty {
        Ty::Param(_) => true,
        Ty::Tuple(items) | Ty::Struct(_, items) | Ty::Enum(_, items) => items.iter().any(has_param),
        Ty::Array(item)
        | Ty::FixedArray(item, _)
        | Ty::Range(item)
        | Ty::Option(item)
        | Ty::Wrapper(_, item) => has_param(item),
        Ty::Result(a, b) | Ty::Map(a, b) => has_param(a) || has_param(b),
        Ty::Fn(fn_ty) => fn_ty.params.iter().any(|p| has_param(&p.ty)) || has_param(&fn_ty.ret),
        _ => false,
    }
}

/// One instance per distinct type-argument list, named after it; the generic
/// itself is gone, and nothing left mentions a type parameter.
#[test]
fn each_distinct_instantiation_gets_one_instance() {
    let (mut program, types, resolution) = lower(
        "fn ident[T](take x: T) -> T {\n    x\n}\n\n\
         fn main() -> Int {\n    let a = ident(1);\n    let b = ident(2);\n    \
         let c = ident(true);\n    if c { a + b } else { 0 }\n}\n",
    );
    tuo_mir::monomorphize(&mut program, &types, &resolution).expect("monomorphizes");
    assert_eq!(names(&program), ["ident[Bool]", "ident[I64]", "main"]);
    assert!(
        tuo_mir::verify(&program, &types).is_empty(),
        "instances verify"
    );
    for function in &program.functions {
        assert!(
            !has_param(&function.ret),
            "{}: generic return",
            function.name
        );
        for local in &function.locals {
            assert!(!has_param(&local.ty), "{}: generic local", function.name);
        }
    }
}

/// A generic calling a generic is closed transitively: `wrap[I64]` calls
/// `ident[I64]`, which `main` never names.
#[test]
fn instances_are_instantiated_in_turn() {
    let (mut program, types, resolution) = lower(
        "fn ident[T](take x: T) -> T {\n    x\n}\n\n\
         fn wrap[T](take x: T) -> Option[T] {\n    Some { value: ident(x) }\n}\n\n\
         fn main() -> Int {\n    match wrap(3) {\n        Some { value } => value,\n        \
         None => 0,\n    }\n}\n",
    );
    tuo_mir::monomorphize(&mut program, &types, &resolution).expect("monomorphizes");
    assert_eq!(names(&program), ["ident[I64]", "main", "wrap[I64]"]);
    assert!(tuo_mir::verify(&program, &types).is_empty());
}

/// Recursion at a fixed type reuses its own instance instead of minting one
/// per call.
#[test]
fn recursion_at_a_fixed_type_reuses_its_instance() {
    let (mut program, types, resolution) = lower(
        "fn depth[T](take x: T, take n: Int) -> T {\n    if n == 0 {\n        return x;\n    }\n    \
         depth(x, n - 1)\n}\n\n\
         fn main() -> Int {\n    depth(7, 3)\n}\n",
    );
    tuo_mir::monomorphize(&mut program, &types, &resolution).expect("monomorphizes");
    assert_eq!(names(&program), ["depth[I64]", "main"]);
}

/// A field of a generic struct is typed at the instance's arguments, so moving
/// `p.first` out of a `Pair[I64, I64]` becomes a copy, as `x` does in
/// `ident[I64]`. Typed as the declared `A`, it stayed a move of a `Copy` value
/// (M0009), and the build failed with an internal verification error instead
/// of the backend's own "generic struct layout" refusal.
#[test]
fn a_generic_struct_field_moves_as_a_copy_once_its_type_is_copy() {
    let (mut program, types, resolution) = lower(
        "struct Pair[A, B] {\n    first: A,\n    second: B,\n}\n\n\
         fn swap_pair[A, B](take p: Pair[A, B]) -> Pair[B, A] {\n    let a = p.first;\n    \
         let b = p.second;\n    Pair { first: b, second: a }\n}\n\n\
         fn main() -> Int {\n    let p = swap_pair(Pair { first: 3, second: 4 });\n    p.first\n}\n",
    );
    tuo_mir::monomorphize(&mut program, &types, &resolution).expect("monomorphizes");
    assert_eq!(names(&program), ["main", "swap_pair[I64, I64]"]);
    assert_eq!(
        tuo_mir::verify(&program, &types),
        vec![],
        "instances verify"
    );
}

/// An uninstantiated generic is dropped, not refused: no native code can
/// reach it.
#[test]
fn an_uninstantiated_generic_is_dropped() {
    let (mut program, types, resolution) =
        lower("fn ident[T](take x: T) -> T {\n    x\n}\n\nfn main() -> Int {\n    0\n}\n");
    tuo_mir::monomorphize(&mut program, &types, &resolution).expect("monomorphizes");
    assert_eq!(names(&program), ["main"]);
}

/// A function reaching itself at a growing type has no finite native form:
/// refused, naming the chain, with the program left as it was.
#[test]
fn a_growing_instantiation_chain_is_refused_and_leaves_the_program_alone() {
    let (mut program, types, resolution) = lower(
        "fn grow[T](take x: T, take n: Int) -> Int {\n    if n == 0 {\n        return 0;\n    }\n    \
         grow(Some { value: x }, n - 1) + 1\n}\n\n\
         fn main() -> Int {\n    grow(1, 4)\n}\n",
    );
    let before = names(&program).join(",");
    let error = tuo_mir::monomorphize(&mut program, &types, &resolution)
        .expect_err("a growing chain must be refused");
    assert!(
        error.message.contains("grow[I64]") && error.message.contains("grow[Option[I64]]"),
        "the refusal names the chain: {}",
        error.message
    );
    assert_eq!(
        names(&program).join(","),
        before,
        "refusal is all-or-nothing"
    );
}

/// A program with no generic function is untouched.
#[test]
fn a_monomorphic_program_is_unchanged() {
    let (mut program, types, resolution) =
        lower("fn inc(take x: Int) -> Int {\n    x + 1\n}\n\nfn main() -> Int {\n    inc(1)\n}\n");
    let before = tuo_mir::render(&program, &resolution);
    tuo_mir::monomorphize(&mut program, &types, &resolution).expect("monomorphizes");
    assert_eq!(tuo_mir::render(&program, &resolution), before);
}
