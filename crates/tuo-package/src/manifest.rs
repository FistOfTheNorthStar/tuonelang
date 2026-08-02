//! The package manifest: `tdg.toml`.
//!
//! A manifest declares one package's **identity**, its **module root**, the
//! **edition** it is written against, and its **dependencies**. It is the
//! human-edited source of truth; the lockfile ([`crate::lockfile`]) is the
//! machine-maintained record derived from resolving it.
//!
//! # Shape
//!
//! ```toml
//! [package]
//! name = "app"          # package identity (see `PackageName`)
//! version = "0.1.0"     # semantic-ish version string (opaque in v0)
//! edition = "2024"      # the language edition this package targets
//!
//! [modules]
//! root = "src"          # directory holding the package's `.tuo` sources
//!
//! [dependencies]
//! util = { path = "../util" }   # a local/path dependency
//! ```
//!
//! Only **path dependencies** exist in v0 — a remote registry is a later
//! addition, and the manifest deliberately advertises no dependency kind the
//! resolver cannot fetch (mirroring the toolchain's standing honesty rule).

use std::collections::BTreeMap;
use std::fmt;

use crate::toml::{self, Value};

/// A validated package name.
///
/// Package identity is the pair (`name`, `version`); the *name* half is
/// constrained so it is a safe directory name, a legal module-path prefix, and
/// unambiguous on the command line: a non-empty ASCII string of lowercase
/// letters, digits, and underscores, starting with a letter.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PackageName(String);

impl PackageName {
    /// Validate and construct a package name.
    ///
    /// # Errors
    ///
    /// Returns [`ManifestError::InvalidName`] if `raw` is empty, does not start
    /// with a lowercase ASCII letter, or contains a character outside
    /// `[a-z0-9_]`.
    pub fn new(raw: &str) -> Result<Self, ManifestError> {
        let mut chars = raw.chars();
        let ok = match chars.next() {
            Some(first) if first.is_ascii_lowercase() => {
                chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            }
            _ => false,
        };
        if ok {
            Ok(Self(raw.to_string()))
        } else {
            Err(ManifestError::InvalidName(raw.to_string()))
        }
    }

    /// The name as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PackageName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The language edition a package is written against.
///
/// The edition selects the language dialect the compiler applies to the
/// package's sources. v0 recognizes exactly one edition, `2024`; the type is an
/// enum (not a free string) so an unknown edition is a manifest error at load
/// time rather than a surprise deep in the pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Edition {
    /// The 2024 edition — the only edition v0 supports (and the default).
    #[default]
    E2024,
}

impl Edition {
    /// The edition's canonical wire string, as written in `tdg.toml`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Edition::E2024 => "2024",
        }
    }

    /// Parse an edition string.
    ///
    /// # Errors
    ///
    /// Returns [`ManifestError::UnknownEdition`] for any string other than a
    /// supported edition — the toolchain never silently accepts an edition it
    /// cannot compile.
    pub fn parse(raw: &str) -> Result<Self, ManifestError> {
        match raw {
            "2024" => Ok(Edition::E2024),
            other => Err(ManifestError::UnknownEdition(other.to_string())),
        }
    }
}

/// Where a dependency's source comes from.
///
/// v0 supports only [`DependencySource::Path`]. The enum leaves an obvious slot
/// for a future `Registry`/`Git` variant without reshaping the manifest model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DependencySource {
    /// A local/path dependency: the package lives at this directory, relative to
    /// the manifest that declares it (or absolute).
    Path(String),
}

/// One entry in `[dependencies]`: the source the named dependency resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dependency {
    /// The dependency's declared name (the key in `[dependencies]`), which is
    /// also the name the package is imported under.
    pub name: PackageName,
    /// Where its source is fetched from.
    pub source: DependencySource,
}

/// A fully parsed and validated `tdg.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    /// The package's own name.
    pub name: PackageName,
    /// The package's version string (opaque in v0 — recorded, not interpreted).
    pub version: String,
    /// The edition the package targets.
    pub edition: Edition,
    /// The directory (relative to the manifest) that holds the package's `.tuo`
    /// module sources. This is the package's single **module root**.
    pub module_root: String,
    /// The package's declared dependencies, keyed by name for stable order.
    pub dependencies: BTreeMap<PackageName, Dependency>,
}

impl Manifest {
    /// The default module root when `[modules].root` is omitted.
    pub const DEFAULT_MODULE_ROOT: &'static str = "src";

