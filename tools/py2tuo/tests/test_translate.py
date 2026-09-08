"""Tests for the Python -> tuonelang compiler.

The load-bearing tests are the round-trip ones: they translate Python, compile
the result with the **real** `tuo` binary, run it, and compare its exit status
against running the same function in Python. A translation is only claimed
correct when both languages agree on the answer.

Tests needing the compiler skip cleanly when `tuo` is absent, so the suite is
runnable without a built toolchain -- but a skip is visible, never a pass.
"""

from __future__ import annotations

import subprocess
import sys
import tempfile
from pathlib import Path

try:
    import pytest
except ImportError:  # pragma: no cover - exercised by run_tests.py
    # The suite is runnable without pytest via `run_tests.py`, which calls the
    # test functions directly. This shim supplies just the decorators they use,
    # so there is one suite rather than two that can drift apart.
    class _PytestShim:
        """Minimal stand-in for the pytest API this module uses."""

        class mark:  # noqa: N801 - mirroring pytest's own lowercase name
            @staticmethod
            def parametrize(_names, _values):
                return lambda function: function

            @staticmethod
            def skipif(_condition, *, reason=""):
                return lambda function: function

        @staticmethod
        def raises(expected):
            import contextlib

            @contextlib.contextmanager
            def _raises():
                caught = type("Caught", (), {"value": None})()
                try:
                    yield caught
                except expected as exc:
                    caught.value = exc
                    return
                raise AssertionError(f"expected {expected.__name__} to be raised")

            return _raises()

    pytest = _PytestShim()  # type: ignore[assignment]

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from py2tuo import CompileError, translate  # noqa: E402
from py2tuo.verify import find_tuo, run_tuo  # noqa: E402

TUO = find_tuo()
needs_tuo = pytest.mark.skipif(TUO is None, reason="the `tuo` binary was not found")


def compile_to_tuo(source: str, module: str = "t") -> str:
    """Translate Python source, failing the test on refusal."""
    return translate(source, module=module)


def run_generated(source: str, module: str = "t") -> int:
    """Translate, compile, and run; return the program's exit status."""
    assert TUO is not None
    generated = compile_to_tuo(source, module)
    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory) / f"{module}.tuo"
        path.write_text(generated, encoding="utf-8")
        checked = run_tuo(TUO, ["check", str(path)])
        assert checked.ok, f"`tuo check` rejected the generated source:\n{checked.output}\n\n{generated}"
        return run_tuo(TUO, ["run", str(path)]).status


def run_python(source: str) -> int:
    """Run the same Python source's `main()` in a subprocess."""
    program = source + "\n\nimport sys\nsys.exit(main())\n"
    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory) / "prog.py"
        path.write_text(program, encoding="utf-8")
        return subprocess.run([sys.executable, str(path)], capture_output=True).returncode


def assert_agrees(source: str) -> int:
    """Assert Python and the translated tuonelang compute the same value.

    Agreement is observed through the process exit status, which is a single
    byte on every platform this runs on: a `main` returning 316 exits 60 in
    BOTH languages. The comparison is therefore taken mod 256 on both sides,
    and the sample programs are written to return small values so the
    truncation never hides a disagreement.
    """
    expected = run_python(source)
    actual = run_generated(source)
    assert actual == expected % 256, (
        f"tuonelang exited {actual}, Python exited {expected} "
        f"({expected % 256} after the one-byte exit status)"
    )
    return actual


# -- the subset translates ------------------------------------------------

ARITHMETIC = """
def main() -> int:
    a = 7
    b = 3
    return a * b + a // b - a % b
"""

CONTROL_FLOW = """
def classify(n: int) -> int:
    if n < 0:
        return 0 - 1
    elif n == 0:
        return 0
    else:
        return 1

def main() -> int:
    return classify(5) + classify(0) + 40
"""

LOOPS = """
def fib(n: int) -> int:
    a = 0
    b = 1
    for _i in range(n):
        t = a + b
        a = b
        b = t
    return a

def main() -> int:
    return fib(10)
"""

