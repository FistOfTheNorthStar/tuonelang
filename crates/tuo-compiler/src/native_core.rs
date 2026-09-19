//! The runnable-core advisory (`T0022`): what `tuo check` accepts but the
//! native backends cannot lower.
//!
//! `tuo check` deliberately accepts a **larger** language than `tuo build`
//! and `tuo run` execute. That gap is real and load-bearing — the MIR
//! interpreter is the reference semantics, so a program outside the native
//! subset still runs under `tuo spec`/`tuo verify`. What the gap must not
//! be is *silent*: before this advisory a program using a `Box`/`Shared`/
//! `Weak` **value** checked clean and only failed at storage-classification
//! time inside a backend, as a whole-program message naming a function but
//! carrying no span.
//!
//! This module closes that by walking the parsed program for the one
//! construct that causes the gap and reporting it as a **warning** at the
//! exact span of the offending type. A warning, not an error, is the whole
//! point: these programs are legal tuonelang and the interpreter executes
//! them, so rejecting them would shrink the language to its backends'
//! current reach.
//!
//! # What is and is not reported
//!
//! Only a wrapper in **storage position** — a parameter, return type, or
//! `let`/`var` annotation — is reported, because that is precisely where the
//! backends' `classify_storage` refuses. A wrapper in a **field or variant
//! payload declaration** is *not* reported: such a declaration lowers
//! perfectly well (the wrapper is a bare pointer, so the aggregate's layout
//! is finite), and it is exactly what `T0016` tells the user to reach for
//! when breaking a recursive type. Warning there would contradict the
//! compiler's own advice, so the two diagnostics stay consistent by
//! construction.
//!
//! Capturing closures — the other half of the gap as it is usually
//! described — need no advisory: the grammar has no closure syntax, so a
//! program cannot express one. The non-first-class function cases are
//! already hard errors (`T0015`) in the type checker.
//!
//! # The generic advisory (`T0023`)
//!
//! Generic functions are the *second* instance of the same gap, and until
//! ADR-0027 they were the undocumented one. A generic `fn` parses, resolves,
//! type-checks, ownership-checks, and **executes on the reference
//! interpreter** — `spec ident { then ident(7) == 7; then ident(true) ==
//! true; }` passes at two distinct instantiations — while both backends
//! refuse it, because `Ty::Param` has no layout until monomorphized
//! (`tuo_runtime::abi::layout_of`). That refusal arrives from
//! `classify_storage` as a whole-program message naming a function and
//! carrying no span, which is exactly what this module exists to prevent.
//!
//! So a generic declaration is warned about at the span of its parameter
//! list, on the same terms as the wrapper advisory: a warning, never an
//! error, because the program is legal tuonelang that the interpreter runs.
//! The advisory is deliberately independent of whether monomorphization is
//! ever scheduled — it is a strict improvement over a spanless backend
//! refusal either way.
//!
//! Only the **declaration** is reported, not each call site: the
//! instantiation is not what the backend refuses, the body is, and one
//! warning per generic function keeps the count proportional to the source
//! rather than to its use. Generic `struct`/`enum` declarations are *not*
//! reported — `require_monomorphic` refuses them at layout time, but they
//! are only reachable through a generic function or an annotation this same
//! pass already sees, and ADR-0027 leaves generic aggregates to a successor
//! (its Q7). `impl` needs no advisory either: the parser refuses it outright
//! (`P0002`), so no generic method can be written.

use tuo_ast::{Ast, Block, GenericParam, Item, Statement, TypeRef};
use tuo_diagnostics::{Diagnostic, DiagnosticCode, Namespace, StructuredValue};

/// The runnable-core advisory code: `T0022`.
fn code() -> DiagnosticCode {
    DiagnosticCode::new(Namespace::Type, 22)
}

/// The generic-declaration advisory code: `T0023` (ADR-0027 Stage A).
fn generic_code() -> DiagnosticCode {
    DiagnosticCode::new(Namespace::Type, 23)
}

/// Warn about every construct `asts` uses that the native backends do not
/// lower, in source order.
///
/// The returned diagnostics are all [`Severity::Warning`], so they never
/// reject a program: `tuo check` prints them, [`CheckResult::has_errors`]
/// ignores them, and `tuo spec`/`tuo verify` are unaffected.
///
/// [`Severity::Warning`]: tuo_diagnostics::Severity::Warning
/// [`CheckResult::has_errors`]: crate::CheckResult::has_errors
pub(crate) fn advisories(asts: &[Ast<'_>]) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for ast in asts {
        for item in ast.file().items() {
            // Only function signatures and bodies carry storage positions.
            // Field and variant-payload declarations are deliberately left
            // alone — see the module docs.
            let Item::Fn(decl) = item else { continue };
            report_generics(decl, &mut out);
            for param in decl.params() {
                if let Some(ty) = param.ty() {
                    report_type(ty, "parameter", &mut out);
                }
            }
            if let Some(ty) = decl.return_type() {
                report_type(ty, "return type", &mut out);
            }
            if let Some(body) = decl.body() {
                report_block(body, &mut out);
            }
        }
    }
    out.sort_by_key(|diagnostic| {
        (
            diagnostic.primary_span.source(),
            diagnostic.primary_span.range().start(),
        )
    });
    out
}