    /// Build a fresh manifest for a new package with no dependencies.
    #[must_use]
    pub fn new(name: PackageName, version: impl Into<String>, edition: Edition) -> Self {
        Self {
            name,
            version: version.into(),
            edition,
            module_root: Self::DEFAULT_MODULE_ROOT.to_string(),
            dependencies: BTreeMap::new(),
        }
    }

    /// Parse a manifest from `tdg.toml` text.
    ///
    /// # Errors
    ///
    /// Returns a [`ManifestError`] describing the first problem: a TOML syntax
    /// error, a missing required field, an invalid name, an unknown edition, or
    /// a dependency whose source kind is not supported.
    pub fn parse(text: &str) -> Result<Self, ManifestError> {
        let doc = toml::parse(text).map_err(ManifestError::Toml)?;

        let package = doc
            .table("package")
            .ok_or(ManifestError::MissingTable("package"))?;
        let name = PackageName::new(string_field(package, "name", "package")?)?;
        let version = string_field(package, "version", "package")?.to_string();
        let edition = match package.get("edition") {
            Some(value) => Edition::parse(
                value
                    .as_str()
                    .ok_or(ManifestError::FieldType("package.edition", "string"))?,
            )?,
            None => Edition::default(),
        };

        let module_root = match doc.table("modules").and_then(|m| m.get("root")) {
            Some(value) => value
                .as_str()
                .ok_or(ManifestError::FieldType("modules.root", "string"))?
                .to_string(),
            None => Self::DEFAULT_MODULE_ROOT.to_string(),
        };

        let mut dependencies = BTreeMap::new();
        if let Some(deps) = doc.table("dependencies") {
            for (key, value) in deps {
                let dep_name = PackageName::new(key)?;
                let source = parse_dependency_source(key, value)?;
                dependencies.insert(
                    dep_name.clone(),
                    Dependency {
                        name: dep_name,
                        source,
                    },
                );
            }
        }

        Ok(Self {
            name,
            version,
            edition,
            module_root,
            dependencies,
        })
    }

    /// Render the manifest to canonical `tdg.toml` text.
    ///
    /// Deterministic: dependencies are emitted in name order, so a manifest
    /// written by `tuo add`/`tuo remove` has stable, diff-friendly output.
    #[must_use]
    pub fn to_toml(&self) -> String {
        let mut out = String::new();
        out.push_str("[package]\n");
        out.push_str(&format!(
            "name = \"{}\"\n",
            toml::escape_string(self.name.as_str())
        ));
        out.push_str(&format!(
            "version = \"{}\"\n",
            toml::escape_string(&self.version)
        ));
        out.push_str(&format!("edition = \"{}\"\n", self.edition.as_str()));
        out.push('\n');
        out.push_str("[modules]\n");
        out.push_str(&format!(
            "root = \"{}\"\n",
            toml::escape_string(&self.module_root)
        ));
        out.push('\n');
        out.push_str("[dependencies]\n");
        for dep in self.dependencies.values() {
            match &dep.source {
                DependencySource::Path(path) => out.push_str(&format!(
                    "{} = {{ path = \"{}\" }}\n",
                    dep.name,
                    toml::escape_string(path)
                )),
            }
        }
        out
    }
}

/// Parse one `[dependencies]` entry's value into a [`DependencySource`].
fn parse_dependency_source(key: &str, value: &Value) -> Result<DependencySource, ManifestError> {
    let table = value.as_table().ok_or_else(|| {
        ManifestError::UnsupportedDependency(format!(
            "dependency `{key}` must be an inline table like `{{ path = \"...\" }}`"
        ))
    })?;
    if let Some(path) = table.get("path") {
        let path = path.as_str().ok_or_else(|| {
            ManifestError::UnsupportedDependency(format!(
                "dependency `{key}`: `path` must be a string"
            ))
        })?;
        return Ok(DependencySource::Path(path.to_string()));
    }
    Err(ManifestError::UnsupportedDependency(format!(
        "dependency `{key}` has no supported source; v0 supports only `path` dependencies"
    )))
}

/// Read a required string field from a table, naming the table for errors.
fn string_field<'a>(
    table: &'a BTreeMap<String, Value>,
    field: &'static str,
    table_name: &'static str,
) -> Result<&'a str, ManifestError> {
    let value = table
        .get(field)
        .ok_or(ManifestError::MissingField(table_name, field))?;
    value
        .as_str()
        .ok_or(ManifestError::FieldType(field, "string"))
}

