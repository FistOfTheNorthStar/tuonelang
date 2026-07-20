# types tests

Type-system fixtures: checking, inference, and type-error diagnostics.

- `fixtures/ok/` — programs that must resolve **and** type-check with zero
  diagnostics.
- `fixtures/err/` — programs that resolve cleanly but contain type errors;
  their diagnostics (code, span, message, structured expected/actual
  values) are snapshotted in `snapshots/<stem>.snap`.
- `snapshots/` — blessed diagnostic snapshots for the `err/` fixtures.

The harness lives in `crates/tuo-types/tests/fixtures.rs`. Bless new or
changed snapshots with:

```bash
TUO_BLESS=1 cargo test -p tuo-types --test fixtures
```

Every fixture must parse cleanly — parser-error corpora live in
`tests/parser/`, not here.
