"""Seed library for tuonelang fine-tuning data.

Every seed is a (concept, task, solution) triple written by hand against the v0
runnable core described in ``REFERENCE.md``. Nothing here is trusted on
assertion: ``generate.py`` compiles every solution through the real ``tuo``
compiler (``check`` + ``spec``) and refuses to emit any example whose solution
does not pass, and ``crates/tuo-cli/tests/training_seeds.rs`` re-admits every
seed on each ``cargo test``. A seed that stops compiling is a loud failure,
never a silent hallucination.

The seeds are files, not strings in this module, so a syntax change breaks
them in the same PR and a reviewer reads tuonelang rather than Python. One seed
is a group of files under ``seeds/<concept>/``, named after the function it is
about::

    seeds/hash-map/most_common.toml          the task and metadata (below)
    seeds/hash-map/most_common.tuo           the solution: a complete program
    seeds/hash-map/most_common.heldout.tuo   optional: held-out spec blocks
    seeds/hash-map/most_common.mutant1.tuo   optional: wrong programs, 1..n

The directory is the concept tag. The ``.toml`` holds ``task`` (required),
``notes``, ``run_exit`` (present exactly when the program has a nullary
``main`` to run natively), ``stdlib_deps``, ``tier``, ``difficulty``, and
``origin`` — each documented on :class:`Seed`. Snippets need no ``module``
header and no ``main``: the compiler accepts a bare set of functions with
colocated specs, so most solutions are the function(s) under test plus their
spec block.

This module is the loader. It refuses an unknown metadata key and a ``.tuo``
file that belongs to no seed, so a typo cannot silently drop data.

A live model's recorded attempts at a training seed's task (``live_attempts.py``)
live beside the seeds, one directory per run, and are attached to their seed::

    live/<run>/run.json                      the model configuration and counts
    live/<run>/hash-map/most_common.tuo      that model's attempt at the task

An attempt for a seed that does not exist is refused like a stray file.

The held-out eval seeds use the same layout but live **outside** this public
repository, in a private checkout named by ``TUO_EVAL_DIR`` (its ``seeds/``
directory): a model trained on scraped GitHub may have seen anything public,
so a score on public tasks measures memorisation. ``EVAL_SEEDS`` is empty when
the variable is unset.
"""
import os
import tomllib
from dataclasses import dataclass, replace
from pathlib import Path

SEEDS_DIR = Path(__file__).resolve().parent / "seeds"
LIVE_DIR = Path(__file__).resolve().parent / "live"
EVAL_DIR = Path(os.environ["TUO_EVAL_DIR"]).resolve() if os.environ.get("TUO_EVAL_DIR") else None


@dataclass(frozen=True)
class Seed:
    """One verified teaching example."""

    concept: str
    """Kebab-case feature tag (e.g. ``fixed-arrays``, ``option-result``)."""

    task: str
    """The natural-language instruction a model must satisfy."""

    solution: str
    """A complete, compiler-valid tuonelang program answering ``task``."""

    runnable: bool = False
    """True if the program has a nullary ``main`` and should be run natively."""

    run_exit: int = 0
    """Expected process exit byte when ``runnable`` (``main() & 0xff``)."""

    notes: str = ""
    """Optional teaching note surfaced in the SFT ``assistant`` rationale."""

    stdlib_deps: tuple[str, ...] = ()
    """Stdlib module names (e.g. ``"core"``) the solution calls. Informational
    since ADR-0038: the toolchain loads every standard-library module a program
    names, so nothing is passed alongside the program and no example tells a
    model to. (The ``std::array`` / ``std::string`` heap ops and the
    ``std::rt`` effects are compiler builtins either way.)"""

    tier: str = ""
    """``executable`` (pure; its specs run), ``effect`` (calls io/fs/net/sync,
    so it runs natively but cannot be specced), or ``interpreter`` (checks and
    specs but is outside the native subset). Empty means *inferred* by
    ``generate.py`` from what the solution calls; a declared tier is checked
    against the inference."""

    difficulty: int = 0
    """1 (one construct) to 5 (a multi-function program with heap data and
    ownership choices); 0 = unrated."""

    heldout_specs: str = ""
    """Extra ``spec`` blocks for an eval seed, kept out of the prompt. The
    generator runs them against the solution (they must pass) and puts them in
    the eval record, so scoring checks behaviour the prompt did not spell out.
    A public seed may not carry them: published, they are not held out."""

    mutants: tuple[str, ...] = ()
    """Plausible wrong solutions: each must *check* and must *fail* a spec.
    The generator verifies both and emits each as a spec-failure repair
    transcript, with the real ``tuo spec`` output as the compiler turn. Only a
    training seed has them, since an eval seed emits no transcripts."""

    origin: str = "hand"
    """Where the seed came from (``hand``, ``py2tuo``, ``benchmark``, ...)."""

    attempts: tuple[tuple[str, str], ...] = ()
    """A live model's recorded attempts at the task, as ``(run, program)``
    pairs in run order (``live/<run>/<concept>/<name>.tuo``). The generator
    emits each one the compiler still has feedback on as a repair transcript
    ending in the solution. Only a training seed has them."""

    name: str = ""
    """The seed's file stem under ``seeds/<concept>/``."""


