//! Dependency resolution and package loading.
//!
//! Resolution walks a root package's manifest, follows every path dependency
//! transitively, reads each package's module-root sources from disk, computes
//! each package's content checksum, and produces two things:
//!
//! * a [`ResolvedGraph`] — the full set of packages (root + all transitive
//!   dependencies) with their loaded module sources, in a form any host can
//!   feed into its own `SourceMap` and pipeline; and
//! * the [`Lockfile`] that pins the graph.
//!
//! This is the layer that lets *the compiler and the agent protocol query a
//! package's symbols without guessing*: a host resolves the graph, loads every
//! module source into a source map, and runs resolution/type-checking over it —
//! the symbols it gets back are the package's real, machine-queryable surface,
//! not an inferred approximation.
//!
//! v0 resolves **local/path dependencies** only, deterministically: the graph
//! is discovered by a fixed walk and the lockfile is written in name order, so
//! the same workspace always resolves to the same lockfile.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::lockfile::{LockedPackage, LockedSource, Lockfile};
use crate::manifest::{DependencySource, Manifest, PackageName};
use crate::sha256;

/// The manifest file name a package directory must contain.
pub const MANIFEST_FILE: &str = "tdg.toml";

/// The lockfile name written beside the root manifest.
pub const LOCKFILE_FILE: &str = "tdg.lock";

/// The extension of a tuonelang module source file.
pub const MODULE_EXTENSION: &str = "tuo";

/// One module source belonging to a resolved package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleSource {
    /// A stable, package-qualified name for diagnostics and interning, e.g.
    /// `"util/src/lib.tuo"`. Unique across the whole resolved graph.
    pub name: String,
    /// The module's full tuonelang source text.
    pub text: String,
}

/// A single resolved package: its identity, where it lives, its loaded module
/// sources (in a stable order), its content checksum, and its direct
/// dependency names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPackage {
    /// The package's name.
    pub name: PackageName,
    /// The package's version, from its manifest.
    pub version: String,
    /// Where the package's source came from.
    pub source: LockedSource,
    /// The absolute directory the package was loaded from.
    pub root_dir: PathBuf,
    /// Every `.tuo` module under the package's module root, in path order.
    pub modules: Vec<ModuleSource>,
    /// The SHA-256 (hex) of the concatenated module sources.
    pub checksum: String,
    /// The names of this package's direct dependencies, in name order.
    pub dependencies: Vec<PackageName>,
}

impl ResolvedPackage {
    /// The locked-graph record for this package.
    fn to_locked(&self) -> LockedPackage {
        LockedPackage {
            name: self.name.as_str().to_string(),
            version: self.version.clone(),
            source: self.source.clone(),
            checksum: self.checksum.clone(),
            dependencies: self
                .dependencies
                .iter()
                .map(|d| d.as_str().to_string())
                .collect(),
        }
    }
}

/// The fully resolved dependency graph of a root package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedGraph {
    /// The name of the root package (its own manifest's package name).
    pub root: PackageName,
    /// Every package in the graph (root + transitive deps), in name order.
    pub packages: Vec<ResolvedPackage>,
}

impl ResolvedGraph {
    /// Every module source across every package in the graph, in a stable
    /// order (package name, then module path). This is exactly what a host
    /// interns into a `SourceMap` to compile or query the whole graph.
    #[must_use]
    pub fn all_modules(&self) -> Vec<&ModuleSource> {
        self.packages
            .iter()
            .flat_map(|p| p.modules.iter())
            .collect()
    }

    /// Look up a resolved package by name.
    #[must_use]
    pub fn package(&self, name: &PackageName) -> Option<&ResolvedPackage> {
        self.packages.iter().find(|p| &p.name == name)
    }

    /// The lockfile pinning this graph.
    #[must_use]
    pub fn to_lockfile(&self) -> Lockfile {
        Lockfile::new(
            self.packages
                .iter()
                .map(ResolvedPackage::to_locked)
                .collect(),
        )
    }
}

