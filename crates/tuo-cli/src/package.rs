//! The `tuo` package commands: `new`, `add`, `remove`, `build`, `check`,
//! `verify`, and `test`.
//!
//! These operate on a **package** — a directory holding a `tdg.toml` manifest
//! and a module root of `.tuo` sources — rather than on a bare list of files.
//! Two families:
//!
//! * **Manifest management** — `new` scaffolds a package; `add`/`remove` edit
//!   its `[dependencies]` and re-resolve the lockfile. These change files on
//!   disk and report a short confirmation.
//! * **Compile/run** — `check`, `build`, `verify`, and `test` resolve the
//!   package's dependency graph ([`tuo_package::resolve`]), load every module
//!   source across the graph into one `SourceMap`, and then drive the **exact
//!   same** front end, backend, and spec runner the file-based commands use.
//!   The package layer only decides *which sources* form the program; the
//!   compiler decides everything else.
//!
//! Resolution is deterministic and path-dependency based, so a package always
//! resolves to the same lockfile. Every compile command re-resolves and rewrites
//! `tdg.lock`, and `verify`/`build`/`test` additionally check the resolved
//! sources against the previous lockfile's checksums so a build never silently
//! compiles drifted dependency bytes.
//!
//! In a machine format, package commands drive the [`crate::protocol`] event
//! stream on stdout exactly like the file-based commands; the compile commands
//! reuse those commands' event shapes verbatim.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::{Value, json};
use tuo_compiler::check_sources;
use tuo_compiler::resolve::SymbolKind;
use tuo_compiler::source::{SourceId, SourceMap};
use tuo_package::resolve::{LOCKFILE_FILE, MANIFEST_FILE};
use tuo_package::{
    Lockfile, Manifest, PackageName, ResolveError, ResolvedGraph, add_path_dependency,
    remove_dependency,
};
use tuo_spec::Selection;

use crate::output::OutputMode;
use crate::protocol::{Event, ProtocolCommand, Status};
use crate::{check, codegen, spec};

/// `tuo new <name> [--path <dir>]`: scaffold a new package.
#[expect(
    clippy::print_stderr,
    reason = "the CLI presentation layer reports scaffolding results on stderr in human mode"
)]
pub(crate) fn new(name: &str, dir: Option<PathBuf>, mode: OutputMode) -> ExitCode {
    let package_name = match PackageName::new(name) {
        Ok(n) => n,
        Err(error) => return simple_error(ProtocolCommand::Package, mode, &error.to_string()),
    };
    // The package directory: `--path` if given, else a directory named after
    // the package under the current directory.
    let target = dir.unwrap_or_else(|| PathBuf::from(name));

    if target.join(MANIFEST_FILE).exists() {
        return simple_error(
            ProtocolCommand::Package,
            mode,
            &format!("{} already exists", target.join(MANIFEST_FILE).display()),
        );
    }

    let files = tuo_package::new_package(&package_name);
    for file in &files {
        let path = target.join(&file.relative_path);
        if let Some(parent) = path.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                return simple_error(
                    ProtocolCommand::Package,
                    mode,
                    &format!("cannot create {}: {error}", parent.display()),
                );
            }
        }
        if let Err(error) = std::fs::write(&path, &file.contents) {
            return simple_error(
                ProtocolCommand::Package,
                mode,
                &format!("cannot write {}: {error}", path.display()),
            );
        }
    }

    if mode.is_machine() {
        emit_simple(
            ProtocolCommand::Package,
            mode,
            Status::Ok,
            json!({ "action": "new", "package": name, "path": target.display().to_string() }),
        );
    } else {
        eprintln!("created package `{name}` at {}", target.display());
    }
    ExitCode::SUCCESS
}

