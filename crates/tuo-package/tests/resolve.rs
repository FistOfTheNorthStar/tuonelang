//! Filesystem-level tests for dependency resolution, over real temporary
//! package trees. These pin the resolver's graph behavior — transitive path
//! dependencies, deterministic ordering, cycle and duplicate detection,
//! checksum drift — directly, without going through the CLI.

use std::fs;
use std::path::{Path, PathBuf};

use tuo_package::{Lockfile, ResolveError, resolve, verify_against_lock};

/// A unique scratch directory per test.
fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("resolve")
        .join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch dir is creatable");
    dir
}

/// Write a minimal package at `dir`: a manifest naming `name` (with the given
/// `[dependencies]` body) and one module under `src/`.
fn write_package(dir: &Path, name: &str, deps: &str, module: &str) {
    fs::create_dir_all(dir.join("src")).expect("src dir");
    fs::write(
        dir.join("tdg.toml"),
        format!(
            "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
             [modules]\nroot = \"src\"\n\n[dependencies]\n{deps}"
        ),
    )
    .expect("write manifest");
    fs::write(dir.join("src/lib.tuo"), module).expect("write module");
}

#[test]
fn resolves_a_transitive_path_graph_in_name_order() {
    let root = scratch("transitive");
    // app -> util -> core
    write_package(
        &root.join("core"),
        "core",
        "",
        "module core;\n\npub fn id(take x: Int) -> Int {\n    x\n}\n",
    );
    write_package(
        &root.join("util"),
        "util",
        "core = { path = \"../core\" }\n",
        "module util;\n\npub fn u() -> Int {\n    0\n}\n",
    );
    write_package(
        &root.join("app"),
        "app",
        "util = { path = \"../util\" }\n",
        "module app;\n\nfn main() -> Int {\n    0\n}\n",
    );

    let graph = resolve(&root.join("app")).expect("resolves");
    assert_eq!(graph.root.as_str(), "app");
    // Every package in the graph, in name order.
    let names: Vec<&str> = graph.packages.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, vec!["app", "core", "util"]);

    // The lockfile pins the whole graph.
    let lock = graph.to_lockfile();
    assert_eq!(lock.packages.len(), 3);
    assert_eq!(
        lock.package("app").expect("app locked").dependencies,
        vec!["util".to_string()]
    );
    assert_eq!(
        lock.package("util").expect("util locked").dependencies,
        vec!["core".to_string()]
    );

    // Every module across the graph is available to a host, in a stable order.
    let modules = graph.all_modules();
    assert_eq!(modules.len(), 3);
}

#[test]
fn detects_a_dependency_cycle() {
    let root = scratch("cycle");
    // a -> b -> a
    write_package(
        &root.join("a"),
        "a",
        "b = { path = \"../b\" }\n",
        "module a;\n\nfn f() -> Int {\n    0\n}\n",
    );
    write_package(
        &root.join("b"),
        "b",
        "a = { path = \"../a\" }\n",
        "module b;\n\nfn g() -> Int {\n    0\n}\n",
    );
    let err = resolve(&root.join("a")).expect_err("cycle is rejected");
    assert!(matches!(err, ResolveError::Cycle(_)), "got {err}");
}

#[test]
fn detects_two_packages_claiming_the_same_name() {
    let root = scratch("duplicate");
    // app depends on two different directories that both call themselves `dup`.
    write_package(
        &root.join("app"),
        "app",
        "dup = { path = \"../dup1\" }\ndup2 = { path = \"../dup2\" }\n",
        "module app;\n\nfn main() -> Int {\n    0\n}\n",
    );
    write_package(
        &root.join("dup1"),
        "dup",
        "",
        "module dup;\n\nfn a() -> Int {\n    0\n}\n",
    );
    write_package(
        &root.join("dup2"),
        "dup",
        "",
        "module dup;\n\nfn b() -> Int {\n    1\n}\n",
    );
    let err = resolve(&root.join("app")).expect_err("duplicate name rejected");
    assert!(
        matches!(err, ResolveError::DuplicateName { .. }),
        "got {err}"
    );
}

#[test]
fn a_missing_dependency_directory_is_reported() {
    let root = scratch("missing_dep");
    write_package(
        &root.join("app"),
        "app",
        "ghost = { path = \"../ghost\" }\n",
        "module app;\n\nfn main() -> Int {\n    0\n}\n",
    );
    let err = resolve(&root.join("app")).expect_err("missing dep rejected");
    assert!(matches!(err, ResolveError::MissingManifest(_)), "got {err}");
}

#[test]
fn checksum_drift_is_detected_against_a_prior_lock() {
    let root = scratch("drift");
    write_package(
        &root.join("app"),
        "app",
        "dep = { path = \"../dep\" }\n",
        "module app;\n\nfn main() -> Int {\n    0\n}\n",
    );
    write_package(
        &root.join("dep"),
        "dep",
        "",
        "module dep;\n\npub fn v() -> Int {\n    1\n}\n",
    );

    // Resolve once and capture the lockfile.
    let first = resolve(&root.join("app")).expect("first resolve");
    let lock = first.to_lockfile();

    // Tamper with the dependency, then re-resolve and verify against the lock.
    fs::write(
        root.join("dep/src/lib.tuo"),
        "module dep;\n\npub fn v() -> Int {\n    2\n}\n",
    )
    .expect("tamper dep");
    let second = resolve(&root.join("app")).expect("second resolve");
    let err = verify_against_lock(&second, &lock).expect_err("drift detected");
    assert!(
        matches!(err, ResolveError::ChecksumMismatch { .. }),
        "got {err}"
    );
}

#[test]
fn editing_the_root_does_not_count_as_drift() {
    let root = scratch("root_drift");
    write_package(
        &root.join("app"),
        "app",
        "",
        "module app;\n\nfn main() -> Int {\n    0\n}\n",
    );
    let first = resolve(&root.join("app")).expect("first resolve");
    let lock = first.to_lockfile();

    // Edit the root package's own source; its checksum changes but this is the
    // package under development, so it must not be flagged as drift.
    fs::write(
        root.join("app/src/lib.tuo"),
        "module app;\n\nfn main() -> Int {\n    1\n}\n",
    )
    .expect("edit root");
    let second = resolve(&root.join("app")).expect("second resolve");
    assert!(
        verify_against_lock(&second, &lock).is_ok(),
        "a root edit is not drift"
    );
}

#[test]
fn resolution_is_deterministic() {
    let root = scratch("deterministic");
    write_package(
        &root.join("app"),
        "app",
        "util = { path = \"../util\" }\n",
        "module app;\n\nfn main() -> Int {\n    0\n}\n",
    );
    write_package(
        &root.join("util"),
        "util",
        "",
        "module util;\n\npub fn u() -> Int {\n    0\n}\n",
    );
    let a = resolve(&root.join("app"))
        .expect("a")
        .to_lockfile()
        .to_toml();
    let b = resolve(&root.join("app"))
        .expect("b")
        .to_lockfile()
        .to_toml();
    assert_eq!(a, b, "the same workspace resolves identically");
    // And the emitted lockfile round-trips through the parser.
    assert!(Lockfile::parse(&a).is_ok(), "lockfile re-parses");
}
