//! The standard library, really compiled and really run.
//!
//! `tuo-stdlib` is a catalog of `.tuo` source; this suite is the proof that the
//! catalog is *true*. It loads every module through the real front end and
//! asserts:
//!
//!   * every module parses, resolves, type-checks, and ownership-checks with
//!     **zero** errors — the whole library, and each module on its own;
//!   * every `spec` in the library **executes** through the reference
//!     interpreter and **passes**, with **no** skipped specs (a skip would mean
//!     the library shipped a spec the executable subset cannot run — dishonest)
//!     — including the ADR-0008 Tier 1 higher-order combinators
//!     (`std::collections::{fold,map_into,filter_into,any,all}`), whose specs
//!     pass a named top-level `fn` as a first-class value and run indirect
//!     calls through the reference interpreter;
//!   * the catalog's machine-queryable surface is coherent (every module is
//!     reachable by path, the count is what the prompt asked for);
//!   * the **three-tier rule** holds textually for every public function: a
//!     pure executable function is exercised by a spec; an `EFFECT:` function
//!     (ADR-0006 — implemented over `std::rt`, effectful, so a spec is
//!     impossible by `R0007`) has **no** spec but names its native CLI test;
//!     and a `CONTRACT:` function has **no** spec (nothing claims to run that
//!     cannot); and
//!   * the effect tier **really performs its effects**: `std::io::println`
//!     prints exactly, and `std::process::exit` terminates with the status's
//!     code, through real native binaries built by the actual `tuo` binary on
//!     both backends (Cranelift and `--release` LLVM) — the executable pin the
//!     `EFFECT:` docs point at.
//!
//! Because these tests compile the exact source `tuo-stdlib` embeds, the
//! library cannot drift from its promises without turning this suite red.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use tuo_compiler::check_sources;
use tuo_compiler::source::{SourceId, SourceMap};
use tuo_spec::{Limits, RunOutcome, Selection};

/// Intern every catalog module into a fresh source map, returning the map and
/// the source ids in catalog order.
fn load_all() -> (SourceMap, Vec<SourceId>) {
    let mut map = SourceMap::new();
    let mut sources = Vec::new();
    for module in tuo_stdlib::MODULES {
        let file = map.intern_file(module.name);
        let id = map
            .add_source(file, module.source)
            .expect("a stdlib module is not too large");
        sources.push(id);
    }
    (map, sources)
}

/// Intern one module into a fresh source map.
fn load_one(module: tuo_stdlib::Module) -> (SourceMap, SourceId) {
    let mut map = SourceMap::new();
    let file = map.intern_file(module.name);
    let id = map
        .add_source(file, module.source)
        .expect("a stdlib module is not too large");
    (map, id)
}

#[test]
fn the_catalog_lists_exactly_its_modules() {
    // The eight initial modules the prompt named, plus the two pure modules
    // grown on top of the runnable core: `std::math` (Int/Float arithmetic) and
    // `std::str` (byte-level string algorithms + integer⇄text conversion).
    let expected = [
        "std::core",
        "std::collections",
        "std::math",
        "std::str",
        "std::io",
        "std::fs",
        "std::time",
        "std::process",
        "std::sync",
        "std::test",
    ];
    let paths: Vec<&str> = tuo_stdlib::MODULES.iter().map(|m| m.path).collect();
    for path in expected {
        assert!(paths.contains(&path), "catalog is missing {path}");
        assert!(
            tuo_stdlib::module(path).is_some(),
            "{path} is not reachable by lookup"
        );
    }
    assert_eq!(
        tuo_stdlib::MODULES.len(),
        expected.len(),
        "the catalog holds exactly its listed modules"
    );
}