ARRAYS = """
def total(xs: list[int]) -> int:
    acc = 0
    i = 0
    while i < len(xs):
        acc += xs[i]
        i += 1
    return acc

def main() -> int:
    xs = [10, 20, 12]
    return total(xs)
"""

MUTATION = """
def bump(xs: list[int]) -> int:
    xs[0] = 5
    return xs[0] + len(xs)

def main() -> int:
    xs = [1, 2]
    return bump(xs)
"""

CHAINED = """
def gate(a: int, b: int, c: int) -> bool:
    return a < b < c

def main() -> int:
    if gate(1, 2, 3) and not gate(3, 2, 1):
        return 7
    return 0
"""

BITWISE = """
def main() -> int:
    x = 12
    return (x << 1) | (x & 5) ^ 3
"""

CONDITIONAL_EXPR = """
def pick(flag: bool, a: int, b: int) -> int:
    return a if flag else b

def main() -> int:
    return pick(True, 9, 4) + pick(False, 9, 4)
"""

RECURSION = """
def fact(n: int) -> int:
    if n <= 1:
        return 1
    return n * fact(n - 1)

def main() -> int:
    return fact(5)
"""

BUILTINS = """
def main() -> int:
    xs = [3, 1, 2]
    return sum(xs) + len(xs) + abs(0 - 4) + min(2, 9) + max(1, 5)
"""

EARLY_RETURN = """
def safe_div(a: int, b: int) -> int:
    if b == 0:
        return 0
    return a // b

def main() -> int:
    return safe_div(100, 5) + safe_div(3, 0)
"""

SORTING = """
def main() -> int:
    xs = [3, 1, 2]
    ys = sorted(xs)
    zs = list(reversed(ys))
    return zs[0] * 10 + ys[0]
"""

LAST_ELEMENT = """
def last(xs: list[int]) -> int:
    return xs[len(xs) - 1]

def main() -> int:
    xs = [7, 9]
    return last(xs)
"""

DICT_INT_KEYS = """
def tally(keys: list[int]) -> int:
    counts: dict[int, int] = {}
    i = 0
    while i < len(keys):
        k = keys[i]
        counts[k] = counts.get(k, 0) + 1
        i += 1
    return counts.get(7, 0) * 10 + len(counts)

def main() -> int:
    xs = [7, 3, 7]
    return tally(xs)
"""

DICT_STR_KEYS = """
def tally() -> int:
    counts: dict[str, int] = {}
    counts["apple"] = 3
    counts["pear"] = 4
    if "apple" in counts:
        return counts.get("apple", 0) + counts.get("missing", 100) + len(counts)
    return 0

def main() -> int:
    return tally()
"""

DICT_KEYS = """
def most(keys: list[int]) -> int:
    counts: dict[int, int] = {}
    i = 0
    while i < len(keys):
        counts[keys[i]] = counts.get(keys[i], 0) + 1
        i += 1
    best = 0
    ks = list(counts.keys())
    j = 0
    while j < len(ks):
        seen = counts.get(ks[j], 0)
        if seen > best:
            best = seen
        j += 1
    return best

def main() -> int:
    xs = [4, 7, 4]
    return most(xs)
"""

STRINGS = """
def greet(a: str, b: str) -> str:
    return a + b

def main() -> int:
    s = greet("hel", "lo")
    return len(s)
"""

DICT_BOOL_VALUES = """
def check(nums: list[int]) -> int:
    seen: dict[int, bool] = {}
    i = 0
    while i < len(nums):
        seen[nums[i]] = True
        i += 1
    hits = 0
    j = 0
    while j < len(nums):
        if seen.get(nums[j], False):
            hits += 1
        j += 1
    return hits * 10 + len(seen)

def main() -> int:
    xs = [4, 7, 4]
    return check(xs)
"""

DATACLASS_BASIC = """
from dataclasses import dataclass


@dataclass
class Task:
    \"\"\"A unit of work.\"\"\"
    id: int
    retries: int


def cost(t: Task) -> int:
    return t.id + t.retries * 10


def main() -> int:
    t = Task(id=3, retries=2)
    return cost(t)
"""