/// `tuo add <name> --path <dir> [--manifest <dir>]`: add a path dependency.
#[expect(
    clippy::print_stderr,
    reason = "the CLI presentation layer reports results on stderr in human mode"
)]
pub(crate) fn add(name: &str, dep_path: &str, dir: &Path, mode: OutputMode) -> ExitCode {
    let dep_name = match PackageName::new(name) {
        Ok(n) => n,
        Err(error) => return simple_error(ProtocolCommand::Package, mode, &error.to_string()),
    };
    let mut manifest = match load_manifest(dir, mode) {
        Ok(m) => m,
        Err(code) => return code,
    };
    if let Err(error) = add_path_dependency(&mut manifest, dep_name, dep_path) {
        return simple_error(ProtocolCommand::Package, mode, &error.to_string());
    }
    if let Err(code) = write_manifest(dir, &manifest, mode) {
        return code;
    }
    // Re-resolve so the lockfile reflects the new dependency (and so a bad path
    // is caught now, not at the next build).
    if let Err(code) = relock(dir, mode) {
        return code;
    }
    if mode.is_machine() {
        emit_simple(
            ProtocolCommand::Package,
            mode,
            Status::Ok,
            json!({ "action": "add", "dependency": name, "path": dep_path }),
        );
    } else {
        eprintln!("added dependency `{name}` (path = \"{dep_path}\")");
    }
    ExitCode::SUCCESS
}

/// `tuo remove <name> [--manifest <dir>]`: remove a dependency.
#[expect(
    clippy::print_stderr,
    reason = "the CLI presentation layer reports results on stderr in human mode"
)]
pub(crate) fn remove(name: &str, dir: &Path, mode: OutputMode) -> ExitCode {
    let dep_name = match PackageName::new(name) {
        Ok(n) => n,
        Err(error) => return simple_error(ProtocolCommand::Package, mode, &error.to_string()),
    };
    let mut manifest = match load_manifest(dir, mode) {
        Ok(m) => m,
        Err(code) => return code,
    };
    if let Err(error) = remove_dependency(&mut manifest, &dep_name) {
        return simple_error(ProtocolCommand::Package, mode, &error.to_string());
    }
    if let Err(code) = write_manifest(dir, &manifest, mode) {
        return code;
    }
    if let Err(code) = relock(dir, mode) {
        return code;
    }
    if mode.is_machine() {
        emit_simple(
            ProtocolCommand::Package,
            mode,
            Status::Ok,
            json!({ "action": "remove", "dependency": name }),
        );
    } else {
        eprintln!("removed dependency `{name}`");
    }
    ExitCode::SUCCESS
}

/// `tuo check [--manifest <dir>]`: resolve the package and front-end-check its
/// whole graph as one program.
pub(crate) fn check(dir: &Path, mode: OutputMode) -> ExitCode {
    compile_command(dir, mode, |map, sources, names, mode| {
        check::report(map, sources, names, ProtocolCommand::Check, mode)
    })
}

/// `tuo build [-o out] [--release] [--manifest <dir>]`: resolve and compile the
/// package to a native executable.
pub(crate) fn build(
    dir: &Path,
    output: Option<PathBuf>,
    release: bool,
    mode: OutputMode,
) -> ExitCode {
    compile_command(dir, mode, move |map, sources, names, mode| {
        codegen::build_loaded(map, sources, names, output.clone(), release, mode)
    })
}

/// `tuo verify [--manifest <dir>]`: every static check plus all specs across the
/// resolved graph.
pub(crate) fn verify(dir: &Path, mode: OutputMode) -> ExitCode {
    compile_command(dir, mode, |map, sources, names, mode| {
        spec::execute(
            map,
            sources,
            &Selection::All,
            names,
            ProtocolCommand::Verify,
            mode,
        )
    })
}

/// `tuo test [--manifest <dir>]`: run the package's specs — the tuonelang test
/// mechanism — across the resolved graph. In v0 a package's tests are its
/// colocated specs, so `test` executes exactly the specs `verify` does; it
/// exists as the conventionally-named command.
pub(crate) fn test(dir: &Path, mode: OutputMode) -> ExitCode {
    compile_command(dir, mode, |map, sources, names, mode| {
        spec::execute(
            map,
            sources,
            &Selection::All,
            names,
            ProtocolCommand::Test,
            mode,
        )
    })
}