#[test]
fn every_module_checks_cleanly_on_its_own() {
    // Each module must stand alone: no module depends on another today, so each
    // type-checks in isolation with zero errors.
    for &module in tuo_stdlib::MODULES {
        let (map, id) = load_one(module);
        let result = check_sources(&map, &[id]);
        assert!(
            !result.has_errors(),
            "{} has front-end errors:\n{:#?}",
            module.path,
            result
                .diagnostics
                .iter()
                .filter(|d| d.severity == tuo_compiler::diagnostics::Severity::Error)
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn the_whole_library_checks_cleanly_together() {
    // Loaded as one program, the modules must not collide: no duplicate
    // top-level definition, no cross-module resolution error.
    let (map, sources) = load_all();
    let result = check_sources(&map, &sources);
    assert!(
        !result.has_errors(),
        "the standard library does not check as one program:\n{:#?}",
        result
            .diagnostics
            .iter()
            .filter(|d| d.severity == tuo_compiler::diagnostics::Severity::Error)
            .collect::<Vec<_>>()
    );
}

#[test]
fn every_spec_in_the_library_runs_and_passes() {
    // The executable promise: every spec the library ships runs through the
    // interpreter and passes — and nothing is skipped. A skip would mean a spec
    // that the v0 executable subset cannot run slipped in; the library must not
    // ship one (the effect- and contract-tier functions are deliberately
    // unspecced — an effectful spec would be an `R0007` front-end error, and a
    // contract has nothing to run).
    let (map, sources) = load_all();
    match tuo_spec::run(&map, &sources, &Selection::All, Limits::default()) {
        RunOutcome::Ran(report) => {
            assert!(
                report.skipped.is_empty(),
                "the standard library shipped a spec the executable subset skips: {:#?}",
                report.skipped
            );
            assert!(
                report.ran() > 0,
                "the standard library must ship executable specs"
            );
            assert!(
                report.passed(),
                "a standard-library spec failed ({} of {} specs):\n{:#?}",
                report.failures(),
                report.ran(),
                report
                    .runs
                    .iter()
                    .filter(|r| !r.passed())
                    .collect::<Vec<_>>()
            );
        }
        RunOutcome::NotChecked(diagnostics) => {
            panic!("the standard library did not check, so no spec ran:\n{diagnostics:#?}");
        }
    }
}

/// The doc tier a public function is marked with in its module source.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tier {
    /// Pure computation: runs, and must be exercised by a spec.
    Pure,
    /// `EFFECT:` — implemented over `std::rt`, effectful; no spec is possible
    /// (`R0007`), so the doc must name the native CLI test that pins it.
    Effect,
    /// `CONTRACT:` — the needed primitive does not exist yet; no spec.
    Contract,
}

/// One public function of a module: its name, tier, and doc-comment text.
struct PublicFn {
    name: String,
    tier: Tier,
    doc: String,
}

/// Extract every `pub fn` with its immediately preceding `///` doc block.
fn public_fns(source: &str) -> Vec<PublicFn> {
    let mut fns = Vec::new();
    let mut doc = String::new();
    for line in source.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("///") {
            doc.push_str(trimmed);
            doc.push('\n');
        } else if let Some(rest) = trimmed.strip_prefix("pub fn ") {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            let tier = if doc.contains("EFFECT") {
                Tier::Effect
            } else if doc.contains("CONTRACT") {
                Tier::Contract
            } else {
                Tier::Pure
            };
            fns.push(PublicFn {
                name,
                tier,
                doc: std::mem::take(&mut doc),
            });
        } else if !trimmed.is_empty() {
            doc.clear();
        }
    }
    fns
}

/// The concatenated text of every `spec … { … }` block in a module source,
/// comments stripped (so a prose mention of a function name never counts as a
/// spec exercising it).
fn spec_block_text(source: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    let mut in_spec = false;
    for line in source.lines() {
        let trimmed = line.trim_start();
        let code = trimmed.split("//").next().unwrap_or("");
        if !in_spec && trimmed.starts_with("spec ") {
            in_spec = true;
        }
        if in_spec {
            out.push_str(code);
            out.push('\n');
            depth += code.matches('{').count();
            depth = depth.saturating_sub(code.matches('}').count());
            if depth == 0 && code.contains('}') {
                in_spec = false;
            }
        }
    }
    out
}

/// Is `name` called (as `name(` with a non-identifier character before it)
/// anywhere in `text`?
fn is_called_in(name: &str, text: &str) -> bool {
    let needle = format!("{name}(");
    let bytes = text.as_bytes();
    let mut from = 0;
    while let Some(pos) = text[from..].find(&needle) {
        let at = from + pos;
        let preceded_by_ident = at > 0 && {
            let b = bytes[at - 1];
            b.is_ascii_alphanumeric() || b == b'_'
        };
        if !preceded_by_ident {
            return true;
        }
        from = at + 1;
    }
    false
}

#[test]
fn the_three_tier_rule_holds_for_every_public_function() {
    // The library's honesty contract, per function:
    //   pure executable  => exercised by a spec in its module;
    //   EFFECT:          => no spec (R0007 makes one impossible) but the doc
    //                       names the native CLI test that pins it;
    //   CONTRACT:        => no spec (nothing claims to run that cannot).
    for &module in tuo_stdlib::MODULES {
        let fns = public_fns(module.source);
        assert!(!fns.is_empty(), "{} exports no public fn", module.path);
        let specs = spec_block_text(module.source);
        for public in fns {
            match public.tier {
                Tier::Pure => {
                    assert!(
                        is_called_in(&public.name, &specs),
                        "{}::{} is pure executable but no spec exercises it",
                        module.path,
                        public.name
                    );
                }
                Tier::Effect => {
                    assert!(
                        !is_called_in(&public.name, &specs),
                        "{}::{} is EFFECT-tier but appears in a spec — an \
                         effectful spec is an R0007 error",
                        module.path,
                        public.name
                    );
                    assert!(
                        public.doc.contains("crates/tuo-cli/tests/stdlib.rs"),
                        "{}::{} is EFFECT-tier but its doc does not name the \
                         native CLI test that pins it",
                        module.path,
                        public.name
                    );
                }
                Tier::Contract => {
                    assert!(
                        !is_called_in(&public.name, &specs),
                        "{}::{} is CONTRACT-tier but appears in a spec — \
                         nothing may claim to run that cannot",
                        module.path,
                        public.name
                    );
                }
            }
        }
    }
}

#[test]
fn the_effect_tier_is_exactly_io_writes_reads_and_process_exit() {
    // ADR-0006 landed descriptor writes/reads and process exit; ADR-0009 landed
    // the allocator, which lets `read_line` accumulate the bytes `read_byte`
    // yields into an owned `String`. The effect tier must list exactly the
    // functions those primitives can implement — no more (an over-claim) and no
    // fewer (a stale contract).
    let mut effect_fns = Vec::new();
    for &module in tuo_stdlib::MODULES {
        for public in public_fns(module.source) {
            if public.tier == Tier::Effect {
                effect_fns.push(format!("{}::{}", module.path, public.name));
            }
        }
    }
    effect_fns.sort();
    assert_eq!(
        effect_fns,
        vec![
            "std::io::print".to_string(),
            "std::io::println".to_string(),
            "std::io::read_line".to_string(),
            "std::process::exit".to_string(),
        ]
    );
}

#[test]
fn each_module_runs_its_own_specs_green() {
    // A per-module view of the same guarantee, so a failure names the module.
    for &module in tuo_stdlib::MODULES {
        let (map, id) = load_one(module);
        match tuo_spec::run(&map, &[id], &Selection::All, Limits::default()) {
            RunOutcome::Ran(report) => {
                assert!(
                    report.skipped.is_empty(),
                    "{} skipped a spec: {:#?}",
                    module.path,
                    report.skipped
                );
                assert!(
                    report.passed(),
                    "{} has a failing spec:\n{:#?}",
                    module.path,
                    report
                        .runs
                        .iter()
                        .filter(|r| !r.passed())
                        .collect::<Vec<_>>()
                );
            }
            RunOutcome::NotChecked(diagnostics) => {
                panic!("{} did not check:\n{diagnostics:#?}", module.path);
            }
        }
    }
}

// ----------------------------------------------------------------------------
// The effect tier, really performed: native binaries through the real `tuo`
// binary, on both backends. These are the tests the `EFFECT:` docs name.
// ----------------------------------------------------------------------------

/// A unique scratch directory per test, rooted under Cargo's per-crate temp
/// directory (so parallel tests never collide).
fn native_workspace(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("stdlib_native")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch workspace is creatable");
    dir
}

/// Write a stdlib module plus a caller program into `dir` and `tuo run` them
/// together with the chosen backend, returning the completed process output.
fn run_with_module(dir: &Path, module: tuo_stdlib::Module, caller: &str, release: bool) -> Output {
    let module_path = dir.join(module.name.replace('/', "_"));
    std::fs::write(&module_path, module.source).expect("module source is writable");
    let caller_path = dir.join("caller.tuo");
    std::fs::write(&caller_path, caller).expect("caller source is writable");
    let mut command = Command::new(env!("CARGO_BIN_EXE_tuo"));
    command.arg("run");
    if release {
        command.arg("--release");
    }
    command
        .arg(&module_path)
        .arg(&caller_path)
        .output()
        .expect("the tuo binary runs")
}

/// The backend a `release` flag selects, for failure messages.
fn backend_name(release: bool) -> &'static str {
    if release { "llvm" } else { "cranelift" }
}

