# Formatter corpus

Golden fixtures for `tuo-fmt`, tuonelang's canonical formatter.

- `fixtures/*.tuo` — inputs in deliberately messy (or malformed) form.
- `golden/*.tuo` — the blessed canonical output for each fixture.

The harness lives in `crates/tuo-fmt/tests/golden.rs` (goldens, idempotence)
and `crates/tuo-fmt/tests/properties.rs` (idempotence, comment preservation,
parse → format → parse structural equivalence, and never-panic sweeps over
both fixture corpora and generated soup).

To regenerate goldens after an intentional formatting change:

```
TUO_BLESS=1 cargo test -p tuo-fmt --test golden
```

Review the diff before committing — goldens define the canonical style.

The parser/lexer fixture corpora deliberately contain *non*-canonical
spacing (that is what they test) and are exempt from `tuo fmt --check`.
Every other `.tuo` corpus in this repository is expected to converge on the
canonical form as tooling lands.