DATACLASS_MUTATION = """
from dataclasses import dataclass


@dataclass
class Task:
    id: int
    retries: int


def bump(t: Task) -> int:
    t.retries = t.retries + 1
    return t.retries


def make(n: int) -> Task:
    return Task(id=n, retries=0)


def total(ts: list[Task]) -> int:
    acc = 0
    i = 0
    while i < len(ts):
        acc = acc + ts[i].id
        i += 1
    return acc


def main() -> int:
    t = make(7)
    b = bump(t)
    xs = [make(3), make(4)]
    return b * 10 + t.retries + total(xs)
"""

DATACLASS_MIXED_FIELDS = """
from dataclasses import dataclass


@dataclass
class Reading:
    ok: bool
    level: float
    count: int


def score(r: Reading) -> int:
    if r.ok:
        return int(r.level * 10.0) + r.count
    return 0


def main() -> int:
    return score(Reading(ok=True, level=1.5, count=2))
"""

DATACLASS_NESTED = """
from dataclasses import dataclass


@dataclass
class Item:
    v: int


@dataclass
class Basket:
    items: list[Item]
    n: int


def size(b: Basket) -> int:
    return len(b.items) + b.n


def main() -> int:
    return size(Basket(items=[Item(v=1)], n=2))
"""

DATACLASS_STRING_FIELD = """
from dataclasses import dataclass


@dataclass
class Label:
    name: str
    n: int


def width(x: Label) -> int:
    return len(x.name) + x.n


def main() -> int:
    return width(Label(name="abc", n=2))
"""

BORROWED_STRING_PARAM = """
def size(s: str) -> int:
    return len(s)


def main() -> int:
    return size("abcd")
"""

PROGRAMS = {
    "sorting": SORTING,
    "dataclass_string_field": DATACLASS_STRING_FIELD,
    "borrowed_string_param": BORROWED_STRING_PARAM,
    "dataclass_nested": DATACLASS_NESTED,
    "dataclass_basic": DATACLASS_BASIC,
    "dataclass_mutation": DATACLASS_MUTATION,
    "dataclass_mixed_fields": DATACLASS_MIXED_FIELDS,
    "dict_bool_values": DICT_BOOL_VALUES,
    "strings": STRINGS,
    "dict_keys": DICT_KEYS,
    "dict_int_keys": DICT_INT_KEYS,
    "dict_str_keys": DICT_STR_KEYS,
    "last_element": LAST_ELEMENT,
    "arithmetic": ARITHMETIC,
    "control_flow": CONTROL_FLOW,
    "loops": LOOPS,
    "arrays": ARRAYS,
    "mutation": MUTATION,
    "chained": CHAINED,
    "bitwise": BITWISE,
    "conditional_expr": CONDITIONAL_EXPR,
    "recursion": RECURSION,
    "builtins": BUILTINS,
    "early_return": EARLY_RETURN,
}


@pytest.mark.parametrize("name", sorted(PROGRAMS))
def test_translates_without_error(name: str) -> None:
    """Every sample program translates into non-empty tuonelang."""
    generated = compile_to_tuo(PROGRAMS[name])
    assert generated.startswith("module ")
    assert "pub fn main() -> Int" in generated


@needs_tuo
@pytest.mark.parametrize("name", sorted(PROGRAMS))
def test_generated_source_is_accepted(name: str) -> None:
    """The real compiler accepts every generated program."""
    run_generated(PROGRAMS[name], module=name)


@needs_tuo
@pytest.mark.parametrize("name", sorted(PROGRAMS))
def test_agrees_with_python(name: str) -> None:
    """The translated program computes what the Python computes.

    This is the test that matters: agreement is measured by running both, never
    asserted from the shape of the generated source.
    """
    assert_agrees(PROGRAMS[name])


# -- the subset boundary is refused, precisely ----------------------------

