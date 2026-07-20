//! The `tuo debug` developer tools: raw dumps of the compiler's syntax
//! representations for a single file.
//!
//! These are diagnostic aids for working on the compiler, **not** stable
//! language protocols — their output format may change at any time.

use std::path::Path;
use std::process::ExitCode;

use tuo_compiler::ast::{self, Ast};
use tuo_compiler::{diagnostics, hir, parser, resolve, source::SourceMap};

/// Which representation to dump.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Dump {
    /// The lossless concrete syntax tree.
    Syntax,
    /// The typed AST views.
    Ast,
    /// The lowered high-level IR.
    Hir,
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
    };
    print!("{rendered}");

    let all = result.all_diagnostics();
    if !all.is_empty() {
        eprint!("{}", diagnostics::render::render_all(&all, &map));
    }
    ExitCode::SUCCESS
}