/// An error from loading or validating a manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    /// The underlying TOML did not parse.
    Toml(toml::TomlError),
    /// A required `[table]` is missing.
    MissingTable(&'static str),
    /// A required field is missing from a table (`table`, `field`).
    MissingField(&'static str, &'static str),
    /// A field had the wrong value kind (`field`, expected-kind).
    FieldType(&'static str, &'static str),
    /// A name (package or dependency) is not a valid package name.
    InvalidName(String),
    /// The `edition` string is not a supported edition.
    UnknownEdition(String),
    /// A dependency uses a source kind v0 does not support.
    UnsupportedDependency(String),
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ManifestError::Toml(e) => write!(f, "manifest is not valid TOML: {e}"),
            ManifestError::MissingTable(t) => write!(f, "manifest is missing the `[{t}]` table"),
            ManifestError::MissingField(t, field) => {
                write!(
                    f,
                    "manifest `[{t}]` is missing the required `{field}` field"
                )
            }
            ManifestError::FieldType(field, kind) => {
                write!(f, "manifest field `{field}` must be a {kind}")
            }
            ManifestError::InvalidName(name) => write!(
                f,
                "`{name}` is not a valid package name (lowercase letters, digits, and \
                 underscores; must start with a letter)"
            ),
            ManifestError::UnknownEdition(e) => {
                write!(
                    f,
                    "edition `{e}` is not supported (this toolchain supports `2024`)"
                )
            }
            ManifestError::UnsupportedDependency(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for ManifestError {}

#[cfg(test)]
mod tests {
    use super::{Dependency, DependencySource, Edition, Manifest, ManifestError, PackageName};

    #[test]
    fn valid_names() {
        assert!(PackageName::new("app").is_ok());
        assert!(PackageName::new("my_util2").is_ok());
        assert!(PackageName::new("").is_err());
        assert!(PackageName::new("2fast").is_err());
        assert!(PackageName::new("Cap").is_err());
        assert!(PackageName::new("has-dash").is_err());
    }

    #[test]
    fn parses_a_full_manifest() {
        let m = Manifest::parse(
            "\
[package]
name = \"app\"
version = \"0.1.0\"
edition = \"2024\"

[modules]
root = \"lib\"

[dependencies]
util = { path = \"../util\" }
",
        )
        .expect("valid");
        assert_eq!(m.name.as_str(), "app");
        assert_eq!(m.version, "0.1.0");
        assert_eq!(m.edition, Edition::E2024);
        assert_eq!(m.module_root, "lib");
        let util = &m.dependencies[&PackageName::new("util").unwrap()];
        assert_eq!(
            *util,
            Dependency {
                name: PackageName::new("util").unwrap(),
                source: DependencySource::Path("../util".into()),
            }
        );
    }

    #[test]
    fn edition_and_module_root_default() {
        let m = Manifest::parse("[package]\nname = \"a\"\nversion = \"0.0.0\"\n").expect("valid");
        assert_eq!(m.edition, Edition::E2024);
        assert_eq!(m.module_root, Manifest::DEFAULT_MODULE_ROOT);
        assert!(m.dependencies.is_empty());
    }

    #[test]
    fn unknown_edition_is_rejected() {
        let e = Manifest::parse("[package]\nname = \"a\"\nversion = \"0\"\nedition = \"2030\"\n")
            .expect_err("unknown edition");
        assert_eq!(e, ManifestError::UnknownEdition("2030".into()));
    }

    #[test]
    fn missing_package_table_is_rejected() {
        let e = Manifest::parse("[modules]\nroot = \"src\"\n").expect_err("no package");
        assert_eq!(e, ManifestError::MissingTable("package"));
    }

    #[test]
    fn non_path_dependency_is_rejected() {
        let e = Manifest::parse(
            "[package]\nname = \"a\"\nversion = \"0\"\n[dependencies]\nx = { git = \"u\" }\n",
        )
        .expect_err("git unsupported");
        assert!(matches!(e, ManifestError::UnsupportedDependency(_)));
    }

    #[test]
    fn round_trips_through_toml() {
        let mut m = Manifest::new(PackageName::new("app").unwrap(), "1.2.3", Edition::E2024);
        m.dependencies.insert(
            PackageName::new("util").unwrap(),
            Dependency {
                name: PackageName::new("util").unwrap(),
                source: DependencySource::Path("../util".into()),
            },
        );
        let text = m.to_toml();
        let reparsed = Manifest::parse(&text).expect("round-trips");
        assert_eq!(m, reparsed);
    }
}