REFUSED = {
    "class": ("class C:\n    pass\n", "class"),
    "async": ("async def f() -> int:\n    return 1\n", "async"),
    "lambda": ("def f() -> int:\n    g = lambda: 1\n    return 1\n", "lambda"),
    "comprehension": ("def f(xs: list[int]) -> list[int]:\n    return [x for x in xs]\n", "comprehension"),
    "try": ("def f() -> int:\n    try:\n        return 1\n    except:\n        return 2\n", "exception"),
    "unannotated": ("def f(x) -> int:\n    return 1\n", "annotation"),
    "no_return_type": ("def f(x: int):\n    return 1\n", "annotation"),
    "method_call": ("def f(xs: list[int]) -> int:\n    return xs.count()\n", "method"),
    "fstring": ("def f(n: int) -> str:\n    return f'{n}'\n", "f-string"),
    "dict": ("def f() -> int:\n    d = {}\n    return 1\n", "dict"),
    "true_division": ("def f(a: int, b: int) -> int:\n    return a / b\n", "division"),
    "decorator": ("@staticmethod\ndef f() -> int:\n    return 1\n", "decorator"),
    "default_arg": ("def f(a: int = 1) -> int:\n    return a\n", "default"),
    "varargs": ("def f(*args: int) -> int:\n    return 1\n", "args"),
    "nested_def": ("def f() -> int:\n    def g() -> int:\n        return 1\n    return 1\n", "nested"),
    "is_operator": ("def f(a: int, b: int) -> bool:\n    return a is b\n", "`is`"),
    "in_operator": ("def f(a: int, xs: list[int]) -> bool:\n    return a in xs\n", "`in`"),
    "slicing": ("def f(xs: list[int]) -> list[int]:\n    return xs[0:2]\n", "slic"),
    "big_int": ("def f() -> int:\n    return 99999999999999999999999\n", "64-bit"),
    # `reversed` is a lazy iterator in Python, so translating it as an Array
    # would hand the generated program powers the original does not have.
    # Python's xs[-1] is the last element; tuonelang traps. Refusing at compile
    # time beats turning a working program into a runtime abort.
    "negative_index": ("def f(xs: list[int]) -> int:\n    return xs[-1]\n", "negative index"),
    "negative_index_assign": ("def f(xs: list[int]) -> int:\n    xs[-1] = 0\n    return 1\n", "negative index"),
    # tuonelang v0 has exactly Map[Str, Int] and Map[Int, Int] (ADR-0011).
    "dict_unsupported_value": ("def f(d: dict[str, str]) -> int:\n    return 1\n", "no tuonelang equivalent"),
    # Python's d[k] raises KeyError; tuonelang has no exceptions.
    "dict_raising_index": ("def f(d: dict[str, int]) -> int:\n    return d['k']\n", "KeyError"),
    "dict_get_no_default": ("def f(d: dict[str, int]) -> int:\n    return d.get('k')\n", "default"),
    # d.keys() is a lazy view in Python: not subscriptable, and it tracks
    # later mutations. Only the materialising list(d.keys()) is translated.
    "dict_keys_view": ("def f(d: dict[str, int]) -> int:\n    return len(d.keys())\n", "lazy view"),
    "dict_values": ("def f(d: dict[str, int]) -> int:\n    return len(d.values())\n", "values"),
    "dict_nonempty_literal": ("def f() -> int:\n    d = {'a': 1}\n    return 1\n", "non-empty dict"),
    # -- @dataclass boundary (structs translate; behaviour does not) --------
    "dataclass_method": (
        "from dataclasses import dataclass\n@dataclass\nclass T:\n    a: int\n"
        "    def go(self) -> int:\n        return self.a\n",
        "method",
    ),
    "dataclass_post_init": (
        "from dataclasses import dataclass\n@dataclass\nclass T:\n    a: int\n"
        "    def __post_init__(self):\n        pass\n",
        "__post_init__",
    ),
    "dataclass_inheritance": (
        "from dataclasses import dataclass\n@dataclass\nclass T(Base):\n    a: int\n",
        "inheritance",
    ),
    "dataclass_frozen": (
        "from dataclasses import dataclass\n@dataclass(frozen=True)\nclass T:\n    a: int\n",
        "frozen",
    ),
    "dataclass_default": (
        "from dataclasses import dataclass\n@dataclass\nclass T:\n    a: int = 3\n",
        "default",
    ),
    "dataclass_classvar": (
        "from dataclasses import dataclass\nfrom typing import ClassVar\n"
        "@dataclass\nclass T:\n    a: ClassVar[int]\n",
        "ClassVar",
    ),
    "dataclass_unannotated_field": (
        "from dataclasses import dataclass\n@dataclass\nclass T:\n    a = 3\n",
        "annotated",
    ),
    "dataclass_no_fields": (
        "from dataclasses import dataclass\n@dataclass\nclass T:\n    pass\n",
        "no fields",
    ),
    "bare_class": ("class T:\n    pass\n", "class definition"),
    "named_tuple": (
        "from typing import NamedTuple\nclass T(NamedTuple):\n    a: int\n",
        "inheritance",
    ),
    "dataclass_positional_construction": (
        "from dataclasses import dataclass\n@dataclass\nclass T:\n    a: int\n    b: int\n"
        "def f() -> int:\n    t = T(1, 2)\n    return t.a\n",
        "positional",
    ),
    "dataclass_missing_field": (
        "from dataclasses import dataclass\n@dataclass\nclass T:\n    a: int\n    b: int\n"
        "def f() -> int:\n    t = T(a=1)\n    return t.a\n",
        "missing field",
    ),
    "dataclass_unknown_field": (
        "from dataclasses import dataclass\n@dataclass\nclass T:\n    a: int\n"
        "def f(t: T) -> int:\n    return t.zzz\n",
        "no field",
    ),
    # tuonelang refuses a struct reaching itself with no indirection (T0016).
    # A `list[T]` field is NOT an escape -- the target rejects `Array[Node]`
    # inside `Node` too -- so the cycle is followed through array elements.
    # A whole-struct `==` passes `tuo check` and then fails in codegen
    # ("reading a whole aggregate as a scalar"), so it is refused here.
    "dataclass_equality": (
        "from dataclasses import dataclass\n@dataclass\nclass T:\n    a: int\n"
        "def f(x: T, y: T) -> bool:\n    return x == y\n",
        "comparing",
    ),
    "dataclass_self_reference": (
        "from dataclasses import dataclass\n@dataclass\nclass Node:\n"
        "    value: int\n    next: Node\n",
        "contains itself",
    ),
    "dataclass_mutual_recursion": (
        "from dataclasses import dataclass\n@dataclass\nclass A:\n    b: B\n"
        "@dataclass\nclass B:\n    a: A\n",
        "contains itself",
    ),
    "dataclass_recursion_via_list": (
        "from dataclasses import dataclass\n@dataclass\nclass Node:\n"
        "    value: int\n    kids: list[Node]\n",
        "contains itself",
    ),
    "dataclass_temporary_to_mut": (
        "from dataclasses import dataclass\n@dataclass\nclass T:\n    a: int\n"
        "def g(t: T) -> int:\n    t.a = 5\n    return t.a\n"
        "def f() -> int:\n    return g(T(a=1))\n",
        "temporary",
    ),
    "reversed_iterator": ("def f(xs: list[int]) -> int:\n    return reversed(xs)[0]\n", "iterator"),
    "tuple": ("def f() -> int:\n    a, b = 1, 2\n    return a\n", "tuple"),
    "import_library": ("import os\ndef f() -> int:\n    return 1\n", "import"),
    "mixed_numeric": ("def f(a: int, b: float) -> float:\n    return a + b\n", "Int and Float"),
}


