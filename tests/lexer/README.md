# lexer tests

Tokenization tests: token kinds, spans, trivia, and lexical error recovery.

## Layout

- `fixtures/*.tuo` — input programs, valid and deliberately malformed.
- `snapshots/*.snap` — the blessed token stream (kind, byte range, lexeme)
  and rendered diagnostics for each fixture.

The harness lives in `crates/tuo-lexer/tests/snapshots.rs`; it discovers every
fixture, so an unpaired fixture fails the `corpus_is_fully_covered` test. To
regenerate snapshots after an intentional change:

```sh
TUO_BLESS=1 cargo test -p tuo-lexer --test snapshots
```

and review the diff like any other code change.

Unit-level lexer tests (keywords, Unicode, malformed literals, robustness
sweeps) live in `crates/tuo-lexer/tests/`; the coverage-guided fuzz target is
`crates/tuo-lexer/fuzz/` (`cargo +nightly fuzz run lex`).
