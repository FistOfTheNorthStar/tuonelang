//! Fuzz target: the parser. Parsing arbitrary input must never crash, and the
//! parse tree must uphold the losslessness invariants (full coverage,
//! byte-identical reconstruction) however broken the input is.
//!
//! A thin call into the shared checker `tuo_fuzz::stages::check_parser`.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|text: &str| {
    tuo_fuzz::guarded("parser", text, tuo_fuzz::stages::check_parser);
});
