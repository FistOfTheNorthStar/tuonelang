//! `tuo` — the command-line entry point for the tuonelang compiler and tooling.
//!
//! Implemented today: the `tuo debug syntax <file>` / `tuo debug ast <file>`
//! developer tools (raw dumps of the CST and the typed AST views — unstable
//! diagnostic output, not a language protocol) plus the built-in `--help`
//! and `--version`. Compiler subcommands such as `build`, `run`, `check`,
//! `spec`, and `verify` are intentionally **not** implemented: they are
//! added as their underlying functionality exists, so that the CLI never
//! advertises behavior the compiler cannot yet perform. See [`cli::Cli`]
//! for the extension point.

mod cli;
mod debug;

use std::process::ExitCode;

use clap::Parser as _;

use crate::cli::Cli;

fn main() -> ExitCode {
    Cli::run(std::env::args_os())
}

// `Cli::run` lives in the `cli` module and is thin enough to test directly.
impl Cli {
    /// Parse the given arguments and dispatch.
    ///
    /// On a plain invocation (`tuo` with no arguments) this prints the help
    /// text and exits successfully, matching common CLI conventions. `--help`
    /// and `--version` are handled by clap before this returns.
    fn run<I, T>(args: I) -> ExitCode
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString> + Clone,
    {
        let cli = Cli::parse_from(args);
        cli.dispatch()
    }
}
