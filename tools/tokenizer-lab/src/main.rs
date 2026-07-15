//! `tokenizer-lab` — measure how candidate tuonelang syntax tokenizes.
//!
//! The lab reads a version-controlled fixture file of syntax candidates and
//! measures each candidate against **every** built-in tokenizer adapter, then
//! emits a machine-readable JSON report (and, optionally, a human-readable
//! table). Measuring across multiple tokenizers is the whole point: it prevents
//! tuonelang syntax from being tuned to a single tokenizer's quirks.
//!
//! This binary is a thin front-end. All measurement logic and the tokenizer
//! adapter interface live in the `tuo-bench` crate, so adding a new tokenizer
//! never touches this file (see the crate README).
//!
//! # Usage
//!
//! ```text
//! tokenizer-lab run --fixtures <file.json> [--out <report.json>] [--table]
//! tokenizer-lab list-tokenizers
//! ```

// This is a user-facing CLI whose purpose is to print reports to stdout/stderr.
// The workspace lints warn on prints in library code; here they are the intended
// output surface, so we allow them at the crate root only.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use tuo_bench::fixtures::FixtureFile;
use tuo_bench::measure::{self, MeasurementReport};
use tuo_bench::tokenizer::Registry;

/// Command-line interface for the tokenizer lab.
#[derive(Debug, Parser)]
#[command(
    name = "tokenizer-lab",
    version,
    about = "Measure how candidate tuonelang syntax tokenizes across multiple tokenizers."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run measurements over a fixture file.
    Run {
        /// Path to a syntax-comparison fixture JSON file.
        #[arg(long)]
        fixtures: PathBuf,
        /// Write the JSON report to this path instead of stdout.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Also print a human-readable comparison table to stderr.
        #[arg(long)]
        table: bool,
    },
    /// List the built-in tokenizer adapters and exit.
    ListTokenizers,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Run {
            fixtures,
            out,
            table,
        } => run(&fixtures, out.as_deref(), table),
        Command::ListTokenizers => {
            list_tokenizers();
            Ok(())
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("tokenizer-lab: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Print the built-in tokenizer adapters.
fn list_tokenizers() {
    let registry = Registry::with_builtin_adapters();
    for tok in registry.iter() {
        println!("{:<12} {}", tok.id(), tok.description());
    }
}

/// Run measurements and emit the report.
fn run(
    fixtures: &std::path::Path,
    out: Option<&std::path::Path>,
    table: bool,
) -> Result<(), String> {
    let text = std::fs::read_to_string(fixtures)
        .map_err(|e| format!("reading fixtures {}: {e}", fixtures.display()))?;
    let fixture_file =
        FixtureFile::from_json(&text).map_err(|e| format!("parsing fixtures: {e}"))?;

    let registry = Registry::with_builtin_adapters();
    let report = measure::run(&registry, &fixture_file);

    let json = report
        .to_json_pretty()
        .map_err(|e| format!("serializing report: {e}"))?;

    match out {
        Some(path) => {
            std::fs::write(path, format!("{json}\n"))
                .map_err(|e| format!("writing {}: {e}", path.display()))?;
            eprintln!("wrote report to {}", path.display());
        }
        None => println!("{json}"),
    }

    if table {
        print_table(&report);
    }

    Ok(())
}

/// Print a compact per-group comparison table to stderr.
///
/// The table shows token counts per tokenizer for each candidate. It is a
/// convenience for humans; the JSON report is the source of truth.
fn print_table(report: &MeasurementReport) {
    eprintln!();
    for group in &report.groups {
        eprintln!("# {} — {}", group.id, group.question);
        // Header: candidate label, then one column per tokenizer.
        eprint!("  {:<24}", "candidate");
        for tok in &report.tokenizers {
            eprint!(" {tok:>12}");
        }
        eprintln!();
        for cand in &group.candidates {
            eprint!("  {:<24}", cand.label);
            for m in &cand.measurements {
                eprint!(" {:>12}", m.token_count);
            }
            eprintln!();
        }
        eprintln!();
    }
    eprintln!(
        "note: token count is one input to syntax choices, not the deciding factor. \
         See tools/tokenizer-lab/README.md."
    );
}
