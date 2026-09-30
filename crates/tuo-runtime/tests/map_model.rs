//! The map shim checked against its reference semantics, in C.
//!
//! `tuo_runtime::map`'s table is C, linked into every built binary, so its
//! only other coverage is end to end: native programs agreeing with the
//! interpreter. This suite drives the shim **directly**, compiling the real
//! [`map_runtime_c_source`] with the model checker in `tests/map_model.c` —
//! thousands of seeded operation sequences against an insertion-ordered
//! association list, over `Int` and `Str` keys and one- and two-word values,
//! plus a structural check that removal moves no entry. It builds with
//! `-Wall -Wextra -Werror` and, where the toolchain has them, AddressSanitizer
//! and UndefinedBehaviorSanitizer, so an out-of-bounds probe or a double free
//! fails here rather than as a corrupted value three programs later.

use std::path::PathBuf;
use std::process::Command;

use tuo_runtime::alloc::ZERO_SIZE_SENTINEL;
use tuo_runtime::map::map_runtime_c_source;

/// A fresh scratch directory for this test's sources and binary.
fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tuo_map_model_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create the scratch directory");
    dir
}

/// Compile the shim and the model checker with `extra` flags; `true` when
/// `cc` produced a binary.
fn compile(dir: &std::path::Path, exe: &std::path::Path, extra: &[&str]) -> bool {
    Command::new("cc")
        .args(["-std=c11", "-O1", "-g", "-Wall", "-Wextra", "-Werror"])
        .arg(format!("-DTUO_SENTINEL={ZERO_SIZE_SENTINEL}"))
        .args(extra)
        .arg(dir.join("map_rt.c"))
        .arg(dir.join("map_model.c"))
        .arg("-o")
        .arg(exe)
        .status()
        .is_ok_and(|status| status.success())
}

#[test]
fn the_map_shim_agrees_with_the_association_list_model() {
    let dir = scratch();
    std::fs::write(dir.join("map_rt.c"), map_runtime_c_source()).expect("write the shim");
    std::fs::write(dir.join("map_model.c"), include_str!("map_model.c"))
        .expect("write the model checker");
    let exe = dir.join("map_model");

    // Sanitizers when the toolchain ships their runtimes; the model check
    // itself runs either way, since it is the load-bearing half.
    let sanitized = compile(
        &dir,
        &exe,
        &["-fsanitize=address,undefined", "-fno-sanitize-recover=all"],
    );
    if !sanitized {
        assert!(
            compile(&dir, &exe, &[]),
            "the map shim and its model checker must compile warning-free"
        );
    }

    let output = Command::new(&exe).output().expect("run the model checker");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        output.status.success(),
        "map shim disagrees with its model (sanitized: {sanitized}):\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "ok\n");
}