/// An error from resolving or loading a package graph.
#[derive(Debug)]
pub enum ResolveError {
    /// A package directory has no `tdg.toml`.
    MissingManifest(PathBuf),
    /// A manifest failed to parse or validate.
    Manifest {
        /// The manifest path.
        path: PathBuf,
        /// The underlying error.
        error: crate::manifest::ManifestError,
    },
    /// A filesystem read failed.
    Io {
        /// The path being read.
        path: PathBuf,
        /// The OS error text.
        error: String,
    },
    /// A package's declared module root does not exist or is not a directory.
    MissingModuleRoot {
        /// The offending package.
        package: PackageName,
        /// The module-root directory that was expected.
        path: PathBuf,
    },
    /// The dependency graph contains a cycle (a package reachable from itself).
    Cycle(Vec<PackageName>),
    /// Two distinct paths in the graph declare the same package name.
    DuplicateName {
        /// The clashing name.
        name: PackageName,
        /// The first directory that claimed it.
        first: PathBuf,
        /// The second directory that claimed it.
        second: PathBuf,
    },
    /// A locked checksum did not match the freshly computed one (drift).
    ChecksumMismatch {
        /// The package whose bytes drifted.
        name: String,
        /// The checksum recorded in the lockfile.
        locked: String,
        /// The checksum computed from the sources on disk.
        actual: String,
    },
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::MissingManifest(p) => {
                write!(f, "no `{MANIFEST_FILE}` found at {}", p.display())
            }
            ResolveError::Manifest { path, error } => {
                write!(f, "{}: {error}", path.display())
            }
            ResolveError::Io { path, error } => {
                write!(f, "cannot read {}: {error}", path.display())
            }
            ResolveError::MissingModuleRoot { package, path } => write!(
                f,
                "package `{package}` module root {} does not exist",
                path.display()
            ),
            ResolveError::Cycle(names) => {
                let chain = names
                    .iter()
                    .map(PackageName::as_str)
                    .collect::<Vec<_>>()
                    .join(" -> ");
                write!(f, "dependency cycle: {chain}")
            }
            ResolveError::DuplicateName {
                name,
                first,
                second,
            } => write!(
                f,
                "two packages both named `{name}`: {} and {}",
                first.display(),
                second.display()
            ),
            ResolveError::ChecksumMismatch {
                name,
                locked,
                actual,
            } => write!(
                f,
                "checksum mismatch for `{name}`: lockfile has {locked}, sources hash to {actual}"
            ),
        }
    }
}

impl std::error::Error for ResolveError {}

/// Resolve the package rooted at `root_dir`, following path dependencies
/// transitively and loading every module source.
///
/// # Errors
///
/// Returns a [`ResolveError`] on a missing/invalid manifest, an unreadable
/// source, a missing module root, a dependency cycle, or a duplicate package
/// name in the graph.
pub fn resolve(root_dir: &Path) -> Result<ResolvedGraph, ResolveError> {
    let root_dir = absolute(root_dir);
    let root_manifest = load_manifest(&root_dir)?;
    let root_name = root_manifest.name.clone();

    // Discover every package by walking path dependencies. `claimed` maps a
    // package name to the directory that defined it, catching duplicates.
    let mut claimed: BTreeMap<PackageName, PathBuf> = BTreeMap::new();
    let mut packages: BTreeMap<PackageName, ResolvedPackage> = BTreeMap::new();

    // Depth-first walk with an explicit on-stack set for cycle detection.
    let mut stack: Vec<PackageName> = Vec::new();
    walk(
        &root_dir,
        &root_manifest,
        true,
        &mut claimed,
        &mut packages,
        &mut stack,
    )?;

    let mut ordered: Vec<ResolvedPackage> = packages.into_values().collect();
    ordered.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(ResolvedGraph {
        root: root_name,
        packages: ordered,
    })
}

/// Verify a resolved graph's **dependencies** against a previously written
/// lockfile: every locked dependency's freshly computed checksum must match the
/// one the lockfile pinned, so a build never silently compiles drifted
/// dependency bytes.
///
/// The **root** package is deliberately exempt: it is the package the developer
/// is actively editing, so its own bytes changing between resolutions is
/// expected, not drift — only the pinned dependency sources are held to the
/// lock. (The lockfile still records the root's checksum, refreshed each time.)
///
/// # Errors
///
/// Returns [`ResolveError::ChecksumMismatch`] on the first *dependency* whose
/// bytes drifted from the lockfile.
pub fn verify_against_lock(graph: &ResolvedGraph, lock: &Lockfile) -> Result<(), ResolveError> {
    for package in &graph.packages {
        // The root package is the one under active development; skip it.
        if matches!(package.source, LockedSource::Root) {
            continue;
        }
        if let Some(locked) = lock.package(package.name.as_str()) {
            if locked.checksum != package.checksum {
                return Err(ResolveError::ChecksumMismatch {
                    name: package.name.as_str().to_string(),
                    locked: locked.checksum.clone(),
                    actual: package.checksum.clone(),
                });
            }
        }
    }
    Ok(())
}

