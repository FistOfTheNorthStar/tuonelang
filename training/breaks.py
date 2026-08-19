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
        apply=lambda s: _bool_for_int(s),
    ),
    Break(
        name="chained-comparison",
        rationale="Writing `a < b < c`, which is a non-associative parse error.",
        apply=lambda s: _chained_comparison(s),
    ),
]


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


def _bool_for_int(s: str) -> str:
    """If a function returns Int, replace the first `-> Int {` body's last
    trailing scalar with `true`. Simplest robust form: append a wrapper is
    fragile, so we instead flip a returned integer literal to `true` only when
    the function's declared return is Int and a lone-literal body exists."""
    import re

    # Target a body of the exact shape `-> Int {\n    <intlit>\n}` (common in seeds
    # like `fn compute() -> Int { 42 }`). Replace that literal with `true`.
    m = re.search(r"-> Int \{\s*\n\s*(-?\d+)\s*\n\}", s)
    if not m:
        return s
    return s[: m.start(1)] + "true" + s[m.end(1) :]


def _chained_comparison(s: str) -> str:
    """Turn the first `a < b` comparison into `a < b < b` (a parse error)."""
    import re

    m = re.search(r"(\b\w+) < (\b\w+)\b", s)
    if not m:
        return s
    a, b = m.group(1), m.group(2)
    return s[: m.start()] + f"{a} < {b} < {b}" + s[m.end() :]
