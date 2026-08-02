//! The lockfile: `tdg.lock`.
//!
//! The lockfile is the machine-maintained record of a **resolved** dependency
//! graph. Where the manifest ([`crate::manifest`]) says *what* a package
//! depends on, the lockfile pins *exactly which bytes* every package in the
//! graph resolved to, so a build is reproducible: rerunning resolution against
//! an unchanged workspace yields a byte-identical lockfile, and a build that
//! finds a source whose checksum no longer matches its locked entry refuses to
//! proceed rather than compile drifted code.
//!
//! # Shape
//!
//! ```toml
//! version = 1                       # lockfile format version
//!
//! [[package]]
//! name = "app"
//! version = "0.1.0"
//! source = "root"                   # the root package under resolution
//! checksum = "<sha256 of its concatenated module sources>"
//! dependencies = ["util"]           # names of its direct dependencies
//!
//! [[package]]
//! name = "util"
//! version = "0.2.0"
//! source = "path+../util"           # a path dependency and where it lived
//! checksum = "<sha256>"
//! dependencies = []
//! ```
//!
//! `[[package]]` entries are always written in package-name order, so the file
//! is deterministic and diff-friendly regardless of resolution order.

use std::fmt;

use crate::toml::{self, Value};

/// The lockfile format version this crate reads and writes. Bumped only on a
/// backwards-incompatible change to the file's shape.
pub const LOCKFILE_VERSION: u64 = 1;

/// How a locked package's source was obtained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockedSource {
    /// The root package under resolution (the workspace being built).
    Root,
    /// A path dependency, recorded with the path it was fetched from.
    Path(String),
}

impl LockedSource {
    /// The wire form written to `source = "..."`.
    #[must_use]
    pub fn to_wire(&self) -> String {
        match self {
            LockedSource::Root => "root".to_string(),
            LockedSource::Path(path) => format!("path+{path}"),
        }
    }

    /// Parse the wire form.
    fn parse(raw: &str) -> Result<Self, LockfileError> {
        if raw == "root" {
            Ok(LockedSource::Root)
        } else if let Some(path) = raw.strip_prefix("path+") {
            Ok(LockedSource::Path(path.to_string()))
        } else {
            Err(LockfileError::UnknownSource(raw.to_string()))
        }
    }
}

/// One resolved package in the locked graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedPackage {
    /// The package's name.
    pub name: String,
    /// The package's version string, copied from its manifest.
    pub version: String,
    /// Where its source came from.
    pub source: LockedSource,
    /// The SHA-256 (hex) of the package's concatenated module sources — the
    /// content hash that makes the build reproducible.
    pub checksum: String,
    /// The names of this package's direct dependencies, in name order.
    pub dependencies: Vec<String>,
}

/// A parsed or freshly resolved `tdg.lock`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lockfile {
    /// The lockfile format version.
    pub version: u64,
    /// Every package in the resolved graph, kept in name order.
    pub packages: Vec<LockedPackage>,
}

impl Lockfile {
    /// Build a lockfile from resolved packages, sorting them into the canonical
    /// name order the file is always written in.
    #[must_use]
    pub fn new(mut packages: Vec<LockedPackage>) -> Self {
        packages.sort_by(|a, b| a.name.cmp(&b.name));
        Self {
            version: LOCKFILE_VERSION,
            packages,
        }
    }

    /// Look up a locked package by name.
    #[must_use]
    pub fn package(&self, name: &str) -> Option<&LockedPackage> {
        self.packages.iter().find(|p| p.name == name)
    }

    /// Parse a lockfile from `tdg.lock` text.
    ///
    /// # Errors
    ///
    /// Returns a [`LockfileError`] on a TOML syntax error, an unsupported
    /// lockfile version, a missing field, or an unknown source form.
    pub fn parse(text: &str) -> Result<Self, LockfileError> {
        let doc = toml::parse(text).map_err(LockfileError::Toml)?;
        let version = doc
            .root
            .get("version")
            .and_then(Value::as_integer)
            .ok_or(LockfileError::MissingVersion)?;
        if version != LOCKFILE_VERSION {
            return Err(LockfileError::UnsupportedVersion(version));
        }

        let mut packages = Vec::new();
        for table in doc.array_table("package") {
            let name = req_string(table, "name")?.to_string();
            let version = req_string(table, "version")?.to_string();
            let source = LockedSource::parse(req_string(table, "source")?)?;
            let checksum = req_string(table, "checksum")?.to_string();
            let dependencies = match table.get("dependencies") {
                Some(value) => value
                    .as_array()
                    .ok_or(LockfileError::FieldType("dependencies", "array"))?
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .map(str::to_string)
                            .ok_or(LockfileError::FieldType("dependencies", "array of strings"))
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                None => Vec::new(),
            };
            packages.push(LockedPackage {
                name,
                version,
                source,
                checksum,
                dependencies,
            });
        }

        // A lockfile read from disk should already be in name order; sort so an
        // in-memory value compares equal regardless of on-disk order.
        Ok(Self::new_with_version(version, packages))
    }

    /// Build a lockfile with an explicit version (used by the parser, which has
    /// already validated the version).
    fn new_with_version(version: u64, mut packages: Vec<LockedPackage>) -> Self {
        packages.sort_by(|a, b| a.name.cmp(&b.name));
        Self { version, packages }
    }

