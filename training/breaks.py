"""Break rules for generating repair-transcript training data.

A *break* is a deterministic edit that turns a correct tuonelang program into a
plausibly-wrong one — the kind of mistake a model trained on other languages
actually makes (``use`` instead of ``import``, ``Some(x)`` instead of
``Some { value: x }``, ``<>`` generics, ``+=``, a bare type mismatch). The
generator applies a break, then runs the *real* compiler to capture the *real*
diagnostic, and emits a transcript:

    user: <task>
    assistant: <broken program>
    tool(compiler): <the compiler's actual diagnostic>
    assistant: <the canonical, correct program>

Because the diagnostic is captured from ``tuo`` — never written by hand — every
repair transcript teaches the model to respond to feedback the compiler truly
produces. A break that does not actually change the source (pattern absent) is
skipped for that seed; a break whose result unexpectedly still compiles is also
skipped (the generator checks), so no transcript ever claims a clean program was
an error.
"""

from collections.abc import Callable
from dataclasses import dataclass


@dataclass(frozen=True)
class Break:
    """A named source transformation that should introduce a compile error."""

    name: str
    """Kebab-case id (e.g. ``use-for-import``)."""

    rationale: str
    """One line on the cross-language mistake this models."""

    apply: Callable[[str], str]
    """Return the broken source, or the input unchanged if not applicable."""

    kind: str = "compile"
    """``compile`` — the result must fail ``tuo check`` (a parse, type, or
    ownership error), and the repair turn shows that diagnostic. ``spec`` — the
    result must still *check* and must *fail* a spec, and the repair turn shows
    the real ``tuo spec`` failure: the logic is wrong, not the spelling."""


def _replace_once(text: str, old: str, new: str) -> str:
    """Replace only the first occurrence; leave text unchanged if ``old`` absent."""
    idx = text.find(old)
    if idx == -1:
        return text
    return text[:idx] + new + text[idx + len(old) :]


BREAKS: list[Break] = [
    Break(
        name="use-for-import",
        rationale="Rust/Python habit: `use`/`from` instead of tuonelang's `import`.",
        apply=lambda s: _replace_once(s, "import ", "use "),
    ),
    Break(
        name="angle-bracket-generics",
        rationale="Writing generics as `Option<Int>` instead of `Option[Int]`.",
        apply=lambda s: _replace_once(
            _replace_once(s, "Option[Int]", "Option<Int>"),
            "Result[Int, Str]",
            "Result<Int, Str>",
        ),
    ),
    Break(
        name="positional-some",
        rationale="Writing `Some(x)` instead of the named-field `Some { value: x }`.",
        apply=lambda s: _positional_some(s),
    ),
    Break(
        name="let-mut-for-var",
        rationale="Writing `let mut x` (Rust) instead of tuonelang's `var x`.",
        apply=lambda s: _replace_once(s, "var ", "let mut "),
    ),
    Break(
        name="compound-assign",
        rationale="Using `+=`, which does not exist in tuonelang.",
        # turn the first `x = x + y` style reassignment into `x += y`
        apply=lambda s: _compound_assign(s),
    ),
    Break(
        name="missing-param-mode",
        rationale="Omitting the mandatory parameter mode (`take`/`in`/`mut`).",
        apply=lambda s: _replace_once(s, "take ", ""),
    ),
    Break(
        name="bool-for-int-return",
        rationale="Returning a Bool where an Int is declared (type mismatch).",
        # A predicate declared `-> Int` still returns its Bool: the brief's
        # `fn f() -> Int { true }`. (The old form only matched a body that was
        # a lone integer literal, which no seed has, so it never fired.)
        apply=lambda s: _replace_once(s, ") -> Bool {", ") -> Int {"),
    ),
    Break(
        name="chained-comparison",
        rationale="Writing `a < b < c`, which is a non-associative parse error.",
        apply=lambda s: _chained_comparison(s),
    ),
    # --- type errors -----------------------------------------------------
    Break(
        name="int-index",
        rationale="Indexing a fixed array with an `Int` instead of `as Usize`.",
        apply=lambda s: _replace_once(s, " as Usize]", "]"),
    ),
    Break(
        name="method-call",
        rationale="`xs.len()` method syntax; tuonelang has free functions only (T0024).",
        apply=lambda s: _method_call(s),
    ),
    Break(
        name="wrong-module-len",
        rationale="`std::str::len` on an owned `String` (the four-way `len`).",
        apply=lambda s: _replace_once(s, "std::string::len(", "std::str::len("),
    ),
    Break(
        name="positional-ok",
        rationale="Writing `Ok(x)` instead of the named-field `Ok { value: x }`.",
        apply=lambda s: _positional(s, "Ok", "value"),
    ),
    # --- ownership errors ------------------------------------------------
    Break(
        name="in-for-mut",
        rationale="Declaring `in` for a parameter the body mutates (needs `mut`).",
        apply=lambda s: _in_for_mut(s),
    ),
    Break(
        name="let-for-var-heap",
        rationale="Binding a growable value with `let`, then mutating it in place.",
        apply=lambda s: _let_for_var_heap(s),
    ),
    # --- spec failures (the program checks; its logic is wrong) ----------
    Break(
        name="off-by-one-bound",
        rationale="An inclusive loop bound written exclusive (`<=` as `<`).",
        apply=lambda s: _in_code(s, lambda c: _replace_once(c, " <= ", " < ")),
        kind="spec",
    ),
    Break(
        name="exclusive-bound-made-inclusive",
        rationale="An exclusive loop bound written inclusive (`<` as `<=`).",
        apply=lambda s: _in_code(s, lambda c: _while_lt_to_le(c)),
        kind="spec",
    ),
    Break(
        name="swapped-operator",
        rationale="A subtraction written as an addition.",
        apply=lambda s: _in_code(s, lambda c: _replace_once(c, " - ", " + ")),
        kind="spec",
    ),
]


