//! Token counting for corpus entries, reusing the research harness's
//! tokenizers ([`tuo_bench::tokenizer`]).
//!
//! A corpus entry records how its source tokenizes under every deterministic,
//! offline tokenizer the harness ships, so downstream LLM-reliability work has
//! real token budgets to reason about. We reuse the harness rather than count
//! tokens a second way — there must be one measurement of record.

use tuo_bench::tokenizer::Registry;

use crate::metadata::TokenCount;

/// Count `source`'s tokens under every tokenizer in `registry`, in registration
/// order. Returns one [`TokenCount`] per tokenizer.
#[must_use]
pub fn count(registry: &Registry, source: &str) -> Vec<TokenCount> {
    registry
        .iter()
        .map(|tokenizer| TokenCount {
            tokenizer: tokenizer.id().to_string(),
            tokens: tokenizer.token_count(source),
        })
        .collect()
}

/// Count `source`'s tokens under the harness's default built-in adapters.
#[must_use]
pub fn count_builtin(source: &str) -> Vec<TokenCount> {
    count(&Registry::with_builtin_adapters(), source)
}
