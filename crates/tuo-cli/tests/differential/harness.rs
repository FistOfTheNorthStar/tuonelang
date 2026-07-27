//! The cross-engine differential harness.
//!
//! A tuonelang program has one reference meaning: what the MIR interpreter
//! ([`tuo_mir_interp`]) computes for its verified MIR. A native backend must
//! agree with that meaning instruction for instruction; where they diverge the
//! backend is wrong (never the interpreter). This module is the reusable engine
//! that pins the agreement: given a program's source it computes the reference
//! [`Outcome`] with the interpreter, produces the native [`Outcome`] by building
//! and running the real `tuo` binary, and compares the two on every behavior the
//! language makes observable today.
//!
//! # What is observable
//!
//! The scalar, control-flow core the backend lowers exposes exactly two
//! observable behaviors, and the harness compares both:
//!
//! * the **return value** of `main` — an integer, surfaced natively as the
//!   process exit status (the v0 entry ABI truncates it to a byte, exactly as a
//!   C `main` return becomes the exit code), and
//! * **deterministic traps** — a divide-by-zero or integer overflow aborts the
//!   interpreter and, natively, terminates with the runtime's fixed trap status
//!   ([`tuo_runtime::TRAP_EXIT_STATUS`]); the two must agree on *which* happens.
//!
//! Two further behaviors named by the differential charter — **stdout** and
//! **observable destruction** — are not yet expressible: the language has no I/O
//! facility, and the backend lowers no type with a destructor. [`Outcome`]
//! carries an explicit place for stdout so the comparison extends the day an
//! output facility lands, and the design note in the suite root records why
//! destruction is out of reach for now. The harness never fabricates a behavior
//! the compiler cannot perform.

// This is a test-support module: its `pub` items are the API the differential
// test binary consumes, but the linter cannot see across the `#[path]` module
// boundary that they are reachable. `pub` is the right, readable visibility here.
#![allow(unreachable_pub)]

use std::path::Path;
use std::process::Command;

use tuo_compiler::source::SourceMap;
use tuo_mir_interp::{Interpreter, RuntimeError, Value};

/// The runtime's fixed trap exit status (mirrors [`tuo_runtime::TRAP_EXIT_STATUS`]).
pub const TRAP_EXIT_STATUS: i32 = tuo_runtime::TRAP_EXIT_STATUS;

/// The complete observable result of running a program through one engine.
///
/// Equality on `Outcome` *is* the differential comparison: two engines agree iff
/// their outcomes are equal. The value is normalized to the byte a process exit
/// status can carry, so the interpreter's wide integer and the native exit code
/// compare on the same footing.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Outcome {
    /// `main` returned normally with this exit byte (the returned integer masked
    /// to its low 8 bits, the width a process exit status carries).
    Returned {
        /// The exit byte: `value & 0xff`.
        exit_byte: i32,
        /// Anything the program wrote to stdout. Always empty today (no I/O
        /// facility exists); present so the comparison covers stdout the moment
        /// one does.
        stdout: Vec<u8>,
    },
    /// The program aborted with the runtime's fixed trap status. Both engines
    /// must reach this together; the interpreter additionally names *why*
    /// ([`Self::trap_label`]), checked when both sides are traps.
    Trapped {
        /// The stable trap label the interpreter reported (e.g.
        /// `"division_by_zero"`), or `None` for the native side, which observes
        /// only that the process aborted with the trap status.
        label: Option<String>,
    },
}

impl Outcome {
    /// A normal return with the given (already-masked) exit byte and no stdout.
    fn returned(exit_byte: i32) -> Self {
        Self::Returned {
            exit_byte,
            stdout: Vec::new(),
        }
    }

    /// Whether the two outcomes agree. A trap agrees with a trap regardless of
    /// the label (the native side cannot observe the label); a return agrees
    /// with a return iff the exit byte and stdout match.
    #[must_use]
    pub fn agrees_with(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Trapped { .. }, Self::Trapped { .. }) => true,
            (
                Self::Returned {
                    exit_byte: a,
                    stdout: sa,
                },
                Self::Returned {
                    exit_byte: b,
                    stdout: sb,
                },
            ) => a == b && sa == sb,
            _ => false,
        }
    }

    /// A short human description for divergence reports.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Returned { exit_byte, stdout } if stdout.is_empty() => {
                format!("returned exit byte {exit_byte}")
            }
            Self::Returned { exit_byte, stdout } => {
                format!(
                    "returned exit byte {exit_byte} with {} stdout bytes",
                    stdout.len()
                )
            }
            Self::Trapped { label: Some(label) } => format!("trapped ({label})"),
            Self::Trapped { label: None } => "trapped".to_owned(),
        }
    }
}