    /// Render the lockfile to canonical `tdg.lock` text.
    ///
    /// Deterministic: packages in name order, dependency lists in name order, a
    /// leading auto-generated banner. The same resolved graph always produces
    /// byte-identical output.
    #[must_use]
    pub fn to_toml(&self) -> String {
        let mut out = String::new();
        out.push_str("# This file is @generated by tuo.\n");
        out.push_str("# It is machine-maintained; edit tdg.toml and re-resolve instead.\n");
        out.push_str(&format!("version = {}\n", self.version));
        for package in &self.packages {
            out.push_str("\n[[package]]\n");
            out.push_str(&format!(
                "name = \"{}\"\n",
                toml::escape_string(&package.name)
            ));
            out.push_str(&format!(
                "version = \"{}\"\n",
                toml::escape_string(&package.version)
            ));
            out.push_str(&format!(
                "source = \"{}\"\n",
                toml::escape_string(&package.source.to_wire())
            ));
            out.push_str(&format!(
                "checksum = \"{}\"\n",
                toml::escape_string(&package.checksum)
            ));
            let deps = package
                .dependencies
                .iter()
                .map(|d| format!("\"{}\"", toml::escape_string(d)))
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!("dependencies = [{deps}]\n"));
        }
        out
    }
}

/// Read a required string field from a `[[package]]` table.
fn req_string<'a>(
    table: &'a std::collections::BTreeMap<String, Value>,
    field: &'static str,
) -> Result<&'a str, LockfileError> {
    table
        .get(field)
        .ok_or(LockfileError::MissingField(field))?
        .as_str()
        .ok_or(LockfileError::FieldType(field, "string"))
}

/// An error from loading a lockfile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockfileError {
    /// The underlying TOML did not parse.
    Toml(toml::TomlError),
    /// The top-level `version` key is missing.
    MissingVersion,
    /// The lockfile version is not one this toolchain reads.
    UnsupportedVersion(u64),
    /// A required `[[package]]` field is missing.
    MissingField(&'static str),
    /// A field had the wrong value kind (`field`, expected-kind).
    FieldType(&'static str, &'static str),
    /// A `source` string used an unrecognized form.
    UnknownSource(String),
}

impl fmt::Display for LockfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LockfileError::Toml(e) => write!(f, "lockfile is not valid TOML: {e}"),
            LockfileError::MissingVersion => write!(f, "lockfile is missing its `version` key"),
            LockfileError::UnsupportedVersion(v) => write!(
                f,
                "lockfile version {v} is not supported (this toolchain writes version {LOCKFILE_VERSION})"
            ),
            LockfileError::MissingField(field) => {
                write!(f, "a `[[package]]` entry is missing its `{field}` field")
            }
            LockfileError::FieldType(field, kind) => {
                write!(f, "lockfile field `{field}` must be a {kind}")
            }
            LockfileError::UnknownSource(s) => {
                write!(f, "lockfile source `{s}` is not a recognized source form")
            }
        }
    }
}

impl std::error::Error for LockfileError {}

#[cfg(test)]
mod tests {
    use super::{LockedPackage, LockedSource, Lockfile, LockfileError};

    fn sample() -> Lockfile {
        Lockfile::new(vec![
            LockedPackage {
                name: "app".into(),
                version: "0.1.0".into(),
                source: LockedSource::Root,
                checksum: "aaaa".into(),
                dependencies: vec!["util".into()],
            },
            LockedPackage {
                name: "util".into(),
                version: "0.2.0".into(),
                source: LockedSource::Path("../util".into()),
                checksum: "bbbb".into(),
                dependencies: vec![],
            },
        ])
    }

    #[test]
    fn round_trips() {
        let lock = sample();
        let text = lock.to_toml();
        let reparsed = Lockfile::parse(&text).expect("round-trips");
        assert_eq!(lock, reparsed);
    }

    #[test]
    fn is_written_in_name_order() {
        // Construct out of order; `new` must canonicalize.
        let lock = Lockfile::new(vec![
            LockedPackage {
                name: "zed".into(),
                version: "1".into(),
                source: LockedSource::Path("z".into()),
                checksum: "z".into(),
                dependencies: vec![],
            },
            LockedPackage {
                name: "app".into(),
                version: "1".into(),
                source: LockedSource::Root,
                checksum: "a".into(),
                dependencies: vec![],
            },
        ]);
        assert_eq!(lock.packages[0].name, "app");
        assert_eq!(lock.packages[1].name, "zed");
    }

    #[test]
    fn deterministic_output() {
        assert_eq!(sample().to_toml(), sample().to_toml());
    }

    #[test]
    fn unsupported_version_is_rejected() {
        let e = Lockfile::parse("version = 2\n").expect_err("v2 unsupported");
        assert_eq!(e, LockfileError::UnsupportedVersion(2));
    }

    #[test]
    fn missing_field_is_rejected() {
        let e = Lockfile::parse("version = 1\n[[package]]\nname = \"a\"\n")
            .expect_err("incomplete package");
        assert!(matches!(e, LockfileError::MissingField(_)));
    }

    #[test]
    fn source_wire_forms() {
        assert_eq!(LockedSource::Root.to_wire(), "root");
        assert_eq!(LockedSource::Path("../x".into()).to_wire(), "path+../x");
    }
}
