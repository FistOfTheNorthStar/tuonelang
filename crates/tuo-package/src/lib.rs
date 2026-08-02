//! The tuonelang package system.
//!
//! This crate defines tuonelang's first package format and the operations over
//! it. A **package** is a directory containing a manifest (`tdg.toml`) and a
//! **module root** holding the package's `.tuo` sources; a workspace of
//! packages is connected by **path dependencies**, pinned by a **lockfile**
//! (`tdg.lock`).
//!
//! The crate is data-and-filesystem only: it defines the manifest/lockfile
//! model, reads and resolves a package graph off disk, computes content
//! checksums, and scaffolds/edits packages. It deliberately holds **no compiler
//! machinery** — resolution produces the package graph as plain source text
//! ([`resolve::ResolvedGraph`]), and a host (the CLI, the agent server) loads
//! that text into its own `SourceMap` and runs its own pipeline. That is what
//! lets *the compiler and the agent protocol query a package's real symbols
//! without guessing*: they compile the exact resolved sources and read the
//! symbols the front end produces.
//!
//! # The format at a glance
//!
//! * **Identity** — a package is identified by (`name`, `version`). The name is
//!   validated ([`manifest::PackageName`]) so it is a safe directory name, a
//!   legal module prefix, and unambiguous on the command line.
//! * **Module roots** — `[modules].root` (default `src`) names the one
//!   directory whose `.tuo` files are the package's modules.
//! * **Dependency resolution** — [`resolve::resolve`] follows path dependencies
//!   transitively, detecting cycles and duplicate names, and returns the full
//!   graph in a deterministic (name) order.
//! * **Lockfile semantics** — [`lockfile::Lockfile`] pins every resolved
//!   package's checksum and direct dependencies; the same workspace always
//!   resolves to a byte-identical lockfile.
//! * **Checksums** — each package's content is hashed with SHA-256
//!   ([`sha256`]) over its module names and text, so a build detects drifted
//!   sources ([`resolve::verify_against_lock`]).
//! * **Edition selection** — `[package].edition` ([`manifest::Edition`]) selects
//!   the language dialect; an unknown edition is rejected at load time, never
//!   silently accepted.
//!
//! v0 supports **local/path dependencies only**; a remote registry is a later
//! addition, and the format advertises no dependency kind the resolver cannot
//! fetch (mirroring the toolchain's standing rule that it never advertises
//! behavior it cannot perform).

pub mod edit;
pub mod lockfile;
pub mod manifest;
pub mod resolve;
pub mod scaffold;
pub mod sha256;
pub mod toml;

pub use edit::{EditError, add_path_dependency, remove_dependency};
pub use lockfile::{LOCKFILE_VERSION, LockedPackage, LockedSource, Lockfile, LockfileError};
pub use manifest::{Dependency, DependencySource, Edition, Manifest, ManifestError, PackageName};
pub use resolve::{
    LOCKFILE_FILE, MANIFEST_FILE, MODULE_EXTENSION, ModuleSource, ResolveError, ResolvedGraph,
    ResolvedPackage, resolve, verify_against_lock,
};
pub use scaffold::{ScaffoldFile, new_package};
