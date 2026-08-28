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
    // grown on top of the runnable core — `std::math` (Int/Float arithmetic)
    // and `std::str` (byte-level string algorithms + integer⇄text
    // conversion) — plus `std::net` (ADR-0014's TCP socket tier).
    let expected = [
        "std::core",
        "std::collections",
        "std::math",
        "std::str",
        "std::json",
        "std::io",
        "std::fs",
        "std::net",
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
fn the_effect_tier_is_exactly_the_os_boundary_wrappers() {
    // ADR-0006 landed descriptor writes/reads and process exit; ADR-0009 landed
    // the allocator, which lets `read_line` accumulate the bytes `read_byte`
    // yields into an owned `String`; ADR-0007 landed structured fork-join;
    // ADR-0013 landed the clock, argv, and file open/close/remove primitives,
    // which made the whole of `std::fs`'s disk tier, `std::process`'s argv
    // pair, and `std::time::now` real; and ADR-0014 landed the socket
    // primitives, which made the whole of `std::net`'s TCP tier real;
    // ADR-0017 added the bounded-wait counterparts to the three operations
    // that otherwise block forever, plus the IPv6 server-side pair and the
    // UDP datagram tier. The
    // effect tier must list exactly the functions those primitives can
    // implement — no more (an over-claim) and no fewer (a stale contract).
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
            "std::fs::exists".to_string(),
            "std::fs::read".to_string(),
            "std::fs::remove".to_string(),
            "std::fs::write".to_string(),
            "std::io::print".to_string(),
            "std::io::println".to_string(),
            "std::io::read_line".to_string(),
            "std::net::accept".to_string(),
            "std::net::accept_timeout".to_string(),
            "std::net::bound_port".to_string(),
            "std::net::close".to_string(),
            "std::net::connect".to_string(),
            "std::net::connect_timeout".to_string(),
            "std::net::listen".to_string(),
            "std::net::listen6".to_string(),
            "std::net::peer_family".to_string(),
            "std::net::read_byte_timeout".to_string(),
            "std::net::udp_bind".to_string(),
            "std::net::udp_byte_at".to_string(),
            "std::net::udp_peer_port".to_string(),
            "std::net::udp_recv".to_string(),
            "std::net::udp_send".to_string(),
            "std::process::arg".to_string(),
            "std::process::arg_count".to_string(),
            "std::process::exit".to_string(),
            "std::sync::channel".to_string(),
            "std::sync::close".to_string(),
            "std::sync::lock".to_string(),
            "std::sync::mutex".to_string(),
            "std::sync::par_map".to_string(),
            "std::sync::recv".to_string(),
            "std::sync::send".to_string(),
            "std::sync::unlock".to_string(),
            "std::time::now".to_string(),
        ]
    );
}

