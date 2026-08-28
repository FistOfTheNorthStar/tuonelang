"""Harvest additional SFT examples from the repository's validated tuonelang.

The seed library (``seeds.py``) is the hand-authored *coverage skeleton*. This
module supplies **volume** from source the repository already keeps green by its
own test suites:

  * ``crates/tuo-stdlib/src/std/*.tuo`` — every ``pub fn`` with a doc comment
    becomes a (doc -> implementation) example. The stdlib is pinned by
    ``tuo-cli/tests/stdlib.rs``, which really compiles every module and runs
    every shipped spec.
  * ``examples/**/src/*.tuo`` — whole multi-function programs, pinned by
    ``tuo-cli/tests/dogfood_examples.rs``.
  * ``corpus/correct/*.tuo`` — compiler-validated correct programs, re-admitted
    on every ``cargo test`` by ``tuo-corpus/tests/shipped_corpus.rs``.

Nothing here is trusted on assertion either: ``generate.py`` compiles every
harvested example through the real ``tuo`` compiler before emitting it, exactly
as it does for a hand-written seed. A harvested item that does not compile is
*dropped* (it is a lower-value bulk source, not a coverage promise), and the
count of drops is reported in ``stats.json`` — silent truncation is visible,
never hidden.

A harvested stdlib function is emitted as a **self-contained** program: the
function alone, which is how a model will be asked to write it. Roughly a third
of the library's ``pub fn``\\ s do not stand alone — they reference a
module-private type (``Pair``) or a private helper that does not cross a file
boundary — and those are dropped by the compile gate rather than shipped with a
context the training prompt never shows. That is the intended trade: an example
teaches only if the target program compiles as presented.

The harvested records are deliberately **held out of the eval split**: the eval
set stays the hand-authored seed slice, so scoring is never contaminated by
material that also appears verbatim in the repository.
"""

from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path

# A `pub fn` signature line: capture the name and the full signature.
_PUB_FN = re.compile(r"^pub fn ([a-z_][a-z0-9_]*)\s*\(")

# Markers that classify a stdlib function's tier (see the three-tier rule).
_EFFECT = "EFFECT:"
_CONTRACT = "CONTRACT:"


@dataclass(frozen=True)
class Harvested:
    """One example mined from committed, test-pinned repository source."""

    origin: str
    """Where it came from (``stdlib``, ``example``, ``corpus``)."""

    name: str
    """A stable identifier (``std::core::min``, ``examples/cli-stats``)."""

    task: str
    """The natural-language instruction reconstructed from the source."""

    solution: str
    """The committed tuonelang source answering ``task``."""

    stdlib_deps: tuple[str, ...] = ()
    """Stdlib modules whose source must be compiled alongside (see Seed)."""

    notes: str = ""
    """Teaching note appended to the assistant answer."""


def _dedent_doc(lines: list[str]) -> str:
    """Strip the leading ``/// `` from a run of doc-comment lines."""
    out = []
    for ln in lines:
        stripped = ln[3:]
        out.append(stripped[1:] if stripped.startswith(" ") else stripped)
    return "\n".join(out).strip()


def _split_doc(doc: str) -> tuple[str, str]:
    """Split a doc comment into (prose, example-block)."""
    if "# Example" not in doc:
        return doc, ""
    head, _, tail = doc.partition("# Example")
    return head.strip(), tail.strip()


def harvest_stdlib(repo: Path) -> list[Harvested]:
    """Mine one example per documented ``pub fn`` in the standard library.

    The doc comment is the *task* (it says what the function must do, in prose
    a model can be asked to satisfy) and the committed body is the *solution*.
    Effect- and contract-tier functions are skipped: their bodies call host
    primitives that cannot be exercised from a spec, so they teach the API
    surface rather than a self-contained implementation, and the seed library
    already covers that tier deliberately.
    """
    out: list[Harvested] = []
    std = repo / "crates" / "tuo-stdlib" / "src" / "std"
    for path in sorted(std.glob("*.tuo")):
        module = path.stem
        lines = path.read_text().split("\n")
        doc: list[str] = []
        i = 0
        while i < len(lines):
            line = lines[i]
            if line.startswith("///"):
                doc.append(line)
                i += 1
                continue
            m = _PUB_FN.match(line)
            if not m or not doc:
                if not line.startswith("///"):
                    doc = []
                i += 1
                continue

            name = m.group(1)
            body_text = _dedent_doc(doc)
            doc = []

            # Skip the non-executable tiers (see the module docstring).
            if _EFFECT in body_text or _CONTRACT in body_text:
                i += 1
                continue

            # Collect the function through its closing brace at column 0.
            start = i
            i += 1
            while i < len(lines) and lines[i] != "}":
                i += 1
            if i >= len(lines):
                break  # unterminated; skip the tail
            fn_src = "\n".join(lines[start : i + 1])
            i += 1

            prose, example = _split_doc(body_text)
            if not prose:
                continue
            task = (
                f"Implement `{module}::{name}` for the tuonelang standard "
                f"library. {prose}"
            )
            if example:
                task += f"\n\nExample:\n{example}"

            out.append(
                Harvested(
                    origin="stdlib",
                    name=f"std::{module}::{name}",
                    task=task,
                    solution=fn_src + "\n",
                    notes=(
                        f"This is the committed implementation from the "
                        f"tuonelang standard library (`std::{module}`)."
                    ),
                )
            )
    return out


def harvest_examples(repo: Path) -> list[Harvested]:
    """Mine each dogfooding example program as a whole-program example."""
    out: list[Harvested] = []
    root = repo / "examples"
    if not root.is_dir():
        return out
    for path in sorted(root.rglob("src/*.tuo")):
        rel = path.relative_to(repo)
        project = path.parent.parent.name
        src = path.read_text()
        # Prefer the program's own leading comment block as the task, if any.
        header = []
        for ln in src.split("\n"):
            if ln.startswith("//"):
                header.append(ln.lstrip("/").strip())
            elif ln.strip() == "" and header:
                break
            elif ln.strip():
                break
        blurb = " ".join(header).strip()
        task = (
            f"Write the tuonelang program `{rel}` from the {project} example."
            + (f" {blurb}" if blurb else "")
        )
        out.append(
            Harvested(
                origin="example",
                name=str(rel),
                task=task,
                solution=src,
                notes=(
                    "This is a committed dogfooding example, kept green by "
                    "`tuo-cli/tests/dogfood_examples.rs`."
                ),
            )
        )
    return out


def harvest_corpus(repo: Path) -> list[Harvested]:
    """Mine the compiler-validated `corpus/correct/` entries."""
    out: list[Harvested] = []
    root = repo / "corpus" / "correct"
    if not root.is_dir():
        return out
    for path in sorted(root.glob("*.tuo")):
        src = path.read_text()
        out.append(
            Harvested(
                origin="corpus",
                name=f"corpus/correct/{path.name}",
                task=(
                    f"Write a correct tuonelang program named `{path.stem}` "
                    f"that the compiler accepts and whose specs pass."
                ),
                solution=src,
                notes=(
                    "From the compiler-validated corpus (`corpus/correct/`), "
                    "re-admitted on every test run by "
                    "`tuo-corpus/tests/shipped_corpus.rs`."
                ),
            )
        )
    return out


def harvest_all(repo: Path) -> list[Harvested]:
    """Every harvested example, in a deterministic order."""
    return harvest_stdlib(repo) + harvest_examples(repo) + harvest_corpus(repo)