/// Recursively load `manifest`'s package (at `dir`) and its path dependencies.
fn walk(
    dir: &Path,
    manifest: &Manifest,
    is_root: bool,
    claimed: &mut BTreeMap<PackageName, PathBuf>,
    packages: &mut BTreeMap<PackageName, ResolvedPackage>,
    stack: &mut Vec<PackageName>,
) -> Result<(), ResolveError> {
    let name = manifest.name.clone();

    // Cycle check: is this name already on the active path?
    if stack.contains(&name) {
        let mut chain = stack.clone();
        chain.push(name);
        return Err(ResolveError::Cycle(chain));
    }

    // Duplicate-name check: has a *different* directory already claimed it?
    if let Some(existing) = claimed.get(&name) {
        if existing == dir {
            // Same package reached by two paths — fine, already loaded.
            return Ok(());
        }
        return Err(ResolveError::DuplicateName {
            name,
            first: existing.clone(),
            second: dir.to_path_buf(),
        });
    }
    claimed.insert(name.clone(), dir.to_path_buf());

    stack.push(name.clone());

    // Resolve and recurse into each path dependency first, so their entries
    // exist before we record this package's dependency names.
    let mut dep_names: Vec<PackageName> = Vec::new();
    for dep in manifest.dependencies.values() {
        match &dep.source {
            DependencySource::Path(rel) => {
                let dep_dir = absolute(&dir.join(rel));
                let dep_manifest = load_manifest(&dep_dir)?;
                // The declared dependency key must match the dependency's own
                // package name, so imports and the lockfile agree.
                dep_names.push(dep_manifest.name.clone());
                walk(&dep_dir, &dep_manifest, false, claimed, packages, stack)?;
            }
        }
    }
    dep_names.sort();

    // Load this package's own module sources.
    let modules = load_modules(dir, manifest)?;
    let checksum = checksum_of(&modules);
    let source = if is_root {
        LockedSource::Root
    } else {
        LockedSource::Path(dir.display().to_string())
    };

    packages.insert(
        name.clone(),
        ResolvedPackage {
            name,
            version: manifest.version.clone(),
            source,
            root_dir: dir.to_path_buf(),
            modules,
            checksum,
            dependencies: dep_names,
        },
    );

    stack.pop();
    Ok(())
}

/// Load a package's manifest from `dir/tdg.toml`.
fn load_manifest(dir: &Path) -> Result<Manifest, ResolveError> {
    let path = dir.join(MANIFEST_FILE);
    if !path.is_file() {
        return Err(ResolveError::MissingManifest(path));
    }
    let text = std::fs::read_to_string(&path).map_err(|e| ResolveError::Io {
        path: path.clone(),
        error: e.to_string(),
    })?;
    Manifest::parse(&text).map_err(|error| ResolveError::Manifest { path, error })
}

/// Read every `.tuo` file under a package's module root, in path order. The
/// module names are prefixed with the package directory's file name so they are
/// unique across the graph and meaningful in diagnostics.
fn load_modules(dir: &Path, manifest: &Manifest) -> Result<Vec<ModuleSource>, ResolveError> {
    let module_root = dir.join(&manifest.module_root);
    if !module_root.is_dir() {
        return Err(ResolveError::MissingModuleRoot {
            package: manifest.name.clone(),
            path: module_root,
        });
    }

    let mut files: Vec<PathBuf> = Vec::new();
    collect_tuo_files(&module_root, &mut files)?;
    files.sort();

    let prefix = manifest.name.as_str();
    let mut modules = Vec::with_capacity(files.len());
    for file in files {
        let text = std::fs::read_to_string(&file).map_err(|e| ResolveError::Io {
            path: file.clone(),
            error: e.to_string(),
        })?;
        // Name relative to the module root, prefixed by the package name.
        let rel = file
            .strip_prefix(&module_root)
            .unwrap_or(&file)
            .display()
            .to_string();
        modules.push(ModuleSource {
            name: format!("{prefix}/{rel}"),
            text,
        });
    }
    Ok(modules)
}

/// Recursively collect `.tuo` files under `dir` into `out`.
fn collect_tuo_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), ResolveError> {
    let entries = std::fs::read_dir(dir).map_err(|e| ResolveError::Io {
        path: dir.to_path_buf(),
        error: e.to_string(),
    })?;
    for entry in entries {
        let entry = entry.map_err(|e| ResolveError::Io {
            path: dir.to_path_buf(),
            error: e.to_string(),
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|e| ResolveError::Io {
            path: path.clone(),
            error: e.to_string(),
        })?;
        if file_type.is_dir() {
            collect_tuo_files(&path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some(MODULE_EXTENSION) {
            out.push(path);
        }
    }
    Ok(())
}

/// The content checksum of a package: SHA-256 over each module's name and text
/// in order, so both the sources and their layout are pinned.
fn checksum_of(modules: &[ModuleSource]) -> String {
    let mut bytes = Vec::new();
    for module in modules {
        bytes.extend_from_slice(module.name.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(module.text.as_bytes());
        bytes.push(0);
    }
    sha256::hex(&bytes)
}

/// Make a path absolute against the current directory without touching the
/// filesystem beyond `canonicalize` when possible; falls back to the joined
/// path so resolution still works for not-yet-existing directories in errors.
fn absolute(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(path))
                .unwrap_or_else(|_| path.to_path_buf())
        }
    })
}
