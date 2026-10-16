//! Build script for `tuo-cli`: precompile the C runtime shims.
//!
//! Every executable `tuo build`/`tuo run` produces links four C shims from
//! `tuo-runtime` — the trap, effect, allocator, and map runtimes. Compiling them
//! from source on every build cost about 110 ms of a 120 ms build, for objects
//! that are identical every time: the shims depend only on `tuo-runtime`'s
//! source and the host target, and both are fixed when `tuo` itself is built.
//! So they are compiled once, here, and embedded in the binary
//! (`runtime_objects.rs` in `OUT_DIR`, read by `codegen::link`).
//!
//! Precompiling is an optimization, never a requirement. When the compiler is
//! being cross-built (`HOST != TARGET`, so the host `cc` would produce objects
//! for the wrong machine) or the C compiler cannot be run here, the generated
//! table is empty and `link` falls back to compiling the shims from source,
//! exactly as before. Cargo reruns this script whenever `tuo-runtime` (a
//! build-dependency) changes, so an embedded object can never be stale.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let host = std::env::var("HOST").unwrap_or_default();
    let target = std::env::var("TARGET").unwrap_or_default();

    let shims = [
        ("trap", tuo_runtime::trap_runtime_c_source()),
        ("effect", tuo_runtime::effect::effect_runtime_c_source()),
        ("alloc", tuo_runtime::alloc::alloc_runtime_c_source()),
        ("map", tuo_runtime::map::map_runtime_c_source()),
    ];

    let objects = if host == target {
        shims
            .iter()
            .map(|(name, source)| compile(&out_dir, name, source).map(|path| (*name, path)))
            .collect::<Option<Vec<_>>>()
    } else {
        None
    };

    let mut table = String::from(
        "/// The runtime shims, precompiled for the host when `tuo` was built, as\n\
         /// `(name, object bytes)`; `None` when they must be compiled at link time.\n",
    );
    match objects {
        Some(objects) => {
            table.push_str(
                "pub(crate) const PRECOMPILED_RUNTIME: Option<&[(&str, &[u8])]> = Some(&[\n",
            );
            for (name, path) in objects {
                let _ = writeln!(
                    table,
                    "    ({name:?}, include_bytes!({:?})),",
                    path.display().to_string()
                );
            }
            table.push_str("]);\n");
        }
        None => {
            println!(
                "cargo:warning=tuo-cli: runtime shims not precompiled; `tuo build` will compile them per build"
            );
            table.push_str(
                "pub(crate) const PRECOMPILED_RUNTIME: Option<&[(&str, &[u8])]> = None;\n",
            );
        }
    }
    std::fs::write(out_dir.join("runtime_objects.rs"), table).expect("write runtime_objects.rs");
}

/// Compile one shim to an object with the same flags `codegen::link` uses for
/// the from-source fallback; `None` if the C compiler is unavailable or fails.
fn compile(out_dir: &Path, name: &str, source: &str) -> Option<PathBuf> {
    let c_path = out_dir.join(format!("tuo_rt_{name}.c"));
    let o_path = out_dir.join(format!("tuo_rt_{name}.o"));
    std::fs::write(&c_path, source).ok()?;
    let status = Command::new("cc")
        .args(["-O2", "-pthread", "-c"])
        .arg(&c_path)
        .arg("-o")
        .arg(&o_path)
        .status()
        .ok()?;
    status.success().then_some(o_path)
}
