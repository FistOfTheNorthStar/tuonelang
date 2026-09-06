# py2tuo

A compiler from a **typed subset of Python** to **tuonelang** source.

This is a developer tool, not part of the shipped tuonelang compiler. It lives
under `tools/` alongside [`tokenizer-lab`](../tokenizer-lab/), and nothing in
the `tuo` binary depends on it.

## Is a Python → tuonelang compiler possible?

**For a subset, yes. In general, no** — and the boundary is the whole design.

| Python | tuonelang |
|---|---|
| dynamically typed | statically typed, signatures never inferred |
| garbage collected | ownership-checked, explicit parameter modes |
| arbitrary-precision `int` | `Int` is `I64` and **traps** on overflow |
| exceptions | no exceptions; `Result[T, E]` and `match` |
| classes, closures, generators | free functions only; function values capture nothing |
| `/` yields a float | no implicit numeric conversion |

A total translation would have to invent semantics tuonelang does not have.
So this tool translates the **overlap exactly** and **refuses everything else**
with a positioned diagnostic naming the construct — the same discipline the
tuonelang CLI holds itself to: *never advertise behavior the compiler cannot
perform, and refuse rather than mis-compile.*

## The claim is verified, not asserted

The generated code is compiled by the **real `tuo` binary**, and the test suite
runs the translated program *and the original Python* and compares the answers.
A translation is only called correct when both languages agree at runtime.

```bash
python3 -m py2tuo verify examples/fizzbuzz.py --run
# tuo check: ok (exit 0)
# tuo spec:  ok (exit 0)
# tuo run:   ok (exit 6)      <- 6 fizzbuzz numbers below 100
```

That round trip found real bugs during development that inspection did not:
early `return` silently becoming a discarded block value, `for` moving out of a
borrowed parameter (`O0003`), `mut` spelled at call sites where the compiler
infers it, and `reversed()` being translated as a list when Python returns a
lazy iterator.

## Usage

```bash
export PYTHONPATH=tools/py2tuo/src        # or `pip install -e tools/py2tuo`

# Translate to stdout (canonicalised by `tuo fmt` when the binary is found).
python3 -m py2tuo build examples/stats.py

# Translate to a file, and fail if the real compiler rejects the result.
python3 -m py2tuo build examples/stats.py -o stats.tuo --check

# Translate, then prove it: tuo check + tuo spec (+ tuo run).
python3 -m py2tuo verify examples/stats.py --run

# Machine-readable output, for driving from another tool.
python3 -m py2tuo --message-format=json build examples/stats.py

# Print the translatable subset.
python3 -m py2tuo supported
```

The `tuo` binary is located via `--tuo`, then `$TUO_BIN`, then `PATH`, then the
repository's own `target/release` and `target/debug`. Without it, `build` still
works (emitting unformatted source and saying so); `verify` requires it.

## What translates

* module-level `def`s, **fully annotated** — every parameter and the return type
* `int`, `bool`, `float`, `str`, `None`, `list[T]`, `Optional[T]`
* arithmetic `+ - * // % **`, bitwise `& | ^ ~ << >>`, `and` / `or` / `not`
* comparisons **including Python's chained form** — `a < b < c` becomes
  `(a < b) && (b < c)`, since the direct spelling is a tuonelang parse error
* `if` / `elif` / `else`, `while`, `for` over `range(..)` or a list
* assignment, augmented assignment (`x += 1` → `x = x + 1`, no `+=` exists),
  and `xs[i] = v` → `std::array::set(xs, i, v)`
* conditional expressions, recursion, calls, passing a function by name
* builtins `len`, `abs`, `min`, `max`, `sum`, `print`, `str`, `int`, `float`,
  `sorted`, `list(reversed(xs))`
* string concatenation `a + b` → `std::string::concat`, with an owned `String`
  reaching the borrowed view through `std::string::as_str`
* **`dict[str, int]` and `dict[int, int]`** — the two shapes tuonelang v0 has
  (ADR-0011): `d[k] = v`, `d.get(k, default)`, `k in d`, `len(d)`, `d.keys()`,
  and `d: dict[str, int] = {}`
* docstrings, carried across as `///` doc comments

Three translations are worth calling out because the naive version compiles and
is *wrong*:

