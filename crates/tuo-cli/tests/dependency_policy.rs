//! Dependency-policy test.
//!
//! Enforces the layered-architecture invariants documented in
//! `ARCHITECTURE.md`: every tuonelang-owned crate is assigned to a **layer**,
//! and a crate may only depend on crates in **strictly lower** layers. This is
//! the general rule that lower compiler layers can never depend on higher
//! layers — the whole pipeline direction (`source → diagnostics → lexer →
//! syntax/parser → AST → HIR/resolution/types → ownership → MIR →
//! interpreter/codegen → orchestration → CLI`) falls out of it, and it also
//! rules out dependency cycles among tuonelang crates by construction (a cycle
//! would need an edge that does not decrease the layer number).
//!
//! A small list of *extra* forbidden edges covers constraints the layer
//! numbers alone cannot express (e.g. the `tuo-compiler` facade stops at the
//! `tuo-codegen` abstraction and must never reach a concrete backend, even
//! though the backends sit on a lower layer).
//!
//! The test reads each workspace crate's `Cargo.toml`, extracts the set of
//! *tuonelang-owned* crates it depends on (across all dependency tables), and
//! checks the rules above. It is deliberately small: a tiny hand-rolled
//! manifest scan rather than a TOML dependency or an architecture framework.
//! It only needs to recognize dependency-table headers and the `tuo-*` keys
//! within them.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Return the workspace root (the parent of the `crates/` directory).
fn workspace_root() -> PathBuf {
    // This test file lives at `crates/tuo-cli/tests/`; the workspace root is
    // three levels up.
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent() // crates/
        .and_then(Path::parent) // workspace root
        .expect("tuo-cli crate should live under <workspace>/crates/")
        .to_path_buf()
}

/// The set of tuonelang-owned crate names, discovered from the `crates/` directory.
fn all_tuo_crates(root: &Path) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let crates_dir = root.join("crates");
    for entry in fs::read_dir(&crates_dir).expect("crates/ directory should exist") {
        let entry = entry.expect("readable directory entry");
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            names.insert(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names
}

/// Extract the set of tuonelang-owned crates that `<crate>/Cargo.toml` depends on,
/// across `[dependencies]`, `[dev-dependencies]`, and `[build-dependencies]`.
///
/// The scan recognizes dependency-table headers (including
/// `[target.'...'.dependencies]`) and, within them, keys of the form
/// `tuo-...`. It only cares about which tuonelang crates are referenced, so it does
/// not attempt to parse dependency values.
fn tuo_dependencies_of(manifest: &Path, known: &BTreeSet<String>) -> BTreeSet<String> {
    let text = fs::read_to_string(manifest)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", manifest.display()));

    let mut deps = BTreeSet::new();
    let mut in_dep_table = false;

    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('[') {
            // A new table header. Determine whether it is a dependency table.
            in_dep_table = is_dependency_table_header(line);
            continue;
        }
        if !in_dep_table || line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Dependency lines look like `name = ...` or `name.workspace = true`.
        // The key is everything before the first `=`, `.`, or whitespace.
        let key = line
            .split(['=', '.', ' ', '\t'])
            .next()
            .unwrap_or("")
            .trim();
        if known.contains(key) {
            deps.insert(key.to_string());
        }
    }

    deps
}

/// Whether a table header names one of the dependency tables we care about.
fn is_dependency_table_header(header: &str) -> bool {
    // Strip the surrounding brackets and any inline comment.
    let inner = header
        .trim_start_matches('[')
        .split(']')
        .next()
        .unwrap_or("")
        .trim();
    // Accept `dependencies`, `dev-dependencies`, `build-dependencies`, and the
    // `target.'cfg(...)'.dependencies` variants (match on the final segment).
    let last = inner.rsplit('.').next().unwrap_or(inner);
    matches!(
        last,
        "dependencies" | "dev-dependencies" | "build-dependencies"
    )
}