/// The shared body of the compile commands: resolve the package graph, verify
/// it against (and rewrite) the lockfile, load its sources, then hand off to
/// `run` (the file-based command's presentation).
fn compile_command(
    dir: &Path,
    mode: OutputMode,
    run: impl FnOnce(&SourceMap, &[SourceId], &[PathBuf], OutputMode) -> ExitCode,
) -> ExitCode {
    let graph = match resolve_and_lock(dir, mode) {
        Ok(graph) => graph,
        Err(code) => return code,
    };
    let (map, sources, names) = load_graph(&graph);
    run(&map, &sources, &names, mode)
}

/// Resolve the package at `dir`, verify it against the prior lockfile (if any),
/// then write the fresh lockfile. Returns the resolved graph.
fn resolve_and_lock(dir: &Path, mode: OutputMode) -> Result<ResolvedGraph, ExitCode> {
    let graph = resolve(dir, mode)?;

    // If a lockfile already exists, the freshly resolved sources must still
    // match its checksums — otherwise a dependency's bytes drifted and we refuse
    // rather than compile something the lock does not describe.
    let lock_path = dir.join(LOCKFILE_FILE);
    if let Ok(text) = std::fs::read_to_string(&lock_path) {
        match Lockfile::parse(&text) {
            Ok(prior) => {
                if let Err(error) = tuo_package::verify_against_lock(&graph, &prior) {
                    return Err(simple_error(
                        ProtocolCommand::Package,
                        mode,
                        &error.to_string(),
                    ));
                }
            }
            Err(error) => {
                return Err(simple_error(
                    ProtocolCommand::Package,
                    mode,
                    &format!("{}: {error}", lock_path.display()),
                ));
            }
        }
    }

    // Write (or refresh) the lockfile from the resolved graph.
    let lockfile = graph.to_lockfile();
    if let Err(error) = std::fs::write(&lock_path, lockfile.to_toml()) {
        return Err(simple_error(
            ProtocolCommand::Package,
            mode,
            &format!("cannot write {}: {error}", lock_path.display()),
        ));
    }
    Ok(graph)
}

/// Re-resolve and rewrite the lockfile without compiling — used by `add`/`remove`
/// so the lockfile stays in sync with the manifest.
fn relock(dir: &Path, mode: OutputMode) -> Result<(), ExitCode> {
    resolve_and_lock(dir, mode).map(|_| ())
}

/// Resolve the package at `dir`, mapping a resolve error to an exit code.
fn resolve(dir: &Path, mode: OutputMode) -> Result<ResolvedGraph, ExitCode> {
    tuo_package::resolve(dir).map_err(|error: ResolveError| {
        simple_error(ProtocolCommand::Package, mode, &error.to_string())
    })
}

/// Load every module source across the resolved graph into one `SourceMap`,
/// returning the map, the source ids, and display names (one per module) for
/// machine-mode reporting.
fn load_graph(graph: &ResolvedGraph) -> (SourceMap, Vec<SourceId>, Vec<PathBuf>) {
    let mut map = SourceMap::new();
    let mut sources = Vec::new();
    let mut names = Vec::new();
    for module in graph.all_modules() {
        let file = map.intern_file(&module.name);
        // A stdlib-sized module always fits; a resolved source came from disk,
        // so if interning fails we still surface it as an empty source rather
        // than panicking. In practice `add_source` only rejects oversize input.
        if let Ok(id) = map.add_source(file, module.text.as_str()) {
            sources.push(id);
            names.push(PathBuf::from(&module.name));
        }
    }
    (map, sources, names)
}

