//! CLI definition for `tuo`.
//!
//! The command surface is intentionally minimal: only functionality the
//! compiler can actually perform is exposed. Today that is `tuo check`,
//! `tuo spec`, `tuo verify`, `tuo fmt`, and the `tuo debug` developer tools;
//! further compiler subcommands (`build`, `run`, …) are added as their
//! functionality is implemented — a new [`Command`] variant plus a match
//! arm in [`Cli::dispatch`].

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{CommandFactory as _, Parser, Subcommand};

use crate::check;
use crate::debug::{self, Dump};
use crate::fmt;
use crate::spec;

/// The `tuo` command-line interface.
#[derive(Debug, Parser)]
#[command(
    name = "tuo",
    version,
    about = "tuonelang — an experimental statically typed, memory-safe native language.",
    long_about = "tuonelang is an experimental compiler project. Compiler subcommands are added \
                  as their functionality is implemented; today the CLI exposes only the \
                  `debug` developer tools.",
    disable_help_subcommand = true
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

/// Top-level subcommands.
///
/// Future compiler commands (`Build`, `Run`, …) slot in as new variants once
/// their functionality exists.
#[derive(Debug, Subcommand)]
enum Command {
    /// Parse, resolve, type-check, and ownership-check a program without
    /// generating code.
    ///
    /// All given files are checked as one program: files declaring the same
    /// `module` path share a scope. Specs are parsed, attached to their
    /// target functions, and checked like any other item (they do not
    /// execute — spec execution arrives with the MIR interpreter).
    /// Diagnostics go to stderr; the exit status is a failure if the
    /// program has errors.
    Check {
        /// The tuonelang source files forming the program.
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Execute the program's colocated specs through the reference MIR
    /// interpreter.
    ///
    /// Runs the front end, lowers each spec to verified MIR, and drives its
    /// assertions in a deterministic sandbox bounded by instruction fuel,
    /// recursion depth, and memory. A `target` argument narrows execution to
    /// the specs of one function (or a free-standing spec of that name). A
    /// program with front-end errors is refused. Results and per-spec timing
    /// go to stderr; the exit status is a failure if any assertion fails or
    /// traps. No latency is promised — the reported timing is measured.
    Spec {
        /// Run only the specs of this function (or the free-standing spec of
        /// this name); omit to run every spec.
        #[arg(long, short)]
        target: Option<String>,
        /// The tuonelang source files forming the program.
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Perform every static check *and* execute the program's specs.
    ///
    /// A superset of `tuo check` and `tuo spec`: the whole front end runs,
    /// and if it accepts the program, every spec is executed. The exit status
    /// is a failure on any front-end error, failing assertion, or trap.
    Verify {
        /// The tuonelang source files forming the program.
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Diagnostic developer tools (unstable output, not a language protocol).
    #[command(subcommand)]
    Debug(DebugCommand),
    /// Rewrite source files into tuonelang's canonical format.
    ///
    /// The canonical form is fixed: one brace style, 4-space indentation,
    /// nothing configurable. Formatting preserves every comment and the
    /// program's meaning (each run is self-verified), and reproduces
    /// malformed regions byte-for-byte. `--check` reports files that are
    /// not canonical instead of rewriting them.
    Fmt {
        /// Report non-canonical files (exit status 1) instead of rewriting.
        #[arg(long)]
        check: bool,
        /// The tuonelang source files to format.
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
}

/// The `tuo debug` tools: raw dumps of compiler-internal representations.
#[derive(Debug, Subcommand)]
enum DebugCommand {
    /// Print the lossless concrete syntax tree of a source file.
    ///
    /// The dump shows every node, token, and comment, including Error nodes
    /// where the parser recovered. Parse diagnostics go to stderr; the exit
    /// code is a failure only if the file cannot be read.
    Syntax {
        /// The tuonelang source file to inspect.
        file: PathBuf,
    },
    /// Print the typed AST view of a source file.
    ///
    /// The dump walks the typed accessors (FnDecl, StructDecl, Expr, …) over
    /// the syntax tree. Parse diagnostics go to stderr; the exit code is a
    /// failure only if the file cannot be read.
    Ast {
        /// The tuonelang source file to inspect.
        file: PathBuf,
    },
    /// Print the lowered high-level IR (HIR) of a source file.
    ///
    /// The dump shows the desugared semantic tree with names resolved to
    /// stable symbol IDs (`name@symN`). Parse and resolution diagnostics go
    /// to stderr; the exit code is a failure only if the file cannot be
    /// read.
    Hir {
        /// The tuonelang source file to inspect.
        file: PathBuf,
    },
    /// Print the lowered mid-level IR (MIR) of a source file.
    ///
    /// MIR is only defined for accepted programs, so the whole front end
    /// (parse, resolve, type check, ownership check) runs first: any error
    /// refuses the dump with a failure exit code. Function bodies outside
    /// the v0 lowering subset are listed as `not lowered` with the reason.
    Mir {
        /// The tuonelang source file to inspect.
        file: PathBuf,
        /// Restrict the dump to the function with this name.
        function: Option<String>,
    },
}

impl Cli {
    /// Execute the parsed command.
    pub(crate) fn dispatch(self) -> ExitCode {
        match self.command {
            Some(Command::Check { files }) => check::run(&files),
            Some(Command::Spec { target, files }) => spec::run(target, &files),
            Some(Command::Verify { files }) => spec::verify(&files),
            Some(Command::Debug(DebugCommand::Syntax { file })) => debug::run(Dump::Syntax, &file),
            Some(Command::Debug(DebugCommand::Ast { file })) => debug::run(Dump::Ast, &file),
            Some(Command::Debug(DebugCommand::Hir { file })) => debug::run(Dump::Hir, &file),
            Some(Command::Debug(DebugCommand::Mir { file, function })) => {
                debug::run(Dump::Mir(function), &file)
            }
            Some(Command::Fmt { check, files }) => fmt::run(check, &files),
            // A bare `tuo` invocation prints help.
            None => match Self::command().print_help() {
                Ok(()) => ExitCode::SUCCESS,
                Err(_) => ExitCode::FAILURE,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The clap command definition must be internally consistent. This also
    /// guarantees `--help` and `--version` are wired up.
    #[test]
    fn command_definition_is_valid() {
        Cli::command().debug_assert();
    }

    /// `--version` is handled by clap and reported as a special "error" that
    /// carries the version output and a success exit status.
    #[test]
    fn version_flag_is_accepted() {
        let err = Cli::try_parse_from(["tuo", "--version"])
            .expect_err("--version should short-circuit parsing");
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayVersion);
    }

    /// `--help` is likewise handled by clap.
    #[test]
    fn help_flag_is_accepted() {
        let err = Cli::try_parse_from(["tuo", "--help"])
            .expect_err("--help should short-circuit parsing");
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
    }

    /// A bare invocation parses successfully with no subcommand selected.
    #[test]
    fn bare_invocation_parses_with_no_subcommand() {
        let cli = Cli::try_parse_from(["tuo"]).expect("bare invocation should parse");
        assert!(cli.command.is_none());
    }
}
