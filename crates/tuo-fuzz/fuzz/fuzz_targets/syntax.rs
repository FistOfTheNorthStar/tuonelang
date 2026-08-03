//! Fuzz target: syntax-tree operations. The CST built for arbitrary input must
//! cover the input and reconstruct it byte-for-byte (losslessness), and building
//! it must never crash.
//!
//! A thin call into the shared checker `tuo_fuzz::stages::check_syntax`.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|text: &str| {
    tuo_fuzz::guarded("syntax", text, tuo_fuzz::stages::check_syntax);
});
