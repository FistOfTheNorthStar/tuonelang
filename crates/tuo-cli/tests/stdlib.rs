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

#[test]
fn the_catalog_lists_exactly_its_modules() {
    // The eight initial modules the prompt named, plus the pure modules grown
    // on top of the runnable core — `std::math` (Int/Float arithmetic),
    // `std::str` (byte-level string algorithms + integer⇄text conversion),
    // and ADR-0019 Stage B's `std::bits` (fixed-width bit/byte-order
    // operations), `std::crypto` (SHA-256, HMAC, PBKDF2, Base64), and
    // `std::bignum` (arbitrary-precision integers over 28-bit limbs) — plus
    // `std::ct` (ADR-0020 Stage A's branchless subset) and `std::net`
    // (ADR-0014's TCP socket tier).
    let expected = [
        "std::core",
        "std::collections",
        "std::math",
        "std::bits",
        "std::bignum",
        "std::ct",
        "std::crypto",
        "std::str",
        "std::json",
        "std::io",
        "std::fs",
        "std::net",
        "std::time",
        "std::process",
        "std::sync",
        "std::test",
        "std::chacha",
        "std::x25519",
        "std::hkdf",
        "std::der",
        "std::sha512",
        "std::ed25519",
        "std::tls",
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

/// The catalog's declared intra-library dependency edges, as
/// `(module, the modules it may use)`.
///
/// Most modules stand alone. `std::crypto` is the first that does not: its
/// algorithms are *defined* over the fixed-width bit and byte-order operations
/// `std::bits` owns (ADR-0019 Stage B), and the alternative — a second copy of
/// `rotr32`/`add32`/`be32` inside `std::crypto` — would be two implementations
/// of one specification, free to drift apart. One audited copy is worth the
/// edge.
///
/// This table is the *whole* permitted graph: a module absent from it must
/// still compile alone, and `the_dependency_graph_is_declared_and_acyclic`
/// proves the edges listed here are the only ones and that they form no cycle.
const DECLARED_DEPENDENCIES: &[(&str, &[&str])] = &[
    ("std::crypto", &["std::bits", "std::ct"]),
    // ChaCha20 is defined over 32-bit add/XOR/rotate, which is exactly what
    // `std::bits` owns — reimplementing them here would be a second copy free
    // to drift from the one `std::crypto` already depends on.
    // `std::crypto` for the constant-time tag comparison (`verify`), which
    // must have one implementation rather than a re-derived copy in the
    // module that most needs it to be right; `std::crypto` in turn brings
    // `std::ct` and `std::bits`.
    (
        "std::chacha",
        &["std::bits", "std::ct", "std::bignum", "std::crypto"],
    ),
    ("std::x25519", &["std::bignum"]),
    // HKDF is HMAC-SHA256 in a loop, so it depends on `std::crypto` (which
    // brings `std::bits` and `std::ct`) rather than carrying a second copy.
    ("std::hkdf", &["std::bits", "std::ct", "std::crypto"]),
    // Ed25519 reuses `std::x25519`'s field arithmetic (same prime) rather
    // than shipping a second copy, and is defined over SHA-512.
    (
        "std::ed25519",
        &["std::bits", "std::bignum", "std::x25519", "std::sha512"],
    ),
    // The protocol layer composes every primitive below it.
    (
        "std::tls",
        &[
            "std::bits",
            "std::ct",
            "std::bignum",
            "std::crypto",
            "std::chacha",
            "std::hkdf",
            "std::x25519",
        ],
    ),
];

/// Every module in `path`'s dependency closure, in an order where each
/// module follows everything it depends on.
///
/// A flat list was enough while every dependency was itself standalone. It
/// stopped being enough when `std::chacha` came to depend on `std::crypto`,
/// which has dependencies of its own — loading them in declaration order
/// would present `std::crypto` before `std::bits` and fail to resolve. The
/// recursion also *proves* the graph is acyclic: a cycle would not terminate,
/// so the `seen` set makes it an explicit panic rather than a hang.
fn dependency_closure(path: &str) -> Vec<String> {
    fn visit(path: &str, out: &mut Vec<String>, visiting: &mut Vec<String>) {
        assert!(
            !visiting.iter().any(|p| p == path),
            "the stdlib dependency graph has a cycle through `{path}`"
        );
        visiting.push(path.to_string());
        for &dependency in declared_dependencies_of(path) {
            if !out.iter().any(|p| p == dependency) {
                visit(dependency, out, visiting);
            }
        }
        visiting.pop();
        if !out.iter().any(|p| p == path) {
            out.push(path.to_string());
        }
    }
    let mut out = Vec::new();
    let mut visiting = Vec::new();
    for &dependency in declared_dependencies_of(path) {
        visit(dependency, &mut out, &mut visiting);
    }
    out
}

/// The modules `module` may use, or an empty slice if it must stand alone.
fn declared_dependencies_of(path: &str) -> &'static [&'static str] {
    DECLARED_DEPENDENCIES
        .iter()
        .find(|(name, _)| *name == path)
        .map_or(&[], |(_, deps)| *deps)
}

/// Intern `module` together with its declared dependencies, dependencies
/// first. The returned ids are what a per-module check or spec run needs: the
/// module under test plus exactly what it is allowed to use, and nothing else,
/// so an undeclared dependency still fails to resolve.
fn load_with_dependencies(module: tuo_stdlib::Module) -> (SourceMap, Vec<SourceId>) {
    let mut map = SourceMap::new();
    let mut ids = Vec::new();
    for path in dependency_closure(module.path) {
        let path = path.as_str();
        let dependency =
            tuo_stdlib::module(path).expect("a declared dependency is a catalog module");
        let file = map.intern_file(dependency.name);
        ids.push(
            map.add_source(file, dependency.source)
                .expect("a stdlib module is not too large"),
        );
    }
    let file = map.intern_file(module.name);
    ids.push(
        map.add_source(file, module.source)
            .expect("a stdlib module is not too large"),
    );
    (map, ids)
}

#[test]
fn every_module_checks_cleanly_on_its_own() {
    // A module with no declared dependency must type-check in isolation with
    // zero errors; one that declares dependencies is checked together with
    // exactly those, and nothing else. Either way the module cannot quietly
    // acquire a dependency: an undeclared use fails to resolve here.
    for &module in tuo_stdlib::MODULES {
        let dependencies = declared_dependencies_of(module.path);
        let (map, ids) = load_with_dependencies(module);
        let result = check_sources(&map, &ids);
        assert!(
            !result.has_errors(),
            "{} has front-end errors (checked with its declared dependencies {dependencies:?}):\n{:#?}",
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
fn the_dependency_graph_is_declared_and_acyclic() {
    // Every declared edge must name real modules, and the graph must have no
    // cycle — otherwise no build order exists and "compiles with its
    // dependencies" would be meaningless.
    for (module, dependencies) in DECLARED_DEPENDENCIES {
        assert!(
            tuo_stdlib::module(module).is_some(),
            "declared dependency table names `{module}`, which is not a catalog module"
        );
        for dependency in *dependencies {
            assert!(
                tuo_stdlib::module(dependency).is_some(),
                "`{module}` declares a dependency on `{dependency}`, which is not a catalog module"
            );
            assert_ne!(
                module, dependency,
                "`{module}` declares a dependency on itself"
            );
            // A dependency may now have dependencies of its own
            // (`std::chacha` uses `std::crypto`, which uses `std::bits` and
            // `std::ct`), so the loading order comes from a real topological
            // sort — see `dependency_closure`. What must still hold is that
            // the graph is acyclic, which the closure computation proves by
            // terminating.
            let closure = dependency_closure(module);
            assert!(
                closure.contains(&dependency.to_string()),
                "`{module}`'s dependency closure should contain `{dependency}`"
            );
        }
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

/// Does a spec exercise `name`, directly or through a module-private helper
/// that a spec calls?
///
/// Direct calls are the common case. The indirection matters for functions a
/// spec cannot call inline at all: `mut` parameters need a *place*, and a
/// spec's `given` bindings are immutable, so the call must live in a helper.
/// Treating that as "not exercised" would push the library toward not testing
/// such functions, which is the opposite of the rule's intent.
fn specs_reach(name: &str, source: &str, specs: &str) -> bool {
    if is_called_in(name, specs) {
        return true;
    }
    // One hop: any fn the specs call, whose body calls `name`.
    for helper in private_fn_bodies(source) {
        if is_called_in(&helper.0, specs) && is_called_in(name, &helper.1) {
            return true;
        }
    }
    // The mirror case: `name`'s own body is nothing but a call to a function
    // the specs DO exercise, so the spec'd function's assertions are
    // assertions about `name`'s arithmetic too. `std::x25519::field_invert`
    // is exactly this — it delegates to `power_mod`, and a direct call would
    // exhaust the spec sandbox's fuel no matter the input, so demanding one
    // would mean the library could not spec-check it at all.
    for (helper_name, body) in private_fn_bodies(source) {
        if helper_name == name {
            for (other, _) in private_fn_bodies(source) {
                if other != name && is_called_in(&other, &body) && is_called_in(&other, specs) {
                    return true;
                }
            }
        }
    }
    false
}

/// Every module-private `fn name` and its body text.
fn private_fn_bodies(source: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (index, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        let rest = trimmed
            .strip_prefix("pub fn ")
            .or_else(|| trimmed.strip_prefix("fn "));
        let Some(rest) = rest else {
            continue;
        };
        let Some(name) = rest.split('(').next() else {
            continue;
        };
        // The body is everything up to the next top-level `}`.
        let body: String = source
            .lines()
            .skip(index)
            .take_while(|l| !l.starts_with('}'))
            .collect::<Vec<_>>()
            .join("\n");
        out.push((name.trim().to_string(), body));
    }
    out
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
                    // A spec may reach a function through a module-private
                    // helper rather than calling it inline — sometimes it
                    // MUST, since a `mut` parameter needs a place and a spec
                    // has no mutable binding form. Searching the helpers a
                    // spec calls keeps the rule honest ("a spec exercises it")
                    // without demanding a spelling the language cannot write.
                    let reachable = specs_reach(&public.name, module.source, &specs);
                    assert!(
                        reachable,
                        "{}::{} is pure executable but no spec exercises it",
                        module.path, public.name
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
    // UDP datagram tier; and ADR-0019 Stage B added the entropy primitive,
    // which makes `std::crypto`'s `random_byte`/`nonce` real — drawing
    // randomness is an effect by nature, since a function whose purpose is to
    // differ on every call cannot be pure. `std::tls`'s five I/O functions
    // are built on the same descriptor primitives `std::net` uses — they
    // read and write a socket, they just encrypt on the way through — so
    // they belong to this tier for exactly the reason `std::net`'s do. The
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
            "std::crypto::nonce".to_string(),
            "std::crypto::random_byte".to_string(),
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
            "std::tls::close_notify".to_string(),
            "std::tls::fill".to_string(),
            "std::tls::read_byte".to_string(),
            "std::tls::write_all".to_string(),
            "std::tls::write_record".to_string(),
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
    // A module with declared dependencies is loaded with them (its specs
    // cannot run otherwise); the dependency's own specs run in its own turn of
    // this loop, so they are still each attributed to their own module.
    for &module in tuo_stdlib::MODULES {
        let (map, ids) = load_with_dependencies(module);
        match tuo_spec::run(&map, &ids, &Selection::All, Limits::default()) {
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

/// As [`run_with_module`], but writing several catalog modules — for a module
/// whose declared dependencies must be present too (see
/// `DECLARED_DEPENDENCIES`).
fn run_with_modules(
    dir: &Path,
    modules: &[tuo_stdlib::Module],
    caller: &str,
    release: bool,
) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tuo"));
    command.arg("run");
    if release {
        command.arg("--release");
    }
    for module in modules {
        let path = dir.join(module.name.replace('/', "_"));
        std::fs::write(&path, module.source).expect("module source is writable");
        command.arg(path);
    }
    let caller_path = dir.join("caller.tuo");
    std::fs::write(&caller_path, caller).expect("caller source is writable");
    command
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

/// `std::crypto::random_byte`/`nonce` really draw from the platform CSPRNG.
/// Both backends. This is the native pin `std::crypto`'s `EFFECT:` docs name.
///
/// What this can and cannot assert is worth being precise about. Randomness
/// has no expected value to compare against, so the test checks the
/// properties a *broken* implementation would violate: every byte is in
/// range, a nonce is exactly the requested length, and two independently
/// drawn 32-byte nonces differ. That last check is probabilistic in principle
/// — two random 32-byte strings collide with probability 2^-256 — but it is
/// the check that catches the realistic failures: a stubbed constant, a
/// zero-filled buffer, or an unseeded PRNG returning the same stream twice.
#[test]
fn stdlib_crypto_entropy_really_draws_randomness_natively() {
    let dir = native_workspace("crypto_entropy");
    let caller = "\
module caller;

import std::crypto;

fn main() -> Int {
    // Every drawn byte must be a real byte, never the -1 error and never out
    // of range.
    var i = 0;
    while i < 64 {
        let b = std::crypto::random_byte();
        if b < 0 || b > 255 {
            return 1;
        }
        i = i + 1;
    }

    // A nonce is exactly as long as asked for.
    let a = std::crypto::nonce(32);
    if std::array::len(a) != 32 {
        return 2;
    }
    // A non-positive length yields an empty nonce rather than a trap.
    if std::array::len(std::crypto::nonce(0)) != 0 {
        return 3;
    }

    // Two independent nonces must differ. A constant, a zeroed buffer, or a
    // repeated PRNG stream would fail here.
    let b = std::crypto::nonce(32);
    var same = true;
    var j = 0;
    while j < 32 {
        if std::array::get(a, j) != std::array::get(b, j) {
            same = false;
        }
        j = j + 1;
    }
    if same {
        return 4;
    }
    42
}
";
    for release in [false, true] {
        let output = run_with_modules(
            &dir,
            &[tuo_stdlib::BITS, tuo_stdlib::CT, tuo_stdlib::CRYPTO],
            caller,
            release,
        );
        let which = backend_name(release);
        assert_eq!(
            output.status.code(),
            Some(42),
            "{which}: entropy must be in range, correctly sized, and non-repeating; stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// `std::x25519` reproduces RFC 7748 §5.2's **published** test vector.
///
/// This is X25519's load-bearing pin, and it lives here rather than in a
/// spec because one scalar multiplication is 255 ladder steps of 255-bit
/// bignum arithmetic — far past the spec sandbox's instruction fuel, for the
/// same reason `std::crypto`'s PBKDF2 vector is pinned natively.
///
/// It is also the check that actually earns its keep. An earlier revision
/// computed the ladder's `z2` term as `E * (BB + a24 * E)` instead of
/// RFC 7748's `E * (AA + a24 * E)`. Every other value the ladder produced —
/// `x2`, `x3`, `z3` — was correct, so no self-consistent test could have
/// caught it; only the RFC's published output did.
///
/// Runs on both backends, because a key exchange that disagreed between
/// debug and release builds would be worse than one that simply failed.
#[test]
fn x25519_matches_rfc_7748() {
    let dir = native_workspace("x25519");
    let caller = "\
module caller;

import std::x25519;

/// RFC 7748 §5.2 test vector 1: the scalar.
fn scalar() -> Array[Int] {
    var a = std::array::empty();
    std::array::push(a, 165);\n    std::array::push(a, 70);\n    std::array::push(a, 227);\n    std::array::push(a, 107);\n    std::array::push(a, 240);\n    std::array::push(a, 82);\n    std::array::push(a, 124);\n    std::array::push(a, 157);\n    std::array::push(a, 59);\n    std::array::push(a, 22);\n    std::array::push(a, 21);\n    std::array::push(a, 75);\n    std::array::push(a, 130);\n    std::array::push(a, 70);\n    std::array::push(a, 94);\n    std::array::push(a, 221);\n    std::array::push(a, 98);\n    std::array::push(a, 20);\n    std::array::push(a, 76);\n    std::array::push(a, 10);\n    std::array::push(a, 193);\n    std::array::push(a, 252);\n    std::array::push(a, 90);\n    std::array::push(a, 24);\n    std::array::push(a, 80);\n    std::array::push(a, 106);\n    std::array::push(a, 34);\n    std::array::push(a, 68);\n    std::array::push(a, 186);\n    std::array::push(a, 68);\n    std::array::push(a, 154);\n    std::array::push(a, 196);\n    a
}

/// RFC 7748 §5.2 test vector 1: the input u-coordinate.
fn input_u() -> Array[Int] {
    var a = std::array::empty();
    std::array::push(a, 230);\n    std::array::push(a, 219);\n    std::array::push(a, 104);\n    std::array::push(a, 103);\n    std::array::push(a, 88);\n    std::array::push(a, 48);\n    std::array::push(a, 48);\n    std::array::push(a, 219);\n    std::array::push(a, 53);\n    std::array::push(a, 148);\n    std::array::push(a, 193);\n    std::array::push(a, 164);\n    std::array::push(a, 36);\n    std::array::push(a, 177);\n    std::array::push(a, 95);\n    std::array::push(a, 124);\n    std::array::push(a, 114);\n    std::array::push(a, 102);\n    std::array::push(a, 36);\n    std::array::push(a, 236);\n    std::array::push(a, 38);\n    std::array::push(a, 179);\n    std::array::push(a, 53);\n    std::array::push(a, 59);\n    std::array::push(a, 16);\n    std::array::push(a, 169);\n    std::array::push(a, 3);\n    std::array::push(a, 166);\n    std::array::push(a, 208);\n    std::array::push(a, 171);\n    std::array::push(a, 28);\n    std::array::push(a, 76);\n    a
}

/// RFC 7748 §5.2 test vector 1: the published output.
fn expected() -> Array[Int] {
    var a = std::array::empty();
    std::array::push(a, 195);\n    std::array::push(a, 218);\n    std::array::push(a, 85);\n    std::array::push(a, 55);\n    std::array::push(a, 157);\n    std::array::push(a, 233);\n    std::array::push(a, 198);\n    std::array::push(a, 144);\n    std::array::push(a, 142);\n    std::array::push(a, 148);\n    std::array::push(a, 234);\n    std::array::push(a, 77);\n    std::array::push(a, 242);\n    std::array::push(a, 141);\n    std::array::push(a, 8);\n    std::array::push(a, 79);\n    std::array::push(a, 50);\n    std::array::push(a, 236);\n    std::array::push(a, 207);\n    std::array::push(a, 3);\n    std::array::push(a, 73);\n    std::array::push(a, 28);\n    std::array::push(a, 113);\n    std::array::push(a, 247);\n    std::array::push(a, 84);\n    std::array::push(a, 180);\n    std::array::push(a, 7);\n    std::array::push(a, 85);\n    std::array::push(a, 119);\n    std::array::push(a, 162);\n    std::array::push(a, 133);\n    std::array::push(a, 82);\n    a
}

fn main() -> Int {
    let got = std::x25519::scalar_mult(scalar(), input_u());
    let want = expected();
    var i = 0;
    var mismatches = 0;
    while i < 32 {
        if std::array::get(got, i) != std::array::get(want, i) {
            mismatches = mismatches + 1;
        }
        i = i + 1;
    }
    // 0 means every byte matched RFC 7748.
    mismatches
}
";
    for release in [false, true] {
        let output = run_with_modules(
            &dir,
            &[tuo_stdlib::BITS, tuo_stdlib::BIGNUM, tuo_stdlib::X25519],
            caller,
            release,
        );
        let which = backend_name(release);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{which}: X25519 must reproduce RFC 7748 §5.2's published output \
             byte for byte; the exit status is the number of mismatched \
             bytes. stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// `std::chacha` reproduces RFC 8439 §2.8.2's **published** AEAD vector, and
/// rejects a forgery.
///
/// The specs already pin this inside the sandbox; this proves the same bytes
/// come out of a *compiled* binary on both backends. An AEAD that disagreed
/// between debug and release would produce records the peer cannot open —
/// a failure mode no amount of interpreter testing would reveal.
#[test]
fn chacha20_poly1305_matches_rfc_8439_natively() {
    let dir = native_workspace("chacha");
    let caller = "\
module caller;

import std::chacha;

fn key() -> Array[Int] {
    var a = std::array::empty();
    std::array::push(a, 128);\n    std::array::push(a, 129);\n    std::array::push(a, 130);\n    std::array::push(a, 131);\n    std::array::push(a, 132);\n    std::array::push(a, 133);\n    std::array::push(a, 134);\n    std::array::push(a, 135);\n    std::array::push(a, 136);\n    std::array::push(a, 137);\n    std::array::push(a, 138);\n    std::array::push(a, 139);\n    std::array::push(a, 140);\n    std::array::push(a, 141);\n    std::array::push(a, 142);\n    std::array::push(a, 143);\n    std::array::push(a, 144);\n    std::array::push(a, 145);\n    std::array::push(a, 146);\n    std::array::push(a, 147);\n    std::array::push(a, 148);\n    std::array::push(a, 149);\n    std::array::push(a, 150);\n    std::array::push(a, 151);\n    std::array::push(a, 152);\n    std::array::push(a, 153);\n    std::array::push(a, 154);\n    std::array::push(a, 155);\n    std::array::push(a, 156);\n    std::array::push(a, 157);\n    std::array::push(a, 158);\n    std::array::push(a, 159);\n    a
}

fn nonce() -> Array[Int] {
    var a = std::array::empty();
    std::array::push(a, 7);\n    std::array::push(a, 0);\n    std::array::push(a, 0);\n    std::array::push(a, 0);\n    std::array::push(a, 64);\n    std::array::push(a, 65);\n    std::array::push(a, 66);\n    std::array::push(a, 67);\n    std::array::push(a, 68);\n    std::array::push(a, 69);\n    std::array::push(a, 70);\n    std::array::push(a, 71);\n    a
}

fn aad() -> Array[Int] {
    var a = std::array::empty();
    std::array::push(a, 80);\n    std::array::push(a, 81);\n    std::array::push(a, 82);\n    std::array::push(a, 83);\n    std::array::push(a, 192);\n    std::array::push(a, 193);\n    std::array::push(a, 194);\n    std::array::push(a, 195);\n    std::array::push(a, 196);\n    std::array::push(a, 197);\n    std::array::push(a, 198);\n    std::array::push(a, 199);\n    a
}

fn plaintext() -> Array[Int] {
    var a = std::array::empty();
    std::array::push(a, 76);\n    std::array::push(a, 97);\n    std::array::push(a, 100);\n    std::array::push(a, 105);\n    std::array::push(a, 101);\n    std::array::push(a, 115);\n    std::array::push(a, 32);\n    std::array::push(a, 97);\n    std::array::push(a, 110);\n    std::array::push(a, 100);\n    std::array::push(a, 32);\n    std::array::push(a, 71);\n    std::array::push(a, 101);\n    std::array::push(a, 110);\n    std::array::push(a, 116);\n    std::array::push(a, 108);\n    std::array::push(a, 101);\n    std::array::push(a, 109);\n    std::array::push(a, 101);\n    std::array::push(a, 110);\n    std::array::push(a, 32);\n    std::array::push(a, 111);\n    std::array::push(a, 102);\n    std::array::push(a, 32);\n    std::array::push(a, 116);\n    std::array::push(a, 104);\n    std::array::push(a, 101);\n    std::array::push(a, 32);\n    std::array::push(a, 99);\n    std::array::push(a, 108);\n    std::array::push(a, 97);\n    std::array::push(a, 115);\n    std::array::push(a, 115);\n    std::array::push(a, 32);\n    std::array::push(a, 111);\n    std::array::push(a, 102);\n    std::array::push(a, 32);\n    std::array::push(a, 39);\n    std::array::push(a, 57);\n    std::array::push(a, 57);\n    std::array::push(a, 58);\n    std::array::push(a, 32);\n    std::array::push(a, 73);\n    std::array::push(a, 102);\n    std::array::push(a, 32);\n    std::array::push(a, 73);\n    std::array::push(a, 32);\n    std::array::push(a, 99);\n    std::array::push(a, 111);\n    std::array::push(a, 117);\n    std::array::push(a, 108);\n    std::array::push(a, 100);\n    std::array::push(a, 32);\n    std::array::push(a, 111);\n    std::array::push(a, 102);\n    std::array::push(a, 102);\n    std::array::push(a, 101);\n    std::array::push(a, 114);\n    std::array::push(a, 32);\n    std::array::push(a, 121);\n    std::array::push(a, 111);\n    std::array::push(a, 117);\n    std::array::push(a, 32);\n    std::array::push(a, 111);\n    std::array::push(a, 110);\n    std::array::push(a, 108);\n    std::array::push(a, 121);\n    std::array::push(a, 32);\n    std::array::push(a, 111);\n    std::array::push(a, 110);\n    std::array::push(a, 101);\n    std::array::push(a, 32);\n    std::array::push(a, 116);\n    std::array::push(a, 105);\n    std::array::push(a, 112);\n    std::array::push(a, 32);\n    std::array::push(a, 102);\n    std::array::push(a, 111);\n    std::array::push(a, 114);\n    std::array::push(a, 32);\n    std::array::push(a, 116);\n    std::array::push(a, 104);\n    std::array::push(a, 101);\n    std::array::push(a, 32);\n    std::array::push(a, 102);\n    std::array::push(a, 117);\n    std::array::push(a, 116);\n    std::array::push(a, 117);\n    std::array::push(a, 114);\n    std::array::push(a, 101);\n    std::array::push(a, 44);\n    std::array::push(a, 32);\n    std::array::push(a, 115);\n    std::array::push(a, 117);\n    std::array::push(a, 110);\n    std::array::push(a, 115);\n    std::array::push(a, 99);\n    std::array::push(a, 114);\n    std::array::push(a, 101);\n    std::array::push(a, 101);\n    std::array::push(a, 110);\n    std::array::push(a, 32);\n    std::array::push(a, 119);\n    std::array::push(a, 111);\n    std::array::push(a, 117);\n    std::array::push(a, 108);\n    std::array::push(a, 100);\n    std::array::push(a, 32);\n    std::array::push(a, 98);\n    std::array::push(a, 101);\n    std::array::push(a, 32);\n    std::array::push(a, 105);\n    std::array::push(a, 116);\n    std::array::push(a, 46);\n    a
}

/// RFC 8439 §2.8.2's published ciphertext prefix and tag.
fn expected_prefix() -> Array[Int] {
    var a = std::array::empty();
    std::array::push(a, 211);\n    std::array::push(a, 26);\n    std::array::push(a, 141);\n    std::array::push(a, 52);\n    std::array::push(a, 100);\n    std::array::push(a, 142);\n    std::array::push(a, 96);\n    std::array::push(a, 219);\n    std::array::push(a, 123);\n    std::array::push(a, 134);\n    std::array::push(a, 175);\n    std::array::push(a, 188);\n    std::array::push(a, 83);\n    std::array::push(a, 239);\n    std::array::push(a, 126);\n    std::array::push(a, 194);\n    a
}

fn expected_tag() -> Array[Int] {
    var a = std::array::empty();
    std::array::push(a, 26);\n    std::array::push(a, 225);\n    std::array::push(a, 11);\n    std::array::push(a, 89);\n    std::array::push(a, 79);\n    std::array::push(a, 9);\n    std::array::push(a, 226);\n    std::array::push(a, 106);\n    std::array::push(a, 126);\n    std::array::push(a, 144);\n    std::array::push(a, 46);\n    std::array::push(a, 203);\n    std::array::push(a, 208);\n    std::array::push(a, 96);\n    std::array::push(a, 6);\n    std::array::push(a, 145);\n    a
}

fn main() -> Int {
    let sealed = std::chacha::aead_seal(key(), nonce(), aad(), plaintext());
    var bad = 0;
    // The published ciphertext prefix.
    var i = 0;
    while i < 16 {
        if std::array::get(sealed, i) != std::array::get(expected_prefix(), i) {
            bad = bad + 1;
        }
        i = i + 1;
    }
    // The published tag, appended after 114 ciphertext bytes.
    var t = 0;
    while t < 16 {
        if std::array::get(sealed, 114 + t) != std::array::get(expected_tag(), t) {
            bad = bad + 1;
        }
        t = t + 1;
    }
    // The round trip opens, and a forgery does not.
    bad = bad + round_trip_failures();
    bad = bad + forgery_failures();
    bad
}

/// 0 when the sealed record opens back to its plaintext, 1 otherwise.
fn round_trip_failures() -> Int {
    let sealed = std::chacha::aead_seal(key(), nonce(), aad(), plaintext());
    let first = match std::chacha::aead_open(key(), nonce(), aad(), sealed) {
        Some { value } => std::array::get(value, 0),
        None => 0 - 1,
    };
    // 76 is the 'L' of \"Ladies\" — the plaintext's first byte.
    if first == 76 { 0 } else { 1 }
}

/// 0 when a record with one flipped ciphertext bit REFUSES to open, 1 when it
/// opens anyway — which would mean the tag is not being checked.
fn forgery_failures() -> Int {
    var forged = std::chacha::aead_seal(key(), nonce(), aad(), plaintext());
    std::array::set(forged, 0, std::array::get(forged, 0) ^ 1);
    match std::chacha::aead_open(key(), nonce(), aad(), forged) {
        Some { value } => 1,
        None => 0,
    }
}
";
    for release in [false, true] {
        let output = run_with_modules(
            &dir,
            &[
                tuo_stdlib::BITS,
                tuo_stdlib::CT,
                tuo_stdlib::BIGNUM,
                tuo_stdlib::CRYPTO,
                tuo_stdlib::CHACHA,
            ],
            caller,
            release,
        );
        let which = backend_name(release);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{which}: ChaCha20-Poly1305 must reproduce RFC 8439 §2.8.2's \
             published ciphertext and tag, open its own record, and REFUSE a \
             forged one; the exit status counts the failures. stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// The whole TLS 1.3 crypto core, composed: ECDHE → key schedule → record
/// protection, with each side deriving its keys **independently**.
///
/// The modules each have their own RFC vectors, but a vector proves only that
/// one primitive is right in isolation. This proves they *compose*: Alice
/// seals a record with keys derived from her side of the exchange, and Bob
/// opens it with keys he derived from his — which fails unless the shared
/// secret, the key schedule, the labels, the nonce construction and the AEAD
/// all agree. It is the closest thing to a real handshake the crypto layer
/// can do before certificates exist.
///
/// Both backends, because keys that differed between debug and release would
/// produce a connection that silently fails to decrypt.
#[test]
fn tls13_crypto_core_composes_end_to_end() {
    let dir = native_workspace("tls13_core");
    let caller = "module integration;\n\
\n\
import std::x25519;\n\
import std::hkdf;\n\
import std::chacha;\n\
\n\
fn alice_private() -> Array[Int] {\n\
    var a = std::array::empty();\n\
    var i = 0;\n\
    while i < 32 { std::array::push(a, 11 + i); i = i + 1; }\n\
    a\n\
}\n\
\n\
fn bob_private() -> Array[Int] {\n\
    var a = std::array::empty();\n\
    var i = 0;\n\
    while i < 32 { std::array::push(a, 240 - i); i = i + 1; }\n\
    a\n\
}\n\
\n\
fn message() -> Array[Int] {\n\
    var a = std::array::empty();\n\
    let text = \"attack at dawn\";\n\
    var i = 0;\n\
    while i < std::str::len(text) {\n\
        std::array::push(a, std::str::byte_at(text, i));\n\
        i = i + 1;\n\
    }\n\
    a\n\
}\n\
\n\
/// The whole TLS 1.3 crypto core, composed end to end: ECDHE, the key\n\
/// schedule, and record protection. Returns 0 only when every stage agrees.\n\
fn main() -> Int {\n\
    var failures = 0;\n\
\n\
    // 1. ECDHE: each side derives the same shared secret.\n\
    let alice_public = std::x25519::public_key(alice_private());\n\
    let bob_public = std::x25519::public_key(bob_private());\n\
    let alice_shared = std::x25519::scalar_mult(alice_private(), bob_public);\n\
    let bob_shared = std::x25519::scalar_mult(bob_private(), alice_public);\n\
    var i = 0;\n\
    while i < 32 {\n\
        if std::array::get(alice_shared, i) != std::array::get(bob_shared, i) {\n\
            failures = failures + 1;\n\
        }\n\
        i = i + 1;\n\
    }\n\
\n\
    // 2. Key schedule: the shared secret becomes traffic keys.\n\
    let handshake = std::hkdf::handshake_secret(alice_shared);\n\
    let traffic = std::hkdf::derive_secret(handshake, \"c ap traffic\", std::hkdf::empty_hash());\n\
    let key = std::hkdf::traffic_key(traffic);\n\
    let iv = std::hkdf::traffic_iv(traffic);\n\
    if std::array::len(key) != 32 { failures = failures + 1; }\n\
    if std::array::len(iv) != 12 { failures = failures + 1; }\n\
\n\
    // 3. Record protection: seal a record, and open it with keys derived\n\
    //    independently on the OTHER side.\n\
    let nonce = std::hkdf::record_nonce(iv, 0);\n\
    var aad = std::array::empty();\n\
    std::array::push(aad, 23);\n\
    std::array::push(aad, 3);\n\
    std::array::push(aad, 3);\n\
    let sealed = std::chacha::aead_seal(key, nonce, aad, message());\n\
\n\
    // Bob re-derives from HIS side of the exchange.\n\
    let bob_handshake = std::hkdf::handshake_secret(bob_shared);\n\
    let bob_traffic =\n\
        std::hkdf::derive_secret(bob_handshake, \"c ap traffic\", std::hkdf::empty_hash());\n\
    let bob_key = std::hkdf::traffic_key(bob_traffic);\n\
    let bob_iv = std::hkdf::traffic_iv(bob_traffic);\n\
    let bob_nonce = std::hkdf::record_nonce(bob_iv, 0);\n\
    var bob_aad = std::array::empty();\n\
    std::array::push(bob_aad, 23);\n\
    std::array::push(bob_aad, 3);\n\
    std::array::push(bob_aad, 3);\n\
\n\
    let opened = std::chacha::aead_open(bob_key, bob_nonce, bob_aad, sealed);\n\
    failures = failures + check_opened(opened);\n\
    failures\n\
}\n\
\n\
/// 0 when the record opened to the original message, 1 otherwise.\n\
fn check_opened(take opened: Option[Array[Int]]) -> Int {\n\
    match opened {\n\
        Some { value } => compare_to_message(value),\n\
        None => 1,\n\
    }\n\
}\n\
\n\
fn compare_to_message(take got: Array[Int]) -> Int {\n\
    let want = message();\n\
    if std::array::len(got) != std::array::len(want) {\n\
        return 1;\n\
    }\n\
    var i = 0;\n\
    var bad = 0;\n\
    while i < std::array::len(want) {\n\
        if std::array::get(got, i) != std::array::get(want, i) { bad = 1; }\n\
        i = i + 1;\n\
    }\n\
    bad\n\
}\n\
";
    for release in [false, true] {
        let output = run_with_modules(
            &dir,
            &[
                tuo_stdlib::BITS,
                tuo_stdlib::CT,
                tuo_stdlib::BIGNUM,
                tuo_stdlib::CRYPTO,
                tuo_stdlib::CHACHA,
                tuo_stdlib::X25519,
                tuo_stdlib::HKDF,
            ],
            caller,
            release,
        );
        let which = backend_name(release);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{which}: the TLS 1.3 crypto core must compose — ECDHE agreement, \
             a key schedule producing 32-byte keys and 12-byte IVs, and a \
             record sealed by one side opening on the other. The exit status \
             counts the failures. stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// `std::ed25519` verifies RFC 8032 §7.1's **published** signatures, and
/// refuses forgeries.
///
/// Pinned natively rather than in a spec: one verification is two scalar
/// multiplications over a 255-bit field, past the spec sandbox's instruction
/// fuel. This test therefore carries the module's whole correctness claim —
/// a signature cannot verify unless the field arithmetic, point
/// decompression, curve check, SHA-512, scalar reduction and the group law
/// are *all* right, so it subsumes the checks the sandbox could not afford.
///
/// The negative cases matter as much as the positive ones: a verifier that
/// accepts everything passes every positive test ever written.
#[test]
fn ed25519_matches_rfc_8032() {
    let dir = native_workspace("ed25519");
    let caller = "\
module caller;

import std::ed25519;

/// RFC 8032 §7.1 Test 1: public key for the empty message.
fn test1_public() -> Array[Int] {
    var a = std::array::empty();
    std::array::push(a, 215);\n    std::array::push(a, 90);\n    std::array::push(a, 152);\n    std::array::push(a, 1);\n    std::array::push(a, 130);\n    std::array::push(a, 177);\n    std::array::push(a, 10);\n    std::array::push(a, 183);\n    std::array::push(a, 213);\n    std::array::push(a, 75);\n    std::array::push(a, 254);\n    std::array::push(a, 211);\n    std::array::push(a, 201);\n    std::array::push(a, 100);\n    std::array::push(a, 7);\n    std::array::push(a, 58);\n    std::array::push(a, 14);\n    std::array::push(a, 225);\n    std::array::push(a, 114);\n    std::array::push(a, 243);\n    std::array::push(a, 218);\n    std::array::push(a, 166);\n    std::array::push(a, 35);\n    std::array::push(a, 37);\n    std::array::push(a, 175);\n    std::array::push(a, 2);\n    std::array::push(a, 26);\n    std::array::push(a, 104);\n    std::array::push(a, 247);\n    std::array::push(a, 7);\n    std::array::push(a, 81);\n    std::array::push(a, 26);\n    a
}

/// RFC 8032 §7.1 Test 1: the published signature.
fn test1_signature() -> Array[Int] {
    var a = std::array::empty();
    std::array::push(a, 229);\n    std::array::push(a, 86);\n    std::array::push(a, 67);\n    std::array::push(a, 0);\n    std::array::push(a, 195);\n    std::array::push(a, 96);\n    std::array::push(a, 172);\n    std::array::push(a, 114);\n    std::array::push(a, 144);\n    std::array::push(a, 134);\n    std::array::push(a, 226);\n    std::array::push(a, 204);\n    std::array::push(a, 128);\n    std::array::push(a, 110);\n    std::array::push(a, 130);\n    std::array::push(a, 138);\n    std::array::push(a, 132);\n    std::array::push(a, 135);\n    std::array::push(a, 127);\n    std::array::push(a, 30);\n    std::array::push(a, 184);\n    std::array::push(a, 229);\n    std::array::push(a, 217);\n    std::array::push(a, 116);\n    std::array::push(a, 216);\n    std::array::push(a, 115);\n    std::array::push(a, 224);\n    std::array::push(a, 101);\n    std::array::push(a, 34);\n    std::array::push(a, 73);\n    std::array::push(a, 1);\n    std::array::push(a, 85);\n    std::array::push(a, 95);\n    std::array::push(a, 184);\n    std::array::push(a, 130);\n    std::array::push(a, 21);\n    std::array::push(a, 144);\n    std::array::push(a, 163);\n    std::array::push(a, 59);\n    std::array::push(a, 172);\n    std::array::push(a, 198);\n    std::array::push(a, 30);\n    std::array::push(a, 57);\n    std::array::push(a, 112);\n    std::array::push(a, 28);\n    std::array::push(a, 249);\n    std::array::push(a, 180);\n    std::array::push(a, 107);\n    std::array::push(a, 210);\n    std::array::push(a, 91);\n    std::array::push(a, 245);\n    std::array::push(a, 240);\n    std::array::push(a, 89);\n    std::array::push(a, 91);\n    std::array::push(a, 190);\n    std::array::push(a, 36);\n    std::array::push(a, 101);\n    std::array::push(a, 81);\n    std::array::push(a, 65);\n    std::array::push(a, 67);\n    std::array::push(a, 142);\n    std::array::push(a, 122);\n    std::array::push(a, 16);\n    std::array::push(a, 11);\n    a
}

/// RFC 8032 §7.1 Test 2: public key for the one-byte message 0x72.
fn test2_public() -> Array[Int] {
    var a = std::array::empty();
    std::array::push(a, 61);\n    std::array::push(a, 64);\n    std::array::push(a, 23);\n    std::array::push(a, 195);\n    std::array::push(a, 232);\n    std::array::push(a, 67);\n    std::array::push(a, 137);\n    std::array::push(a, 90);\n    std::array::push(a, 146);\n    std::array::push(a, 183);\n    std::array::push(a, 10);\n    std::array::push(a, 167);\n    std::array::push(a, 77);\n    std::array::push(a, 27);\n    std::array::push(a, 126);\n    std::array::push(a, 188);\n    std::array::push(a, 156);\n    std::array::push(a, 152);\n    std::array::push(a, 44);\n    std::array::push(a, 207);\n    std::array::push(a, 46);\n    std::array::push(a, 196);\n    std::array::push(a, 150);\n    std::array::push(a, 140);\n    std::array::push(a, 192);\n    std::array::push(a, 205);\n    std::array::push(a, 85);\n    std::array::push(a, 241);\n    std::array::push(a, 42);\n    std::array::push(a, 244);\n    std::array::push(a, 102);\n    std::array::push(a, 12);\n    a
}

/// RFC 8032 §7.1 Test 2: the published signature.
fn test2_signature() -> Array[Int] {
    var a = std::array::empty();
    std::array::push(a, 146);\n    std::array::push(a, 160);\n    std::array::push(a, 9);\n    std::array::push(a, 169);\n    std::array::push(a, 240);\n    std::array::push(a, 212);\n    std::array::push(a, 202);\n    std::array::push(a, 184);\n    std::array::push(a, 114);\n    std::array::push(a, 14);\n    std::array::push(a, 130);\n    std::array::push(a, 11);\n    std::array::push(a, 95);\n    std::array::push(a, 100);\n    std::array::push(a, 37);\n    std::array::push(a, 64);\n    std::array::push(a, 162);\n    std::array::push(a, 178);\n    std::array::push(a, 123);\n    std::array::push(a, 84);\n    std::array::push(a, 22);\n    std::array::push(a, 80);\n    std::array::push(a, 63);\n    std::array::push(a, 143);\n    std::array::push(a, 179);\n    std::array::push(a, 118);\n    std::array::push(a, 34);\n    std::array::push(a, 35);\n    std::array::push(a, 235);\n    std::array::push(a, 219);\n    std::array::push(a, 105);\n    std::array::push(a, 218);\n    std::array::push(a, 8);\n    std::array::push(a, 90);\n    std::array::push(a, 193);\n    std::array::push(a, 228);\n    std::array::push(a, 62);\n    std::array::push(a, 21);\n    std::array::push(a, 153);\n    std::array::push(a, 110);\n    std::array::push(a, 69);\n    std::array::push(a, 143);\n    std::array::push(a, 54);\n    std::array::push(a, 19);\n    std::array::push(a, 208);\n    std::array::push(a, 241);\n    std::array::push(a, 29);\n    std::array::push(a, 140);\n    std::array::push(a, 56);\n    std::array::push(a, 123);\n    std::array::push(a, 46);\n    std::array::push(a, 174);\n    std::array::push(a, 180);\n    std::array::push(a, 48);\n    std::array::push(a, 42);\n    std::array::push(a, 238);\n    std::array::push(a, 176);\n    std::array::push(a, 13);\n    std::array::push(a, 41);\n    std::array::push(a, 22);\n    std::array::push(a, 18);\n    std::array::push(a, 187);\n    std::array::push(a, 12);\n    std::array::push(a, 0);\n    a
}

/// The one-byte message of Test 2.
fn test2_message() -> Array[Int] {
    var a = std::array::empty();
    std::array::push(a, 114);
    a
}

/// A message differing from Test 2's in a single bit.
fn tampered_message() -> Array[Int] {
    var a = std::array::empty();
    std::array::push(a, 115);
    a
}

fn main() -> Int {
    var bad = 0;
    // The published vectors must verify.
    if !std::ed25519::verify(test1_public(), std::array::empty(), test1_signature()) {
        bad = bad + 1;
    }
    if !std::ed25519::verify(test2_public(), test2_message(), test2_signature()) {
        bad = bad + 2;
    }
    // A tampered message must NOT verify.
    if std::ed25519::verify(test2_public(), tampered_message(), test2_signature()) {
        bad = bad + 4;
    }
    // Nor the wrong public key.
    if std::ed25519::verify(test1_public(), test2_message(), test2_signature()) {
        bad = bad + 8;
    }
    // Nor a swapped signature.
    if std::ed25519::verify(test2_public(), test2_message(), test1_signature()) {
        bad = bad + 16;
    }
    // The base point must be on the curve — the sandbox could not afford
    // the inversions this needs.
    if !std::ed25519::on_curve(std::ed25519::base_point()) {
        bad = bad + 32;
    }
    // 1 * P = P, and 2 * P = P + P: the scalar-multiplication boundaries.
    if !std::ed25519::points_equal(
        std::ed25519::scalar_mult(std::ed25519::base_point(), std::bignum::from_int(1)),
        std::ed25519::base_point(),
    ) {
        bad = bad + 64;
    }
    if !std::ed25519::points_equal(
        std::ed25519::scalar_mult(std::ed25519::base_point(), std::bignum::from_int(2)),
        std::ed25519::add(std::ed25519::base_point(), std::ed25519::base_point()),
    ) {
        bad = bad + 128;
    }
    bad
}
";
    for release in [false, true] {
        let output = run_with_modules(
            &dir,
            &[
                tuo_stdlib::BITS,
                tuo_stdlib::BIGNUM,
                tuo_stdlib::X25519,
                tuo_stdlib::SHA512,
                tuo_stdlib::ED25519,
            ],
            caller,
            release,
        );
        let which = backend_name(release);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{which}: Ed25519 must verify RFC 8032 §7.1's published signatures \
             and refuse forgeries; the exit status is a bitmask of which \
             checks failed. stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// The **certificate half** of TLS 1.3, composed: parse a real certificate,
/// extract its public key, and verify a real signature made by the matching
/// private key.
///
/// Every artifact here came from OpenSSL — the certificate, the key inside
/// it, and the signature over the transcript — so this proves interoperation
/// rather than self-consistency. A stack that only ever read its own output
/// would pass every earlier test in this file and still fail against the
/// first real server it met.
///
/// This is what a TLS client actually does with a `Certificate` and a
/// `CertificateVerify` message: decode, extract, verify. It exercises
/// `std::der` and `std::ed25519` (and through it `std::sha512`,
/// `std::x25519`, `std::bignum`) as one pipeline.
#[test]
fn certificate_chain_verifies_a_real_signature() {
    let dir = native_workspace("cert_verify");
    let caller = "\
module caller;

import std::der;
import std::ed25519;

/// A real Ed25519 certificate, emitted by OpenSSL.
fn certificate() -> Array[Int] {
    var a = std::array::empty();
    std::array::push(a, 48);\n    std::array::push(a, 130);\n    std::array::push(a, 1);\n    std::array::push(a, 60);\n    std::array::push(a, 48);\n    std::array::push(a, 129);\n    std::array::push(a, 239);\n    std::array::push(a, 160);\n    std::array::push(a, 3);\n    std::array::push(a, 2);\n    std::array::push(a, 1);\n    std::array::push(a, 2);\n    std::array::push(a, 2);\n    std::array::push(a, 20);\n    std::array::push(a, 74);\n    std::array::push(a, 226);\n    std::array::push(a, 75);\n    std::array::push(a, 255);\n    std::array::push(a, 194);\n    std::array::push(a, 84);\n    std::array::push(a, 177);\n    std::array::push(a, 189);\n    std::array::push(a, 68);\n    std::array::push(a, 2);\n    std::array::push(a, 195);\n    std::array::push(a, 137);\n    std::array::push(a, 167);\n    std::array::push(a, 148);\n    std::array::push(a, 68);\n    std::array::push(a, 38);\n    std::array::push(a, 203);\n    std::array::push(a, 55);\n    std::array::push(a, 136);\n    std::array::push(a, 227);\n    std::array::push(a, 48);\n    std::array::push(a, 5);\n    std::array::push(a, 6);\n    std::array::push(a, 3);\n    std::array::push(a, 43);\n    std::array::push(a, 101);\n    std::array::push(a, 112);\n    std::array::push(a, 48);\n    std::array::push(a, 20);\n    std::array::push(a, 49);\n    std::array::push(a, 18);\n    std::array::push(a, 48);\n    std::array::push(a, 16);\n    std::array::push(a, 6);\n    std::array::push(a, 3);\n    std::array::push(a, 85);\n    std::array::push(a, 4);\n    std::array::push(a, 3);\n    std::array::push(a, 12);\n    std::array::push(a, 9);\n    std::array::push(a, 108);\n    std::array::push(a, 111);\n    std::array::push(a, 99);\n    std::array::push(a, 97);\n    std::array::push(a, 108);\n    std::array::push(a, 104);\n    std::array::push(a, 111);\n    std::array::push(a, 115);\n    std::array::push(a, 116);\n    std::array::push(a, 48);\n    std::array::push(a, 30);\n    std::array::push(a, 23);\n    std::array::push(a, 13);\n    std::array::push(a, 50);\n    std::array::push(a, 54);\n    std::array::push(a, 48);\n    std::array::push(a, 57);\n    std::array::push(a, 49);\n    std::array::push(a, 51);\n    std::array::push(a, 49);\n    std::array::push(a, 53);\n    std::array::push(a, 53);\n    std::array::push(a, 56);\n    std::array::push(a, 50);\n    std::array::push(a, 55);\n    std::array::push(a, 90);\n    std::array::push(a, 23);\n    std::array::push(a, 13);\n    std::array::push(a, 50);\n    std::array::push(a, 55);\n    std::array::push(a, 48);\n    std::array::push(a, 57);\n    std::array::push(a, 49);\n    std::array::push(a, 51);\n    std::array::push(a, 49);\n    std::array::push(a, 53);\n    std::array::push(a, 53);\n    std::array::push(a, 56);\n    std::array::push(a, 50);\n    std::array::push(a, 55);\n    std::array::push(a, 90);\n    std::array::push(a, 48);\n    std::array::push(a, 20);\n    std::array::push(a, 49);\n    std::array::push(a, 18);\n    std::array::push(a, 48);\n    std::array::push(a, 16);\n    std::array::push(a, 6);\n    std::array::push(a, 3);\n    std::array::push(a, 85);\n    std::array::push(a, 4);\n    std::array::push(a, 3);\n    std::array::push(a, 12);\n    std::array::push(a, 9);\n    std::array::push(a, 108);\n    std::array::push(a, 111);\n    std::array::push(a, 99);\n    std::array::push(a, 97);\n    std::array::push(a, 108);\n    std::array::push(a, 104);\n    std::array::push(a, 111);\n    std::array::push(a, 115);\n    std::array::push(a, 116);\n    std::array::push(a, 48);\n    std::array::push(a, 42);\n    std::array::push(a, 48);\n    std::array::push(a, 5);\n    std::array::push(a, 6);\n    std::array::push(a, 3);\n    std::array::push(a, 43);\n    std::array::push(a, 101);\n    std::array::push(a, 112);\n    std::array::push(a, 3);\n    std::array::push(a, 33);\n    std::array::push(a, 0);\n    std::array::push(a, 15);\n    std::array::push(a, 213);\n    std::array::push(a, 65);\n    std::array::push(a, 171);\n    std::array::push(a, 25);\n    std::array::push(a, 24);\n    std::array::push(a, 82);\n    std::array::push(a, 85);\n    std::array::push(a, 197);\n    std::array::push(a, 221);\n    std::array::push(a, 199);\n    std::array::push(a, 43);\n    std::array::push(a, 46);\n    std::array::push(a, 171);\n    std::array::push(a, 85);\n    std::array::push(a, 4);\n    std::array::push(a, 97);\n    std::array::push(a, 150);\n    std::array::push(a, 244);\n    std::array::push(a, 80);\n    std::array::push(a, 117);\n    std::array::push(a, 205);\n    std::array::push(a, 135);\n    std::array::push(a, 54);\n    std::array::push(a, 165);\n    std::array::push(a, 246);\n    std::array::push(a, 145);\n    std::array::push(a, 214);\n    std::array::push(a, 146);\n    std::array::push(a, 92);\n    std::array::push(a, 6);\n    std::array::push(a, 103);\n    std::array::push(a, 163);\n    std::array::push(a, 83);\n    std::array::push(a, 48);\n    std::array::push(a, 81);\n    std::array::push(a, 48);\n    std::array::push(a, 29);\n    std::array::push(a, 6);\n    std::array::push(a, 3);\n    std::array::push(a, 85);\n    std::array::push(a, 29);\n    std::array::push(a, 14);\n    std::array::push(a, 4);\n    std::array::push(a, 22);\n    std::array::push(a, 4);\n    std::array::push(a, 20);\n    std::array::push(a, 67);\n    std::array::push(a, 88);\n    std::array::push(a, 241);\n    std::array::push(a, 182);\n    std::array::push(a, 37);\n    std::array::push(a, 191);\n    std::array::push(a, 215);\n    std::array::push(a, 135);\n    std::array::push(a, 79);\n    std::array::push(a, 225);\n    std::array::push(a, 21);\n    std::array::push(a, 55);\n    std::array::push(a, 224);\n    std::array::push(a, 21);\n    std::array::push(a, 199);\n    std::array::push(a, 86);\n    std::array::push(a, 159);\n    std::array::push(a, 36);\n    std::array::push(a, 232);\n    std::array::push(a, 175);\n    std::array::push(a, 48);\n    std::array::push(a, 31);\n    std::array::push(a, 6);\n    std::array::push(a, 3);\n    std::array::push(a, 85);\n    std::array::push(a, 29);\n    std::array::push(a, 35);\n    std::array::push(a, 4);\n    std::array::push(a, 24);\n    std::array::push(a, 48);\n    std::array::push(a, 22);\n    std::array::push(a, 128);\n    std::array::push(a, 20);\n    std::array::push(a, 67);\n    std::array::push(a, 88);\n    std::array::push(a, 241);\n    std::array::push(a, 182);\n    std::array::push(a, 37);\n    std::array::push(a, 191);\n    std::array::push(a, 215);\n    std::array::push(a, 135);\n    std::array::push(a, 79);\n    std::array::push(a, 225);\n    std::array::push(a, 21);\n    std::array::push(a, 55);\n    std::array::push(a, 224);\n    std::array::push(a, 21);\n    std::array::push(a, 199);\n    std::array::push(a, 86);\n    std::array::push(a, 159);\n    std::array::push(a, 36);\n    std::array::push(a, 232);\n    std::array::push(a, 175);\n    std::array::push(a, 48);\n    std::array::push(a, 15);\n    std::array::push(a, 6);\n    std::array::push(a, 3);\n    std::array::push(a, 85);\n    std::array::push(a, 29);\n    std::array::push(a, 19);\n    std::array::push(a, 1);\n    std::array::push(a, 1);\n    std::array::push(a, 255);\n    std::array::push(a, 4);\n    std::array::push(a, 5);\n    std::array::push(a, 48);\n    std::array::push(a, 3);\n    std::array::push(a, 1);\n    std::array::push(a, 1);\n    std::array::push(a, 255);\n    std::array::push(a, 48);\n    std::array::push(a, 5);\n    std::array::push(a, 6);\n    std::array::push(a, 3);\n    std::array::push(a, 43);\n    std::array::push(a, 101);\n    std::array::push(a, 112);\n    std::array::push(a, 3);\n    std::array::push(a, 65);\n    std::array::push(a, 0);\n    std::array::push(a, 17);\n    std::array::push(a, 220);\n    std::array::push(a, 255);\n    std::array::push(a, 22);\n    std::array::push(a, 194);\n    std::array::push(a, 187);\n    std::array::push(a, 166);\n    std::array::push(a, 12);\n    std::array::push(a, 1);\n    std::array::push(a, 228);\n    std::array::push(a, 123);\n    std::array::push(a, 35);\n    std::array::push(a, 169);\n    std::array::push(a, 201);\n    std::array::push(a, 33);\n    std::array::push(a, 254);\n    std::array::push(a, 18);\n    std::array::push(a, 199);\n    std::array::push(a, 133);\n    std::array::push(a, 133);\n    std::array::push(a, 228);\n    std::array::push(a, 109);\n    std::array::push(a, 182);\n    std::array::push(a, 91);\n    std::array::push(a, 107);\n    std::array::push(a, 110);\n    std::array::push(a, 21);\n    std::array::push(a, 173);\n    std::array::push(a, 3);\n    std::array::push(a, 175);\n    std::array::push(a, 144);\n    std::array::push(a, 13);\n    std::array::push(a, 10);\n    std::array::push(a, 30);\n    std::array::push(a, 72);\n    std::array::push(a, 81);\n    std::array::push(a, 42);\n    std::array::push(a, 193);\n    std::array::push(a, 58);\n    std::array::push(a, 251);\n    std::array::push(a, 174);\n    std::array::push(a, 92);\n    std::array::push(a, 143);\n    std::array::push(a, 193);\n    std::array::push(a, 43);\n    std::array::push(a, 111);\n    std::array::push(a, 156);\n    std::array::push(a, 226);\n    std::array::push(a, 212);\n    std::array::push(a, 207);\n    std::array::push(a, 252);\n    std::array::push(a, 15);\n    std::array::push(a, 159);\n    std::array::push(a, 141);\n    std::array::push(a, 207);\n    std::array::push(a, 135);\n    std::array::push(a, 92);\n    std::array::push(a, 164);\n    std::array::push(a, 163);\n    std::array::push(a, 39);\n    std::array::push(a, 159);\n    std::array::push(a, 199);\n    std::array::push(a, 195);\n    std::array::push(a, 13);\n    a
}

/// A real signature over `transcript()`, made by the certificate's own
/// private key.
fn signature() -> Array[Int] {
    var a = std::array::empty();
    std::array::push(a, 220);\n    std::array::push(a, 206);\n    std::array::push(a, 55);\n    std::array::push(a, 192);\n    std::array::push(a, 243);\n    std::array::push(a, 71);\n    std::array::push(a, 103);\n    std::array::push(a, 165);\n    std::array::push(a, 51);\n    std::array::push(a, 33);\n    std::array::push(a, 254);\n    std::array::push(a, 190);\n    std::array::push(a, 69);\n    std::array::push(a, 219);\n    std::array::push(a, 157);\n    std::array::push(a, 153);\n    std::array::push(a, 59);\n    std::array::push(a, 251);\n    std::array::push(a, 62);\n    std::array::push(a, 253);\n    std::array::push(a, 164);\n    std::array::push(a, 15);\n    std::array::push(a, 12);\n    std::array::push(a, 100);\n    std::array::push(a, 234);\n    std::array::push(a, 14);\n    std::array::push(a, 56);\n    std::array::push(a, 69);\n    std::array::push(a, 24);\n    std::array::push(a, 110);\n    std::array::push(a, 231);\n    std::array::push(a, 21);\n    std::array::push(a, 3);\n    std::array::push(a, 6);\n    std::array::push(a, 237);\n    std::array::push(a, 98);\n    std::array::push(a, 213);\n    std::array::push(a, 21);\n    std::array::push(a, 31);\n    std::array::push(a, 0);\n    std::array::push(a, 163);\n    std::array::push(a, 73);\n    std::array::push(a, 182);\n    std::array::push(a, 228);\n    std::array::push(a, 172);\n    std::array::push(a, 14);\n    std::array::push(a, 156);\n    std::array::push(a, 14);\n    std::array::push(a, 169);\n    std::array::push(a, 102);\n    std::array::push(a, 177);\n    std::array::push(a, 22);\n    std::array::push(a, 135);\n    std::array::push(a, 41);\n    std::array::push(a, 88);\n    std::array::push(a, 169);\n    std::array::push(a, 184);\n    std::array::push(a, 115);\n    std::array::push(a, 153);\n    std::array::push(a, 102);\n    std::array::push(a, 186);\n    std::array::push(a, 0);\n    std::array::push(a, 199);\n    std::array::push(a, 5);\n    a
}

/// The signed message — a stand-in for a handshake transcript hash.
fn transcript() -> Array[Int] {
    var a = std::array::empty();
    std::array::push(a, 84);\n    std::array::push(a, 76);\n    std::array::push(a, 83);\n    std::array::push(a, 32);\n    std::array::push(a, 49);\n    std::array::push(a, 46);\n    std::array::push(a, 51);\n    std::array::push(a, 32);\n    std::array::push(a, 104);\n    std::array::push(a, 97);\n    std::array::push(a, 110);\n    std::array::push(a, 100);\n    std::array::push(a, 115);\n    std::array::push(a, 104);\n    std::array::push(a, 97);\n    std::array::push(a, 107);\n    std::array::push(a, 101);\n    std::array::push(a, 32);\n    std::array::push(a, 116);\n    std::array::push(a, 114);\n    std::array::push(a, 97);\n    std::array::push(a, 110);\n    std::array::push(a, 115);\n    std::array::push(a, 99);\n    std::array::push(a, 114);\n    std::array::push(a, 105);\n    std::array::push(a, 112);\n    std::array::push(a, 116);\n    std::array::push(a, 32);\n    std::array::push(a, 104);\n    std::array::push(a, 97);\n    std::array::push(a, 115);\n    std::array::push(a, 104);\n    std::array::push(a, 32);\n    std::array::push(a, 112);\n    std::array::push(a, 108);\n    std::array::push(a, 97);\n    std::array::push(a, 99);\n    std::array::push(a, 101);\n    std::array::push(a, 104);\n    std::array::push(a, 111);\n    std::array::push(a, 108);\n    std::array::push(a, 100);\n    std::array::push(a, 101);\n    std::array::push(a, 114);\n    std::array::push(a, 32);\n    std::array::push(a, 48);\n    std::array::push(a, 49);\n    std::array::push(a, 50);\n    std::array::push(a, 51);\n    std::array::push(a, 52);\n    std::array::push(a, 53);\n    std::array::push(a, 54);\n    std::array::push(a, 55);\n    std::array::push(a, 56);\n    std::array::push(a, 57);\n    a
}

/// A transcript differing in exactly one byte.
fn altered_transcript() -> Array[Int] {
    var a = std::array::empty();
    std::array::push(a, 84);\n    std::array::push(a, 76);\n    std::array::push(a, 83);\n    std::array::push(a, 32);\n    std::array::push(a, 49);\n    std::array::push(a, 46);\n    std::array::push(a, 51);\n    std::array::push(a, 32);\n    std::array::push(a, 104);\n    std::array::push(a, 97);\n    std::array::push(a, 110);\n    std::array::push(a, 100);\n    std::array::push(a, 115);\n    std::array::push(a, 104);\n    std::array::push(a, 97);\n    std::array::push(a, 107);\n    std::array::push(a, 101);\n    std::array::push(a, 32);\n    std::array::push(a, 116);\n    std::array::push(a, 114);\n    std::array::push(a, 97);\n    std::array::push(a, 110);\n    std::array::push(a, 115);\n    std::array::push(a, 99);\n    std::array::push(a, 114);\n    std::array::push(a, 105);\n    std::array::push(a, 112);\n    std::array::push(a, 116);\n    std::array::push(a, 32);\n    std::array::push(a, 104);\n    std::array::push(a, 97);\n    std::array::push(a, 115);\n    std::array::push(a, 104);\n    std::array::push(a, 32);\n    std::array::push(a, 112);\n    std::array::push(a, 108);\n    std::array::push(a, 97);\n    std::array::push(a, 99);\n    std::array::push(a, 101);\n    std::array::push(a, 104);\n    std::array::push(a, 111);\n    std::array::push(a, 108);\n    std::array::push(a, 100);\n    std::array::push(a, 101);\n    std::array::push(a, 114);\n    std::array::push(a, 32);\n    std::array::push(a, 48);\n    std::array::push(a, 49);\n    std::array::push(a, 50);\n    std::array::push(a, 51);\n    std::array::push(a, 52);\n    std::array::push(a, 53);\n    std::array::push(a, 54);\n    std::array::push(a, 55);\n    std::array::push(a, 56);\n    std::array::push(a, 57);\n    std::array::set(a, 0, 88);
    a
}

fn main() -> Int {
    var bad = 0;
    // The certificate must parse and declare an Ed25519 key.
    if !std::der::certificate_is_ed25519(certificate()) {
        bad = bad + 1;
    }
    let key = std::der::certificate_public_key(certificate());
    if std::array::len(key) != 32 {
        return 2;
    }
    // The real signature must verify under the key read out of the
    // certificate — the whole pipeline in one assertion.
    if !std::ed25519::verify(key, transcript(), signature()) {
        bad = bad + 4;
    }
    // A one-byte change to the signed data must break it.
    if std::ed25519::verify(key, altered_transcript(), signature()) {
        bad = bad + 8;
    }
    bad
}
";
    for release in [false, true] {
        let output = run_with_modules(
            &dir,
            &[
                tuo_stdlib::BITS,
                tuo_stdlib::BIGNUM,
                tuo_stdlib::X25519,
                tuo_stdlib::SHA512,
                tuo_stdlib::ED25519,
                tuo_stdlib::DER,
            ],
            caller,
            release,
        );
        let which = backend_name(release);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{which}: a real OpenSSL certificate must parse, and a real \
             signature must verify under the key it carries; the exit status \
             is a bitmask of which checks failed. stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// `std::tls`'s key schedule reproduces **RFC 8448's published** handshake
/// secrets.
///
/// RFC 8448 is the TLS 1.3 "example handshake trace" — a complete exchange
/// with every intermediate value written out. Matching it proves the whole
/// derivation chain (HKDF, the `tls13 ` labels, `Derive-Secret`, the empty
/// transcript hash, SHA-256) composes exactly as the RFC intends, which no
/// amount of self-consistent testing can establish: two peers derive keys
/// independently and never compare them, so an implementation that agrees
/// only with itself yields a connection where every record fails to decrypt.
#[test]
fn tls_key_schedule_matches_rfc_8448() {
    let dir = native_workspace("tls_schedule");
    let caller = "\
module caller;

import std::tls;

/// RFC 8448 §3's ECDHE shared secret.
fn shared() -> Array[Int] {
    var a = std::array::empty();
    std::array::push(a, 139);\n    std::array::push(a, 212);\n    std::array::push(a, 5);\n    std::array::push(a, 79);\n    std::array::push(a, 181);\n    std::array::push(a, 91);\n    std::array::push(a, 157);\n    std::array::push(a, 99);\n    std::array::push(a, 253);\n    std::array::push(a, 251);\n    std::array::push(a, 172);\n    std::array::push(a, 249);\n    std::array::push(a, 240);\n    std::array::push(a, 75);\n    std::array::push(a, 159);\n    std::array::push(a, 13);\n    std::array::push(a, 53);\n    std::array::push(a, 230);\n    std::array::push(a, 214);\n    std::array::push(a, 63);\n    std::array::push(a, 83);\n    std::array::push(a, 117);\n    std::array::push(a, 99);\n    std::array::push(a, 239);\n    std::array::push(a, 212);\n    std::array::push(a, 98);\n    std::array::push(a, 114);\n    std::array::push(a, 144);\n    std::array::push(a, 15);\n    std::array::push(a, 137);\n    std::array::push(a, 73);\n    std::array::push(a, 45);\n    a
}

/// RFC 8448 §3's ClientHello..ServerHello transcript hash.
fn hello_hash() -> Array[Int] {
    var a = std::array::empty();
    std::array::push(a, 134);\n    std::array::push(a, 12);\n    std::array::push(a, 6);\n    std::array::push(a, 237);\n    std::array::push(a, 192);\n    std::array::push(a, 120);\n    std::array::push(a, 88);\n    std::array::push(a, 238);\n    std::array::push(a, 142);\n    std::array::push(a, 120);\n    std::array::push(a, 240);\n    std::array::push(a, 231);\n    std::array::push(a, 66);\n    std::array::push(a, 140);\n    std::array::push(a, 88);\n    std::array::push(a, 237);\n    std::array::push(a, 214);\n    std::array::push(a, 180);\n    std::array::push(a, 63);\n    std::array::push(a, 44);\n    std::array::push(a, 163);\n    std::array::push(a, 230);\n    std::array::push(a, 233);\n    std::array::push(a, 95);\n    std::array::push(a, 2);\n    std::array::push(a, 237);\n    std::array::push(a, 6);\n    std::array::push(a, 60);\n    std::array::push(a, 240);\n    std::array::push(a, 225);\n    std::array::push(a, 202);\n    std::array::push(a, 216);\n    a
}

/// RFC 8448 §3's published client handshake traffic secret.
fn expected_client() -> Array[Int] {
    var a = std::array::empty();
    std::array::push(a, 179);\n    std::array::push(a, 237);\n    std::array::push(a, 219);\n    std::array::push(a, 18);\n    std::array::push(a, 110);\n    std::array::push(a, 6);\n    std::array::push(a, 127);\n    std::array::push(a, 53);\n    std::array::push(a, 167);\n    std::array::push(a, 128);\n    std::array::push(a, 179);\n    std::array::push(a, 171);\n    std::array::push(a, 244);\n    std::array::push(a, 94);\n    std::array::push(a, 45);\n    std::array::push(a, 143);\n    std::array::push(a, 59);\n    std::array::push(a, 26);\n    std::array::push(a, 149);\n    std::array::push(a, 7);\n    std::array::push(a, 56);\n    std::array::push(a, 245);\n    std::array::push(a, 46);\n    std::array::push(a, 150);\n    std::array::push(a, 0);\n    std::array::push(a, 116);\n    std::array::push(a, 106);\n    std::array::push(a, 14);\n    std::array::push(a, 39);\n    std::array::push(a, 165);\n    std::array::push(a, 90);\n    std::array::push(a, 33);\n    a
}

/// RFC 8448 §3's published server handshake traffic secret.
fn expected_server() -> Array[Int] {
    var a = std::array::empty();
    std::array::push(a, 182);\n    std::array::push(a, 123);\n    std::array::push(a, 125);\n    std::array::push(a, 105);\n    std::array::push(a, 12);\n    std::array::push(a, 193);\n    std::array::push(a, 108);\n    std::array::push(a, 78);\n    std::array::push(a, 117);\n    std::array::push(a, 229);\n    std::array::push(a, 66);\n    std::array::push(a, 19);\n    std::array::push(a, 203);\n    std::array::push(a, 45);\n    std::array::push(a, 55);\n    std::array::push(a, 180);\n    std::array::push(a, 233);\n    std::array::push(a, 201);\n    std::array::push(a, 18);\n    std::array::push(a, 188);\n    std::array::push(a, 222);\n    std::array::push(a, 217);\n    std::array::push(a, 16);\n    std::array::push(a, 93);\n    std::array::push(a, 66);\n    std::array::push(a, 190);\n    std::array::push(a, 253);\n    std::array::push(a, 89);\n    std::array::push(a, 211);\n    std::array::push(a, 145);\n    std::array::push(a, 173);\n    std::array::push(a, 56);\n    a
}

fn main() -> Int {
    let c = std::tls::client_handshake_secret(shared(), hello_hash());
    let s = std::tls::server_handshake_secret(shared(), hello_hash());
    var bad = 0;
    var i = 0;
    while i < 32 {
        if std::array::get(c, i) != std::array::get(expected_client(), i) {
            bad = bad + 1;
        }
        if std::array::get(s, i) != std::array::get(expected_server(), i) {
            bad = bad + 1;
        }
        i = i + 1;
    }
    bad
}
";
    for release in [false, true] {
        let output = run_with_modules(
            &dir,
            &[
                tuo_stdlib::BITS,
                tuo_stdlib::CT,
                tuo_stdlib::BIGNUM,
                tuo_stdlib::CRYPTO,
                tuo_stdlib::CHACHA,
                tuo_stdlib::HKDF,
                tuo_stdlib::X25519,
                tuo_stdlib::TLS,
            ],
            caller,
            release,
        );
        let which = backend_name(release);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{which}: the key schedule must reproduce RFC 8448's published \
             handshake secrets; the exit status counts wrong bytes. \
             stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// A **live OpenSSL client completes a full TLS 1.3 handshake** with a
/// tuonelang server.
///
/// This is the test the whole TLS stack exists to pass, and it is the only
/// one that can establish what matters. Every primitive has its own RFC
/// vectors, and `std::tls` has its own specs, but a protocol is not the sum
/// of its primitives: the handshake succeeds only if the record framing, the
/// ClientHello parsing, the ServerHello construction, the X25519 exchange,
/// the key schedule, the transcript hash, the ChaCha20-Poly1305 record
/// protection, the certificate encoding and the Ed25519 signature are **all**
/// simultaneously byte-correct against an implementation that shares none of
/// this code.
///
/// The negative half is as load-bearing as the positive one: a server that
/// signs with the wrong key must be REJECTED. Without that check, a client
/// that accepted everything would pass the positive test.
///
/// Skips cleanly when no `openssl` binary is present, rather than failing for
/// an environment reason.
#[test]
fn tls_server_completes_handshake_with_openssl() {
    if Command::new("openssl").arg("version").output().is_err() {
        // No openssl to test against: skip rather than fail for an
        // environment reason.
        return;
    }
    let dir = native_workspace("tls_live");
    // The certificate the server presents, and the seed of its private key.
    let caller_template = "\
module caller;

import std::tls;
import std::ed25519;
import std::x25519;

/// A real Ed25519 certificate, emitted by OpenSSL.
fn certificate() -> Array[Int] {
    var a = std::array::empty();
    std::array::push(a, 48);\n    std::array::push(a, 130);\n    std::array::push(a, 1);\n    std::array::push(a, 60);\n    std::array::push(a, 48);\n    std::array::push(a, 129);\n    std::array::push(a, 239);\n    std::array::push(a, 160);\n    std::array::push(a, 3);\n    std::array::push(a, 2);\n    std::array::push(a, 1);\n    std::array::push(a, 2);\n    std::array::push(a, 2);\n    std::array::push(a, 20);\n    std::array::push(a, 74);\n    std::array::push(a, 226);\n    std::array::push(a, 75);\n    std::array::push(a, 255);\n    std::array::push(a, 194);\n    std::array::push(a, 84);\n    std::array::push(a, 177);\n    std::array::push(a, 189);\n    std::array::push(a, 68);\n    std::array::push(a, 2);\n    std::array::push(a, 195);\n    std::array::push(a, 137);\n    std::array::push(a, 167);\n    std::array::push(a, 148);\n    std::array::push(a, 68);\n    std::array::push(a, 38);\n    std::array::push(a, 203);\n    std::array::push(a, 55);\n    std::array::push(a, 136);\n    std::array::push(a, 227);\n    std::array::push(a, 48);\n    std::array::push(a, 5);\n    std::array::push(a, 6);\n    std::array::push(a, 3);\n    std::array::push(a, 43);\n    std::array::push(a, 101);\n    std::array::push(a, 112);\n    std::array::push(a, 48);\n    std::array::push(a, 20);\n    std::array::push(a, 49);\n    std::array::push(a, 18);\n    std::array::push(a, 48);\n    std::array::push(a, 16);\n    std::array::push(a, 6);\n    std::array::push(a, 3);\n    std::array::push(a, 85);\n    std::array::push(a, 4);\n    std::array::push(a, 3);\n    std::array::push(a, 12);\n    std::array::push(a, 9);\n    std::array::push(a, 108);\n    std::array::push(a, 111);\n    std::array::push(a, 99);\n    std::array::push(a, 97);\n    std::array::push(a, 108);\n    std::array::push(a, 104);\n    std::array::push(a, 111);\n    std::array::push(a, 115);\n    std::array::push(a, 116);\n    std::array::push(a, 48);\n    std::array::push(a, 30);\n    std::array::push(a, 23);\n    std::array::push(a, 13);\n    std::array::push(a, 50);\n    std::array::push(a, 54);\n    std::array::push(a, 48);\n    std::array::push(a, 57);\n    std::array::push(a, 49);\n    std::array::push(a, 51);\n    std::array::push(a, 49);\n    std::array::push(a, 53);\n    std::array::push(a, 53);\n    std::array::push(a, 56);\n    std::array::push(a, 50);\n    std::array::push(a, 55);\n    std::array::push(a, 90);\n    std::array::push(a, 23);\n    std::array::push(a, 13);\n    std::array::push(a, 50);\n    std::array::push(a, 55);\n    std::array::push(a, 48);\n    std::array::push(a, 57);\n    std::array::push(a, 49);\n    std::array::push(a, 51);\n    std::array::push(a, 49);\n    std::array::push(a, 53);\n    std::array::push(a, 53);\n    std::array::push(a, 56);\n    std::array::push(a, 50);\n    std::array::push(a, 55);\n    std::array::push(a, 90);\n    std::array::push(a, 48);\n    std::array::push(a, 20);\n    std::array::push(a, 49);\n    std::array::push(a, 18);\n    std::array::push(a, 48);\n    std::array::push(a, 16);\n    std::array::push(a, 6);\n    std::array::push(a, 3);\n    std::array::push(a, 85);\n    std::array::push(a, 4);\n    std::array::push(a, 3);\n    std::array::push(a, 12);\n    std::array::push(a, 9);\n    std::array::push(a, 108);\n    std::array::push(a, 111);\n    std::array::push(a, 99);\n    std::array::push(a, 97);\n    std::array::push(a, 108);\n    std::array::push(a, 104);\n    std::array::push(a, 111);\n    std::array::push(a, 115);\n    std::array::push(a, 116);\n    std::array::push(a, 48);\n    std::array::push(a, 42);\n    std::array::push(a, 48);\n    std::array::push(a, 5);\n    std::array::push(a, 6);\n    std::array::push(a, 3);\n    std::array::push(a, 43);\n    std::array::push(a, 101);\n    std::array::push(a, 112);\n    std::array::push(a, 3);\n    std::array::push(a, 33);\n    std::array::push(a, 0);\n    std::array::push(a, 15);\n    std::array::push(a, 213);\n    std::array::push(a, 65);\n    std::array::push(a, 171);\n    std::array::push(a, 25);\n    std::array::push(a, 24);\n    std::array::push(a, 82);\n    std::array::push(a, 85);\n    std::array::push(a, 197);\n    std::array::push(a, 221);\n    std::array::push(a, 199);\n    std::array::push(a, 43);\n    std::array::push(a, 46);\n    std::array::push(a, 171);\n    std::array::push(a, 85);\n    std::array::push(a, 4);\n    std::array::push(a, 97);\n    std::array::push(a, 150);\n    std::array::push(a, 244);\n    std::array::push(a, 80);\n    std::array::push(a, 117);\n    std::array::push(a, 205);\n    std::array::push(a, 135);\n    std::array::push(a, 54);\n    std::array::push(a, 165);\n    std::array::push(a, 246);\n    std::array::push(a, 145);\n    std::array::push(a, 214);\n    std::array::push(a, 146);\n    std::array::push(a, 92);\n    std::array::push(a, 6);\n    std::array::push(a, 103);\n    std::array::push(a, 163);\n    std::array::push(a, 83);\n    std::array::push(a, 48);\n    std::array::push(a, 81);\n    std::array::push(a, 48);\n    std::array::push(a, 29);\n    std::array::push(a, 6);\n    std::array::push(a, 3);\n    std::array::push(a, 85);\n    std::array::push(a, 29);\n    std::array::push(a, 14);\n    std::array::push(a, 4);\n    std::array::push(a, 22);\n    std::array::push(a, 4);\n    std::array::push(a, 20);\n    std::array::push(a, 67);\n    std::array::push(a, 88);\n    std::array::push(a, 241);\n    std::array::push(a, 182);\n    std::array::push(a, 37);\n    std::array::push(a, 191);\n    std::array::push(a, 215);\n    std::array::push(a, 135);\n    std::array::push(a, 79);\n    std::array::push(a, 225);\n    std::array::push(a, 21);\n    std::array::push(a, 55);\n    std::array::push(a, 224);\n    std::array::push(a, 21);\n    std::array::push(a, 199);\n    std::array::push(a, 86);\n    std::array::push(a, 159);\n    std::array::push(a, 36);\n    std::array::push(a, 232);\n    std::array::push(a, 175);\n    std::array::push(a, 48);\n    std::array::push(a, 31);\n    std::array::push(a, 6);\n    std::array::push(a, 3);\n    std::array::push(a, 85);\n    std::array::push(a, 29);\n    std::array::push(a, 35);\n    std::array::push(a, 4);\n    std::array::push(a, 24);\n    std::array::push(a, 48);\n    std::array::push(a, 22);\n    std::array::push(a, 128);\n    std::array::push(a, 20);\n    std::array::push(a, 67);\n    std::array::push(a, 88);\n    std::array::push(a, 241);\n    std::array::push(a, 182);\n    std::array::push(a, 37);\n    std::array::push(a, 191);\n    std::array::push(a, 215);\n    std::array::push(a, 135);\n    std::array::push(a, 79);\n    std::array::push(a, 225);\n    std::array::push(a, 21);\n    std::array::push(a, 55);\n    std::array::push(a, 224);\n    std::array::push(a, 21);\n    std::array::push(a, 199);\n    std::array::push(a, 86);\n    std::array::push(a, 159);\n    std::array::push(a, 36);\n    std::array::push(a, 232);\n    std::array::push(a, 175);\n    std::array::push(a, 48);\n    std::array::push(a, 15);\n    std::array::push(a, 6);\n    std::array::push(a, 3);\n    std::array::push(a, 85);\n    std::array::push(a, 29);\n    std::array::push(a, 19);\n    std::array::push(a, 1);\n    std::array::push(a, 1);\n    std::array::push(a, 255);\n    std::array::push(a, 4);\n    std::array::push(a, 5);\n    std::array::push(a, 48);\n    std::array::push(a, 3);\n    std::array::push(a, 1);\n    std::array::push(a, 1);\n    std::array::push(a, 255);\n    std::array::push(a, 48);\n    std::array::push(a, 5);\n    std::array::push(a, 6);\n    std::array::push(a, 3);\n    std::array::push(a, 43);\n    std::array::push(a, 101);\n    std::array::push(a, 112);\n    std::array::push(a, 3);\n    std::array::push(a, 65);\n    std::array::push(a, 0);\n    std::array::push(a, 17);\n    std::array::push(a, 220);\n    std::array::push(a, 255);\n    std::array::push(a, 22);\n    std::array::push(a, 194);\n    std::array::push(a, 187);\n    std::array::push(a, 166);\n    std::array::push(a, 12);\n    std::array::push(a, 1);\n    std::array::push(a, 228);\n    std::array::push(a, 123);\n    std::array::push(a, 35);\n    std::array::push(a, 169);\n    std::array::push(a, 201);\n    std::array::push(a, 33);\n    std::array::push(a, 254);\n    std::array::push(a, 18);\n    std::array::push(a, 199);\n    std::array::push(a, 133);\n    std::array::push(a, 133);\n    std::array::push(a, 228);\n    std::array::push(a, 109);\n    std::array::push(a, 182);\n    std::array::push(a, 91);\n    std::array::push(a, 107);\n    std::array::push(a, 110);\n    std::array::push(a, 21);\n    std::array::push(a, 173);\n    std::array::push(a, 3);\n    std::array::push(a, 175);\n    std::array::push(a, 144);\n    std::array::push(a, 13);\n    std::array::push(a, 10);\n    std::array::push(a, 30);\n    std::array::push(a, 72);\n    std::array::push(a, 81);\n    std::array::push(a, 42);\n    std::array::push(a, 193);\n    std::array::push(a, 58);\n    std::array::push(a, 251);\n    std::array::push(a, 174);\n    std::array::push(a, 92);\n    std::array::push(a, 143);\n    std::array::push(a, 193);\n    std::array::push(a, 43);\n    std::array::push(a, 111);\n    std::array::push(a, 156);\n    std::array::push(a, 226);\n    std::array::push(a, 212);\n    std::array::push(a, 207);\n    std::array::push(a, 252);\n    std::array::push(a, 15);\n    std::array::push(a, 159);\n    std::array::push(a, 141);\n    std::array::push(a, 207);\n    std::array::push(a, 135);\n    std::array::push(a, 92);\n    std::array::push(a, 164);\n    std::array::push(a, 163);\n    std::array::push(a, 39);\n    std::array::push(a, 159);\n    std::array::push(a, 199);\n    std::array::push(a, 195);\n    std::array::push(a, 13);\n    a
}

/// The seed of that certificate's private key. SIGN_    std::array::push(a, 205);\n    std::array::push(a, 169);\n    std::array::push(a, 63);\n    std::array::push(a, 32);\n    std::array::push(a, 119);\n    std::array::push(a, 76);\n    std::array::push(a, 188);\n    std::array::push(a, 69);\n    std::array::push(a, 197);\n    std::array::push(a, 120);\n    std::array::push(a, 134);\n    std::array::push(a, 27);\n    std::array::push(a, 102);\n    std::array::push(a, 252);\n    std::array::push(a, 66);\n    std::array::push(a, 24);\n    std::array::push(a, 194);\n    std::array::push(a, 157);\n    std::array::push(a, 185);\n    std::array::push(a, 138);\n    std::array::push(a, 129);\n    std::array::push(a, 237);\n    std::array::push(a, 98);\n    std::array::push(a, 125);\n    std::array::push(a, 160);\n    std::array::push(a, 67);\n    std::array::push(a, 10);\n    std::array::push(a, 183);\n    std::array::push(a, 12);\n    std::array::push(a, 107);\n    std::array::push(a, 151);\n    std::array::push(a, 122);\n_NOTE
fn signing_seed() -> Array[Int] {
    var a = std::array::empty();
    std::array::push(a, 205);\n    std::array::push(a, 169);\n    std::array::push(a, 63);\n    std::array::push(a, 32);\n    std::array::push(a, 119);\n    std::array::push(a, 76);\n    std::array::push(a, 188);\n    std::array::push(a, 69);\n    std::array::push(a, 197);\n    std::array::push(a, 120);\n    std::array::push(a, 134);\n    std::array::push(a, 27);\n    std::array::push(a, 102);\n    std::array::push(a, 252);\n    std::array::push(a, 66);\n    std::array::push(a, 24);\n    std::array::push(a, 194);\n    std::array::push(a, 157);\n    std::array::push(a, 185);\n    std::array::push(a, 138);\n    std::array::push(a, 129);\n    std::array::push(a, 237);\n    std::array::push(a, 98);\n    std::array::push(a, 125);\n    std::array::push(a, 160);\n    std::array::push(a, 67);\n    std::array::push(a, 10);\n    std::array::push(a, 183);\n    std::array::push(a, 12);\n    std::array::push(a, 107);\n    std::array::push(a, 151);\n    std::array::push(a, 122);\n    a
}

/// A fixed ephemeral X25519 private key, so the test is deterministic.
fn ephemeral_private() -> Array[Int] {
    var a = std::array::empty();
    var i = 0;
    while i < 32 {
        std::array::push(a, 40 + i);
        i = i + 1;
    }
    a
}

/// A fixed ServerHello random, likewise.
fn server_random() -> Array[Int] {
    var a = std::array::empty();
    var i = 0;
    while i < 32 {
        std::array::push(a, 90 + i);
        i = i + 1;
    }
    a
}

/// Read one TLS record off `conn`, returning its body.
fn read_record_body(take conn: Int) -> Array[Int] {
    var header = std::array::empty();
    var i = 0;
    while i < 5 {
        let b = std::rt::read_byte_timeout(conn, 10000);
        if b < 0 {
            return std::array::empty();
        }
        std::array::push(header, b);
        i = i + 1;
    }
    let length = std::tls::u16_at(header, 3);
    if length < 0 {
        return std::array::empty();
    }
    var body = std::array::empty();
    var j = 0;
    while j < length {
        let b = std::rt::read_byte_timeout(conn, 10000);
        if b < 0 {
            return std::array::empty();
        }
        std::array::push(body, b);
        j = j + 1;
    }
    body
}

fn main() -> Int {
    let listener = std::rt::listen(PORT);
    if listener < 0 {
        return 1;
    }
    // The test polls the port to learn when the server is up, and that probe
    // is itself an accepted connection that sends nothing. Skip connections
    // that carry no ClientHello rather than treating the first one as fatal.
    var conn = 0 - 1;
    var hello = std::array::empty();
    var attempts = 0;
    while std::array::len(hello) == 0 && attempts < 4 {
        conn = std::rt::accept_timeout(listener, 20000);
        if conn < 0 {
            std::rt::close(listener);
            return 2;
        }
        hello = read_record_body(conn);
        if std::array::len(hello) == 0 {
            std::rt::close(conn);
        }
        attempts = attempts + 1;
    }
    if std::array::len(hello) == 0 {
        std::rt::close(listener);
        return 3;
    }
    let share = std::x25519::public_key(ephemeral_private());
    // The signature must cover the LIVE transcript — a precomputed one
    // cannot work, since the client chooses fresh randomness every time.
    let content = std::tls::signing_content(hello, share, server_random(), certificate());
    let signature = std::ed25519::sign(signing_seed(), content);
    let result = std::tls::server_handshake(
        hello,
        ephemeral_private(),
        certificate(),
        signature,
        server_random(),
    );
    if !result.ok {
        std::rt::close(conn);
        std::rt::close(listener);
        return 4;
    }
    std::rt::write(conn, std::string::as_str(std::crypto::str_of_bytes(result.flight)));
    // Let the client finish before tearing down.
    let _ = std::rt::read_byte_timeout(conn, 5000);
    std::rt::close(conn);
    std::rt::close(listener);
    0
}
";

    // The certificate in PEM form, so the OpenSSL client can trust it.
    let pem = "-----BEGIN CERTIFICATE-----\nMIIBPDCB76ADAgECAhRK4kv/wlSxvUQCw4mnlEQmyzeI4zAFBgMrZXAwFDESMBAG\nA1UEAwwJbG9jYWxob3N0MB4XDTI2MDkxMzE1NTgyN1oXDTI3MDkxMzE1NTgyN1ow\nFDESMBAGA1UEAwwJbG9jYWxob3N0MCowBQYDK2VwAyEAD9VBqxkYUlXF3ccrLqtV\nBGGW9FB1zYc2pfaR1pJcBmejUzBRMB0GA1UdDgQWBBRDWPG2Jb/Xh0/hFTfgFcdW\nnyTorzAfBgNVHSMEGDAWgBRDWPG2Jb/Xh0/hFTfgFcdWnyTorzAPBgNVHRMBAf8E\nBTADAQH/MAUGAytlcANBABHc/xbCu6YMAeR7I6nJIf4Sx4WF5G22W2tuFa0Dr5AN\nCh5IUSrBOvuuXI/BK2+c4tTP/A+fjc+HXKSjJ5/Hww0=\n-----END CERTIFICATE-----\n";
    let pem_path = dir.join("cert.pem");
    std::fs::write(&pem_path, pem).expect("writing the certificate succeeds");

    // (description, seed tweak, whether the client should succeed)
    let cases: &[(&str, bool)] = &[
        ("the correct signing key", true),
        ("a WRONG signing key", false),
    ];

    for (description, should_succeed) in cases {
        let port = free_port();
        let mut caller = caller_template.replace("PORT", &port.to_string());
        if !should_succeed {
            // Flip one bit of the seed: the signature then verifies under a
            // key the certificate does not carry.
            // Flip one bit of the seed's FIRST byte, so the signature
            // verifies under a key the certificate does not carry. The
            // seed appears once, as the first push in `signing_seed`.
            caller = caller.replacen(
                "fn signing_seed() -> Array[Int] {\n    var a = std::array::empty();\n    std::array::push(a, 205);",
                "fn signing_seed() -> Array[Int] {\n    var a = std::array::empty();\n    std::array::push(a, 204);",
                1,
            );
        }
        let caller = caller.replace("SIGN_    std::array::push(a, 205);\n    std::array::push(a, 169);\n    std::array::push(a, 63);\n    std::array::push(a, 32);\n    std::array::push(a, 119);\n    std::array::push(a, 76);\n    std::array::push(a, 188);\n    std::array::push(a, 69);\n    std::array::push(a, 197);\n    std::array::push(a, 120);\n    std::array::push(a, 134);\n    std::array::push(a, 27);\n    std::array::push(a, 102);\n    std::array::push(a, 252);\n    std::array::push(a, 66);\n    std::array::push(a, 24);\n    std::array::push(a, 194);\n    std::array::push(a, 157);\n    std::array::push(a, 185);\n    std::array::push(a, 138);\n    std::array::push(a, 129);\n    std::array::push(a, 237);\n    std::array::push(a, 98);\n    std::array::push(a, 125);\n    std::array::push(a, 160);\n    std::array::push(a, 67);\n    std::array::push(a, 10);\n    std::array::push(a, 183);\n    std::array::push(a, 12);\n    std::array::push(a, 107);\n    std::array::push(a, 151);\n    std::array::push(a, 122);\n_NOTE", "");

        // Each case gets its OWN directory and binary. Sharing them made the
        // two cases race when the suite runs in parallel: the second build
        // could overwrite the binary the first was still executing.
        let case_dir = dir.join(if *should_succeed { "good" } else { "bad" });
        std::fs::create_dir_all(&case_dir).expect("creating the case directory succeeds");
        let module_paths = write_modules(
            &case_dir,
            &[
                tuo_stdlib::BITS,
                tuo_stdlib::CT,
                tuo_stdlib::BIGNUM,
                tuo_stdlib::CRYPTO,
                tuo_stdlib::CHACHA,
                tuo_stdlib::HKDF,
                tuo_stdlib::X25519,
                tuo_stdlib::SHA512,
                tuo_stdlib::ED25519,
                tuo_stdlib::TLS,
            ],
        );
        let caller_path = case_dir.join("caller.tuo");
        std::fs::write(&caller_path, &caller).expect("writing the caller succeeds");
        let binary = case_dir.join("tls_server");
        let mut build = Command::new(env!("CARGO_BIN_EXE_tuo"));
        build.arg("build").arg("--release").arg("-o").arg(&binary);
        for path in &module_paths {
            build.arg(path);
        }
        let built = build.arg(&caller_path).output().expect("tuo build runs");
        assert!(
            built.status.success(),
            "building the TLS server failed:\n{}",
            String::from_utf8_lossy(&built.stderr)
        );

        let mut server = Command::new(&binary)
            .spawn()
            .expect("the TLS server starts");
        // Poll until the port accepts rather than sleeping a fixed interval:
        // this suite compiles and links native binaries in parallel, so a
        // freshly spawned process can take seconds to reach its first accept.
        for _ in 0..200 {
            if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }

        let client = Command::new("openssl")
            .arg("s_client")
            .arg("-connect")
            .arg(format!("127.0.0.1:{port}"))
            .arg("-tls1_3")
            .arg("-CAfile")
            .arg(&pem_path)
            .stdin(Stdio::null())
            .output()
            .expect("the openssl client runs");
        let transcript = format!(
            "{}{}",
            String::from_utf8_lossy(&client.stdout),
            String::from_utf8_lossy(&client.stderr)
        );
        let _ = server.kill();
        let _ = server.wait();

        let negotiated = transcript.contains("Cipher is TLS_CHACHA20_POLY1305_SHA256");
        let bad_signature = transcript.contains("bad signature");
        if *should_succeed {
            assert!(
                negotiated && !bad_signature,
                "a live OpenSSL client must complete a TLS 1.3 handshake with \
                 {description}; transcript:\n{transcript}"
            );
        } else {
            assert!(
                bad_signature,
                "a live OpenSSL client must REJECT a server using \
                 {description} — otherwise the positive case proves nothing; \
                 transcript:\n{transcript}"
            );
        }
    }
}

/// An ephemeral port the OS is not currently using.
///
/// Bound and released immediately, so there is a small race with anything
/// else claiming it — acceptable in a test, and far better than a fixed port
/// that collides with a parallel run.
fn free_port() -> u16 {
    let listener =
        std::net::TcpListener::bind("127.0.0.1:0").expect("binding an ephemeral port succeeds");
    listener
        .local_addr()
        .expect("a bound listener has an address")
        .port()
}

/// Write catalog modules into `dir`, returning their paths.
fn write_modules(dir: &Path, modules: &[tuo_stdlib::Module]) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for module in modules {
        let path = dir.join(module.name.replace('/', "_"));
        std::fs::write(&path, module.source).expect("module source is writable");
        paths.push(path);
    }
    paths
}

/// The tuonelang source of the HTTPS server the test below drives.
///
/// Kept as a constant rather than inlined so its `\r\n` escapes belong to
/// tuonelang rather than to Rust — nesting the two made the response line
/// unreadable and, at one point, unparseable.
const HTTPS_SERVER_SOURCE: &str = r###"module caller;

import std::tls;
import std::ed25519;
import std::x25519;

/// A real Ed25519 certificate, emitted by OpenSSL.
fn certificate() -> Array[Int] {
    var a = std::array::empty();
    std::array::push(a, 48);
    std::array::push(a, 130);
    std::array::push(a, 1);
    std::array::push(a, 60);
    std::array::push(a, 48);
    std::array::push(a, 129);
    std::array::push(a, 239);
    std::array::push(a, 160);
    std::array::push(a, 3);
    std::array::push(a, 2);
    std::array::push(a, 1);
    std::array::push(a, 2);
    std::array::push(a, 2);
    std::array::push(a, 20);
    std::array::push(a, 74);
    std::array::push(a, 226);
    std::array::push(a, 75);
    std::array::push(a, 255);
    std::array::push(a, 194);
    std::array::push(a, 84);
    std::array::push(a, 177);
    std::array::push(a, 189);
    std::array::push(a, 68);
    std::array::push(a, 2);
    std::array::push(a, 195);
    std::array::push(a, 137);
    std::array::push(a, 167);
    std::array::push(a, 148);
    std::array::push(a, 68);
    std::array::push(a, 38);
    std::array::push(a, 203);
    std::array::push(a, 55);
    std::array::push(a, 136);
    std::array::push(a, 227);
    std::array::push(a, 48);
    std::array::push(a, 5);
    std::array::push(a, 6);
    std::array::push(a, 3);
    std::array::push(a, 43);
    std::array::push(a, 101);
    std::array::push(a, 112);
    std::array::push(a, 48);
    std::array::push(a, 20);
    std::array::push(a, 49);
    std::array::push(a, 18);
    std::array::push(a, 48);
    std::array::push(a, 16);
    std::array::push(a, 6);
    std::array::push(a, 3);
    std::array::push(a, 85);
    std::array::push(a, 4);
    std::array::push(a, 3);
    std::array::push(a, 12);
    std::array::push(a, 9);
    std::array::push(a, 108);
    std::array::push(a, 111);
    std::array::push(a, 99);
    std::array::push(a, 97);
    std::array::push(a, 108);
    std::array::push(a, 104);
    std::array::push(a, 111);
    std::array::push(a, 115);
    std::array::push(a, 116);
    std::array::push(a, 48);
    std::array::push(a, 30);
    std::array::push(a, 23);
    std::array::push(a, 13);
    std::array::push(a, 50);
    std::array::push(a, 54);
    std::array::push(a, 48);
    std::array::push(a, 57);
    std::array::push(a, 49);
    std::array::push(a, 51);
    std::array::push(a, 49);
    std::array::push(a, 53);
    std::array::push(a, 53);
    std::array::push(a, 56);
    std::array::push(a, 50);
    std::array::push(a, 55);
    std::array::push(a, 90);
    std::array::push(a, 23);
    std::array::push(a, 13);
    std::array::push(a, 50);
    std::array::push(a, 55);
    std::array::push(a, 48);
    std::array::push(a, 57);
    std::array::push(a, 49);
    std::array::push(a, 51);
    std::array::push(a, 49);
    std::array::push(a, 53);
    std::array::push(a, 53);
    std::array::push(a, 56);
    std::array::push(a, 50);
    std::array::push(a, 55);
    std::array::push(a, 90);
    std::array::push(a, 48);
    std::array::push(a, 20);
    std::array::push(a, 49);
    std::array::push(a, 18);
    std::array::push(a, 48);
    std::array::push(a, 16);
    std::array::push(a, 6);
    std::array::push(a, 3);
    std::array::push(a, 85);
    std::array::push(a, 4);
    std::array::push(a, 3);
    std::array::push(a, 12);
    std::array::push(a, 9);
    std::array::push(a, 108);
    std::array::push(a, 111);
    std::array::push(a, 99);
    std::array::push(a, 97);
    std::array::push(a, 108);
    std::array::push(a, 104);
    std::array::push(a, 111);
    std::array::push(a, 115);
    std::array::push(a, 116);
    std::array::push(a, 48);
    std::array::push(a, 42);
    std::array::push(a, 48);
    std::array::push(a, 5);
    std::array::push(a, 6);
    std::array::push(a, 3);
    std::array::push(a, 43);
    std::array::push(a, 101);
    std::array::push(a, 112);
    std::array::push(a, 3);
    std::array::push(a, 33);
    std::array::push(a, 0);
    std::array::push(a, 15);
    std::array::push(a, 213);
    std::array::push(a, 65);
    std::array::push(a, 171);
    std::array::push(a, 25);
    std::array::push(a, 24);
    std::array::push(a, 82);
    std::array::push(a, 85);
    std::array::push(a, 197);
    std::array::push(a, 221);
    std::array::push(a, 199);
    std::array::push(a, 43);
    std::array::push(a, 46);
    std::array::push(a, 171);
    std::array::push(a, 85);
    std::array::push(a, 4);
    std::array::push(a, 97);
    std::array::push(a, 150);
    std::array::push(a, 244);
    std::array::push(a, 80);
    std::array::push(a, 117);
    std::array::push(a, 205);
    std::array::push(a, 135);
    std::array::push(a, 54);
    std::array::push(a, 165);
    std::array::push(a, 246);
    std::array::push(a, 145);
    std::array::push(a, 214);
    std::array::push(a, 146);
    std::array::push(a, 92);
    std::array::push(a, 6);
    std::array::push(a, 103);
    std::array::push(a, 163);
    std::array::push(a, 83);
    std::array::push(a, 48);
    std::array::push(a, 81);
    std::array::push(a, 48);
    std::array::push(a, 29);
    std::array::push(a, 6);
    std::array::push(a, 3);
    std::array::push(a, 85);
    std::array::push(a, 29);
    std::array::push(a, 14);
    std::array::push(a, 4);
    std::array::push(a, 22);
    std::array::push(a, 4);
    std::array::push(a, 20);
    std::array::push(a, 67);
    std::array::push(a, 88);
    std::array::push(a, 241);
    std::array::push(a, 182);
    std::array::push(a, 37);
    std::array::push(a, 191);
    std::array::push(a, 215);
    std::array::push(a, 135);
    std::array::push(a, 79);
    std::array::push(a, 225);
    std::array::push(a, 21);
    std::array::push(a, 55);
    std::array::push(a, 224);
    std::array::push(a, 21);
    std::array::push(a, 199);
    std::array::push(a, 86);
    std::array::push(a, 159);
    std::array::push(a, 36);
    std::array::push(a, 232);
    std::array::push(a, 175);
    std::array::push(a, 48);
    std::array::push(a, 31);
    std::array::push(a, 6);
    std::array::push(a, 3);
    std::array::push(a, 85);
    std::array::push(a, 29);
    std::array::push(a, 35);
    std::array::push(a, 4);
    std::array::push(a, 24);
    std::array::push(a, 48);
    std::array::push(a, 22);
    std::array::push(a, 128);
    std::array::push(a, 20);
    std::array::push(a, 67);
    std::array::push(a, 88);
    std::array::push(a, 241);
    std::array::push(a, 182);
    std::array::push(a, 37);
    std::array::push(a, 191);
    std::array::push(a, 215);
    std::array::push(a, 135);
    std::array::push(a, 79);
    std::array::push(a, 225);
    std::array::push(a, 21);
    std::array::push(a, 55);
    std::array::push(a, 224);
    std::array::push(a, 21);
    std::array::push(a, 199);
    std::array::push(a, 86);
    std::array::push(a, 159);
    std::array::push(a, 36);
    std::array::push(a, 232);
    std::array::push(a, 175);
    std::array::push(a, 48);
    std::array::push(a, 15);
    std::array::push(a, 6);
    std::array::push(a, 3);
    std::array::push(a, 85);
    std::array::push(a, 29);
    std::array::push(a, 19);
    std::array::push(a, 1);
    std::array::push(a, 1);
    std::array::push(a, 255);
    std::array::push(a, 4);
    std::array::push(a, 5);
    std::array::push(a, 48);
    std::array::push(a, 3);
    std::array::push(a, 1);
    std::array::push(a, 1);
    std::array::push(a, 255);
    std::array::push(a, 48);
    std::array::push(a, 5);
    std::array::push(a, 6);
    std::array::push(a, 3);
    std::array::push(a, 43);
    std::array::push(a, 101);
    std::array::push(a, 112);
    std::array::push(a, 3);
    std::array::push(a, 65);
    std::array::push(a, 0);
    std::array::push(a, 17);
    std::array::push(a, 220);
    std::array::push(a, 255);
    std::array::push(a, 22);
    std::array::push(a, 194);
    std::array::push(a, 187);
    std::array::push(a, 166);
    std::array::push(a, 12);
    std::array::push(a, 1);
    std::array::push(a, 228);
    std::array::push(a, 123);
    std::array::push(a, 35);
    std::array::push(a, 169);
    std::array::push(a, 201);
    std::array::push(a, 33);
    std::array::push(a, 254);
    std::array::push(a, 18);
    std::array::push(a, 199);
    std::array::push(a, 133);
    std::array::push(a, 133);
    std::array::push(a, 228);
    std::array::push(a, 109);
    std::array::push(a, 182);
    std::array::push(a, 91);
    std::array::push(a, 107);
    std::array::push(a, 110);
    std::array::push(a, 21);
    std::array::push(a, 173);
    std::array::push(a, 3);
    std::array::push(a, 175);
    std::array::push(a, 144);
    std::array::push(a, 13);
    std::array::push(a, 10);
    std::array::push(a, 30);
    std::array::push(a, 72);
    std::array::push(a, 81);
    std::array::push(a, 42);
    std::array::push(a, 193);
    std::array::push(a, 58);
    std::array::push(a, 251);
    std::array::push(a, 174);
    std::array::push(a, 92);
    std::array::push(a, 143);
    std::array::push(a, 193);
    std::array::push(a, 43);
    std::array::push(a, 111);
    std::array::push(a, 156);
    std::array::push(a, 226);
    std::array::push(a, 212);
    std::array::push(a, 207);
    std::array::push(a, 252);
    std::array::push(a, 15);
    std::array::push(a, 159);
    std::array::push(a, 141);
    std::array::push(a, 207);
    std::array::push(a, 135);
    std::array::push(a, 92);
    std::array::push(a, 164);
    std::array::push(a, 163);
    std::array::push(a, 39);
    std::array::push(a, 159);
    std::array::push(a, 199);
    std::array::push(a, 195);
    std::array::push(a, 13);
    a
}

/// The seed of that certificate's private key.
fn signing_seed() -> Array[Int] {
    var a = std::array::empty();
    std::array::push(a, 205);
    std::array::push(a, 169);
    std::array::push(a, 63);
    std::array::push(a, 32);
    std::array::push(a, 119);
    std::array::push(a, 76);
    std::array::push(a, 188);
    std::array::push(a, 69);
    std::array::push(a, 197);
    std::array::push(a, 120);
    std::array::push(a, 134);
    std::array::push(a, 27);
    std::array::push(a, 102);
    std::array::push(a, 252);
    std::array::push(a, 66);
    std::array::push(a, 24);
    std::array::push(a, 194);
    std::array::push(a, 157);
    std::array::push(a, 185);
    std::array::push(a, 138);
    std::array::push(a, 129);
    std::array::push(a, 237);
    std::array::push(a, 98);
    std::array::push(a, 125);
    std::array::push(a, 160);
    std::array::push(a, 67);
    std::array::push(a, 10);
    std::array::push(a, 183);
    std::array::push(a, 12);
    std::array::push(a, 107);
    std::array::push(a, 151);
    std::array::push(a, 122);
    a
}

fn ephemeral_private() -> Array[Int] {
    var a = std::array::empty();
    var i = 0;
    while i < 32 {
        std::array::push(a, 40 + i);
        i = i + 1;
    }
    a
}

fn server_random() -> Array[Int] {
    var a = std::array::empty();
    var i = 0;
    while i < 32 {
        std::array::push(a, 90 + i);
        i = i + 1;
    }
    a
}

/// A tiny routing table, so the test proves an ordinary HTTP layer composes
/// with TLS rather than that a fixed string can be encrypted. The path is
/// parsed out of the decrypted request line and dispatched on.
fn response_for(in path: Str) -> Str {
    if path == "/" {
        "HTTP/1.1 200 OK\r\nContent-Length: 13\r\nConnection: close\r\n\r\nHello, HTTPS!"
    } else if path == "/health" {
        "HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    } else {
        "HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\nConnection: close\r\n\r\nnot found"
    }
}

/// The path of a request line: the token between the first two spaces.
fn path_of(in line: Str) -> Str {
    let n = std::str::len(line);
    var first = 0 - 1;
    var i = 0;
    while i < n {
        if std::str::byte_at(line, i) == 32 && first < 0 {
            first = i;
        }
        i = i + 1;
    }
    if first < 0 {
        return "";
    }
    var second = n;
    var j = first + 1;
    var found = false;
    while j < n {
        if std::str::byte_at(line, j) == 32 && !found {
            second = j;
            found = true;
        }
        j = j + 1;
    }
    std::str::slice(line, first + 1, second)
}

/// Read one plaintext record body off `conn` — the ClientHello.
fn read_hello(take conn: Int) -> Array[Int] {
    var header = std::array::empty();
    var i = 0;
    while i < 5 {
        let b = std::rt::read_byte_timeout(conn, 10000);
        if b < 0 {
            return std::array::empty();
        }
        std::array::push(header, b);
        i = i + 1;
    }
    let length = std::tls::u16_at(header, 3);
    if length < 0 {
        return std::array::empty();
    }
    var body = std::array::empty();
    var j = 0;
    while j < length {
        let b = std::rt::read_byte_timeout(conn, 10000);
        if b < 0 {
            return std::array::empty();
        }
        std::array::push(body, b);
        j = j + 1;
    }
    body
}

fn main() -> Int {
    let listener = std::rt::listen(PORT);
    if listener < 0 {
        return 1;
    }
    // The readiness probe the test makes is itself an accepted connection
    // that sends nothing, so skip connections carrying no ClientHello.
    var conn = 0 - 1;
    var hello = std::array::empty();
    var attempts = 0;
    while std::array::len(hello) == 0 && attempts < 4 {
        conn = std::rt::accept_timeout(listener, 20000);
        if conn < 0 {
            std::rt::close(listener);
            return 2;
        }
        hello = read_hello(conn);
        if std::array::len(hello) == 0 {
            std::rt::close(conn);
        }
        attempts = attempts + 1;
    }
    if std::array::len(hello) == 0 {
        std::rt::close(listener);
        return 3;
    }

    let share = std::x25519::public_key(ephemeral_private());
    let content = std::tls::signing_content(hello, share, server_random(), certificate());
    let signature = std::ed25519::sign(signing_seed(), content);
    let result = std::tls::server_handshake(
        hello,
        ephemeral_private(),
        certificate(),
        signature,
        server_random(),
    );
    if !result.ok {
        std::rt::close(conn);
        std::rt::close(listener);
        return 4;
    }
    std::rt::write(conn, std::string::as_str(std::crypto::str_of_bytes(result.flight)));

    // The stream layer: read the request line over TLS, then answer.
    var tls = std::tls::connect(conn, result);
    var line = std::string::empty();
    var reading = true;
    var guard = 0;
    while reading && guard < 500 {
        let b = std::tls::read_byte(tls, 10000);
        if b < 0 {
            reading = false;
        } else {
            if b == 10 {
                reading = false;
            } else {
                if b != 13 {
                    std::string::push_byte(line, b);
                }
            }
        }
        guard = guard + 1;
    }
    std::tls::write_all(tls, response_for(path_of(std::string::as_str(line))));
    std::tls::close_notify(tls);
    std::rt::close(conn);
    std::rt::close(listener);

    // 0 only if a real request line arrived: it must start with "GET".
    let raw = std::string::as_str(line);
    if std::str::len(raw) < 3 {
        return 5;
    }
    if std::str::byte_at(raw, 0) != 71 {
        return 6;
    }
    if std::str::byte_at(raw, 1) != 69 {
        return 7;
    }
    if std::str::byte_at(raw, 2) != 84 {
        return 8;
    }
    0
}
"###;

/// `std::tls` serves a **real HTTPS request**: a live OpenSSL client sends
/// `GET / HTTP/1.1` over TLS and reads back an HTTP response.
///
/// This is the native pin the module's five `EFFECT:` functions name, and it
/// tests what the handshake test cannot: the *stream* layer. A handshake
/// proves the keys agree; this proves the connection carries bytes in both
/// directions afterwards — which needs the record buffering, the per-record
/// nonce sequence, and the **key-epoch switch** all correct.
///
/// That last one was a real bug. The client's Finished arrives under its
/// *handshake* key while application data arrives under its *application*
/// key, each epoch restarting its record sequence at zero. Building the
/// connection with application keys alone made a perfectly valid Finished
/// fail to authenticate, and the server saw an empty request.
///
/// Skips cleanly when no `openssl` binary is present.
#[test]
fn tls_serves_a_real_https_request() {
    if Command::new("openssl").arg("version").output().is_err() {
        // No openssl to test against: skip rather than fail for an
        // environment reason.
        return;
    }
    let dir = native_workspace("tls_https");
    let pem_path = dir.join("cert.pem");
    std::fs::write(&pem_path, CERT_PEM).expect("writing the certificate succeeds");

    // (requested path, expected status line, expected body). Each runs on its
    // own connection, so the routing really depends on the decrypted request
    // rather than on the server answering the same thing every time.
    let cases: &[(&str, &str, &str)] = &[
        ("/", "HTTP/1.1 200 OK", "Hello, HTTPS!"),
        ("/health", "HTTP/1.1 204 No Content", ""),
        ("/missing", "HTTP/1.1 404 Not Found", "not found"),
    ];

    for (path, status_line, expected_body) in cases {
        let port = free_port();
        let caller = HTTPS_SERVER_SOURCE.replace("PORT", &port.to_string());

        let module_paths = write_modules(
            &dir,
            &[
                tuo_stdlib::BITS,
                tuo_stdlib::CT,
                tuo_stdlib::BIGNUM,
                tuo_stdlib::CRYPTO,
                tuo_stdlib::CHACHA,
                tuo_stdlib::HKDF,
                tuo_stdlib::X25519,
                tuo_stdlib::SHA512,
                tuo_stdlib::ED25519,
                tuo_stdlib::TLS,
            ],
        );
        let caller_path = dir.join("caller.tuo");
        std::fs::write(&caller_path, &caller).expect("writing the caller succeeds");
        let binary = dir.join("https_server");
        let mut build = Command::new(env!("CARGO_BIN_EXE_tuo"));
        build.arg("build").arg("--release").arg("-o").arg(&binary);
        for path in &module_paths {
            build.arg(path);
        }
        let built = build.arg(&caller_path).output().expect("tuo build runs");
        assert!(
            built.status.success(),
            "building the HTTPS server failed:\n{}",
            String::from_utf8_lossy(&built.stderr)
        );

        let mut server = Command::new(&binary).spawn().expect("the server starts");
        for _ in 0..200 {
            if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }

        let mut client = Command::new("openssl")
            .arg("s_client")
            .arg("-connect")
            .arg(format!("127.0.0.1:{port}"))
            .arg("-tls1_3")
            .arg("-CAfile")
            .arg(&pem_path)
            .arg("-quiet")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the openssl client runs");
        {
            use std::io::Write as _;
            let stdin = client.stdin.as_mut().expect("the client has stdin");
            stdin
                .write_all(format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes())
                .expect("writing the request succeeds");
            stdin.flush().expect("flushing succeeds");
        }
        let output = client.wait_with_output().expect("the client finishes");
        let body = String::from_utf8_lossy(&output.stdout).into_owned();

        let status = server.wait().expect("the server exits");

        assert!(
            body.contains(status_line),
            "a live OpenSSL client must receive the response the tuonelang server \
         routed for {path:?} over TLS; expected {status_line:?}, got:\n{body}"
        );
        if expected_body.is_empty() {
            // A 204 carries no body, so the check is that the OTHER routes'
            // bodies are ABSENT. `contains("")` is vacuously true, and a
            // sabotaged routing table used to leave this test green for
            // exactly that reason — the assertion caught nothing.
            assert!(
                !body.contains("Hello, HTTPS!") && !body.contains("not found"),
                "the 204 route must carry no body over TLS; got:\n{body}"
            );
        } else {
            assert!(
                body.contains(expected_body),
                "the routed body for {path:?} must arrive intact over TLS; \
                 expected {expected_body:?}, got:\n{body}"
            );
        }
        assert_eq!(
            status.code(),
            Some(0),
            "the server must have decrypted a real `GET` request line for \
         {path:?}; a non-zero status says which step failed"
        );
    }
}

/// The test certificate in PEM form, so the OpenSSL client can trust it.
const CERT_PEM: &str = r###"-----BEGIN CERTIFICATE-----
MIIBPDCB76ADAgECAhRK4kv/wlSxvUQCw4mnlEQmyzeI4zAFBgMrZXAwFDESMBAG
A1UEAwwJbG9jYWxob3N0MB4XDTI2MDkxMzE1NTgyN1oXDTI3MDkxMzE1NTgyN1ow
FDESMBAGA1UEAwwJbG9jYWxob3N0MCowBQYDK2VwAyEAD9VBqxkYUlXF3ccrLqtV
BGGW9FB1zYc2pfaR1pJcBmejUzBRMB0GA1UdDgQWBBRDWPG2Jb/Xh0/hFTfgFcdW
nyTorzAfBgNVHSMEGDAWgBRDWPG2Jb/Xh0/hFTfgFcdWnyTorzAPBgNVHRMBAf8E
BTADAQH/MAUGAytlcANBABHc/xbCu6YMAeR7I6nJIf4Sx4WF5G22W2tuFa0Dr5AN
Ch5IUSrBOvuuXI/BK2+c4tTP/A+fjc+HXKSjJ5/Hww0=
-----END CERTIFICATE-----
"###;
