//! Build script for `tuo-codegen`.
//!
//! It records the host target triple this crate is built for as the
//! compile-time environment variable `TUO_CODEGEN_HOST_TRIPLE`, so
//! [`current_host_triple`](../fn.current_host_triple.html) can name the one
//! target every v0 backend must support without depending on a runtime
//! `target-lexicon` lookup in the interface crate.

fn main() {
    // Cargo sets `TARGET` to the triple being compiled for; for a normal
    // (non-cross) build of the compiler this is the host the `tuo` binary runs
    // on. Re-export it under our own name for `env!` to read.
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_owned());
    println!("cargo:rustc-env=TUO_CODEGEN_HOST_TRIPLE={target}");
}
