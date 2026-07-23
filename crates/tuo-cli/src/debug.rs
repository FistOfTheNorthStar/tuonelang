//! The `tuo debug` developer tools: raw dumps of the compiler's internal
//! representations for a single file (syntax, AST, HIR, MIR).
//!
//! These are diagnostic aids for working on the compiler, **not** stable
//! language protocols — their output format may change at any time. The
//! syntax dumps tolerate malformed input (the parser is total); the MIR
//! dump requires an accepted program, since MIR is only defined once the
//! front end passes, and the lowered MIR is verified before it is printed
//! (a consumer must never accept unverified MIR).

use std::path::Path;
use std::process::ExitCode;

use tuo_compiler::ast::{self, Ast};
use tuo_compiler::{diagnostics, hir, mir, parser, resolve, source::SourceMap};

/// Which representation to dump.
#[derive(Clone, Debug)]
pub(crate) enum Dump {
    /// The lossless concrete syntax tree.
    Syntax,
    /// The typed AST views.
    Ast,
    /// The lowered high-level IR.
    Hir,
    /// The lowered mid-level IR, optionally restricted to one function.
    Mir(Option<String>),
}

/// Dump `file`'s syntax or AST to stdout; parse diagnostics go to stderr.
///
/// The dump itself always succeeds on parseable-with-recovery input (the
/// parser is total); the exit code is a failure only when the file cannot
/// be read at all.
#[expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "this is the CLI presentation layer: stdout carries the dump, stderr the diagnostics"
)]
pub(crate) fn run(dump: Dump, path: &Path) -> ExitCode {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("error: cannot read {}: {error}", path.display());
            return ExitCode::FAILURE;
        }
    };
    let mut map = SourceMap::new();
    let file = map.intern_file(&path.display().to_string());
    let id = match map.add_source(file, text.as_str()) {
        Ok(id) => id,
        Err(error) => {
            eprintln!("error: {}: {error}", path.display());
            return ExitCode::FAILURE;
        }
    };
    if let Dump::Mir(function) = &dump {
        return run_mir(&map, id, function.as_deref());
    }
    let result = parser::parse(map.source(id));

    let rendered = match dump {
        Dump::Syntax => result.tree.render(&text),
        Dump::Ast => ast::render(Ast::new(&result.tree, &text)),
        Dump::Hir => {
            let asts = [Ast::new(&result.tree, &text)];
            let resolution = resolve::resolve(&asts);
            let lowered = hir::lower(&asts, &resolution);
            let rendered = hir::render(&lowered, &resolution);
            if !resolution.diagnostics().is_empty() {
                eprint!(
                    "{}",
                    diagnostics::render::render_all(resolution.diagnostics(), &map)
                );
            }
            rendered
        }
        Dump::Mir(_) => unreachable!("handled above"),
    };
    print!("{rendered}");

    let all = result.all_diagnostics();
    if !all.is_empty() {
        eprint!("{}", diagnostics::render::render_all(&all, &map));
    }
    ExitCode::SUCCESS
}

/// Dump the lowered MIR. Unlike the syntax dumps, MIR of an erroneous
/// program is undefined, so the whole front end must pass first; any
/// front-end error is reported and the dump refused.
#[expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "this is the CLI presentation layer: stdout carries the dump, stderr the diagnostics"
)]
fn run_mir(map: &SourceMap, id: tuo_compiler::source::SourceId, filter: Option<&str>) -> ExitCode {
    let check = tuo_compiler::check_sources(map, &[id]);
    if !check.diagnostics.is_empty() {
        eprint!(
            "{}",
            diagnostics::render::render_all(&check.diagnostics, map)
        );
    }
    if check.has_errors() {
        eprintln!("error: cannot lower MIR: the program has front-end errors");
        return ExitCode::FAILURE;
    }
    let parse = parser::parse(map.source(id));
    let asts = [Ast::new(&parse.tree, map.source(id).text())];
    let lowered_hir = hir::lower(&asts, &check.resolution);
    let program = mir::lower(&lowered_hir, &check.resolution, &check.types);
    // MIR is only meaningful when it verifies; a consumer must never accept
    // unverified MIR, so refuse the dump if lowering produced malformed MIR
    // (this would be a compiler bug, not a user error).
    let problems = mir::verify(&program, &check.types);
    if !problems.is_empty() {
        eprint!("{}", diagnostics::render::render_all(&problems, map));
        eprintln!("error: internal: lowered MIR failed verification");
        return ExitCode::FAILURE;
    }
    let mut program = program;
    if let Some(name) = filter {
        program.functions.retain(|function| function.name == name);
        program.skipped.retain(|skipped| skipped.name == name);
        if program.functions.is_empty() && program.skipped.is_empty() {
            eprintln!("error: no function named `{name}`");
            return ExitCode::FAILURE;
        }
    }
    print!("{}", mir::render(&program, &check.resolution));
    ExitCode::SUCCESS
}
