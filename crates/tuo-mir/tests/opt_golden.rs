//! Before/after golden tests for the MIR optimization passes
//! (`tests/mir/opt/fixtures/`).
//!
//! For each accepted `*.tuo` fixture the suite lowers to verified MIR, then
//! renders the raw MIR into a `*.mir` sibling and the optimized MIR into a
//! `*.opt.mir` sibling, checking both match byte-for-byte (bless with
//! `TUO_BLESS=1`). The diff between the two files *is* the visible effect of
//! the pass pipeline. Both the raw and the optimized MIR must pass the
//! mandatory `tuo_mir::verify` gate.
//!
//! This suite pins the *syntactic* effect of optimization. Its **semantic**
//! counterpart — that the reference interpreter produces the same outcome for
//! the raw and the optimized MIR, including preserving traps — lives in
//! `tuo-cli/tests/opt_semantics.rs`, because the interpreter (`tuo-mir-interp`)
//! sits above `tuo-mir` in the dependency layering and may not be a dependency
//! here.
//!
//! Bless the goldens after an intended change with:
//!
//! ```bash
//! TUO_BLESS=1 cargo test -p tuo-mir --test opt_golden
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use tuo_ast::Ast;
use tuo_mir::Program;
use tuo_resolve::Resolution;
use tuo_source::SourceMap;
use tuo_types::TypeckResult;

fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/mir/opt/fixtures")
}

/// Lower a fixture to verified MIR, returning the program, its type-check
/// result, and a resolution for rendering. Panics on any front-end diagnostic
/// (fixtures must be accepted).
fn lower(name: &str, text: &str) -> (Program, TypeckResult, Resolution) {
    let mut map = SourceMap::new();
    let file = map.intern_file(name);
    let id = map.add_source(file, text).expect("fixture fits");
    let parse = tuo_parser::parse(map.source(id));
    assert_eq!(parse.diagnostics, vec![], "{name}: parse errors");
    let asts = [Ast::new(&parse.tree, text)];
    let resolution = tuo_resolve::resolve(&asts);
    assert_eq!(resolution.diagnostics(), &[], "{name}: resolution errors");
    let types = tuo_types::check(&asts, &resolution);
    assert_eq!(types.diagnostics(), &[], "{name}: type errors");
    let ownership = tuo_ownership::check(&asts, &resolution, &types);
    assert_eq!(ownership.diagnostics(), &[], "{name}: ownership errors");
    let hir = tuo_hir::lower(&asts, &resolution);
    let program = tuo_mir::lower(&hir, &resolution, &types);
    assert!(
        tuo_mir::verify(&program, &types).is_empty(),
        "{name}: raw MIR failed verification"
    );
    (program, types, resolution)
}

fn fixtures(root: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = fs::read_dir(root)
        .expect("fixtures dir exists")
        .map(|entry| entry.expect("readable entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "tuo"))
        .collect();
    entries.sort();
    entries
}

fn bless_or_check(golden_path: &Path, rendered: &str, label: &str) {
    if std::env::var_os("TUO_BLESS").is_some() {
        fs::write(golden_path, rendered).expect("golden is writable");
    }
    let golden = fs::read_to_string(golden_path).unwrap_or_else(|_| {
        panic!(
            "missing golden {} — run with TUO_BLESS=1 to create it",
            golden_path.display()
        )
    });
    assert_eq!(
        rendered, golden,
        "{label}: MIR diverged — rerun with TUO_BLESS=1 after verifying"
    );
}

/// The `foo.opt.mir` golden path for a `foo.tuo` fixture.
fn opt_golden_path(fixture: &Path) -> PathBuf {
    fixture.with_file_name(format!(
        "{}.opt.mir",
        fixture.file_stem().expect("stem").to_string_lossy()
    ))
}

#[test]
fn opt_before_after_goldens_match() {
    let root = fixtures_root();
    let entries = fixtures(&root);
    assert!(entries.len() >= 4, "opt fixture corpus went missing");

    for path in entries {
        let name = path.file_name().expect("has name").to_string_lossy();
        let text = fs::read_to_string(&path).expect("fixture readable");

        let (raw, types, resolution) = lower(&name, &text);

        // Before: the raw lowering.
        let raw_render = tuo_mir::render(&raw, &resolution);
        bless_or_check(
            &path.with_extension("mir"),
            &raw_render,
            &format!("{name} (raw)"),
        );

        // After: the optimized MIR, re-verified.
        let mut optimized = raw.clone();
        let report = tuo_mir::optimize(&mut optimized, &types);
        assert!(
            tuo_mir::verify(&optimized, &types).is_empty(),
            "{name}: optimized MIR failed verification"
        );
        assert!(
            !report.passes.is_empty(),
            "{name}: the optimizer reported no passes"
        );
        let opt_render = tuo_mir::render(&optimized, &resolution);
        bless_or_check(
            &opt_golden_path(&path),
            &opt_render,
            &format!("{name} (opt)"),
        );

        // Optimization is deterministic: optimizing again yields the same MIR.
        let mut again = raw.clone();
        let _ = tuo_mir::optimize(&mut again, &types);
        assert_eq!(
            tuo_mir::render(&again, &resolution),
            opt_render,
            "{name}: optimization is nondeterministic"
        );
    }
}

#[test]
fn opt_goldens_have_no_orphans() {
    let root = fixtures_root();
    for entry in fs::read_dir(&root).expect("fixtures dir exists") {
        let path = entry.expect("readable entry").path();
        if path.extension().is_some_and(|ext| ext == "mir") {
            // Both `foo.mir` and `foo.opt.mir` must have a `foo.tuo` sibling.
            let stem = path
                .file_name()
                .expect("name")
                .to_string_lossy()
                .trim_end_matches(".opt.mir")
                .trim_end_matches(".mir")
                .to_owned();
            let fixture = root.join(format!("{stem}.tuo"));
            assert!(
                fixture.exists(),
                "golden {:?} has no .tuo fixture",
                path.file_name()
            );
        }
    }
}