@pytest.mark.parametrize("name", sorted(REFUSED))
def test_refuses_with_a_positioned_diagnostic(name: str) -> None:
    """Each excluded construct is refused with a diagnostic naming it.

    Refusing is the tool's contract: an untranslatable construct must never be
    emitted as tuonelang that means something else.
    """
    source, expected_fragment = REFUSED[name]
    with pytest.raises(CompileError) as caught:
        compile_to_tuo(source)
    diagnostic = caught.value.diagnostic
    assert diagnostic.code
    assert diagnostic.line > 0, "a diagnostic must point at a real source line"
    combined = (diagnostic.message + " " + " ".join(diagnostic.notes)).lower()
    assert expected_fragment.lower() in combined, (
        f"diagnostic did not mention {expected_fragment!r}: {diagnostic.render('t.py')}"
    )


def test_invalid_python_is_reported_as_such() -> None:
    """Input that is not Python at all gets a syntax diagnostic, not a crash."""
    with pytest.raises(CompileError) as caught:
        compile_to_tuo("def f( ->\n")
    assert caught.value.diagnostic.code == "PY0000"


def test_duplicate_function_is_refused() -> None:
    """tuonelang has no overloading, so a redefinition cannot be translated."""
    with pytest.raises(CompileError) as caught:
        compile_to_tuo("def f() -> int:\n    return 1\ndef f() -> int:\n    return 2\n")
    assert "more than once" in caught.value.diagnostic.message


