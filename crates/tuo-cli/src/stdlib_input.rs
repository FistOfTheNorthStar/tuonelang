//! The standard-library modules a program names, loaded beside it (ADR-0038).
//!
//! The library is written in tuonelang and consumed as input — the compiler
//! facade never links it — so it is the host's job to put the modules a
//! program uses into the program's [`SourceMap`]. Every command that compiles
//! user files does it here, through one function, so `check`, `spec`,
//! `verify`, `build`, and `run` agree on what a program means. The selection
//! itself is the catalog's ([`tuo_stdlib::referenced_by`]); the user's files
//! stay the [scope](tuo_spec::run_scoped) whose specs run.

use tuo_compiler::source::{SourceId, SourceMap};

/// Add the catalog modules `user`'s text names (and their dependencies) to
/// `map`, returning the whole program: `user` first, then the library.
pub(crate) fn with_referenced_stdlib(map: &mut SourceMap, user: &[SourceId]) -> Vec<SourceId> {
    let modules = {
        let texts: Vec<&str> = user.iter().map(|&id| map.source(id).text()).collect();
        tuo_stdlib::referenced_by(texts)
    };
    let mut program = user.to_vec();
    for module in modules {
        let file = map.intern_file(module.name);
        // A catalog module is far below the source-size limit; one that is not
        // is a catalog defect `tuo-cli/tests/stdlib.rs` would already reject.
        if let Ok(id) = map.add_source(file, module.source) {
            program.push(id);
        }
    }
    program
}

/// The catalog modules `texts` name, for a host that builds its program some
/// other way (the incremental session behind `verify --affected-by`).
pub(crate) fn referenced_modules(texts: &[&str]) -> Vec<tuo_stdlib::Module> {
    tuo_stdlib::referenced_by(texts.iter().copied())
}

/// Is `id` a standard-library module this host loaded — interned under the
/// catalog's own file name with the catalog's own text? A catalog file a user
/// passes explicitly is interned under its path, so it counts as theirs.
pub(crate) fn is_library(map: &SourceMap, id: SourceId) -> bool {
    let Some(text) = map.get_source(id) else {
        return false;
    };
    let name = map.file_name(text.file());
    tuo_stdlib::MODULES
        .iter()
        .any(|module| module.name == name && module.source == text.text())
}
