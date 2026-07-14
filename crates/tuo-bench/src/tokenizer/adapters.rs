//! Deterministic, offline tokenizer adapters that ship with the harness.
//!
//! These three adapters model *distinct* splitting behaviors so that a syntax
//! choice can be evaluated against more than one tokenization strategy. None of
//! them embed an external vocabulary or touch the network, so measurements are
//! fully reproducible in CI.
//!
//! They are intentionally simple and **do not** claim to reproduce any specific
//! production tokenizer's exact token counts. Real BPE tokenizers (tiktoken,
//! Claude) are added later as additional [`Tokenizer`] implementations; see the
//! `tools/tokenizer-lab` README for how.

use super::{Token, Tokenizer};

/// Baseline: one token per UTF-8 byte.
///
/// This is the theoretical upper bound on token count and the lower bound on
/// bytes-per-token (always 1.0). It anchors the other adapters: any real
/// tokenizer should do no worse than this.
#[derive(Debug, Default, Clone)]
pub struct ByteTokenizer;

impl ByteTokenizer {
    /// Create a [`ByteTokenizer`].
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Tokenizer for ByteTokenizer {
    fn id(&self) -> &str {
        "bytes"
    }

    fn description(&self) -> &str {
        "One token per UTF-8 byte (baseline upper bound on token count)."
    }

    fn encode(&self, input: &str) -> Vec<Token> {
        // Emit each byte as its own token. We keep the token text as the raw
        // byte re-interpreted through a lossless Latin-1-style mapping so the
        // tokens still round-trip when concatenated as bytes; for measurement
        // we only rely on the count and byte length, both of which are exact.
        input
            .bytes()
            .map(|b| Token::new((b as char).to_string()))
            .collect()
    }
}

/// Splits input into maximal runs of one *class* of character.
///
/// The classes are: identifier characters (`XID`-ish: alphanumeric or `_`),
/// whitespace runs, and individual punctuation characters (each its own token).
/// This models a naive word-level tokenizer and is a useful contrast to the
/// byte and subword adapters.
#[derive(Debug, Default, Clone)]
pub struct WhitespaceTokenizer;

impl WhitespaceTokenizer {
    /// Create a [`WhitespaceTokenizer`].
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

/// The character class used by [`WhitespaceTokenizer`] to decide run boundaries.
#[derive(PartialEq, Eq, Clone, Copy)]
enum CharClass {
    Ident,
    Space,
    /// Punctuation is never merged: each punctuation char is its own token.
    Punct,
}

fn classify(c: char) -> CharClass {
    if c.is_alphanumeric() || c == '_' {
        CharClass::Ident
    } else if c.is_whitespace() {
        CharClass::Space
    } else {
        CharClass::Punct
    }
}

impl Tokenizer for WhitespaceTokenizer {
    fn id(&self) -> &str {
        "whitespace"
    }

    fn description(&self) -> &str {
        "Maximal runs of identifier chars or whitespace; each punctuation char is its own token."
    }

    fn encode(&self, input: &str) -> Vec<Token> {
        let mut tokens = Vec::new();
        let mut current = String::new();
        let mut current_class: Option<CharClass> = None;

        for c in input.chars() {
            let class = classify(c);
            // Punctuation never merges; identifier/space runs merge with their
            // own class only.
            let merge = matches!(class, CharClass::Ident | CharClass::Space)
                && current_class == Some(class);
            if merge {
                current.push(c);
            } else {
                if !current.is_empty() {
                    tokens.push(Token::new(std::mem::take(&mut current)));
                }
                current.push(c);
                current_class = Some(class);
            }
        }
        if !current.is_empty() {
            tokens.push(Token::new(current));
        }
        tokens
    }
}

/// A heuristic that approximates subword (BPE-style) tokenization of code,
/// without any external vocabulary.
///
/// It captures the two behaviors that most affect how code tokenizes under
/// production BPE tokenizers:
///
/// 1. **Leading-space attachment.** A space before a word is attached to that
///    word (`" fn"` is one unit), mirroring GPT-2/tiktoken-style byte-level BPE
///    where ` word` is a common merged token.
/// 2. **A small "vocabulary" of common code fragments** that encode as a single
///    token; anything not in it falls back to shorter fragments and finally to
///    per-character tokens. Short, common keywords therefore cost one token
///    while rarer identifiers cost several — the effect syntax design cares
///    about.
///
/// This is a *model*, tunable via its fragment list, not a claim of fidelity to
/// any real tokenizer. Its value is as a third, subword-flavored data point.
#[derive(Debug, Clone)]
pub struct GptLikeTokenizer {
    /// Common code fragments that encode as a single token, longest first.
    vocab: Vec<&'static str>,
}

impl Default for GptLikeTokenizer {
    fn default() -> Self {
        Self::new()
    }
}

impl GptLikeTokenizer {
    /// Create a [`GptLikeTokenizer`] with the default code-fragment vocabulary.
    #[must_use]
    pub fn new() -> Self {
        // A deliberately small, general set of fragments common in curly-brace
        // languages. Kept sorted longest-first at construction so matching is
        // greedy-longest. This vocabulary is intentionally NOT biased toward any
        // particular tuonelang keyword spelling, so it does not prejudge the
        // syntax comparisons.
        let mut vocab = vec![
            // multi-character operators / punctuation
            "->", "=>", "::", "==", "!=", "<=", ">=", "&&", "||",
            // very common short code words across languages
            "fn", "func", "let", "var", "int", "str", "for", "in", "if", "else", "return", "struct",
            "enum", "match", "import", "pub", "true", "false", "self",
            // common word-pieces so novel identifiers split plausibly
            "func", "tion", "ction", "String", "Int", "Str", "value", "self", "spec", "test",
            "given", "when", "then", "name", "type", "add",
        ];
        // Deduplicate while preserving determinism, then sort longest-first so
        // greedy matching prefers longer fragments.
        vocab.sort_unstable();
        vocab.dedup();
        vocab.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        Self { vocab }
    }

