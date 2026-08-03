# Regression fixtures

Every file here is an exact input that once violated a stage invariant — a
discovered bug turned into a permanent, checked-in obligation.

## Layout

```
regressions/<stage>/<content-hash>.tuo
```

- `<stage>` names the checker in `tuo_fuzz::stages` the input is replayed
  through (`lexer`, `syntax`, `parser`, `fmt`, `front-end`, `ast-lowering`,
  `mir`, `interp`).
- `<content-hash>` is a content-addressed FNV-1a stem, so the same crash is
  filed exactly once.
- The file's bytes **are** the crashing input — no metadata wrapper — so a
  fixture is trivially replayable, diffable, and human-readable.

## How a fixture gets here

Automatically. When a `cargo fuzz` target or the stable sweep hits an input that
panics a checker, `tuo_fuzz::regression::record` writes it here before the crash
propagates (see `../fuzz/fuzz_targets/README.md`). A developer fixes the bug and
commits the new fixture in the same change.

## How they stay fixed

`tests/sweep.rs::committed_regressions_stay_fixed` calls
`tuo_fuzz::regression::replay_all`, which re-runs every fixture through its
stage's checker on every `cargo test`. A fixture that panics again fails the
build — so a fixed bug can never silently return.

## Current fixtures

- `front-end/` — deeply nested binary-operator chains
  (`x + x + … + 0`). A long left-associative infix chain parses iteratively
  (Pratt precedence-climbing) but builds a left-nested `BinaryExpr` tree as deep
  as the chain is long, which the recursive front-end tree-walk overflowed on.
  Fixed by extending the parser's depth pre-scan to bound binary-operator chain
  length (rejected as `P0003` before parsing), keeping every downstream stage's
  recursion bounded. Discovered by the fuzz sweep; the fix is pinned in
  `tuo-parser/tests/recovery.rs`.
```
