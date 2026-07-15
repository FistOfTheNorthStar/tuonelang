//! CLI definition for `tuo`.
//!
//! The command surface is intentionally minimal. A [`Command`] enum is present
//! as the extension point for future subcommands, but no subcommands are
//! defined yet — adding one is a matter of a new enum variant plus a match arm
//! in [`Cli::dispatch`].

use std::process::ExitCode;

use clap::{CommandFactory as _, Parser, Subcommand};

/// The `tuo` command-line interface.
#[derive(Debug, Parser)]
#[command(
    name = "tuo",
    version,
    about = "tuonelang — an experimental statically typed, memory-safe native language.",
    long_about = "tuonelang is an experimental compiler project. This CLI currently exposes only \
                  `--help` and `--version`; compiler subcommands are added as their \
                  functionality is implemented.",
    // Show help when invoked with no subcommand, once subcommands exist. For
    // now the parse always yields no subcommand and we handle that in dispatch.
    disable_help_subcommand = true
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

/// Top-level subcommands.
///
/// This enum is deliberately empty of real commands. It exists so that future
/// work can add variants (`Build`, `Run`, `Check`, `Spec`, `Verify`, …)
/// without restructuring argument parsing.
#[derive(Debug, Subcommand)]
enum Command {}

impl Cli {
    /// Execute the parsed command.
    pub(crate) fn dispatch(self) -> ExitCode {
        match self.command {
            // No subcommands exist yet, so a bare `tuo` invocation prints help.
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
