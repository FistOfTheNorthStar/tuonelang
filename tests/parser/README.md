# parser tests

Parsing tests: syntax-tree shape and parse-error diagnostics.

## Layout

- `fixtures/ok/*.tuo` — programs that must parse with **zero** diagnostics.
- `fixtures/err/*.tuo` — deliberately broken programs that must produce
  diagnostics while retaining the valid constructs around the damage.
- `snapshots/*.snap` — the blessed syntax tree (with token lexemes) and
  rendered diagnostics for each fixture.

The harness lives in `crates/tuo-parser/tests/snapshots.rs`; it discovers
every fixture, so an unpaired fixture fails the `corpus_is_fully_covered`
test. Every fixture — valid or broken — must also satisfy the losslessness
invariants: full token coverage and byte-identical text reconstruction. To
regenerate snapshots after an intentional change:

```sh
TUO_BLESS=1 cargo test -p tuo-parser --test snapshots
```

and review the diff like any other code change.

Structural precedence tests, recovery-metric tests (diagnostic code,
location, continuation, constructs retained), and robustness sweeps live in
`crates/tuo-parser/tests/`.