    /// Try to match one vocabulary fragment at the start of `rest`.
    fn match_fragment(&self, rest: &str) -> Option<&'static str> {
        self.vocab
            .iter()
            .copied()
            .find(|frag| rest.starts_with(frag))
    }
}

impl Tokenizer for GptLikeTokenizer {
    fn id(&self) -> &str {
        "gpt-like"
    }

    fn description(&self) -> &str {
        "Heuristic subword model: leading-space attachment plus a small greedy code-fragment vocabulary."
    }

    fn encode(&self, input: &str) -> Vec<Token> {
        let mut tokens = Vec::new();
        let mut rest = input;

        while !rest.is_empty() {
            // 1. Leading-space attachment: a single leading space becomes part
            //    of the next token's prefix. Runs of >1 space each tokenize
            //    individually (as in byte-level BPE for long whitespace).
            let mut prefix_len = 0;
            if rest.starts_with(' ') && !rest.starts_with("  ") {
                prefix_len = 1;
            }
            let after_prefix = &rest[prefix_len..];

            if after_prefix.is_empty() {
                // Trailing lone space.
                tokens.push(Token::new(&rest[..prefix_len]));
                break;
            }

            // 2. Try to match a vocabulary fragment right after the prefix.
            if let Some(frag) = self.match_fragment(after_prefix) {
                let end = prefix_len + frag.len();
                tokens.push(Token::new(&rest[..end]));
                rest = &rest[end..];
                continue;
            }

            // 3. Fallback: consume the prefix plus one character. Using one
            //    char (not one byte) keeps tokens on UTF-8 boundaries and the
            //    output lossless.
            let next_char = after_prefix.chars().next().expect("non-empty");
            let end = prefix_len + next_char.len_utf8();
            tokens.push(Token::new(&rest[..end]));
            rest = &rest[end..];
        }

        tokens
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every shipped adapter must be lossless: concatenated token text equals
    /// the input. This is the property the measurement engine relies on.
    fn assert_lossless(tok: &dyn Tokenizer, input: &str) {
        let joined: String = tok.encode(input).iter().map(|t| t.text.as_str()).collect();
        assert_eq!(joined, input, "adapter `{}` was not lossless", tok.id());
    }

    const SAMPLES: &[&str] = &[
        "",
        "fn add(a: Int, b: Int) -> Int { return a + b; }",
        "let name: Str = \"tuonelang\";",
        "spec \"add is commutative\" { given a: Int; }",
        "résumé_café := 1;", // multi-byte UTF-8
        "x    y",            // long whitespace run
    ];

    #[test]
    fn byte_tokenizer_is_lossless_by_bytes_and_counts_bytes() {
        let tok = ByteTokenizer::new();
        for s in SAMPLES {
            // For the byte tokenizer, count equals the UTF-8 byte length.
            assert_eq!(tok.token_count(s), s.len());
        }
    }

    #[test]
    fn whitespace_tokenizer_is_lossless() {
        let tok = WhitespaceTokenizer::new();
        for s in SAMPLES {
            assert_lossless(&tok, s);
        }
    }

    #[test]
    fn gpt_like_tokenizer_is_lossless() {
        let tok = GptLikeTokenizer::new();
        for s in SAMPLES {
            assert_lossless(&tok, s);
        }
    }

    #[test]
    fn adapters_are_deterministic() {
        let toks: Vec<Box<dyn Tokenizer>> = vec![
            Box::new(ByteTokenizer::new()),
            Box::new(WhitespaceTokenizer::new()),
            Box::new(GptLikeTokenizer::new()),
        ];
        for tok in &toks {
            for s in SAMPLES {
                assert_eq!(tok.encode(s), tok.encode(s));
            }
        }
    }

    #[test]
    fn gpt_like_merges_common_keyword_into_one_token() {
        let tok = GptLikeTokenizer::new();
        // "fn" is in the vocabulary, so a leading-space "fn" is a single token.
        let tokens = tok.encode(" fn");
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].text, " fn");
    }
}