def test_reserved_word_is_refused() -> None:
    """A Python name that is a tuonelang keyword cannot be emitted as-is."""
    with pytest.raises(CompileError) as caught:
        compile_to_tuo("def f() -> int:\n    match = 1\n    return match\n")
    assert "reserved" in caught.value.diagnostic.message


# -- generated-source properties ------------------------------------------

def test_mutated_local_becomes_var() -> None:
    """A reassigned Python local becomes `var`; a write-once local stays `let`."""
    generated = compile_to_tuo(LOOPS)
    assert "var a = 0;" in generated
    assert "let t = " in generated


def test_augmented_assignment_is_expanded() -> None:
    """tuonelang has no `+=`, so the compiler must expand it."""
    generated = compile_to_tuo(ARRAYS)
    assert "+=" not in generated
    assert "acc = acc + " in generated


def test_chained_comparison_is_expanded() -> None:
    """`a < b < c` is a parse error in tuonelang and must become a conjunction."""
    generated = compile_to_tuo(CHAINED)
    assert "&&" in generated


def test_dict_uses_the_map_builtins() -> None:
    """A translated dict uses `std::map`, needing no stdlib source module."""
    generated = compile_to_tuo(DICT_STR_KEYS)
    assert "std::map::empty()" in generated
    assert "std::map::insert(" in generated
    assert "std::map::contains_key(" in generated
    assert "import std::" not in generated


def test_dict_get_collapses_the_option() -> None:
    """`d.get(k, default)` becomes a `match` over `Option[V]`.

    tuonelang's std::map::get returns Option[V]; Python's default is exactly
    what collapses it, which is why the two-argument form translates and the
    one-argument form cannot.
    """
    generated = compile_to_tuo(DICT_INT_KEYS)
    assert "match std::map::get(" in generated
    assert "None =>" in generated


def test_docstring_becomes_doc_comment() -> None:
    """A Python docstring is carried across as a `///` doc comment."""
    generated = compile_to_tuo('def f() -> int:\n    """Explain."""\n    return 1\n')
    assert "/// Explain." in generated


def test_output_needs_no_stdlib_file() -> None:
    """Generated source is self-contained: it imports no stdlib source module.

    Array and string operations are compiler builtins, so the output compiles
    on its own without a stdlib `.tuo` file being supplied alongside it.
    """
    for source in PROGRAMS.values():
        assert "import std::" not in compile_to_tuo(source)


@needs_tuo
def test_generated_source_is_canonically_formatted() -> None:
    """`tuo fmt --check` accepts the tool's formatted output."""
    from py2tuo.verify import format_source

    formatted, result = format_source(compile_to_tuo(LOOPS), tuo=TUO or "")
    assert result.ok, result.output
    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory) / "t.tuo"
        path.write_text(formatted, encoding="utf-8")
        assert run_tuo(TUO or "", ["fmt", "--check", str(path)]).ok


