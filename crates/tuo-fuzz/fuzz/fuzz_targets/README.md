# Coverage-guided fuzz targets

One target per pipeline stage. Each is a thin call into a shared invariant
checker in `tuo_fuzz::stages` — the **same** checker the stable
`tests/sweep.rs` sweep drives — so the coverage-guided fuzzer and the
deterministic CI sweep can never disagree about what "correct" means.

| target         | stage(s) covered                          | checker                       |
|----------------|-------------------------------------------|-------------------------------|
| `lex`          | lexer input                               | `stages::check_lexer`         |
| `syntax`       | syntax-tree operations                    | `stages::check_syntax`        |
| `parse`        | parser input                              | `stages::check_parser`        |
| `fmt`          | formatter (idempotence, meaning-preserve) | `stages::check_fmt`           |
| `ast_lowering` | AST → HIR lowering                        | `stages::check_ast_lowering`  |
| `front_end`    | resolve + type check + ownership check    | `stages::check_front_end`     |
| `mir`          | HIR → MIR lowering + MIR verifier         | `stages::check_mir`           |
| `interp`       | MIR interpreter (no structural panics)    | `stages::check_interp`        |

## Running

cargo-fuzz needs a nightly toolchain and is not a workspace member:

```bash
cargo install cargo-fuzz
cd crates/tuo-fuzz
cargo +nightly fuzz run parse        # or lex, syntax, fmt, ast_lowering, front_end, mir, interp
```

## What happens on a crash

Each target wraps its checker in `tuo_fuzz::guarded`, which — before letting the
panic propagate to libFuzzer — records the exact crashing input as a committed
regression fixture under `../regressions/<stage>/<hash>.tuo`. Commit that file:
`tests/sweep.rs::committed_regressions_stay_fixed` then replays it on every
`cargo test`, so a fixed bug can never silently return. libFuzzer's own
`artifacts/` reproduction is written too, for minimization.

## Relationship to the stable sweep

You do **not** need a nightly toolchain or cargo-fuzz to get value here: the
invariants run in ordinary CI via `cargo test -p tuo-fuzz` (the fixed-seed
corpus sweep). These targets add coverage-guided search on top of that baseline.
```
