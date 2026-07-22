# mir tests

Golden lowering tests for MIR — tuonelang's single executable semantic
representation (`tuo-mir`), the IR shared by the interpreter and every
native backend.

## Golden corpus (`golden/`)

Each `*.tuo` fixture is an accepted (front-end-clean) program; its lowered
MIR is blessed into a sibling `*.mir` file. The suite in
`crates/tuo-mir/tests/golden.rs` re-lowers every fixture and asserts the
render matches its golden byte-for-byte, and that lowering is
deterministic (equal input → equal MIR). The fixtures cover, by area:

- `arithmetic.tuo` — arithmetic, comparisons, `&&`/`||` short-circuiting,
  unary `-`/`!`, and numeric casts.
- `control_flow.tuo` — `if`/`else`, `while`, `for` over ranges, `loop`
  with a `break` value, `continue`, and early `return`.
- `ownership.tuo` — moves, `in`/`mut`/`take` call arguments, drop
  elaboration at scope ends and assignments, partial moves, and deferred
  initialization.
- `adts.tuo` — struct/enum construction and field access, `match` with
  discriminant switches and guards, `Option`/`Result`, and `?`.
- `match_moves.tuo` — owned scrutinees: payload extraction plus the drop
  of the husk left behind.
- `calls.tuo` — every parameter mode, constants, generics, `impl`
  functions (recorded as `not lowered` pending the trait system).

Bless after an intended change with:

```bash
TUO_BLESS=1 cargo test -p tuo-mir --test golden
```

and review the diff before committing.

## Corpus coverage

A second test in the same file lowers the entire accepted ownership
corpus (`tests/ownership/fixtures/ok/`) and asserts that every function
either lowers or is skipped for one of the **documented** v0 limits
(listed in `tuo-mir`'s crate docs) — never for an undocumented reason, and
nothing panics. The whole accepted corpus lowers today; the skip list is
the escape hatch for constructs the v0 subset does not yet cover (method
calls, indirect calls, and so on).

## Why golden, not hand-written

MIR is verbose and every instruction's meaning is fixed by the crate
documentation, so the useful invariant is *stability*: a change to lowering
shows up as a reviewable diff against the blessed output. Semantic
execution tests belong to the MIR interpreter (`tuo-mir-interp`), which
lands next.