/// `tuo package symbols [--manifest <dir>]`: the machine-queryable symbol
/// surface of a resolved package graph.
///
/// This answers "what does this package export?" **without guessing**: it
/// resolves and compiles the real package sources and reports the actual
/// public, module-level symbols the front end produced — the same `Resolution`
/// symbols the agent protocol and LSP project. It is a read-only query (it does
/// not write a lockfile) and is available only in a machine format, since it is
/// a protocol for tools, not a human report.
pub(crate) fn symbols(dir: &Path, mode: OutputMode) -> ExitCode {
    if !mode.is_machine() {
        return simple_error(
            ProtocolCommand::Package,
            mode,
            "`package symbols` is a machine query; run it with --message-format=json",
        );
    }
    let graph = match resolve(dir, mode) {
        Ok(graph) => graph,
        Err(code) => return code,
    };
    let (map, sources, _names) = load_graph(&graph);
    let checked = check_sources(&map, &sources);

    let mut list: Vec<Value> = Vec::new();
    for (_, symbol) in checked.resolution.symbols() {
        if !symbol.is_pub {
            continue;
        }
        if !matches!(
            symbol.kind,
            SymbolKind::Function | SymbolKind::Struct | SymbolKind::Enum
        ) {
            continue;
        }
        list.push(json!({
            "name": symbol.name,
            "kind": symbol.kind.noun(),
        }));
    }
    // Stable order so the query is deterministic.
    list.sort_by(|a, b| {
        let ka = (
            a["kind"].as_str().unwrap_or(""),
            a["name"].as_str().unwrap_or(""),
        );
        let kb = (
            b["kind"].as_str().unwrap_or(""),
            b["name"].as_str().unwrap_or(""),
        );
        ka.cmp(&kb)
    });

    emit_simple(
        ProtocolCommand::Package,
        mode,
        Status::Ok,
        json!({
            "action": "symbols",
            "package": graph.root.as_str(),
            "symbols": list,
            "has_errors": checked.has_errors(),
        }),
    );
    ExitCode::SUCCESS
}

/// Load the manifest at `dir/tdg.toml`, reporting a read/parse error.
fn load_manifest(dir: &Path, mode: OutputMode) -> Result<Manifest, ExitCode> {
    let path = dir.join(MANIFEST_FILE);
    let text = std::fs::read_to_string(&path).map_err(|error| {
        simple_error(
            ProtocolCommand::Package,
            mode,
            &format!("cannot read {}: {error}", path.display()),
        )
    })?;
    Manifest::parse(&text).map_err(|error| {
        simple_error(
            ProtocolCommand::Package,
            mode,
            &format!("{path:?}: {error}"),
        )
    })
}

/// Write a manifest back to `dir/tdg.toml` in canonical form.
fn write_manifest(dir: &Path, manifest: &Manifest, mode: OutputMode) -> Result<(), ExitCode> {
    let path = dir.join(MANIFEST_FILE);
    std::fs::write(&path, manifest.to_toml()).map_err(|error| {
        simple_error(
            ProtocolCommand::Package,
            mode,
            &format!("cannot write {}: {error}", path.display()),
        )
    })
}

/// Report a one-off error through `mode` and return a failing exit code.
#[expect(
    clippy::print_stderr,
    reason = "the CLI presentation layer reports errors on stderr in human mode"
)]
fn simple_error(command: ProtocolCommand, mode: OutputMode, message: &str) -> ExitCode {
    if mode.is_machine() {
        emit_simple(command, mode, Status::Error, json!({ "message": message }));
    } else {
        eprintln!("error: {message}");
    }
    ExitCode::FAILURE
}

/// Emit a single-`item`, single-`finished` protocol exchange for a package
/// action (the manifest commands and `symbols` are not streamed like a spec
/// run; they report one result).
fn emit_simple(command: ProtocolCommand, mode: OutputMode, status: Status, payload: Value) {
    let Some(mut emitter) = mode.emitter(command) else {
        return;
    };
    let write = (|| -> std::io::Result<()> {
        emitter.emit(&Event::started(&[] as &[String]))?;
        let mut item = payload;
        if item.get("kind").is_none() {
            if let Value::Object(map) = &mut item {
                map.insert(
                    "kind".to_string(),
                    Value::String("package_result".to_string()),
                );
            }
        }
        emitter.emit(&Event::item(status, item))?;
        emitter.emit(&Event::finished(status, json!({})))?;
        emitter.finish()
    })();
    if write.is_err() {
        mode.log("protocol: stdout write failed");
    }
}
