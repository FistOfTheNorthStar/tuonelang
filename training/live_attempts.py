#!/usr/bin/env python3
"""Record a live model's failed attempts at the training tasks.

The repair transcripts ``generate.py`` makes from ``breaks.py`` show mistakes
a person predicted. A model's own mistakes are the ones it will make again,
so this asks a model (any OpenAI-compatible endpoint, as ``run_eval.py``
does) for every public training seed's task, and keeps each attempt the
compiler has feedback on: one that does not check, or whose own specs fail.
``generate.py`` turns each into a repair transcript: the attempt, the real
compiler output, then the seed's reference solution.

Only training seeds are asked, never the private eval tasks: a transcript of
a model's attempt at an eval task would teach the eval set.

An attempt is kept as source, ``training/live/<run>/<concept>/<name>.tuo``,
with ``training/live/<run>/run.json`` recording the model configuration and
the counts. Model output is not reproducible from a commit; the attempts
are, so ``generate.py`` still produces the same dataset from the same tree.
It recompiles every attempt, and drops one the compiler no longer has
feedback on.

Usage:
    TUO_BIN=target/release/tuo python3 training/live_attempts.py --model qwen3-coder:30b
    python3 training/live_attempts.py --model qwen3-coder:30b --prime
    python3 training/live_attempts.py --model gemma4:latest --reasoning-effort none --prime
"""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
import time
import urllib.error
from datetime import datetime, timezone
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from generate import REPO, SYSTEM_PROMPT, check_ok, spec_failure, tuo_cmd  # noqa: E402
from run_eval import BRIEF_PREAMBLE, SEED, ask, brief, slug  # noqa: E402
from score_eval import extract_code  # noqa: E402
from seeds import SEEDS  # noqa: E402

LIVE = HERE / "live"
"""Where the recorded attempts live, one directory per run."""


def has_feedback(code: str) -> bool:
    """Does the compiler have something to say about ``code``: a front-end
    error, or a spec of its own that fails?"""
    ok, _ = check_ok(code)
    return not ok or spec_failure(code) is not None


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--model", required=True, help="the model name the endpoint serves")
    ap.add_argument("--endpoint", default="http://localhost:11434/v1", help="OpenAI-compatible base URL (default: local Ollama)")
    ap.add_argument("--api-key-env", help="environment variable holding a bearer token")
    ap.add_argument("--prime", action="store_true", help="append the language brief to the system prompt")
    ap.add_argument("--temperature", type=float, default=0.0)
    ap.add_argument("--max-tokens", type=int, help="cap on generated tokens (a reasoning model needs one)")
    ap.add_argument("--reasoning-effort", choices=["none", "low", "medium", "high"])
    ap.add_argument("--timeout", type=float, default=1800, help="seconds per request")
    ap.add_argument("--run", help="the run's directory name (default: <model>[-primed])")
    ap.add_argument("--limit", type=int, default=0, help="first N seeds only")
    args = ap.parse_args()

    name = args.run or f"{slug(args.model)}{'-primed' if args.prime else ''}"
    out = LIVE / name
    if out.exists():
        ap.error(f"{out.relative_to(REPO)} exists; remove it to record the run again")
    system = SYSTEM_PROMPT + (BRIEF_PREAMBLE + brief() if args.prime else "")
    seeds = SEEDS[: args.limit] if args.limit else SEEDS

    counts = {"asked": 0, "kept": 0, "passed": 0, "empty": 0, "errors": 0}
    attempts: dict[str, str] = {}
    started = datetime.now(timezone.utc).isoformat(timespec="seconds")
    print(f"{args.model}{' (primed)' if args.prime else ''}: {len(seeds)} training tasks -> {out.relative_to(REPO)}")
    for index, seed in enumerate(seeds, 1):
        counts["asked"] += 1
        began = time.monotonic()
        try:
            completion = ask(args, system, seed.task)["completion"]
        except (urllib.error.URLError, TimeoutError) as error:
            counts["errors"] += 1
            print(f"  [{index:3d}/{len(seeds)}] {seed.concept}/{seed.name}: request failed: {error}")
            continue
        code = extract_code(completion)
        if not code.strip():
            verdict = "empty"
        elif has_feedback(code):
            verdict = "kept"
            attempts[f"{seed.concept}/{seed.name}"] = code
        else:
            verdict = "passed"
        counts[verdict] += 1
        print(f"  [{index:3d}/{len(seeds)}] {seed.concept + '/' + seed.name:40s} {time.monotonic() - began:6.1f}s {verdict}")

    for key, code in attempts.items():
        path = out / f"{key}.tuo"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(code)
    compiler = subprocess.run(tuo_cmd() + ["--version"], cwd=REPO, capture_output=True, text=True).stdout.strip()
    meta = {
        "model": args.model,
        "endpoint": args.endpoint,
        "temperature": args.temperature,
        "seed": SEED,
        "primed": args.prime,
        "max_tokens": args.max_tokens,
        "reasoning_effort": args.reasoning_effort,
        "system_sha256": hashlib.sha256(system.encode()).hexdigest(),
        "compiler": compiler,
        "started": started,
        "finished": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        "counts": counts,
    }
    out.mkdir(parents=True, exist_ok=True)
    (out / "run.json").write_text(json.dumps(meta, indent=2, sort_keys=True) + "\n")
    print(f"\n{counts['kept']} attempts kept of {counts['asked']} asked: " + ", ".join(f"{k} {v}" for k, v in counts.items()))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
