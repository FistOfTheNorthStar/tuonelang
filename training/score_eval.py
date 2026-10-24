#!/usr/bin/env python3
"""Score a model's completions on the held-out eval set — through the real compiler.

This is the honest scorer for a fine-tuned tuonelang model. It never trusts a
self-reported verdict: it extracts the ```tuo code block from each model
completion, compiles it with the real ``tuo`` compiler, runs its specs, and (for
runnable tasks) runs it natively and checks the exit byte. The metrics are the
ones the project's own harnesses use:

  * Compile@1  — fraction whose ``tuo check`` passes.
  * SpecPass@1 — fraction whose ``tuo spec`` runs every spec green.
  * TestPass@1 — fraction of tasks with held-out specs whose held-out specs
    also pass (behaviour the prompt did not spell out).
  * Run@1      — fraction of runnable tasks whose native exit byte matches.

Every metric is also reported per concept: the eval split holds out a share of
each concept's seeds, so an overall number can be read area by area.

Input: a JSONL file where each line is ``{"task": ..., "completion": "..."}``
matching a task in the eval set (matched by exact ``task`` string). A
completion may be the raw assistant text; the scorer pulls the fenced ```tuo
block out of it.

The eval set is private (see ``seeds.py``): it is read from
``$TUO_EVAL_DIR/dataset/eval_heldout.jsonl``, which ``generate.py`` writes, or
from ``--eval PATH``. The scorer prints the set's SHA-256, which the public
``training/dataset/manifest.json`` records, so a score names the set it was
measured on.

Usage:
    TUO_EVAL_DIR=../tuonelang-eval python3 training/score_eval.py completions.jsonl
    TUO_BIN=target/debug/tuo python3 training/score_eval.py --eval set.jsonl completions.jsonl

To sanity-check the harness itself, score the reference solutions (they must hit
100% on every metric):
    python3 training/score_eval.py --reference
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
# Reuse the exact compiler-invocation helpers the generator validates seeds with.
from generate import check_ok, run_exit, spec_ok  # noqa: E402
from seeds import EVAL_DIR  # noqa: E402

_FENCE = re.compile(r"```(?:tuo|tuonelang)?\s*\n(.*?)```", re.DOTALL)


def extract_code(completion: str) -> str:
    """Pull the first fenced code block out of a completion, or return it whole."""
    m = _FENCE.search(completion)
    return (m.group(1) if m else completion).strip() + "\n"


def load_eval(path: Path) -> dict[str, dict]:
    """Map each held-out task string to its record."""
    tasks = {}
    for line in path.read_text().splitlines():
        line = line.strip()
        if line:
            rec = json.loads(line)
            tasks[rec["task"]] = rec
    return tasks


def score_one(code: str, rec: dict) -> dict:
    """Compile/spec/test/run one candidate against its held-out task record."""
    result = {"compile": False, "spec": False, "test": None, "run": None}
    ok, _ = check_ok(code)
    result["compile"] = ok
    if not ok:
        return result
    ok, _ = spec_ok(code)
    result["spec"] = ok
    heldout = rec.get("heldout_specs") or ""
    if heldout:
        ok, _ = spec_ok(code.rstrip() + "\n\n" + heldout)
        result["test"] = ok
    if rec.get("runnable"):
        code_exit, _ = run_exit(code)
        result["run"] = code_exit == rec.get("run_exit")
    return result


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "completions",
        nargs="?",
        help="JSONL of {task, completion}; omit with --reference",
    )
    ap.add_argument(
        "--reference",
        action="store_true",
        help="score the eval set's own reference solutions (expect 100 pct)",
    )
    ap.add_argument(
        "--eval",
        type=Path,
        help="the eval set (default: $TUO_EVAL_DIR/dataset/eval_heldout.jsonl)",
    )
    args = ap.parse_args()

    eval_path = args.eval or (EVAL_DIR / "dataset" / "eval_heldout.jsonl" if EVAL_DIR else None)
    if eval_path is None:
        ap.error("the eval set is private: set TUO_EVAL_DIR or pass --eval")
    if not eval_path.is_file():
        ap.error(f"no eval set at {eval_path}; run generate.py with TUO_EVAL_DIR set")
    tasks = load_eval(eval_path)
    print(f"Eval set {eval_path} (sha256 {hashlib.sha256(eval_path.read_bytes()).hexdigest()})")

    pairs: list[tuple[dict, str]] = []
    if args.reference:
        for rec in tasks.values():
            pairs.append((rec, rec["reference_solution"]))
    else:
        if not args.completions:
            ap.error("provide a completions file or --reference")
        for line in Path(args.completions).read_text().splitlines():
            line = line.strip()
            if not line:
                continue
            got = json.loads(line)
            rec = tasks.get(got["task"])
            if rec is None:
                print(f"  [skip] no eval task matches: {got['task'][:60]}")
                continue
            pairs.append((rec, extract_code(got["completion"])))

    n = len(pairs)
    if n == 0:
        print("No scorable completions.")
        return 1

    n_compile = n_spec = 0
    n_test_ok = n_tested = 0
    n_run_ok = n_runnable = 0
    by_concept: dict[str, list[int]] = {}
    for rec, code in pairs:
        r = score_one(code, rec)
        n_compile += int(r["compile"])
        n_spec += int(r["spec"])
        if r["test"] is not None:
            n_tested += 1
            n_test_ok += int(r["test"])
        if r["run"] is not None:
            n_runnable += 1
            n_run_ok += int(r["run"])
        row = by_concept.setdefault(rec["concept"], [0, 0, 0])
        row[0] += 1
        row[1] += int(r["compile"])
        row[2] += int(r["spec"])
        mark = "ok " if r["spec"] else "ERR"
        print(f"  [{mark}] {rec['concept']:22s} {rec['task'][:52]}")

    print(f"\nScored {n} completions:")
    print(f"  Compile@1  {n_compile}/{n} = {n_compile / n:.1%}")
    print(f"  SpecPass@1 {n_spec}/{n} = {n_spec / n:.1%}")
    if n_tested:
        print(f"  TestPass@1 {n_test_ok}/{n_tested} = {n_test_ok / n_tested:.1%}")
    if n_runnable:
        print(f"  Run@1      {n_run_ok}/{n_runnable} = {n_run_ok / n_runnable:.1%}")
    print("\nBy concept (tasks, Compile@1, SpecPass@1):")
    for concept, (total, compiled, specced) in sorted(by_concept.items()):
        print(f"  {concept:24s} {total:3d}  {compiled}/{total}  {specced}/{total}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
