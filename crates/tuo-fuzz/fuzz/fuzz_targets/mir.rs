//! Fuzz target: HIR → MIR lowering and the MIR verifier. Lowering an accepted
//! program must yield MIR the verifier accepts; the verifier is total on any
//! `Program` (exercised via the interpreter's mandatory verify gate).
//!
//! A thin call into the shared checker `tuo_fuzz::stages::check_mir`.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|text: &str| {
    tuo_fuzz::guarded("mir", text, tuo_fuzz::stages::check_mir);
});
