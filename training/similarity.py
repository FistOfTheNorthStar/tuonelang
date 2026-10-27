"""Near-duplicate detection for the training material.

An exact-match test misses the duplicates that matter: two seeds written
apart converge on the same program with different names, and a task is
easily paraphrased. This module compares programs by their *shape* and tasks
by their wording, so ``generate.py`` can refuse an eval task that is a
renamed copy of training material, and a training seed that is a renamed
copy of a public benchmark task.

A program is fingerprinted as a set of token shingles (runs of ``K`` tokens)
after normalizing what a rename or a new test case changes:

* comments, ``spec`` blocks (tests are not the solution) and ``pub`` are
  dropped;
* a local name (a lowercase identifier) becomes its first-occurrence index,
  so ``fn sum(in xs: ...)`` and ``fn total(in values: ...)`` match;
* number, string and character literals become ``NUM``, ``STR`` and ``CHR``;
* keywords, punctuation, type and variant names (capitalized identifiers),
  and path segments (``std::core::min``) are kept, since they *are* the shape.

A task is fingerprinted as a set of word trigrams, lowercased, with code in
backticks kept as written.

Similarity is the Jaccard index of two fingerprints. A program needs
``MIN_SHINGLES`` shingles to be compared by it: a one-line body has too
little shape for a partial overlap to mean anything (``a + b`` is
everywhere). Two smaller programs still match when their shapes are
identical and at least ``MIN_IDENTICAL`` tokens long, which is a renamed copy
however short.
"""

from __future__ import annotations

import re
from dataclasses import dataclass

K = 5
"""Tokens per program shingle."""

MIN_SHINGLES = 12
"""Smaller programs are compared only for an identical shape."""

MIN_IDENTICAL = 8
"""Shorter shapes (``fn f() {}``) are never reported, identical or not."""

KEYWORDS = frozenset(
    """module import pub fn let var const struct enum interface impl where if
    else match for in while loop break continue return take mut move Box
    Shared Weak spec given when then assert unsafe as true false self Self""".split()
)

_TOKEN = re.compile(
    r"""
    (?P<ws>\s+)
  | (?P<comment>//[^\n]*)
  | (?P<str>"(?:[^"\\\n]|\\.)*")
  | (?P<chr>'(?:[^'\\\n]|\\[^\n]|\\u\{[0-9a-fA-F]+\})')
  | (?P<num>[0-9][0-9_]*(?:\.[0-9][0-9_]*)?(?:[a-zA-Z][a-zA-Z0-9]*)?)
  | (?P<ident>[A-Za-z_][A-Za-z0-9_]*)
  | (?P<punct>::|->|=>|==|!=|<=|>=|&&|\|\||<<|>>|\.\.=?|[^\sA-Za-z0-9_])
    """,
    re.VERBOSE,
)


def tokens(source: str) -> list[str]:
    """``source``'s tokens, comments and whitespace dropped."""
    out = []
    for m in _TOKEN.finditer(source):
        kind = m.lastgroup
        if kind in ("ws", "comment"):
            continue
        out.append(m.group())
    return out


def _without_specs(toks: list[str]) -> list[str]:
    """``toks`` with every ``spec NAME { ... }`` block removed."""
    out: list[str] = []
    i = 0
    while i < len(toks):
        if toks[i] == "spec" and (i == 0 or toks[i - 1] in ("}", ";")):
            j = i
            while j < len(toks) and toks[j] != "{":
                j += 1
            depth = 0
            while j < len(toks):
                depth += {"{": 1, "}": -1}.get(toks[j], 0)
                j += 1
                if depth == 0:
                    break
            i = j
            continue
        out.append(toks[i])
        i += 1
    return out