#[test]
fn the_contract_tier_is_empty() {
    // ADR-0015 discharged the last CONTRACT stubs (`std::sync::lock`/
    // `unlock`): the library no longer advertises anything it cannot run. A
    // future contract may enter honestly (marked `CONTRACT:`, no spec), but
    // one reappearing silently would be a regression this pin reports.
    for &module in tuo_stdlib::MODULES {
        for public in public_fns(module.source) {
            assert_ne!(
                public.tier,
                Tier::Contract,
                "{}::{} is CONTRACT-tier, but the contract tier has been \
                 empty since ADR-0015 — either implement it or update this \
                 pin deliberately",
                module.path,
                public.name
            );
        }
    }
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

/// `std::sync::par_map(square, [1..=6], 3)` really forks, computes, and joins
/// on real OS threads, returning the squares in task order: the summed result
/// is `main`'s exit status. Both backends. This is the native pin
/// `std::sync`'s `EFFECT:` doc names (ADR-0007).
#[test]
fn par_map_runs_natively() {
    let dir = native_workspace("par_map");
    let caller = "\
module caller;

import std::sync;

fn square(take x: Int) -> Int {
    x * x
}

fn main() -> Int {
    var tasks = std::array::empty();
    var i = 1;
    while i <= 6 {
        std::array::push(tasks, i);
        i = i + 1;
    }
    let results = std::sync::par_map(square, tasks, 3);
    var total = 0;
    var j = 0;
    while j < std::array::len(results) {
        total = total + std::array::get(results, j);
        j = j + 1;
    }
    total
}
";
    for release in [false, true] {
        let output = run_with_module(&dir, tuo_stdlib::SYNC, caller, release);
        let which = backend_name(release);
        assert_eq!(
            output.status.code(),
            Some(91),
            "{which}: par_map(square, [1..6], 3) sums to 1+4+9+16+25+36 = 91 \
             in task order; stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
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

/// Write a stdlib module plus a caller program into `dir` and `tuo build`
/// them into an executable, returning its path — so a test can run it with
/// real command-line arguments and a chosen working directory (which
/// `tuo run` does not forward).
fn build_with_module(
    dir: &Path,
    module: tuo_stdlib::Module,
    caller: &str,
    release: bool,
) -> PathBuf {
    let module_path = dir.join(module.name.replace('/', "_"));
    std::fs::write(&module_path, module.source).expect("module source is writable");
    let caller_path = dir.join("caller.tuo");
    std::fs::write(&caller_path, caller).expect("caller source is writable");
    let exe = dir.join("caller.exe");
    let mut command = Command::new(env!("CARGO_BIN_EXE_tuo"));
    command.arg("build");
    if release {
        command.arg("--release");
    }
    let output = command
        .arg("-o")
        .arg(&exe)
        .arg(&module_path)
        .arg(&caller_path)
        .output()
        .expect("the tuo binary runs");
    assert!(
        output.status.success(),
        "build succeeds; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    exe
}

/// `std::time::now()` really reads the monotonic clock (ADR-0013): two
/// instants in sequence give a non-negative `elapsed`, and `render` turns it
/// into text `main` prints. Both backends. This is the native pin
/// `std::time`'s `EFFECT:` doc names.
#[test]
fn stdlib_time_now_really_reads_the_clock_natively() {
    let dir = native_workspace("time_now");
    let caller = "\
module caller;

import std::time;

fn main() -> Int {
    let start = std::time::now();
    let stop = std::time::now();
    let span = std::time::elapsed(start, stop);
    if std::time::as_nanos(span) < 0 {
        return 1;
    }
    if std::time::lt(span, std::time::zero()) {
        return 2;
    }
    std::rt::write_string(1, std::time::render(std::time::zero()));
    0
}
";
    for release in [false, true] {
        let output = run_with_module(&dir, tuo_stdlib::TIME, caller, release);
        let which = backend_name(release);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{which}: the clock never runs backwards between two now() reads; stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "0s",
            "{which}: render(zero()) prints its canonical spelling"
        );
    }
}

/// `std::process::arg_count()`/`arg(i)` really see the command line
/// (ADR-0013): the built executable, run with `["tuo", "lang"]`, reports 3
/// arguments and echoes argument 2 back through `write_string`. Both
/// backends. This is the native pin `std::process`'s argv `EFFECT:` docs
/// name.
#[test]
fn stdlib_process_args_really_read_the_command_line_natively() {
    let caller = "\
module caller;

import std::process;

fn main() -> Int {
    std::rt::write_string(1, std::process::arg(2));
    std::process::arg_count()
}
";
    for release in [false, true] {
        let which = backend_name(release);
        let dir = native_workspace(&format!("process_args_{which}"));
        let exe = build_with_module(&dir, tuo_stdlib::PROCESS, caller, release);
        let output = Command::new(&exe)
            .args(["tuo", "lang"])
            .output()
            .expect("the exe runs");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "lang",
            "{which}: arg(2) is the second real argument"
        );
        assert_eq!(
            output.status.code(),
            Some(3),
            "{which}: two arguments plus the program name is 3; stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// The whole `std::fs` disk tier really touches the disk (ADR-0013): `write`
/// creates the file, `exists` sees it, `read` gets the exact bytes back,
/// `remove` deletes it, and afterwards `exists` is false and `read` is
/// `Err(NotFound)`. Both backends, in a scratch working directory. This is
/// the native pin `std::fs`'s `EFFECT:` docs name.
#[test]
fn stdlib_fs_write_read_exists_remove_really_touch_the_disk_natively() {
    let caller = "\
module caller;

import std::fs;

fn main() -> Int {
    let path = \"stdlib_fs.tmp\";
    match std::fs::exists(path) {
        Ok { value } => if value { return 10; },
        Err { error } => { return 11; },
    }
    match std::fs::write(path, \"hi!\") {
        Ok { value } => if value != 3 { return 12; },
        Err { error } => { return 13; },
    }
    match std::fs::exists(path) {
        Ok { value } => if !value { return 14; },
        Err { error } => { return 15; },
    }
    match std::fs::read(path) {
        Ok { value } => {
            if std::string::len(value) != 3 { return 16; }
            if std::string::byte_at(value, 0) != 104 { return 17; }
            if std::string::byte_at(value, 2) != 33 { return 18; }
        },
        Err { error } => { return 19; },
    }
    match std::fs::remove(path) {
        Ok { value } => {},
        Err { error } => { return 20; },
    }
    match std::fs::exists(path) {
        Ok { value } => if value { return 21; },
        Err { error } => { return 22; },
    }
    match std::fs::read(path) {
        Ok { value } => { return 23; },
        Err { error } => if !std::fs::is_not_found(error) { return 24; },
    }
    0
}
";
    for release in [false, true] {
        let which = backend_name(release);
        let dir = native_workspace(&format!("fs_roundtrip_{which}"));
        let exe = build_with_module(&dir, tuo_stdlib::FS, caller, release);
        let output = Command::new(&exe)
            .current_dir(&dir)
            .output()
            .expect("the exe runs");
        assert_eq!(
            output.status.code(),
            Some(0),
            "{which}: the full fs write/exists/read/remove roundtrip succeeds \
             (a nonzero status names the failing step); stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// The whole `std::net` socket tier really touches the network (ADR-0014):
/// one process listens on an ephemeral loopback port, connects to itself
/// (TCP's backlog completes the handshake before `accept` runs), accepts,
/// moves bytes both ways through the ordinary descriptor seam, and closes
/// all three descriptors. Both backends. This is the native pin `std::net`'s
/// `EFFECT:` docs name.
#[test]
fn stdlib_net_listen_connect_accept_really_touch_the_network_natively() {
    let caller = "\
module caller;

import std::net;

fn main() -> Int {
    let listener = std::net::listen(0);
    if !std::net::is_descriptor(listener) { return 10; }
    let port = std::net::bound_port(listener);
    if port <= 0 { return 11; }
    let client = std::net::connect(\"127.0.0.1\", port);
    if !std::net::is_descriptor(client) { return 12; }
    let server = std::net::accept(listener);
    if !std::net::is_descriptor(server) { return 13; }
    if std::rt::write(client, \"ping\") != 4 { return 14; }
    if std::rt::read_byte(server) != 112 { return 15; }
    if std::rt::write(server, \"o\") != 1 { return 16; }
    if std::rt::read_byte(client) != 111 { return 17; }
    if std::net::close(client) != 0 { return 18; }
    if std::net::close(server) != 0 { return 19; }
    if std::net::close(listener) != 0 { return 20; }
    0
}
";
    for release in [false, true] {
        let which = backend_name(release);
        let dir = native_workspace(&format!("net_roundtrip_{which}"));
        let exe = build_with_module(&dir, tuo_stdlib::NET, caller, release);
        let output = Command::new(&exe).output().expect("the exe runs");
        assert_eq!(
            output.status.code(),
            Some(0),
            "{which}: the full listen/connect/accept/roundtrip/close sequence \
             succeeds (a nonzero status names the failing step); stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// The whole `std::sync` channel and mutex tier really synchronizes
/// (ADR-0015): FIFO order through `channel`/`send`/`recv`, the unambiguous
/// closed signal, and the error-checked `mutex`/`lock`/`unlock` lifecycle —
/// the operations the old CONTRACT stubs could only describe. Both
/// backends. This is the native pin `std::sync`'s channel/mutex `EFFECT:`
/// docs name.
#[test]
fn stdlib_sync_channels_and_mutexes_really_synchronize_natively() {
    let caller = "\
module caller;

import std::sync;

fn main() -> Int {
    let ch = std::sync::channel();
    if ch < 0 { return 10; }
    if std::sync::send(ch, 5) != 0 { return 11; }
    if std::sync::send(ch, 6) != 0 { return 12; }
    if std::sync::recv(ch) != 5 { return 13; }
    if std::sync::close(ch) != 0 { return 14; }
    if std::sync::recv(ch) != 6 { return 15; }
    if std::sync::recv(ch) != 0 - 1 { return 16; }
    let m = std::sync::mutex();
    if m < 0 { return 17; }
    if std::sync::lock(m) != 0 { return 18; }
    if std::sync::unlock(m) != 0 { return 19; }
    if std::sync::unlock(m) != 0 - 1 { return 20; }
    0
}
";
    for release in [false, true] {
        let which = backend_name(release);
        let dir = native_workspace(&format!("sync_roundtrip_{which}"));
        let output = run_with_module(&dir, tuo_stdlib::SYNC, caller, release);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{which}: the full channel/mutex lifecycle through the std::sync \
             wrappers succeeds (a nonzero status names the failing step); \
             stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// `std::json` runs **natively** (ADR-0016): the whole
/// parse → navigate → render pipeline compiles and runs on both backends,
/// exercising the data increment for real — `Array[Float]` elements, the
/// in-place `std::array::set` (with owned-`String` slot drops in the key
/// column), and the owning arena aggregate. Exit 0 only when every
/// navigation answer and the canonical re-render agree with the spec'd
/// semantics.
#[test]
fn stdlib_json_parses_navigates_and_renders_natively() {
    let caller = "\
module caller;

import std::json;

fn main() -> Int {
    let text = \"{\\\"id\\\": 42, \\\"name\\\": \\\"tuo\\\", \\\"xs\\\": [1.5, 2, 3]}\";
    let d = match std::json::parse(text) {
        Ok { value } => value,
        Err { error } => { return 10; },
    };
    if std::json::kind_of(d, std::json::root()) != std::json::kind_object() { return 11; }
    let id = std::json::member(d, std::json::root(), \"id\");
    if std::json::num_of(d, id) != 42.0 { return 12; }
    let name = std::json::member(d, std::json::root(), \"name\");
    let name_text = std::json::text_of(d, name);
    if std::string::as_str(name_text) != \"tuo\" { return 13; }
    let xs = std::json::member(d, std::json::root(), \"xs\");
    if std::json::child_count(d, xs) != 3 { return 14; }
    if std::json::num_of(d, std::json::first_child(d, xs)) != 1.5 { return 15; }
    let rendered = std::json::render(d);
    if std::string::as_str(rendered)
        != \"{\\\"id\\\":42,\\\"name\\\":\\\"tuo\\\",\\\"xs\\\":[1.5,2,3]}\" {
        return 16;
    }
    0
}
";
    for release in [false, true] {
        let which = backend_name(release);
        let dir = native_workspace(&format!("json_roundtrip_{which}"));
        let output = run_with_module(&dir, tuo_stdlib::JSON, caller, release);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{which}: the full parse/navigate/render pipeline succeeds \
             natively (a nonzero status names the failing step); stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// `std::str::split` returns an `Array[String]` that now runs **natively**
/// (the ADR-0012 owned-element increment): the split pieces are heap-owning
/// elements read back by deep-copying `get` and freed by the recursive drop
/// glue, and `join` folds an `Array[Str]` back into one owned `String`. Both
/// backends. Until the increment landed this program was interpreter-only —
/// this pin is the payoff the ADR names.
#[test]
fn stdlib_split_and_join_run_natively() {
    let caller = "\
module caller;

import std::str;

fn main() -> Int {
    let parts = std::str::split(\"a,bb,ccc\", \",\");
    let third = std::string::len(std::array::get(parts, 2));
    var pieces = std::array::empty();
    std::array::push(pieces, \"x\");
    std::array::push(pieces, \"yz\");
    let joined = std::str::join(pieces, \"-\");
    std::array::len(parts) * 10 + third + std::string::len(joined)
}
";
    for release in [false, true] {
        let which = backend_name(release);
        let dir = native_workspace(&format!("split_join_{which}"));
        let output = run_with_module(&dir, tuo_stdlib::STR, caller, release);
        assert_eq!(
            output.status.code(),
            Some(37),
            "{which}: split(\"a,bb,ccc\", \",\") has 3 parts (30) with a 3-byte \
             third part, and join([\"x\", \"yz\"], \"-\") is \"x-yz\" (4 bytes); \
             stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
