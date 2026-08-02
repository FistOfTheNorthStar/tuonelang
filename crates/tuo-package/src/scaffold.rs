//! Scaffolding a new package (`tuo new`).
//!
//! A new package is the smallest thing that resolves, checks, and runs: a
//! manifest, a module root, and one `main` module. The scaffolder returns the
//! files to write as plain (relative-path, contents) pairs so the CLI owns the
//! actual filesystem writes and their error reporting.

use crate::manifest::{Edition, Manifest, PackageName};
use crate::resolve::MANIFEST_FILE;

/// One file the scaffolder wants written, relative to the new package directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScaffoldFile {
    /// The path relative to the package directory.
    pub relative_path: String,
    /// The file's contents.
    pub contents: String,
}

/// The files a freshly scaffolded package consists of: its manifest and a
/// starter `main` module under the default module root.
///
/// The starter program is deliberately the minimal valid one — a nullary
/// `main` returning `0` — so a new package passes `tuo check` and `tuo run`
/// immediately.
#[must_use]
pub fn new_package(name: &PackageName) -> Vec<ScaffoldFile> {
    let manifest = Manifest::new(name.clone(), "0.1.0", Edition::E2024);
    let main = format!(
        "\
// The entry module of `{name}`. `main` is nullary and returns the process
// exit status. Run it with `tuo run` (once the package is built) or check it
// with `tuo check`.

module {name};

/// The program entry point.
fn main() -> Int {{
    0
}}

/// A worked example that the reference interpreter can execute.
spec main {{
    then main() == 0;
}}
"
    );

    vec![
        ScaffoldFile {
            relative_path: MANIFEST_FILE.to_string(),
            contents: manifest.to_toml(),
        },
        ScaffoldFile {
            relative_path: format!("{}/main.tuo", manifest.module_root),
            contents: main,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::new_package;
    use crate::manifest::{Manifest, PackageName};

    #[test]
    fn scaffolds_a_manifest_and_a_main_module() {
        let name = PackageName::new("demo").unwrap();
        let files = new_package(&name);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].relative_path, "tdg.toml");
        assert_eq!(files[1].relative_path, "src/main.tuo");

        // The scaffolded manifest re-parses to the same package.
        let m = Manifest::parse(&files[0].contents).expect("valid manifest");
        assert_eq!(m.name, name);

        // The starter module declares `module demo;` and a `main`.
        assert!(files[1].contents.contains("module demo;"));
        assert!(files[1].contents.contains("fn main() -> Int"));
    }
}