/// The layer assignment for every tuonelang-owned crate.
///
/// A crate may only depend on crates with a **strictly lower** layer number.
/// Numbers are spaced by 10 so a future crate can slot between existing layers
/// without renumbering everything. This table is the machine-checked twin of
/// the "Dependency architecture" section in `ARCHITECTURE.md` — keep the two
/// in sync.
const LAYERS: &[(&str, u32)] = &[
    // Foundation.
    ("tuo-source", 0),
    ("tuo-diagnostics", 10),
    // Infrastructure and front-end data structures over the foundation.
    ("tuo-db", 20),
    ("tuo-lexer", 20),
    // The lossless CST is built over the lexer's tokens (lexer → syntax in
    // the pipeline), so it sits strictly above tuo-lexer.
    ("tuo-syntax", 25),
    // Typed AST views are read-only accessors over the CST (and its token
    // stream), so tuo-ast sits strictly above tuo-syntax and below the
    // passes that consume the views.
    ("tuo-ast", 27),
    // Front-end passes.
    ("tuo-parser", 30),
    // Semantic analysis pipeline. HIR sits *above* resolution: it is
    // lowered from the typed AST views plus resolution's stable IDs, so
    // its nodes can represent resolved names.
    ("tuo-resolve", 40),
    ("tuo-hir", 45),
    ("tuo-types", 50),
    ("tuo-ownership", 60),
    // The single executable semantic representation.
    ("tuo-mir", 70),
    // Native runtime support: below the backends so generated code / codegen
    // may link against it, above MIR which must stay execution-agnostic.
    ("tuo-runtime", 75),
    // MIR consumers: the reference interpreter and the backend-agnostic
    // codegen interface.
    ("tuo-mir-interp", 80),
    ("tuo-codegen", 80),
    // Concrete backends and the spec runner (drives the interpreter).
    ("tuo-codegen-cranelift", 90),
    ("tuo-codegen-llvm", 90),
    ("tuo-spec", 90),
    // The standard library ships with the toolchain; it is written *in*
    // tuonelang and must not become a dependency of any compiler stage.
    ("tuo-stdlib", 90),
    // Orchestration facade: wires the stages, stops at the `tuo-codegen`
    // abstraction (concrete backends are additionally forbidden below).
    ("tuo-compiler", 100),
    // Tooling surfaces over the shared engine.
    ("tuo-fmt", 110),
    ("tuo-package", 110),
    ("tuo-lsp", 110),
    ("tuo-agent", 110),
    ("tuo-bench", 110),
    // The corpus pipeline composes the layer-110 tools (it reuses the formatter
    // and the research harness's tokenizers) on top of the compiler facade, so
    // it sits strictly above them and below the CLI.
    ("tuo-corpus", 115),
    // The `tuo` binary: may orchestrate anything below it.
    ("tuo-cli", 120),
];

/// Look up a crate's layer, or `None` if it is not classified.
fn layer_of(name: &str) -> Option<u32> {
    LAYERS
        .iter()
        .find(|(crate_name, _)| *crate_name == name)
        .map(|(_, layer)| *layer)
}

/// A forbidden dependency edge: `from` must not depend on `to`.
///
/// Only for constraints the layer numbers cannot express — i.e. edges that
/// point *downward* in the layer map but are still architecturally forbidden.
struct Forbidden {
    from: &'static str,
    to: &'static str,
    rationale: &'static str,
}

const FORBIDDEN_EDGES: &[Forbidden] = &[
    Forbidden {
        from: "tuo-compiler",
        to: "tuo-codegen-cranelift",
        rationale: "the facade stops at the `tuo-codegen` abstraction; \
                    it must never depend on a concrete backend",
    },
    Forbidden {
        from: "tuo-compiler",
        to: "tuo-codegen-llvm",
        rationale: "the facade stops at the `tuo-codegen` abstraction; \
                    it must never depend on a concrete backend",
    },
    Forbidden {
        from: "tuo-compiler",
        to: "tuo-stdlib",
        rationale: "the stdlib is written in tuonelang and consumed as input, \
                    not linked into the compiler",
    },
];

#[test]
fn every_workspace_crate_has_a_layer() {
    let root = workspace_root();
    let known = all_tuo_crates(&root);

    // Every crate on disk must be classified (adding a crate forces an
    // explicit layering decision) …
    for name in &known {
        assert!(
            layer_of(name).is_some(),
            "crate `{name}` exists under crates/ but has no entry in the LAYERS \
             table of this test; assign it a layer (and update ARCHITECTURE.md)",
        );
    }
    // … and every classified crate must exist on disk (no stale entries).
    for (name, _) in LAYERS {
        assert!(
            known.contains(*name),
            "LAYERS table references `{name}`, which does not exist under crates/",
        );
    }
}

#[test]
fn dependencies_flow_strictly_downward() {
    let root = workspace_root();
    let known = all_tuo_crates(&root);

    for (name, layer) in LAYERS {
        let manifest = root.join("crates").join(name).join("Cargo.toml");
        let deps = tuo_dependencies_of(&manifest, &known);
        for dep in &deps {
            let dep_layer = layer_of(dep)
                .unwrap_or_else(|| panic!("`{name}` depends on unclassified crate `{dep}`"));
            assert!(
                dep_layer < *layer,
                "layering violation: `{name}` (layer {layer}) depends on `{dep}` \
                 (layer {dep_layer}); dependencies must point to strictly lower \
                 layers — lower compiler layers can never depend on higher ones",
            );
        }
    }
}

#[test]
fn extra_forbidden_edges_are_absent() {
    let root = workspace_root();
    let known = all_tuo_crates(&root);

    for edge in FORBIDDEN_EDGES {
        assert!(
            known.contains(edge.from),
            "policy references unknown crate `{}`",
            edge.from
        );
        let manifest = root.join("crates").join(edge.from).join("Cargo.toml");
        let deps = tuo_dependencies_of(&manifest, &known);
        assert!(
            !deps.contains(edge.to),
            "forbidden dependency: `{}` must not depend on `{}` ({})",
            edge.from,
            edge.to,
            edge.rationale,
        );
    }
}
