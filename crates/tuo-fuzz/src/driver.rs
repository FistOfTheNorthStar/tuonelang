//! The shared fuzz-target driver: run a stage checker on one input and, if it
//! crashes, file the input as a regression fixture before the crash propagates.
//!
//! Every `cargo fuzz` target is a one-liner into [`guarded`], so the
//! auto-recording policy lives in exactly one place and each target stays a thin
//! wrapper around a [`stages`](crate::stages) checker. Recording *before*
//! re-raising the panic means libFuzzer still sees the crash (and writes its own
//! artifact for minimization), while the crate's own content-addressed fixture
//! is captured too — the committed, replayable half of the mechanism.

use crate::regression;

/// Run `check(input)` for `stage`; if it panics, record `input` as a regression
/// fixture under `regressions/<stage>/`, then re-raise the panic.
///
/// The recording is best-effort: an I/O failure while filing the fixture is
/// swallowed (the original panic is what matters and must still surface). A
/// successful run returns normally, having done nothing but the check.
///
/// This is the auto-fixture write path. The read path — replaying every
/// committed fixture — is [`regression::replay_all`], run by the stable test
/// suite.
pub fn guarded(stage: &'static str, input: &str, check: fn(&str)) {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(input)));
    if let Err(payload) = outcome {
        // File the crashing input as a committed-able fixture. Ignore a write
        // error: the panic below is the signal that must not be lost.
        let _ = regression::record(stage, input);
        std::panic::resume_unwind(payload);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guarded_passes_through_a_clean_run() {
        // A total checker on valid input returns normally, no fixture written.
        guarded(
            "lexer",
            "fn main() -> Int { 0 }\n",
            crate::stages::check_lexer,
        );
    }

    #[test]
    fn guarded_records_then_reraises_on_panic() {
        // Redirect the fixture root by running against a stage that exists, but
        // with a checker that always panics — proving the panic still
        // propagates. We catch it here so the test itself passes.
        fn always_panics(_: &str) {
            panic!("synthetic crash");
        }
        let result = std::panic::catch_unwind(|| {
            guarded("front-end", "synthetic-crash-input-xyz", always_panics);
        });
        assert!(result.is_err(), "guarded must re-raise the panic");
        // The synthetic input was filed under regressions/front-end/. Clean it
        // up so the committed corpus is not polluted by this test.
        let stem = regression::fixture_stem("synthetic-crash-input-xyz");
        let path = regression::corpus_root()
            .join("front-end")
            .join(format!("{stem}.tuo"));
        if path.exists() {
            let _ = std::fs::remove_file(&path);
        }
    }
}
