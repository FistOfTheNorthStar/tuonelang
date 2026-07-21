# ownership tests

Executable semantic fixtures for the v0 ownership model. The normative rules
live in `specification/ownership.md` (adopted by ADR-0003); this corpus is
their executable counterpart — specification and fixtures are maintained
together, and a behavioral change to one is a change to the other.

The ownership checker (`tuo-ownership`) consumes this corpus as its
**acceptance suite**: `crates/tuo-ownership/tests/fixtures.rs` runs the full
pipeline over every fixture and enforces the contract below, and
`crates/tuo-ownership/tests/property.rs` adds an exhaustive property sweep
over small control-flow combinations (checker-accepted programs must be
dynamically safe under path enumeration).

## Layout

| Directory | Contract |
|-----------|----------|
| `fixtures/ok/` | Every case **must compile** under the ownership checker. |
| `fixtures/err/` | Every case **must fail** with the annotated diagnostic. |

Both corpora must pass the front end (`parse → resolve → type-check`) with
zero diagnostics today — `err/` programs are ownership errors *only*.

## Conventions

- One **case** = one top-level function named `case_<scenario>`. Each corpus
  holds at least 100 cases (ADR-0003). Non-`case_` items are shared helpers.
- In `err/`, the offending line carries a trailing annotation:

  ```
  consume(b);
  peek(b); // ERROR: O0001 `b` was moved into the `take` argument
  ```

  Each case carries **exactly one** annotation; the code must be one of
  `O0001`–`O0009` as defined in `specification/ownership.md` §15, and every
  documented code is exercised by at least one case.
- Files group cases by the specification section they exercise; each file's
  header comment names it.

## Enforcement

The harness enforces both halves: `ok/` fixtures must produce **zero**
ownership diagnostics, and each `err/` fixture must produce **exactly** the
annotated code on each annotated line — no more, no fewer. The annotation
format above makes mapping (file, line, code) onto emitted diagnostics
mechanical. A behavioral change to the checker that shifts a diagnostic is
therefore a change to this corpus, and via ADR-0003 a change to
`specification/ownership.md` as well.
