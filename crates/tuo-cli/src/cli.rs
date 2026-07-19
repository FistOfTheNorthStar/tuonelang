//! CLI definition for `tuo`.
//!
//! The command surface is intentionally minimal: only functionality the
//! compiler can actually perform is exposed. Today that is the `tuo debug`
//! developer tools; compiler subcommands (`build`, `run`, `check`, `spec`,
//! `verify`, …) are added as their functionality is implemented — a new
//! [`Command`] variant plus a match arm in [`Cli::dispatch`].

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{CommandFactory as _, Parser, Subcommand};

use crate::debug::{self, Dump};

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
/// Future compiler commands (`Build`, `Run`, `Check`, `Spec`, `Verify`, …)
/// slot in as new variants once their functionality exists.
#[derive(Debug, Subcommand)]
enum Command {
    /// Diagnostic developer tools (unstable output, not a language protocol).
    #[command(subcommand)]
    Debug(DebugCommand),
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
}

impl Cli {
    /// Execute the parsed command.
    pub(crate) fn dispatch(self) -> ExitCode {
        match self.command {
            Some(Command::Debug(DebugCommand::Syntax { file })) => debug::run(Dump::Syntax, &file),
            Some(Command::Debug(DebugCommand::Ast { file })) => debug::run(Dump::Ast, &file),
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