/// `std::io::println("hi")` really prints `hi\n` — exactly — and returns
/// `Ok { value: 3 }`, whose payload `main` surfaces as the exit status. Both
/// backends. This is the native pin `std::io`'s `EFFECT:` docs name.
#[test]
fn stdlib_println_really_prints_natively() {
    let dir = native_workspace("println");
    let caller = "\
module caller;

import std::io;

fn main() -> Int {
    match std::io::println(\"hi\") {
        Ok { value } => value,
        Err { error } => 100 + std::io::error_code(error),
    }
}
";
    for release in [false, true] {
        let output = run_with_module(&dir, tuo_stdlib::IO, caller, release);
        let which = backend_name(release);
        assert_eq!(
            output.status.code(),
            Some(3),
            "{which}: println(\"hi\") returns Ok {{ value: 3 }} (2 text bytes + \
             the newline); stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "hi\n",
            "{which}: the printed bytes must land on stdout, exactly"
        );
    }
}

/// `std::process::exit(failure(9))` really terminates the process with status
/// 9: the trailing `0` in `main` is never reached and nothing is printed. Both
/// backends. This is the native pin `std::process`'s `EFFECT:` doc names.
#[test]
fn stdlib_process_exit_really_exits_natively() {
    let dir = native_workspace("process_exit");
    let caller = "\
module caller;

import std::process;

fn main() -> Int {
    std::process::exit(std::process::failure(9));
    0
}
";
    for release in [false, true] {
        let output = run_with_module(&dir, tuo_stdlib::PROCESS, caller, release);
        let which = backend_name(release);
        assert_eq!(
            output.status.code(),
            Some(9),
            "{which}: exit(failure(9)) is the process status, not the 0 main \
             would return; stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "",
            "{which}: nothing may land on stdout"
        );
    }
}