# The metadata keys a seed's ``.toml`` may carry. ``run_exit`` stands for both
# ``runnable`` and ``run_exit``; everything else is a field of the same name.
METADATA_KEYS = {"task", "notes", "run_exit", "stdlib_deps", "tier", "difficulty", "origin"}


class SeedError(Exception):
    """A seed directory that does not describe its seeds exactly."""


def load_seed(concept_dir: Path, name: str) -> Seed:
    """Read the seed ``name`` from ``concept_dir``."""
    meta_path = concept_dir / f"{name}.toml"
    try:
        meta = tomllib.loads(meta_path.read_text())
    except tomllib.TOMLDecodeError as err:
        raise SeedError(f"{meta_path}: {err}") from err
    unknown = sorted(set(meta) - METADATA_KEYS)
    if unknown:
        raise SeedError(f"{meta_path}: unknown key(s) {', '.join(unknown)}")
    if "task" not in meta:
        raise SeedError(f"{meta_path}: missing `task`")
    solution = concept_dir / f"{name}.tuo"
    if not solution.is_file():
        raise SeedError(f"{meta_path}: no solution file {solution.name}")
    heldout = concept_dir / f"{name}.heldout.tuo"
    mutants = []
    while (mutant := concept_dir / f"{name}.mutant{len(mutants) + 1}.tuo").is_file():
        mutants.append(mutant.read_text())
    return Seed(
        concept=concept_dir.name,
        task=meta["task"],
        solution=solution.read_text(),
        runnable="run_exit" in meta,
        run_exit=meta.get("run_exit", 0),
        notes=meta.get("notes", ""),
        stdlib_deps=tuple(meta.get("stdlib_deps", ())),
        tier=meta.get("tier", ""),
        difficulty=meta.get("difficulty", 0),
        heldout_specs=heldout.read_text() if heldout.is_file() else "",
        mutants=tuple(mutants),
        origin=meta.get("origin", "hand"),
        name=name,
    )


def load_seeds(root: Path = SEEDS_DIR) -> list[Seed]:
    """Every seed under ``root``, ordered by concept, then name."""
    seeds = []
    for concept_dir in sorted(p for p in root.iterdir() if p.is_dir()):
        names = sorted(p.stem for p in concept_dir.glob("*.toml"))
        seeds.extend(load_seed(concept_dir, name) for name in names)
        owned = set()
        for seed in seeds[len(seeds) - len(names) :]:
            owned.add(f"{seed.name}.toml")
            owned.add(f"{seed.name}.tuo")
            if seed.heldout_specs:
                owned.add(f"{seed.name}.heldout.tuo")
            owned.update(f"{seed.name}.mutant{n}.tuo" for n in range(1, len(seed.mutants) + 1))
        stray = sorted(p.name for p in concept_dir.iterdir() if p.name not in owned)
        if stray:
            raise SeedError(f"{concept_dir}: file(s) belonging to no seed: {', '.join(stray)}")
    return seeds


def attach_attempts(seeds: list[Seed], root: Path = LIVE_DIR) -> list[Seed]:
    """``seeds``, each with the live attempts recorded under ``root``."""
    if not root.is_dir():
        return seeds
    by_key: dict[str, list[tuple[str, str]]] = {}
    for run in sorted(p for p in root.iterdir() if p.is_dir()):
        if not (run / "run.json").is_file():
            raise SeedError(f"{run}: a live run without run.json")
        for path in sorted(run.glob("*/*.tuo")):
            by_key.setdefault(f"{path.parent.name}/{path.stem}", []).append((run.name, path.read_text()))
        stray = sorted(
            str(p.relative_to(run))
            for p in run.rglob("*")
            if p.is_file() and p.name != "run.json" and not (p.suffix == ".tuo" and p.parent.parent == run)
        )
        if stray:
            raise SeedError(f"{run}: file(s) that are not attempts: {', '.join(stray)}")
    known = {f"{seed.concept}/{seed.name}" for seed in seeds}
    orphans = sorted(set(by_key) - known)
    if orphans:
        raise SeedError(f"{root}: attempt(s) at no seed's task: {', '.join(orphans)}")
    return [replace(seed, attempts=tuple(by_key.get(f"{seed.concept}/{seed.name}", ()))) for seed in seeds]


SEEDS: list[Seed] = attach_attempts(load_seeds())
EVAL_SEEDS: list[Seed] = load_seeds(EVAL_DIR / "seeds") if EVAL_DIR else []