# -- @dataclass -> struct: the translation decisions worth pinning -----------

def test_dataclass_becomes_a_struct_declaration() -> None:
    """A `@dataclass` of annotated fields becomes a tuonelang `struct`."""
    generated = compile_to_tuo(DATACLASS_BASIC)
    assert "struct Task {" in generated
    assert "id: Int" in generated
    assert "retries: Int" in generated
    # The decorator and its import are erased: both describe a type the
    # translation has already resolved.
    assert "dataclass" not in generated
    assert "import" not in generated


def test_dataclass_docstring_becomes_a_doc_comment() -> None:
    """A dataclass docstring carries across, as a function's already does."""
    assert "/// A unit of work." in compile_to_tuo(DATACLASS_BASIC)


def test_a_read_only_struct_parameter_is_borrowed() -> None:
    """A struct parameter the body only reads is `in` — a read-only borrow.

    It is not `take`: that would move the caller's value for no reason. It is
    not `mut`: tuonelang would then demand a mutable place at every call site.
    """
    assert "pub fn cost(in t: Task)" in compile_to_tuo(DATACLASS_BASIC)


def test_a_mutated_struct_parameter_takes_a_mut_borrow() -> None:
    """A struct parameter whose FIELD is written is `mut`.

    This is the load-bearing half of the mode rule. Emitting `in` here would
    produce code the real compiler rejects with `O0004` — the mis-translation
    the rule exists to prevent — so the field write must count as mutating the
    parameter itself. A `mut` borrow also writes through to the caller, which
    is exactly what the Python does.
    """
    generated = compile_to_tuo(DATACLASS_MUTATION)
    assert "pub fn bump(mut t: Task)" in generated
    assert "pub fn total(in ts: Array[Task])" in generated


def test_struct_literal_names_every_field_in_declaration_order() -> None:
    """Construction emits fields in DECLARATION order, not call order.

    tuonelang struct literals name every field, so any order compiles; fixing
    it to the declaration makes the output canonical rather than dependent on
    how the Python happened to spell the call.
    """
    source = (
        "from dataclasses import dataclass\n"
        "@dataclass\n"
        "class T:\n    a: int\n    b: int\n"
        "def main() -> int:\n    t = T(b=2, a=1)\n    return t.a\n"
    )
    assert "T { a: 1, b: 2 }" in compile_to_tuo(source)


def test_a_struct_can_be_a_field_type() -> None:
    """A dataclass field may be another dataclass, including one declared later."""
    source = (
        "from dataclasses import dataclass\n"
        "@dataclass\n"
        "class Outer:\n    inner: Inner\n    n: int\n"
        "@dataclass\n"
        "class Inner:\n    v: int\n"
        "def main() -> int:\n"
        "    o = Outer(inner=Inner(v=4), n=1)\n"
        "    return o.inner.v + o.n\n"
    )
    generated = compile_to_tuo(source)
    assert "struct Outer { inner: Inner, n: Int }" in generated.replace("\n    ", " ").replace(
        ",\n}", " }"
    ) or "inner: Inner" in generated
    assert "o.inner.v" in generated


def test_a_string_literal_is_converted_where_an_owned_String_is_wanted() -> None:
    """A `str` field holds an owned `String`; a literal is a borrowed `Str`.

    The two are assignment-compatible in this tool's own type rule but NOT in
    the target, which reports `T0001` (expected `String`, found `Str`). The
    conversion therefore has to appear in the emitted text.
    """
    generated = compile_to_tuo(DATACLASS_STRING_FIELD)
    assert "std::string::from_str(\"abc\")" in generated


def test_len_picks_the_module_matching_the_string_type() -> None:
    """`len` on a borrowed `Str` is `std::str::len`, not `std::string::len`.

    The same operation lives in a different module per string type, and the
    wrong one is a `T0001` in the real compiler.
    """
    assert "std::str::len(s)" in compile_to_tuo(BORROWED_STRING_PARAM)
    assert "std::string::len(x.name)" in compile_to_tuo(DATACLASS_STRING_FIELD)
