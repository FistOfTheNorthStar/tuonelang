//! CLI definition for `tuo`.
//!
//! The command surface is intentionally minimal: only functionality the
//! compiler can actually perform is exposed. Today that is `tuo check`,
//! `tuo spec`, `tuo verify`, `tuo fmt`, `tuo build`, `tuo run`, the package
//! commands (`tuo new`/`add`/`remove`/`test` and the package-aware forms of
//! `check`/`build`/`verify`, plus `tuo package symbols`), `tuo corpus validate`,
//! `tuo bench report`, `tuo cheatsheet`, `tuo agent --stdio`,
//! and the `tuo debug` developer tools. Further compiler subcommands are added
//! as their functionality is implemented — a new [`Command`] variant plus a
//! match arm in [`Cli::dispatch`].

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{CommandFactory as _, Parser, Subcommand};

use crate::agent;
use crate::bench;
use crate::cheatsheet;
use crate::check;
use crate::codegen;
use crate::corpus::{self, CorpusCategory, CorpusOrigin};
use crate::debug::{self, Dump};
use crate::fmt;
use crate::output::{MessageFormat, OutputMode};
use crate::package;
use crate::spec;

/// The `tuo` command-line interface.
#[derive(Debug, Parser)]
#[command(
    name = "tuo",
    version,
    about = "tuonelang — an experimental statically typed, memory-safe native language.",
    long_about = "tuonelang is an experimental compiler project. Compiler subcommands are added \
                  as their functionality is implemented; today the CLI exposes check, spec, \
                  verify, fmt, build, run, the package commands (new, add, remove, test, \
                  package), corpus, bench, cheatsheet, the agent protocol (`agent --stdio`), \
                  and the `debug` developer tools.",
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
        /// The tuonelang source files forming the program. Omit to check the
        /// package in the manifest directory (`--manifest`, default `.`)
        /// instead, resolving its dependency graph first.
        files: Vec<PathBuf>,
        /// Operate on the package rooted at this directory (its `tdg.toml`)
        /// rather than a bare file list. Ignored when files are given.
        #[arg(long, value_name = "DIR")]
        manifest: Option<PathBuf>,
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
        /// input files); omit to run every spec. Only applies to the file-list
        /// form.
        #[arg(long = "affected-by", value_name = "FILE")]
        affected_by: Option<PathBuf>,
        /// The tuonelang source files forming the program. Omit to verify the
        /// package in the manifest directory instead.
        files: Vec<PathBuf>,
        /// Operate on the package rooted at this directory. Ignored when files
        /// are given.
        #[arg(long, value_name = "DIR")]
        manifest: Option<PathBuf>,
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
        /// Write the executable here (defaults to the first input's name, or
        /// the package name in package mode).
        #[arg(long, short)]
        output: Option<PathBuf>,
        /// Build an optimized release binary with the LLVM backend instead of
        /// the default debug build with Cranelift.
        #[arg(long)]
        release: bool,
        /// The tuonelang source files forming the program. Omit to build the
        /// package in the manifest directory instead.
        files: Vec<PathBuf>,
        /// Operate on the package rooted at this directory. Ignored when files
        /// are given.
        #[arg(long, value_name = "DIR")]
        manifest: Option<PathBuf>,
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
    /// Emit a dense, context-injectable language brief for a coding agent
    /// or a local model (ADR-0018).
    ///
    /// The brief is **generated from the compiler's own sources**: the syntax
    /// section carries `grammar.ebnf`'s version marker and every listed
    /// standard-library signature is the one the type checker inferred for
    /// this build, so the text cannot advertise a function that does not
    /// exist or a signature that has changed. Written to stdout, it is meant
    /// to be pasted into a model's context before asking it to write
    /// tuonelang. Human output is plain text; a machine format carries the
    /// same brief as a protocol item.
    Cheatsheet,
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
    /// Scaffold a new package.
    ///
    /// Creates a directory named after the package (or `--path <dir>`)
    /// containing a `tdg.toml` manifest and a starter `main` module under the
    /// default module root. The new package checks and runs immediately.
    New {
        /// The package name (lowercase letters, digits, and underscores;
        /// must start with a letter).
        name: String,
        /// Create the package here instead of in a directory named after it.
        #[arg(long, value_name = "DIR")]
        path: Option<PathBuf>,
    },
    /// Add a path dependency to the package manifest.
    ///
    /// Records `<name> = { path = "<path>" }` under `[dependencies]`, then
    /// re-resolves the graph and rewrites `tdg.lock`. v0 supports only path
    /// dependencies; a remote registry is a later addition.
    Add {
        /// The dependency's package name (must match the dependency's own
        /// manifest name).
        name: String,
        /// The path to the dependency package, relative to this manifest.
        #[arg(long, value_name = "PATH")]
        path: String,
        /// The directory holding the manifest to edit (default `.`).
        #[arg(long, value_name = "DIR")]
        manifest: Option<PathBuf>,
    },
    /// Remove a dependency from the package manifest.
    ///
    /// Drops the named entry from `[dependencies]`, then re-resolves and
    /// rewrites `tdg.lock`.
    Remove {
        /// The dependency's package name.
        name: String,
        /// The directory holding the manifest to edit (default `.`).
        #[arg(long, value_name = "DIR")]
        manifest: Option<PathBuf>,
    },
    /// Run the package's tests: its colocated specs, across the resolved graph.
    ///
    /// Resolves the package's dependencies, loads the whole graph as one
    /// program, and executes every spec through the reference interpreter —
    /// the same execution `tuo verify` performs. In v0 a package's tests *are*
    /// its specs, so `test` and `verify` run the same set; `test` is the
    /// conventionally-named command.
    Test {
        /// The directory holding the package manifest (default `.`).
        #[arg(long, value_name = "DIR")]
        manifest: Option<PathBuf>,
    },
    /// Query a resolved package (machine protocol only).
    #[command(subcommand)]
    Package(PackageCommand),
    /// Run programs through the compiler-validated corpus pipeline.
    #[command(subcommand)]
    Corpus(CorpusCommand),
    /// Score a recorded code-generation benchmark run.
    #[command(subcommand)]
    Bench(BenchCommand),
}

