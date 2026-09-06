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

use tuo_ast::{Ast, Block, Item, Statement, TypeRef};
use tuo_diagnostics::{Diagnostic, DiagnosticCode, Namespace, StructuredValue};

/// The runnable-core advisory code: `T0022`.
fn code() -> DiagnosticCode {
    DiagnosticCode::new(Namespace::Type, 22)
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
