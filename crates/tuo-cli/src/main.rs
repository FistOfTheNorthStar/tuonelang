//! `tuo` — the command-line entry point for the tuonelang compiler and tooling.
//!
//! Implemented today: `tuo check <files>` (parse, resolve, type-check, and
//! ownership-check a program — specs included — without generating code),
//! `tuo spec [target] <files>` and `tuo verify <files>` (execute the
//! program's colocated specs through the reference MIR interpreter),
//! `tuo fmt [--check] <files>` (the canonical source formatter),
//! `tuo build [-o out] <files>` and `tuo run <files>` (compile the program to
//! native code with the Cranelift backend and, for `run`, execute it), and the
//! `tuo debug syntax|ast|hir|mir <file>` developer tools (raw dumps of
//! compiler-internal representations — unstable diagnostic output, not a
//! language protocol), plus the built-in `--help` and `--version`. Further
//! compiler subcommands are added as their underlying functionality exists, so
//! that the CLI never advertises behavior the compiler cannot yet perform. See
//! [`cli::Cli`] for the extension point.
//!
//! Every result-producing command reports through a shared [`output`] mode
//! selected by the global `--message-format` flag: `human` (default) for a
//! terminal, or the versioned machine [`protocol`] as `json` (one envelope) or
//! `json-lines` (streamed). In a machine format stdout carries protocol output
//! only, and internal logging reaches stderr solely under `--log`.

mod check;
mod cli;
mod codegen;
mod debug;
mod fmt;
mod output;
mod protocol;
mod spec;

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
