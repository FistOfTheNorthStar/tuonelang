//! Fuzz target: the formatter. Formatting arbitrary input must never crash and
//! never return unverified output; it must be idempotent; and valid formatted
//! source must remain parseable with the same diagnostics.
//!
//! A thin call into the shared checker `tuo_fuzz::stages::check_fmt`.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|text: &str| {
    tuo_fuzz::guarded("fmt", text, tuo_fuzz::stages::check_fmt);
});
