//! CLI definition for `tuo`.
//!
//! The command surface is intentionally minimal: only functionality the
//! compiler can actually perform is exposed. Today that is `tuo check`,
//! `tuo spec`, `tuo verify`, `tuo fmt`, `tuo build`, `tuo run`,
//! `tuo agent --stdio`, and the `tuo debug` developer tools. Further compiler
//! subcommands are added as their functionality is implemented — a new
//! [`Command`] variant plus a match arm in [`Cli::dispatch`].

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{CommandFactory as _, Parser, Subcommand};

use crate::agent;
use crate::check;
use crate::codegen;
use crate::debug::{self, Dump};
use crate::fmt;
use crate::output::{MessageFormat, OutputMode};
use crate::spec;

/// The `tuo` command-line interface.
#[derive(Debug, Parser)]
#[command(
    name = "tuo",
    version,
    about = "tuonelang — an experimental statically typed, memory-safe native language.",
    long_about = "tuonelang is an experimental compiler project. Compiler subcommands are added \
                  as their functionality is implemented; today the CLI exposes check, spec, \
                  verify, fmt, build, run, the agent protocol (`agent --stdio`), and the \
                  `debug` developer tools.",
    disable_help_subcommand = true
)]
pub(crate) struct Cli {
    /// How to report results: `human` (default, for a terminal) or the
    /// versioned machine protocol as `json` (one envelope) or `json-lines`
    /// (streamed, one event per line). In a machine format stdout carries
    /// protocol output only.
    #[arg(long, global = true, value_enum, default_value_t = MessageFormat::Human)]
    message_format: MessageFormat,
    /// Emit internal logging to stderr. Off by default; in a machine format
    /// stderr is silent unless this is set, so a consumer parsing stdout is
    /// never disturbed by log noise.
    #[arg(long, global = true)]
    log: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

/// Top-level subcommands.
///
/// Further compiler commands slot in as new variants once their functionality
/// exists.
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
    ///
    /// `--affected-by <file>` restricts execution to the specs whose semantic
    /// dependencies touch a symbol defined in `<file>` (which must be one of
    /// the input files) — the specs an edit to that file could have changed,
    /// selected through the incremental dependency graph. Omit it to run every
    /// spec.
    Verify {
        /// Run only the specs affected by an edit to this file (one of the
        /// input files); omit to run every spec.
        #[arg(long = "affected-by", value_name = "FILE")]
        affected_by: Option<PathBuf>,
        /// The tuonelang source files forming the program.
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Compile a program to a native executable.
    ///
    /// Runs the front end, lowers the accepted program to verified MIR, and
    /// generates native code, linking the result (with the tuonelang runtime)
    /// into an executable. The program's entry is the function named `main`,
    /// which must be nullary and return an integer — its value is the process
    /// exit status. A program using a feature outside the native backend's
    /// current subset is refused with a clear note (the interpreter, via
    /// `tuo spec`/`tuo verify`, remains the reference).
    ///
    /// The default (debug) build uses the Cranelift backend, which emits
    /// unoptimized code quickly. `--release` selects the LLVM backend and runs
    /// LLVM's standard optimization pipeline; the two backends and the
    /// interpreter agree on every program's result (pinned by the differential
    /// suite), so `--release` changes only speed, never meaning.
    Build {
        /// Write the executable here (defaults to the first input's name).
        #[arg(long, short)]
        output: Option<PathBuf>,
        /// Build an optimized release binary with the LLVM backend instead of
        /// the default debug build with Cranelift.
        #[arg(long)]
        release: bool,
        /// The tuonelang source files forming the program.
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Compile a program and run it, propagating its exit status.
    ///
    /// A superset of `tuo build`: it compiles to a temporary executable, runs
    /// it, and exits with the program's own status — the integer its `main`
    /// entry returns. That value equals what the reference interpreter yields
    /// running the same entry, so a native run and an interpreted run agree.
    /// `--release` uses the optimized LLVM backend, exactly as `tuo build`.
    Run {
        /// Compile with the optimized LLVM backend instead of the default
        /// debug Cranelift backend.
        #[arg(long)]
        release: bool,
        /// The tuonelang source files forming the program.
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Serve the tuonelang agent protocol over stdio.
    ///
    /// Speaks a versioned, JSON-lines request/response protocol: a coding agent
    /// writes one request per line to stdin and reads one response per line
    /// from stdout. The server keeps one compiler database alive across every
    /// request, so an agent editing a program does not restart the compiler
    /// per edit. It answers compiler-intelligence queries — diagnostics,
    /// types, definitions, references, symbols, signatures, importable names,
    /// specs, and safe fixes — reusing the same shared queries as `tuo check`
    /// and the language server; it embeds no LLM and is not itself an AI model.
    ///
    /// `--stdio` selects the (only, today) transport; it is required so the CLI
    /// never advertises a transport it does not have.
    Agent {
        /// Serve the protocol over standard input/output.
        #[arg(long, required = true)]
        stdio: bool,
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
    ///
    /// With `--opt` the MIR optimization passes (the ones the native build
    /// runs) are applied before printing, so the dump shows the optimized MIR
    /// a backend consumes rather than the raw lowering.
    Mir {
        /// The tuonelang source file to inspect.
        file: PathBuf,
        /// Restrict the dump to the function with this name.
        function: Option<String>,
        /// Run the MIR optimization passes before printing (show the
        /// optimized MIR the native backend consumes).
        #[arg(long)]
        opt: bool,
    },
}

impl Cli {
    /// Execute the parsed command.
    pub(crate) fn dispatch(self) -> ExitCode {
        let mode = OutputMode::new(self.message_format, self.log);
        match self.command {
            Some(Command::Check { files }) => check::run(&files, mode),
            Some(Command::Spec { target, files }) => spec::run(target, &files, mode),
            Some(Command::Verify { affected_by, files }) => {
                spec::verify(affected_by.as_deref(), &files, mode)
            }
            Some(Command::Build {
                output,
                release,
                files,
            }) => codegen::build(output, release, &files, mode),
            Some(Command::Run { release, files }) => codegen::run(release, &files, mode),
            // The agent protocol is its own versioned JSON-lines contract on
            // stdio, independent of `--message-format` (which governs the
            // result-producing commands' output). `mode` is passed only so
            // internal logging honors `--log`.
            Some(Command::Agent { stdio: _ }) => agent::run(mode),
            // The `debug` dumps are developer tools with deliberately unstable
            // output, not a language protocol — so they have no machine
            // encoding. Refuse a machine format here rather than emit
            // unversioned JSON that consumers might come to depend on.
            Some(Command::Debug(command)) => {
                if mode.is_machine() {
                    return reject_machine_debug(mode);
                }
                match command {
                    DebugCommand::Syntax { file } => debug::run(Dump::Syntax, &file),
                    DebugCommand::Ast { file } => debug::run(Dump::Ast, &file),
                    DebugCommand::Hir { file } => debug::run(Dump::Hir, &file),
                    DebugCommand::Mir {
                        file,
                        function,
                        opt,
                    } => debug::run(
                        Dump::Mir {
                            filter: function,
                            opt,
                        },
                        &file,
                    ),
                }
            }
            Some(Command::Fmt { check, files }) => fmt::run(check, &files, mode),
            // A bare `tuo` invocation prints help.
            None => match Self::command().print_help() {
                Ok(()) => ExitCode::SUCCESS,
                Err(_) => ExitCode::FAILURE,
            },
        }
    }
}

/// The `debug` tools have no machine protocol; a `--message-format` other than
/// `human` is a usage error. Reports it on stderr (a usage error, not protocol
/// output) and fails.
#[expect(
    clippy::print_stderr,
    reason = "this is the CLI presentation layer: a usage error goes to stderr"
)]
fn reject_machine_debug(_mode: OutputMode) -> ExitCode {
    eprintln!(
        "error: `tuo debug` has no machine protocol (its output is an unstable developer aid); \
         run it with --message-format=human"
    );
    ExitCode::FAILURE
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
