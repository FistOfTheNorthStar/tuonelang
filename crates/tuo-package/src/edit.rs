//! Manifest editing (`tuo add` / `tuo remove`).
//!
//! These operate on an in-memory [`Manifest`] and re-render it; the CLI reads
//! `tdg.toml`, applies one of these, and writes the canonical text back. Both
//! are idempotent-friendly and report a precise [`EditError`] rather than
//! silently doing nothing.

use crate::manifest::{Dependency, DependencySource, Manifest, PackageName};

/// An error from editing a manifest's dependency set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    /// `tuo add` was asked to add a dependency that is already present with a
    /// *different* source (adding an identical one is a no-op success).
    AlreadyPresent {
        /// The dependency name.
        name: PackageName,
    },
    /// `tuo remove` was asked to remove a dependency that is not declared.
    NotPresent {
        /// The dependency name.
        name: PackageName,
    },
    /// A dependency cannot name the package itself.
    SelfDependency,
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditError::AlreadyPresent { name } => {
                write!(
                    f,
                    "dependency `{name}` is already declared with a different source"
                )
            }
            EditError::NotPresent { name } => write!(f, "dependency `{name}` is not declared"),
            EditError::SelfDependency => write!(f, "a package cannot depend on itself"),
        }
    }
}

impl std::error::Error for EditError {}

/// Add a path dependency named `name` at `path` to `manifest`.
///
/// Adding a dependency that is already present with the *same* path is a
/// successful no-op; a conflicting source is an error, so `tuo add` never
/// silently rewrites an existing entry.
///
/// # Errors
///
/// [`EditError::SelfDependency`] if `name` is the package's own name, or
/// [`EditError::AlreadyPresent`] if it already exists with a different source.
pub fn add_path_dependency(
    manifest: &mut Manifest,
    name: PackageName,
    path: &str,
) -> Result<(), EditError> {
    if name == manifest.name {
        return Err(EditError::SelfDependency);
    }
    let source = DependencySource::Path(path.to_string());
    if let Some(existing) = manifest.dependencies.get(&name) {
        if existing.source == source {
            return Ok(());
        }
        return Err(EditError::AlreadyPresent { name });
    }
    manifest
        .dependencies
        .insert(name.clone(), Dependency { name, source });
    Ok(())
}

/// Remove the dependency named `name` from `manifest`.
///
/// # Errors
///
/// [`EditError::NotPresent`] if the dependency is not declared, so `tuo remove`
/// reports a typo rather than exiting successfully having done nothing.
pub fn remove_dependency(manifest: &mut Manifest, name: &PackageName) -> Result<(), EditError> {
    if manifest.dependencies.remove(name).is_none() {
        return Err(EditError::NotPresent { name: name.clone() });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{EditError, add_path_dependency, remove_dependency};
    use crate::manifest::{DependencySource, Edition, Manifest, PackageName};

    fn base() -> Manifest {
        Manifest::new(PackageName::new("app").unwrap(), "0.1.0", Edition::E2024)
    }

    #[test]
    fn adds_and_removes() {
        let mut m = base();
        let util = PackageName::new("util").unwrap();
        add_path_dependency(&mut m, util.clone(), "../util").expect("adds");
        assert_eq!(
            m.dependencies[&util].source,
            DependencySource::Path("../util".into())
        );
        remove_dependency(&mut m, &util).expect("removes");
        assert!(m.dependencies.is_empty());
    }

    #[test]
    fn adding_the_same_path_twice_is_a_noop() {
        let mut m = base();
        let util = PackageName::new("util").unwrap();
        add_path_dependency(&mut m, util.clone(), "../util").unwrap();
        add_path_dependency(&mut m, util.clone(), "../util").expect("idempotent");
        assert_eq!(m.dependencies.len(), 1);
    }

    #[test]
    fn adding_a_conflicting_source_errors() {
        let mut m = base();
        let util = PackageName::new("util").unwrap();
        add_path_dependency(&mut m, util.clone(), "../util").unwrap();
        let e = add_path_dependency(&mut m, util.clone(), "../other").expect_err("conflict");
        assert_eq!(e, EditError::AlreadyPresent { name: util });
    }

    #[test]
    fn cannot_depend_on_self() {
        let mut m = base();
        let app = PackageName::new("app").unwrap();
        assert_eq!(
            add_path_dependency(&mut m, app, "../app").expect_err("self"),
            EditError::SelfDependency
        );
    }

    #[test]
    fn removing_absent_errors() {
        let mut m = base();
        let ghost = PackageName::new("ghost").unwrap();
        assert_eq!(
            remove_dependency(&mut m, &ghost).expect_err("absent"),
            EditError::NotPresent { name: ghost }
        );
    }
}
