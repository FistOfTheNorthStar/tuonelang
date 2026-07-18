//! Fuzz target: lexing arbitrary UTF-8 must never panic, and the lexer's
//! structural invariants must hold on every input.
//!
//! Run with `cargo +nightly fuzz run lex` from `crates/tuo-lexer`. The same
//! invariants also run on stable in `tests/robustness.rs` over a fixed-seed
//! input generator; this target adds coverage-guided search on top.

#![no_main]

use libfuzzer_sys::fuzz_target;
use tuo_lexer::{TokenKind, lex};
use tuo_source::SourceMap;

fuzz_target!(|text: &str| {
    let mut map = SourceMap::new();
    let file = map.intern_file("fuzz.tuo");
    // Inputs longer than SourceText's u32 limit are rejected by
    // construction, which is itself correct behavior — just skip them.
    let Ok(source) = map.add_source(file, text) else {
        return;
    };
    let result = lex(map.source(source));

    // The stream always ends with a zero-width EOF at end of input.
    let (eof, rest) = result.tokens.split_last().expect("never empty");
    assert_eq!(eof.kind, TokenKind::Eof);
    assert_eq!(eof.range().start().as_usize(), text.len());

    // Losslessness: non-EOF tokens tile the input exactly, on char
    // boundaries, with no gaps, overlaps, or zero-width tokens.
    let mut cursor = 0;
    for token in rest {
        assert_eq!(token.range().start().as_usize(), cursor);
        assert!(!token.range().is_empty());
        cursor = token.range().end().as_usize();
        let _ = &text[token.range().start().as_usize()..token.range().end().as_usize()];
    }
    assert_eq!(cursor, text.len());

    // Diagnostics and Error tokens accompany each other.
    let has_error_token = rest.iter().any(|t| t.kind == TokenKind::Error);
    assert_eq!(has_error_token, !result.diagnostics.is_empty());
});
