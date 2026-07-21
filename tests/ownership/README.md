# ownership tests

Executable semantic fixtures for the v0 ownership model. The normative rules
live in `specification/ownership.md` (adopted by ADR-0003); this corpus is
their executable counterpart — specification and fixtures are maintained
together, and a behavioral change to one is a change to the other.

The ownership checker (`tuo-ownership`) is **not implemented yet**. The
corpus exists first, so that implementation begins only once specification
and fixtures agree; `crates/tuo-ownership/tests/fixtures.rs` guards the
conventions below until the checker consumes the corpus directly.

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

## When the checker lands

The harness gains the enforcement half: `ok/` fixtures must produce zero
ownership diagnostics, and each `err/` case must produce exactly the
annotated code at the annotated line. The annotation format above is chosen
so that mapping (file, line, code) onto emitted diagnostics is mechanical.