* **`for x in xs` becomes an indexed `while`.** tuonelang's `for` consumes the
  array it iterates, so over a borrowed parameter it is rejected (`O0003`) —
  and Python's `for` does not consume the list. The indexed loop reads through
  the borrow, which is what the Python means.
* **`d.get(k, default)` translates but `d[k]` does not.** tuonelang's
  `std::map::get` returns `Option[V]`; Python's `d[k]` raises `KeyError`, and
  tuonelang has no exceptions. The default is exactly what collapses the
  `Option`, so the two-argument `get` becomes a `match` and the raising form is
  refused with that explanation.
* **`let` vs `var` is decided by a pre-scan.** A local is `var` when the body
  reassigns it *or* passes it to a function that takes it `mut`, because a
  `mut` borrow of a `let` binding is refused (`O0004`).

Generated output is **self-contained**: array and string operations are compiler
builtins, and Python builtins with no builtin equivalent become small private
`py_*` helpers, so no stdlib `.tuo` file has to be supplied alongside it.

## What is refused

Each of these produces a diagnostic naming the construct and pointing at the
line — never a silent mistranslation:

classes · decorators · nested functions · lambdas · comprehensions ·
`try`/`except`/`raise` · `async`/`await` · `set`/`tuple` · slicing ·
f-strings · `*args`/`**kwargs` · default arguments · `is` · `in` · truthiness ·
method-call syntax · library imports · unannotated parameters · mixed
`Int`/`Float` arithmetic · `/` true division · integer literals wider than 64
bits · redefining a function (tuonelang has no overloading) · Python names that
are tuonelang keywords · negative indices (`xs[-1]` is the *last* element in
Python and a **trap** in tuonelang) · dict shapes other than the two above,
`d[k]`, `d.get(k)` without a default, and `.values()`/`.items()`/`.pop()`

### Why only two dict shapes

tuonelang v0 defines its map operations over exactly `Map[Str, Int]` and
`Map[Int, Int]`. A survey of 1,870 CPython stdlib files found 58 dict-typed
annotations, of which **7% are one of those two shapes** — the rest are
`dict[str, Any]`, `dict[str, str]`, bare `dict`, unions, and nested maps.

So the coverage is narrow, but the shapes that *are* covered are the ones that
carry the weight in translatable code: counting and indexing by `str` or `int`.
`counts[k] = counts.get(k, 0) + 1` translates exactly and runs natively.
Everything else is refused by *shape*, naming the Python type the user wrote.

One refusal is load-bearing beyond the subset: `dict[int, bool]` passes
`tuo check` but fails during codegen with a raw verifier error, so refusing it
up front turns a confusing backend crash into a clear message.

```
$ python3 -m py2tuo build bad.py
bad.py:4:5: error[PY0001] unsupported Python construct: comprehension
    note: translate into an explicit loop, or use std::collections::map_into
          with a named top-level function
```

Diagnostic codes are namespaced by stage, mirroring tuonelang's own convention:
`PY` (not valid Python, or Python the subset excludes), `TY` (annotations
missing, unsupported, or inconsistent), `SEM` (valid annotated Python whose
*semantics* have no faithful translation).

## Tests

```bash
cd tools/py2tuo
python3 -m venv .venv && .venv/bin/pip install pytest
.venv/bin/python -m pytest tests/ -q
```

The suite covers three things: every sample program **translates**; the real
`tuo` **accepts** the result; and the translated program **agrees with CPython**
at runtime. Tests needing the compiler skip cleanly when `tuo` is absent — a
skip is visible, never a pass.

Note the runtime comparison observes a process exit status, which is one byte:
`main` returning 316 exits 60 in *both* languages. The comparison is taken
mod 256 on both sides and the samples return small values, so the truncation
cannot hide a disagreement.

## Files

```
src/py2tuo/
  __init__.py     the `translate` entry point
  compiler.py     the Python AST -> tuonelang translation
  types.py        the Python-annotation -> tuonelang-type mapping
  emit.py         indentation-aware source writing
  diagnostics.py  structured, positioned diagnostics
  verify.py       drives the real `tuo` binary
  cli.py          the command-line front end
examples/         Python inputs that translate, compile, and run
tests/            translation, acceptance, and Python-agreement tests
```