def _in_code(s: str, edit: Callable[[str], str]) -> str:
    """Apply ``edit`` to the code before the first ``spec`` block only, so a
    spec-failure break changes the implementation, never the test."""
    import re

    m = re.search(r"^spec ", s, flags=re.MULTILINE)
    cut = m.start() if m else len(s)
    return edit(s[:cut]) + s[cut:]


def _method_call(s: str) -> str:
    """Rewrite the first ``std::<module>::len(x)`` into ``x.len()``."""
    import re

    m = re.search(r"std::(?:str|string|array|map)::len\((\w+)\)", s)
    if not m:
        return s
    return s[: m.start()] + f"{m.group(1)}.len()" + s[m.end() :]


def _positional(s: str, variant: str, field: str) -> str:
    """Rewrite the first ``Variant { field: EXPR }`` into ``Variant(EXPR)``."""
    import re

    m = re.search(variant + r" \{ " + field + r": ([^}]+?) \}", s)
    if not m:
        return s
    return s[: m.start()] + f"{variant}({m.group(1)})" + s[m.end() :]


def _in_for_mut(s: str) -> str:
    """Turn the first ``mut name: <heap type>`` parameter into ``in``."""
    import re

    m = re.search(r"\bmut (\w+): (Array\[|String\b|Map\[)", s)
    if not m:
        return s
    return s[: m.start()] + "in" + s[m.start() + len("mut") :]


def _let_for_var_heap(s: str) -> str:
    """Turn the first ``var x = std::<heap>::empty()`` into ``let``."""
    import re

    m = re.search(r"\bvar (\w+) = std::(?:array|string|map)::empty\(\)", s)
    if not m:
        return s
    return s[: m.start()] + "let" + s[m.start() + len("var") :]


def _while_lt_to_le(s: str) -> str:
    """Turn the first ``while i < n`` loop condition into ``while i <= n``."""
    import re

    m = re.search(r"\bwhile (\w+) < ", s)
    if not m:
        return s
    return s[: m.start()] + f"while {m.group(1)} <= " + s[m.end() :]


def _positional_some(s: str) -> str:
    """Rewrite the first ``Some { value: EXPR }`` into ``Some(EXPR)``."""
    import re

    m = re.search(r"Some \{ value: ([^}]+?) \}", s)
    if not m:
        return s
    return s[: m.start()] + f"Some({m.group(1)})" + s[m.end() :]


def _compound_assign(s: str) -> str:
    """Rewrite the first ``name = name + expr;`` into ``name += expr;``."""
    import re

    m = re.search(r"(\b\w+) = \1 \+ ([^;]+);", s)
    if not m:
        return s
    name, rhs = m.group(1), m.group(2)
    return s[: m.start()] + f"{name} += {rhs};" + s[m.end() :]


def _chained_comparison(s: str) -> str:
    """Turn the first `a < b` comparison into `a < b < b` (a parse error)."""
    import re

    m = re.search(r"(\b\w+) < (\b\w+)\b", s)
    if not m:
        return s
    a, b = m.group(1), m.group(2)
    return s[: m.start()] + f"{a} < {b} < {b}" + s[m.end() :]
