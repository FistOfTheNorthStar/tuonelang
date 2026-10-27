#!/usr/bin/env python3
"""What the training material may not overlap, and the checks that enforce it.

``similarity.py`` measures how alike two programs or tasks are; this module
decides which pairs are a problem. There are three:

* **An eval task that resembles training material, a public benchmark task,
  or another eval task.** A held-out score on it would measure recall, not
  skill. ``generate.py`` refuses to emit.
* **A training seed that resembles a public codegen-benchmark task.** A
  model fine-tuned on it would score on that benchmark by recall. Refused
  here and in ``generate.py``.
* **Two training seeds that are the same program.** The copy adds no
  coverage and doubles the weight of one solution. Refused. A *harvested*
  example that copies a seed, or an earlier harvested example, is instead
  dropped by ``generate.py`` and counted in ``stats.json``, since the
  harvest is bulk material nobody wrote for the dataset.

The thresholds come from measuring the material as it stood on 2026-10-25:
renamed copies scored 0.90 to 1.00; the most similar pairs of genuinely
different programs (two loops over an array with different bodies, two
if/else chains) scored up to 0.76; and no two task texts scored above 0.22.
The eval threshold is the strictest because an eval task is the most
expensive thing to contaminate.

Run it directly to check the public material (CI does), or to review the most
similar pairs before adding seeds::

    python3 training/overlap.py            # refuse on any overlap above
    python3 training/overlap.py --top 10   # ...and list the closest pairs

With ``TUO_EVAL_DIR`` set it checks the private eval seeds too. Never paste
that output anywhere public: it names eval tasks.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent
sys.path.insert(0, str(HERE))
from harvest import Harvested, harvest_all  # noqa: E402
from seeds import EVAL_SEEDS, SEEDS, Seed  # noqa: E402
from similarity import Item, Match, jaccard, near_duplicates, program_similarity  # noqa: E402

EVAL_PROGRAM = 0.7
"""An eval solution this similar to any other material is refused."""

BENCH_PROGRAM = 0.8
"""A training program this similar to a benchmark reference is refused."""

TRAIN_PROGRAM = 0.9
"""Two training programs this similar are the same program."""

TASK = 0.5
"""Two task texts this similar are the same task, whatever the programs."""

BENCH = REPO / "benchmarks" / "llm" / "codegen"


def seed_item(seed: Seed, prefix: str) -> Item:
    return Item(f"{prefix}:{seed.concept}/{seed.name}", seed.task, seed.solution)


def harvest_item(item: Harvested) -> Item:
    return Item(f"harvest:{item.name}", item.task, item.solution)


def bench_items() -> list[Item]:
    """Every public codegen-benchmark task, with its reference solution when
    it has one."""
    items = []
    for path in sorted((BENCH / "tasks").glob("*.json")):
        for entry in json.loads(path.read_text())["tasks"]:
            task = entry["task"]
            ref = next(BENCH.glob(f"sources/*/{task['id']}/reference.tuo"), None)
            program = ref.read_text() if ref else ""
            items.append(Item(f"benchmark:{task['id']}", task["instruction"], program))
    return items


def bench_functions() -> dict[str, str]:
    """The function each public codegen-benchmark task asks for (the first
    signature its instruction names), mapped to the task."""
    names = {}
    for instruction in sorted(BENCH.glob("sources/*/*/instruction.md")):
        m = re.search(r"`(?:fn )?([a-z_][a-z_0-9]*)\s*[\[(]", instruction.read_text())
        if m:
            names.setdefault(m.group(1), instruction.parent.name)
    return names


def eval_contamination(
    eval_: list[Seed], train: list[Seed], harvested: list[Harvested]
) -> list[str]:
    """Every reason an eval seed's score would measure recall: its task or
    solution is close to training material, a benchmark task, or another
    eval task; or it is named after the same function as a training seed of
    its concept or a benchmark task. (The name test is coarse on purpose;
    two seeds written apart still converge on `parse_port` when the idea is
    the obvious one.)"""
    found = []
    names = {(s.concept, s.name) for s in train if s.name != "program"}
    bench = bench_functions()
    for seed in eval_:
        label = f"{seed.concept}/{seed.name}"
        if seed.name in bench:
            found.append(
                f"{label}: the public codegen-benchmark task "
                f"`{bench[seed.name]}` is about the same function"
            )
        if (seed.concept, seed.name) in names:
            found.append(f"{label}: a training seed of the same concept is about the same function")

    items = [seed_item(s, "eval") for s in eval_]
    others = (
        [seed_item(s, "seed") for s in train]
        + [harvest_item(h) for h in harvested]
        + bench_items()
    )
    matches = near_duplicates(items, others, EVAL_PROGRAM, TASK)
    matches += near_duplicates(items, None, EVAL_PROGRAM, TASK)
    found += [str(m) for m in matches]
    return found


def benchmark_overlap(train: list[Seed], harvested: list[Harvested]) -> list[Match]:
    """Training material close to a public benchmark task."""
    items = [seed_item(s, "seed") for s in train] + [harvest_item(h) for h in harvested]
    return near_duplicates(items, bench_items(), BENCH_PROGRAM, TASK)


def seed_duplicates(train: list[Seed]) -> list[Match]:
    """Pairs of training seeds that are the same program. Their tasks are
    not compared: seeds of one concept share its vocabulary."""
    return near_duplicates([seed_item(s, "seed") for s in train], None, TRAIN_PROGRAM, None)


def harvest_duplicates(train: list[Seed], harvested: list[Harvested]) -> list[int]:
    """The indices into ``harvested`` of each example whose program copies a
    seed or an earlier harvested example."""
    kept = [seed_item(s, "seed") for s in train]
    dropped = []
    for i, h in enumerate(harvested):
        item = harvest_item(h)
        if any(program_similarity(item, k) >= TRAIN_PROGRAM for k in kept):
            dropped.append(i)
        else:
            kept.append(item)
    return dropped


def closest(items: list[Item], others: list[Item] | None, n: int) -> list[str]:
    """The ``n`` most similar pairs, by program or by task, for review."""
    pairs = (
        ((a, b) for a in items for b in others)
        if others is not None
        else ((a, b) for i, a in enumerate(items) for b in items[i + 1 :])
    )
    scored = sorted(
        (
            (max(program_similarity(a, b), jaccard(a.task_fp, b.task_fp)), a.label, b.label)
            for a, b in pairs
        ),
        reverse=True,
    )
    return [f"{score:.2f}  {a} ~ {b}" for score, a, b in scored[:n]]


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawTextHelpFormatter)
    ap.add_argument("--top", type=int, default=0, help="also list the N closest pairs")
    args = ap.parse_args()

    harvested = harvest_all(REPO)
    problems = [f"benchmark overlap: {m}" for m in benchmark_overlap(SEEDS, harvested)]
    problems += [f"duplicate seeds: {m}" for m in seed_duplicates(SEEDS)]
    if EVAL_SEEDS:
        problems += [f"eval contamination: {p}" for p in eval_contamination(EVAL_SEEDS, SEEDS, harvested)]

    if args.top:
        seeds = [seed_item(s, "seed") for s in SEEDS]
        harvest = [harvest_item(h) for h in harvested]
        sections = {
            "training seeds": (seeds, None),
            "training material vs benchmark": (seeds + harvest, bench_items()),
        }
        if EVAL_SEEDS:
            ev = [seed_item(s, "eval") for s in EVAL_SEEDS]
            sections["eval vs everything else"] = (ev, seeds + harvest + bench_items())
        for title, (items, others) in sections.items():
            print(f"Closest pairs, {title}:")
            for line in closest(items, others, args.top):
                print(f"  {line}")

    n_eval = f" and {len(EVAL_SEEDS)} eval seeds" if EVAL_SEEDS else ""
    print(
        f"Checked {len(SEEDS)} training seeds{n_eval} against {len(harvested)} harvested "
        f"examples and {len(bench_items())} benchmark tasks."
    )
    if problems:
        print(f"{len(problems)} overlap(s) refused:")
        for p in problems:
            print(f"  {p}")
        return 1
    print("No overlap above the thresholds.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