/// Write a stdlib module plus a caller program into `dir`, `tuo run` them with
/// `stdin` piped into the built binary's standard input, and return the output.
/// This is the read path a user drives; the effect being pinned is that the
/// program really *reads* what is fed to it.
fn run_with_module_stdin(
    dir: &Path,
    module: tuo_stdlib::Module,
    caller: &str,
    stdin: &[u8],
    release: bool,
) -> Output {
    let module_path = dir.join(module.name.replace('/', "_"));
    std::fs::write(&module_path, module.source).expect("module source is writable");
    let caller_path = dir.join("caller.tuo");
    std::fs::write(&caller_path, caller).expect("caller source is writable");
    let mut command = Command::new(env!("CARGO_BIN_EXE_tuo"));
    command.arg("run");
    if release {
        command.arg("--release");
    }
    let mut child = command
        .arg(&module_path)
        .arg(&caller_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the tuo binary spawns");
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(stdin)
        .expect("stdin is writable");
    child.wait_with_output().expect("the tuo binary completes")
}

/// `std::io::read_line()` really reads a line from stdin and builds it as an
/// owned `String` (ADR-0009): the program echoes the line back through
/// `std::rt::write_string`, so the captured stdout is the exact line (newline
/// stripped) and the exit status is its byte length. It also proves the EOF
/// path (`Err { IoError::Eof }` on empty input) and the no-trailing-newline
/// path. Both backends. This is the native pin `std::io`'s `read_line`
/// `EFFECT:` doc names.
#[test]
fn stdlib_read_line_really_reads_natively() {
    let caller = "\
module caller;

import std::io;

fn main() -> Int {
    match std::io::read_line() {
        Ok { value } => std::rt::write_string(1, value),
        Err { error } => 200 + std::io::error_code(error),
    }
}
";
    for release in [false, true] {
        let which = backend_name(release);
        let dir = native_workspace(&format!("read_line_{which}"));

        // A full line: read "hello, heap" (11 bytes), the trailing newline is
        // consumed but not part of the value; a second line is left unread.
        let output = run_with_module_stdin(
            &dir,
            tuo_stdlib::IO,
            caller,
            b"hello, heap\nsecond line\n",
            release,
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "hello, heap",
            "{which}: read_line must echo the first line without its newline"
        );
        assert_eq!(
            output.status.code(),
            Some(11),
            "{which}: the echoed line is 11 bytes, surfaced as the exit status; stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );

        // No trailing newline: the final bytes are still a line.
        let output = run_with_module_stdin(&dir, tuo_stdlib::IO, caller, b"abc", release);
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "abc",
            "{which}: a line without a trailing newline is still read"
        );
        assert_eq!(output.status.code(), Some(3), "{which}: 3 bytes read");

        // Empty input: the very first read is EOF, so `Err { Eof }` (code 0),
        // surfaced as 200 + 0 = 200, and nothing is written.
        let output = run_with_module_stdin(&dir, tuo_stdlib::IO, caller, b"", release);
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "",
            "{which}: EOF writes nothing"
        );
        assert_eq!(
            output.status.code(),
            Some(200),
            "{which}: empty stdin is Err {{ IoError::Eof }} (200 + code 0); stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
