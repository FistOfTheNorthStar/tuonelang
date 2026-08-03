//! Fuzz target: the lexer. Arbitrary UTF-8 must never crash the lexer, and the
//! token stream's losslessness invariants must hold on every input.
//!
//! A thin call into the shared checker `tuo_fuzz::stages::check_lexer` — the
//! same function the stable `tests/sweep.rs` sweep drives. On a crash, the input
//! is auto-filed as a regression fixture under `regressions/lexer/` (see
//! `fuzz_targets/README.md`).

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|text: &str| {
    tuo_fuzz::guarded("lexer", text, tuo_fuzz::stages::check_lexer);
});
