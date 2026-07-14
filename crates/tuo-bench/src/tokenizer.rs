//! The tokenizer adapter interface and a registry of available tokenizers.
//!
//! A tokenizer is anything that can turn a source string into a sequence of
//! tokens. The harness is *data-driven*: it never hard-codes a tokenizer, it
//! only ever calls [`Tokenizer::encode`] through this trait. Adding a new
//! tokenizer (including a real production BPE) is therefore a matter of
//! implementing this trait and registering it — the measurement engine
//! ([`crate::measure`]) and the output schema do not change.

pub mod adapters;

use serde::{Deserialize, Serialize};

/// A single token produced by a [`Tokenizer`].
///
/// The token carries its decoded text so that measurements like
/// bytes-per-token can be computed without re-consulting the tokenizer, and so
/// that machine-readable output can show *how* a construct was split.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Token {
    /// The substring of the input this token represents, as the tokenizer
    /// decoded it. Concatenating every token's `text` in order reconstructs the
    /// original input exactly (adapters in this crate guarantee round-trip).
    pub text: String,
}

impl Token {
    /// Construct a token from any string-like value.
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    /// The number of UTF-8 bytes this token spans.
    #[must_use]
    pub fn byte_len(&self) -> usize {
        self.text.len()
    }
}

/// A tokenizer adapter: the single extension point of the tokenizer lab.
///
/// Implementations must be **deterministic** — `encode` called twice on the
/// same input yields identical tokens — and, for adapters shipped in this
/// crate, **lossless**: concatenating the returned tokens' text reproduces the
/// input byte-for-byte. External-vocabulary adapters added later are not
/// required to be lossless, but must remain deterministic so measurements are
/// reproducible.
pub trait Tokenizer {
    /// A short, stable, machine-friendly identifier (e.g. `"bytes"`,
    /// `"whitespace"`, `"gpt-like"`). Used as a key in machine-readable output,
    /// so it must be unique within a registry and stable across runs.
    fn id(&self) -> &str;

    /// A one-line human-readable description of what this tokenizer models.
    fn description(&self) -> &str;

    /// Encode `input` into an ordered sequence of tokens.
    fn encode(&self, input: &str) -> Vec<Token>;

    /// Convenience: the number of tokens `input` encodes to.
    fn token_count(&self, input: &str) -> usize {
        self.encode(input).len()
    }
}

/// An ordered collection of tokenizer adapters to measure against.
///
/// The registry is what makes the harness multi-tokenizer by construction: the
/// measurement engine iterates every registered tokenizer, so no syntax
/// decision can be pinned to a single tokenizer's behavior.
#[derive(Default)]
pub struct Registry {
    tokenizers: Vec<Box<dyn Tokenizer>>,
}

impl Registry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The registry containing every deterministic, offline adapter that ships
    /// with this crate. This is the default set used by the tokenizer lab.
    #[must_use]
    pub fn with_builtin_adapters() -> Self {
        let mut registry = Self::new();
        registry.register(Box::new(adapters::ByteTokenizer::new()));
        registry.register(Box::new(adapters::WhitespaceTokenizer::new()));
        registry.register(Box::new(adapters::GptLikeTokenizer::new()));
        registry
    }

    /// Register a tokenizer adapter.
    pub fn register(&mut self, tokenizer: Box<dyn Tokenizer>) {
        self.tokenizers.push(tokenizer);
    }

    /// Iterate the registered tokenizers in registration order.
    pub fn iter(&self) -> impl Iterator<Item = &dyn Tokenizer> {
        self.tokenizers.iter().map(AsRef::as_ref)
    }

    /// The number of registered tokenizers.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tokenizers.len()
    }

    /// Whether the registry has no tokenizers.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tokenizers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_registry_has_stable_unique_ids() {
        let registry = Registry::with_builtin_adapters();
        assert!(registry.len() >= 3);
        let mut ids: Vec<&str> = registry.iter().map(Tokenizer::id).collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "tokenizer ids must be unique");
    }

    #[test]
    fn token_byte_len_counts_utf8_bytes() {
        // "é" is two UTF-8 bytes.
        assert_eq!(Token::new("é").byte_len(), 2);
        assert_eq!(Token::new("fn").byte_len(), 2);
    }
}