/// Report `decl` if it declares generic parameters (`T0023`).
///
/// The span is the parameter list itself rather than the function name,
/// because the list is what makes the body unlowerable and what a reader
/// would delete to fix it.
///
/// An empty list (`fn f[]`) is reported as its own `P0001` parse error and
/// still yields a `GenericParams` node, so the guard below is load-bearing
/// rather than defensive: such a function declares no type parameters, has
/// nothing to substitute, and the backends lower it fine. Warning there
/// would add noise on top of a parse error that already says what is wrong.
fn report_generics(decl: tuo_ast::FnDecl<'_>, out: &mut Vec<Diagnostic>) {
    let Some(generics) = decl.generics() else {
        return;
    };
    let mut names = generics.params().filter_map(GenericParam::name).peekable();
    if names.peek().is_none() {
        return;
    }
    let listed = names.collect::<Vec<_>>();
    let verb = if listed.len() == 1 { "has" } else { "have" };
    let listed = listed.join("`, `");
    let Some(span) = generics.span() else { return };
    let text = generics.text();
    let name = decl.name().unwrap_or("this function");
    out.push(
        Diagnostic::warning(
            generic_code(),
            format!(
                "generic function `{name}` is outside the native runnable core: it \
                 type-checks and runs under `tuo spec`/`tuo verify`, but \
                 `tuo build`/`tuo run` cannot lower it"
            ),
            span,
        )
        .with_primary_label(format!(
            "`{listed}` {verb} no layout until monomorphized, so no native backend \
             can lower this body"
        ))
        .with_help(
            "monomorphization is deferred (ADR-0027); the reference interpreter \
             executes this program today. To build natively, write one concrete \
             function per type instead of a generic one",
        )
        .with_actual(StructuredValue::Name(text.to_owned())),
    );
}

/// Report the wrapper-typed `let`/`var` annotations in `block`.
fn report_block(block: Block<'_>, out: &mut Vec<Diagnostic>) {
    for statement in block.statements() {
        let (Statement::Let(binding) | Statement::Var(binding)) = statement else {
            continue;
        };
        if let Some(ty) = binding.ty() {
            report_type(ty, "local binding", out);
        }
    }
}

/// Report `ty` if it contains a heap wrapper anywhere in the written type.
///
/// The search covers the whole type, not just its head, so a nested wrapper
/// (`Array[Box[Int]]`, `Box[Box[Int]]`) is caught too — the backends refuse
/// those at the same classification step.
fn report_type(ty: TypeRef<'_>, position: &str, out: &mut Vec<Diagnostic>) {
    let Some(wrapper) = first_wrapper(ty) else {
        return;
    };
    let Some(span) = wrapper.span() else { return };
    let text = wrapper.text();
    let kind = wrapper.wrapper().unwrap_or("Box");
    out.push(
        Diagnostic::warning(
            code(),
            format!(
                "`{kind}[T]` is outside the native runnable core: this {position} \
                 type-checks, but `tuo build`/`tuo run` cannot lower it"
            ),
            span,
        )
        .with_primary_label(format!("`{text}` is not lowered by any native backend"))
        .with_help(
            "heap-wrapper *values* await a later ADR; `tuo spec`/`tuo verify` still run \
             this program on the reference interpreter. A wrapper in a struct field or \
             enum payload is fine — only parameter, return, and `let`/`var` positions \
             are affected",
        )
        .with_actual(StructuredValue::Name(text.to_owned())),
    );
}

/// The first wrapper type at or under `ty`, in source order, or `None` if
/// the written type contains none.
fn first_wrapper<'a>(ty: TypeRef<'a>) -> Option<tuo_ast::WrapperType<'a>> {
    match ty {
        TypeRef::Wrapper(wrapper) => Some(wrapper),
        // A generic path (`Array[Box[Int]]`, `Map[Str, Box[Int]]`) hides a
        // wrapper in its arguments.
        TypeRef::Path(path) => path
            .args()
            .and_then(|args| args.types().find_map(first_wrapper)),
        TypeRef::FixedArray(array) => array.element().and_then(first_wrapper),
        // A function *type* is a code pointer; its written parameter and
        // return types are the callee's storage positions, and the callee's
        // own declaration is where they are reported. Descending here would
        // double-report the same refusal.
        TypeRef::Fn(_) | TypeRef::Unit(_) => None,
    }
}
