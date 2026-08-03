//! Fuzz target: AST lowering. Building the typed AST views and lowering to HIR
//! over arbitrary input must never crash — unresolved and malformed constructs
//! become poison nodes, not panics.
//!
//! A thin call into the shared checker `tuo_fuzz::stages::check_ast_lowering`.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|text: &str| {
    tuo_fuzz::guarded("ast-lowering", text, tuo_fuzz::stages::check_ast_lowering);
});