/// The `tuo bench` commands: the code-generation evaluation harness.
#[derive(Debug, Subcommand)]
enum BenchCommand {
    /// Score a recorded benchmark run by re-compiling the model's outputs.
    ///
    /// The evaluation harness embeds no LLM: an external runner records a model's
    /// generations into a run file, and this command *proves* that run's metrics
    /// by recompiling every recorded output through the real front end and spec
    /// runner (never trusting the recorded booleans). It first verifies the task
    /// set's content-digest pins, so a silently-edited benchmark is refused. It
    /// reports Parse@1, Check@1, SpecPass@1, TestPass@1, Repair@1, repair cycles,
    /// generated tokens, feedback latency, invented symbols, and the unrelated-edit
    /// rate, in a machine format as a protocol item and in human mode as a table.
    Report {
        /// The pinned task-set file (JSON) the run was produced against.
        #[arg(value_name = "TASKS")]
        tasks: PathBuf,
        /// The recorded benchmark-run file (JSON) to score.
        #[arg(value_name = "RUN")]
        run: PathBuf,
    },
}

/// The `tuo corpus` commands: drive the compiler-validated corpus pipeline.
#[derive(Debug, Subcommand)]
enum CorpusCommand {
    /// Validate one or more source files through the full corpus pipeline.
    ///
    /// Runs the required, ordered gauntlet — format → parse → resolve → type
    /// check → ownership → MIR verify → specs/tests → native execution — driving
    /// the real compiler stages, and reports the per-stage results together with
    /// the entry's metadata (language version, origin, features, complexity, and
    /// token counts). `--category` names which corpus contract to check against
    /// (default `correct`); the command *proves* the candidate meets it rather
    /// than trusting the label. In a machine format the full metadata is emitted
    /// as a protocol item.
    Validate {
        /// Which corpus contract to validate against.
        #[arg(long, value_enum, default_value_t = CorpusCategory::Correct)]
        category: CorpusCategory,
        /// The candidate's stated source origin (recorded in the metadata).
        #[arg(long, value_enum, default_value_t = CorpusOrigin::Human)]
        origin: CorpusOrigin,
        /// The tuonelang source files forming the candidate program.
        #[arg(required = true, value_name = "FILE")]
        files: Vec<PathBuf>,
    },
}

/// The `tuo package` queries: read-only projections of a resolved package.
#[derive(Debug, Subcommand)]
enum PackageCommand {
    /// List a package's public, module-level symbols.
    ///
    /// Resolves and compiles the package's real sources and reports the actual
    /// exported functions, structs, and enums the front end produced — the same
    /// symbols the agent protocol and LSP project, so a tool can query what a
    /// package offers **without guessing**. Available only in a machine format
    /// (`--message-format=json`); it is a protocol for tools, not a human
    /// report.
    Symbols {
        /// The directory holding the package manifest (default `.`).
        #[arg(long, value_name = "DIR")]
        manifest: Option<PathBuf>,
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
            Some(Command::Check { files, manifest }) => {
                if files.is_empty() {
                    package::check(&manifest_dir(manifest), mode)
                } else {
                    check::run(&files, mode)
                }
            }
            Some(Command::Spec { target, files }) => spec::run(target, &files, mode),
            Some(Command::Verify {
                affected_by,
                files,
                manifest,
            }) => {
                if files.is_empty() {
                    package::verify(&manifest_dir(manifest), mode)
                } else {
                    spec::verify(affected_by.as_deref(), &files, mode)
                }
            }
            Some(Command::Build {
                output,
                release,
                files,
                manifest,
            }) => {
                if files.is_empty() {
                    package::build(&manifest_dir(manifest), output, release, mode)
                } else {
                    codegen::build(output, release, &files, mode)
                }
            }
            Some(Command::Run { release, files }) => codegen::run(release, &files, mode),
            Some(Command::New { name, path }) => package::new(&name, path, mode),
            Some(Command::Add {
                name,
                path,
                manifest,
            }) => package::add(&name, &path, &manifest_dir(manifest), mode),
            Some(Command::Remove { name, manifest }) => {
                package::remove(&name, &manifest_dir(manifest), mode)
            }
            Some(Command::Test { manifest }) => package::test(&manifest_dir(manifest), mode),
            Some(Command::Package(PackageCommand::Symbols { manifest })) => {
                package::symbols(&manifest_dir(manifest), mode)
            }
            Some(Command::Corpus(CorpusCommand::Validate {
                category,
                origin,
                files,
            })) => corpus::validate(category, origin, &files, mode),
            Some(Command::Bench(BenchCommand::Report { tasks, run })) => {
                bench::report(&tasks, &run, mode)
            }
            // The agent protocol is its own versioned JSON-lines contract on
            // stdio, independent of `--message-format` (which governs the
            // result-producing commands' output). `mode` is passed only so
            // internal logging honors `--log`.
            Some(Command::Cheatsheet) => cheatsheet::run(mode),
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

/// The directory a package command operates on: the given `--manifest <dir>`,
/// or the current directory when omitted.
fn manifest_dir(manifest: Option<PathBuf>) -> PathBuf {
    manifest.unwrap_or_else(|| PathBuf::from("."))
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
