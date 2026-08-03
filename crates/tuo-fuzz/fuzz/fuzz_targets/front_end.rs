//! Fuzz target: the whole front end — resolution, the type checker, and the
//! ownership checker. Driving all three over arbitrary input must never crash;
//! every reported diagnostic must carry a well-formed in-bounds span.
//!
//! A thin call into the shared checker `tuo_fuzz::stages::check_front_end`.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|text: &str| {
    tuo_fuzz::guarded("front-end", text, tuo_fuzz::stages::check_front_end);
});