def shape(source: str) -> list[str]:
    """``source``'s normalized token stream (see the module docs)."""
    toks = _without_specs(tokens(source))
    names: dict[str, str] = {}
    out = []
    for i, tok in enumerate(toks):
        if tok[0] == '"':
            out.append("STR")
        elif tok[0] == "'":
            out.append("CHR")
        elif tok[0].isdigit():
            out.append("NUM")
        elif tok == "pub":
            continue  # visibility does not change what a program does
        elif tok in KEYWORDS or not (tok[0].islower() or tok[0] == "_"):
            out.append(tok)
        elif (i > 0 and toks[i - 1] == "::") or (i + 1 < len(toks) and toks[i + 1] == "::"):
            out.append(tok)  # a path segment: `std::core::min`
        elif toks[i - 1 : i] == ["module"]:
            continue  # the module's own name says nothing about the program
        else:
            out.append(names.setdefault(tok, f"v{len(names)}"))
    return out


def program_shape(source: str) -> tuple[str, ...]:
    """``source``'s shape without its ``module`` header."""
    s = shape(source)
    return tuple(s[2:] if s[:1] == ["module"] else s)  # `module` and its `;`


def program_fingerprint(source: str) -> frozenset[tuple[str, ...]]:
    """The set of ``K``-token shingles of ``source``'s shape."""
    s = program_shape(source)
    return frozenset(s[i : i + K] for i in range(len(s) - K + 1))


_WORD = re.compile(r"`[^`]*`|[a-z0-9_]+")


def task_fingerprint(task: str) -> frozenset[tuple[str, ...]]:
    """The set of word trigrams of ``task``."""
    words = _WORD.findall(task.lower())
    return frozenset(tuple(words[i : i + 3]) for i in range(len(words) - 2))


def jaccard(a: frozenset, b: frozenset) -> float:
    if not a or not b:
        return 0.0
    return len(a & b) / len(a | b)


@dataclass(frozen=True)
class Item:
    """One piece of material to compare: a seed, a harvested example, or a
    benchmark task. ``program`` may be empty (a task with no reference)."""

    label: str
    task: str
    program: str

    @property
    def program_shape(self) -> tuple[str, ...]:
        return _fp_cache(self.program, program_shape)

    @property
    def program_fp(self) -> frozenset:
        return _fp_cache(self.program, program_fingerprint)

    @property
    def task_fp(self) -> frozenset:
        return _fp_cache(self.task, task_fingerprint)


_CACHE: dict[tuple[str, str], frozenset | tuple] = {}


def _fp_cache(text: str, fn):
    key = (fn.__name__, text)
    if key not in _CACHE:
        _CACHE[key] = fn(text)
    return _CACHE[key]


@dataclass(frozen=True)
class Match:
    """``a`` is a near-duplicate of ``b`` by ``what`` (``program``/``task``)."""

    a: Item
    b: Item
    what: str
    score: float

    def __str__(self) -> str:
        return f"{self.a.label} ~ {self.b.label}: {self.what} similarity {self.score:.2f}"


def program_similarity(a: Item, b: Item) -> float:
    """1.0 for identical shapes of ``MIN_IDENTICAL`` tokens or more, else
    the Jaccard index of the fingerprints when both have ``MIN_SHINGLES``,
    else 0.0."""
    if len(a.program_shape) >= MIN_IDENTICAL and a.program_shape == b.program_shape:
        return 1.0
    if len(a.program_fp) >= MIN_SHINGLES and len(b.program_fp) >= MIN_SHINGLES:
        return jaccard(a.program_fp, b.program_fp)
    return 0.0


def near_duplicates(
    items: list[Item],
    against: list[Item] | None,
    program_threshold: float,
    task_threshold: float | None,
) -> list[Match]:
    """Every pair whose programs or tasks are at least as similar as the
    thresholds: ``items`` against ``against``, or, when that is ``None``,
    ``items`` against each other. Each pair is reported once, by its program
    when both match. A ``task_threshold`` of ``None`` compares programs only."""
    found = []
    pairs = (
        ((a, b) for a in items for b in against)
        if against is not None
        else ((a, b) for i, a in enumerate(items) for b in items[i + 1 :])
    )
    for a, b in pairs:
        score = program_similarity(a, b)
        if score >= program_threshold:
            found.append(Match(a, b, "program", score))
            continue
        if task_threshold is None:
            continue
        score = jaccard(a.task_fp, b.task_fp)
        if score >= task_threshold:
            found.append(Match(a, b, "task", score))
    return found
