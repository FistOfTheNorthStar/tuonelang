#!/usr/bin/env python3
"""Score a model's completions on the held-out eval set — through the real compiler.

This is the honest scorer for a fine-tuned tuonelang model. It never trusts a
self-reported verdict: it extracts the ```tuo code block from each model
completion, compiles it with the real ``tuo`` compiler, runs its specs, and (for
runnable tasks) runs it natively and checks the exit byte. The metrics are the
ones the project's own harnesses use:

  * Compile@1  — fraction whose ``tuo check`` passes.
  * SpecPass@1 — fraction whose ``tuo spec`` runs every spec green.
  * Run@1      — fraction of runnable tasks whose native exit byte matches.

Input: a JSONL file where each line is ``{"task": ..., "completion": "..."}``
matching a task in ``eval_heldout.jsonl`` (matched by exact ``task`` string). A
completion may be the raw assistant text; the scorer pulls the fenced ```tuo
block out of it.

Usage:
    python3 training/score_eval.py completions.jsonl
    TUO_BIN=target/debug/tuo python3 training/score_eval.py completions.jsonl

To sanity-check the harness itself, score the reference solutions (they must hit
100% on every metric):
    python3 training/score_eval.py --reference
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
# Reuse the exact compiler-invocation helpers the generator validates seeds with.
from generate import check_ok, run_exit, spec_ok  # noqa: E402

EVAL = HERE / "dataset" / "eval_heldout.jsonl"

_FENCE = re.compile(r"```(?:tuo|tuonelang)?\s*\n(.*?)```", re.DOTALL)


def extract_code(completion: str) -> str:
    """Pull the first fenced code block out of a completion, or return it whole."""
    m = _FENCE.search(completion)
    return (m.group(1) if m else completion).strip() + "\n"


def load_eval() -> dict[str, dict]:
    """Map each held-out task string to its record."""
    tasks = {}
    for line in EVAL.read_text().splitlines():
        line = line.strip()
        if line:
            rec = json.loads(line)
            tasks[rec["task"]] = rec
    return tasks


def score_one(code: str, rec: dict) -> dict:
    """Compile/spec/run one candidate against its held-out task record."""
    deps = tuple(rec.get("stdlib_deps", []))
    result = {"compile": False, "spec": False, "run": None}
    ok, _ = check_ok(code, deps)
    result["compile"] = ok
    if not ok:
        return result
    ok, _ = spec_ok(code, deps)
    result["spec"] = ok
    if rec.get("runnable"):
        code_exit, _ = run_exit(code, deps)
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
    args = ap.parse_args()

    tasks = load_eval()

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
    n_run_ok = n_runnable = 0
    for rec, code in pairs:
        r = score_one(code, rec)
        n_compile += int(r["compile"])
        n_spec += int(r["spec"])
        if r["run"] is not None:
            n_runnable += 1
            n_run_ok += int(r["run"])
        mark = "ok " if r["spec"] else "ERR"
        print(f"  [{mark}] {rec['concept']:22s} {rec['task'][:52]}")

    print(f"\nScored {n} completions:")
    print(f"  Compile@1  {n_compile}/{n} = {n_compile / n:.1%}")
    print(f"  SpecPass@1 {n_spec}/{n} = {n_spec / n:.1%}")
    if n_runnable:
        print(f"  Run@1      {n_run_ok}/{n_runnable} = {n_run_ok / n_runnable:.1%}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
