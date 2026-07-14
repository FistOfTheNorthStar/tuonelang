//! Dependency-policy test.
//!
//! Enforces the hard crate-dependency invariants documented in
//! `ARCHITECTURE.md`. It reads each workspace crate's `Cargo.toml`, extracts the
//! set of *tuonelang-owned* crates it depends on (across all dependency tables), and
//! asserts that no forbidden edge exists.
//!
//! This is deliberately small: it scans manifests with a tiny hand-rolled
//! parser rather than pulling in a TOML dependency or an architecture
//! framework. It only needs to recognize dependency-table headers and the
//! `tuo-*` keys within them.

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

/// A forbidden dependency edge: `from` must not depend on `to`.
struct Forbidden {
    from: &'static str,
    to: &'static str,
    rationale: &'static str,
}

/// The hard invariants from `ARCHITECTURE.md`, expressed as forbidden edges.
const FORBIDDEN_EDGES: &[Forbidden] = &[
    Forbidden {
        from: "tuo-source",
        to: "tuo-parser",
        rationale: "source infrastructure must not depend on parser infrastructure",
    },
    Forbidden {
        from: "tuo-lexer",
        to: "tuo-types",
        rationale: "lexer must not depend on type checking",
    },
    Forbidden {
        from: "tuo-syntax",
        to: "tuo-codegen",
        rationale: "syntax must not depend on code generation",
    },
    Forbidden {
        from: "tuo-syntax",
        to: "tuo-codegen-cranelift",
        rationale: "syntax must not depend on code generation",
    },
    Forbidden {
        from: "tuo-syntax",
        to: "tuo-codegen-llvm",
        rationale: "syntax must not depend on code generation",
    },
    Forbidden {
        from: "tuo-parser",
        to: "tuo-codegen-cranelift",
        rationale: "parser must not depend on the Cranelift backend",
    },
    Forbidden {
        from: "tuo-parser",
        to: "tuo-codegen-llvm",
        rationale: "parser must not depend on the LLVM backend",
    },
    Forbidden {
        from: "tuo-mir",
        to: "tuo-codegen-cranelift",
        rationale: "MIR must not depend on Cranelift; backend types must not appear in MIR",
    },
    Forbidden {
        from: "tuo-mir",
        to: "tuo-codegen-llvm",
        rationale: "MIR must not depend on LLVM; backend types must not appear in MIR",
    },
    Forbidden {
        from: "tuo-mir",
        to: "tuo-codegen",
        rationale: "MIR must not depend on code generation",
    },
];

/// Semantic and pipeline crates that must never depend on CLI presentation.
const NON_CLI_CRATES: &[&str] = &[
    "tuo-source",
    "tuo-diagnostics",
    "tuo-db",
    "tuo-lexer",
    "tuo-syntax",
    "tuo-parser",
    "tuo-ast",
    "tuo-hir",
    "tuo-resolve",
    "tuo-types",
    "tuo-ownership",
    "tuo-mir",
    "tuo-mir-interp",
    "tuo-spec",
    "tuo-codegen",
    "tuo-codegen-cranelift",
    "tuo-codegen-llvm",
    "tuo-compiler",
];

#[test]
fn forbidden_dependency_edges_are_absent() {
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

#[test]
fn semantic_crates_do_not_depend_on_cli() {
    let root = workspace_root();
    let known = all_tuo_crates(&root);

    for &name in NON_CLI_CRATES {
        let manifest = root.join("crates").join(name).join("Cargo.toml");
        let deps = tuo_dependencies_of(&manifest, &known);
        assert!(
            !deps.contains("tuo-cli"),
            "forbidden dependency: `{name}` must not depend on CLI presentation (`tuo-cli`)",
        );
    }
}