/// A recorded disagreement between the two engines on one program.
#[derive(Clone, Debug)]
pub struct Divergence {
    /// The program source the engines disagreed on.
    pub source: String,
    /// What the reference interpreter computed.
    pub interpreter: Outcome,
    /// What the native binary produced.
    pub native: Outcome,
}

impl Divergence {
    /// A full, copy-pasteable report of the disagreement.
    #[must_use]
    pub fn report(&self) -> String {
        format!(
            "cross-engine divergence — the native backend disagrees with the reference \
             interpreter:\n  interpreter: {}\n  native:      {}\n--- program ---\n{}\n---------------",
            self.interpreter.describe(),
            self.native.describe(),
            self.source.trim_end(),
        )
    }
}

/// Whether `source` is accepted by the front end (parses, resolves, type- and
/// ownership-checks). The generator only emits accepted programs, but the
/// shrinker proposes textual reductions that may not type-check, so it gates
/// each candidate through this before treating it as a valid repro.
#[must_use]
pub fn accepts(source: &str) -> bool {
    let mut map = SourceMap::new();
    let file = map.intern_file("differential.tuo");
    let Ok(id) = map.add_source(file, source) else {
        return false;
    };
    !tuo_compiler::check_sources(&map, &[id]).has_errors()
}

/// Compute the reference outcome of `source` with the MIR interpreter.
///
/// Panics if the program does not type-check or its MIR does not verify — those
/// are generator or front-end bugs, not differential findings, and the caller
/// only ever hands the harness programs it has already accepted.
#[must_use]
pub fn interpret(source: &str) -> Outcome {
    let mut map = SourceMap::new();
    let file = map.intern_file("differential.tuo");
    let id = map.add_source(file, source).expect("source interns");

    let check = tuo_compiler::check_sources(&map, &[id]);
    assert!(
        !check.has_errors(),
        "the harness only runs accepted programs; this one has front-end errors:\n{source}"
    );
    let parse = tuo_compiler::parser::parse(map.source(id));
    let asts = [tuo_compiler::ast::Ast::new(
        &parse.tree,
        map.source(id).text(),
    )];
    let hir = tuo_compiler::hir::lower(&asts, &check.resolution);
    let program = tuo_compiler::mir::lower(&hir, &check.resolution, &check.types);
    let interpreter =
        Interpreter::new(&program, &check.types).expect("accepted program lowers to verified MIR");

    match interpreter.run("main", Vec::new()) {
        Ok(Value::Int(value, _)) => Outcome::returned((value & 0xff) as i32),
        Ok(other) => panic!(
            "differential `main` must return an integer; got {}",
            other.render()
        ),
        Err(RuntimeError { kind, .. }) => Outcome::Trapped {
            label: Some(kind.label().to_owned()),
        },
    }
}

/// Compute the native outcome of the program at `path` with the real `tuo`
/// binary: `tuo run <path>`, classified by exit status.
///
/// A native run either returns (exit byte) or traps (the fixed trap status). Any
/// other exit — a build refusal, a signal — is a harness/environment failure,
/// not a differential finding, and panics with the captured stderr so the cause
/// is visible.
#[must_use]
pub fn run_native(path: &Path) -> Outcome {
    let output = Command::new(env!("CARGO_BIN_EXE_tuo"))
        .arg("run")
        .arg(path)
        .output()
        .expect("the tuo binary runs");
    let code = output
        .status
        .code()
        .expect("the native process exits with a code, not a signal");

    if code == TRAP_EXIT_STATUS {
        return Outcome::Trapped { label: None };
    }
    // A `tuo run` that neither returns a program value nor traps did not actually
    // run the program — most likely the backend *refused* it (a construct outside
    // the v0 subset). A refusal is a harness-precondition violation, not a
    // differential finding: the caller must only submit programs inside the
    // subset, so it is surfaced loudly rather than mistaken for a return of the
    // build's failure exit code. The refusal is identified by the backend's own
    // "subset"/"does not lower" marker on stderr (the same signal
    // `codegen_differential` checks), not by the exit code, which a program is
    // free to return deliberately.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("subset") && !stderr.contains("does not lower"),
        "`tuo run` refused the program as outside the backend subset (exit {code}); \
         the generator must only emit programs inside it. stderr:\n{stderr}"
    );
    Outcome::returned(code)
}

/// Run one program through both engines and return `Some(Divergence)` iff they
/// disagree. The program must already be accepted and inside the backend subset;
/// the harness writes it to `path` for the native run.
#[must_use]
pub fn diff_program(source: &str, path: &Path) -> Option<Divergence> {
    std::fs::write(path, source).expect("scratch program is writable");
    let interpreter = interpret(source);
    let native = run_native(path);
    if interpreter.agrees_with(&native) {
        None
    } else {
        Some(Divergence {
            source: source.to_owned(),
            interpreter,
            native,
        })
    }
}
