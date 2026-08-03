//! Fuzz target: the MIR interpreter. Verified MIR — the only MIR the interpreter
//! accepts through its mandatory verify gate — must never trigger a structural
//! panic: a run terminates as a value or a structured trap, never a Rust panic
//! and never `TrapKind::Internal`.
//!
//! A thin call into the shared checker `tuo_fuzz::stages::check_interp`.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|text: &str| {
    tuo_fuzz::guarded("interp", text, tuo_fuzz::stages::check_interp);
});
