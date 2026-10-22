"""Harvest additional SFT examples from the repository's validated tuonelang.

The seed library (``seeds/``) is the hand-authored *coverage skeleton*. This
module supplies **volume** from source the repository already keeps green by its
own test suites:

  * ``crates/tuo-stdlib/src/std/*.tuo`` — every documented, specced ``pub fn``
    becomes a **call-site** example: its own spec, moved into a user module
    and written with full paths (ADR-0038 made the library callable). The
    stdlib is pinned by ``tuo-cli/tests/stdlib.rs``, which really compiles
    every module and runs every shipped spec.
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

A harvested call site is a **self-contained** program — a user module whose
spec calls the library — which is exactly how a model will use it. A spec that
reaches a module-private type or helper does not compile outside its module,
and is dropped by the compile gate rather than shipped with a context the
training prompt never shows: an example teaches only if the program compiles
as presented.

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

# A spec block named for a function: `spec name {`.
_SPEC = re.compile(r"^spec ([a-z_][a-z0-9_]*) \{")

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
    """Stdlib modules the example calls (informational; see Seed)."""

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
    """Mine one **call-site** example per documented, specced ``pub fn``.

    Copying a library function's body taught a model to re-implement the
    library — the opposite of the "one obvious API, call it" rule the brief
    states. Since ADR-0038 a program can simply call the library, so this
    emits what that looks like instead: the function's own committed spec,
    moved into a user module, with every call to the module's public
    functions written as the full path (`std::core::min(2, 5)`). The task names
    the function, its declaration, and what its doc comment says it does.

    Effect- and contract-tier functions are skipped (they cannot be specced).
    A spec that leans on a module-private type or helper does not compile once
    moved out of the module, and the generator's compile gate drops it.
    """
    out: list[Harvested] = []
    std = repo / "crates" / "tuo-stdlib" / "src" / "std"
    for path in sorted(std.glob("*.tuo")):
        module = path.stem
        lines = path.read_text().split("\n")
        public: set[str] = set()
        documented: dict[str, tuple[str, str]] = {}  # name -> (prose, decl)
        specs: dict[str, str] = {}  # target name -> spec body
        doc: list[str] = []
        i = 0
        while i < len(lines):
            line = lines[i]
            if line.startswith("///"):
                doc.append(line)
                i += 1
                continue
            m = _PUB_FN.match(line)
            if m:
                name = m.group(1)
                public.add(name)
                body_text = _dedent_doc(doc)
                if body_text and _EFFECT not in body_text and _CONTRACT not in body_text:
                    prose, _ = _split_doc(body_text)
                    decl = line[len("pub ") :].rstrip(" {")
                    if prose:
                        documented[name] = (prose, decl)
            s = _SPEC.match(line)
            if s:
                body = []
                i += 1
                while i < len(lines) and lines[i] != "}":
                    body.append(lines[i])
                    i += 1
                specs.setdefault(s.group(1), "\n".join(body))
            if not line.startswith("///"):
                doc = []
            i += 1

        for name, (prose, decl) in documented.items():
            body = specs.get(name)
            if body is None:
                continue
            qualified = re.sub(
                r"(?<![\w:.])(" + "|".join(sorted(public, key=len, reverse=True)) + r")\(",
                lambda m: f"std::{module}::{m.group(1)}(",
                body,
            )
            path_name = f"std::{module}::{name}"
            task = (
                f"Call the standard library's `{path_name}` from your own "
                f"program and show what it returns with a spec. It is declared "
                f"`{decl}`. {prose}"
            )
            solution = f'module example;\n\nspec "{path_name}" {{\n{qualified}\n}}\n'
            out.append(
                Harvested(
                    origin="stdlib",
                    name=path_name,
                    task=task,
                    solution=solution,
                    notes=(
                        f"`{path_name}` is called by its full path from any "
                        f"program; the toolchain loads `std::{module}` because "
                        f"the program names it, so there is nothing to import "
                        f"or copy. The spec is the library's own."
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
