#!/usr/bin/env python3
"""Ask a model for a completion of every held-out eval task.

This produces the completions file ``score_eval.py`` scores. Each task is sent
once, as the training data would send it: the record's own system prompt and
the task as the user turn. Requests go to any OpenAI-compatible
``/v1/chat/completions`` endpoint (Ollama by default), at temperature 0 with
the same fixed seed ``tuo bench run`` sends, so a model that can be
deterministic is.

``--prime`` appends the compiler-generated language brief (``tuo cheatsheet``)
to the system prompt, worded exactly as ``tuo bench run --prime`` words it.
The same model with and without the brief is the gap a fine-tune has to close.

The completions repeat each task, so they are as private as the eval set:
the output goes to ``$TUO_EVAL_DIR/results/`` by default, and a path inside
this public repository is refused. Next to it, ``<out>.meta.json`` records the
model configuration, the system prompt's SHA-256 and the eval set's SHA-256.

Usage:
    TUO_EVAL_DIR=../tuonelang-eval python3 training/run_eval.py --model qwen3-coder:30b
    python3 training/run_eval.py --model qwen3-coder:30b --prime
    python3 training/run_eval.py --endpoint https://host/v1 --api-key-env KEY --model NAME
    TUO_EVAL_DIR=../tuonelang-eval python3 training/score_eval.py \\
        "$TUO_EVAL_DIR/results/qwen3-coder-30b.jsonl"

A run that stops part way can be continued with ``--resume``: tasks already in
the output file are not asked again.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from generate import REPO, tuo_cmd  # noqa: E402
from score_eval import load_eval  # noqa: E402
from seeds import EVAL_DIR  # noqa: E402

SEED = 7
"""The sampling seed ``tuo bench run`` sends (``crates/tuo-cli/src/bench.rs``)."""

BRIEF_PREAMBLE = (
    "\n\nThe following is the tuonelang language brief, generated from the "
    "compiler. It is the authority on syntax and on which standard-library "
    "functions exist.\n\n"
)
"""How ``tuo bench run --prime`` introduces the brief, word for word."""


def brief() -> str:
    """The language brief, as ``tuo cheatsheet`` prints it."""
    p = subprocess.run(tuo_cmd() + ["cheatsheet"], cwd=str(REPO), capture_output=True, text=True)
    if p.returncode != 0:
        raise SystemExit(f"tuo cheatsheet failed: {p.stderr.strip()}")
    return p.stdout


def slug(model: str) -> str:
    """A file name for a model: ``qwen3-coder:30b`` becomes ``qwen3-coder-30b``."""
    return re.sub(r"[^A-Za-z0-9.]+", "-", model).strip("-")


def ask(args: argparse.Namespace, system: str, task: str) -> dict:
    """One chat completion; returns the reply and the server's token counts."""
    body = {
        "model": args.model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": task},
        ],
        "temperature": args.temperature,
        "seed": SEED,
        "stream": False,
    }
    if args.max_tokens:
        body["max_tokens"] = args.max_tokens
    if args.reasoning_effort:
        body["reasoning_effort"] = args.reasoning_effort
    headers = {"Content-Type": "application/json"}
    if args.api_key_env:
        key = os.environ.get(args.api_key_env)
        if not key:
            raise SystemExit(f"${args.api_key_env} is not set")
        headers["Authorization"] = f"Bearer {key}"
    request = urllib.request.Request(
        args.endpoint.rstrip("/") + "/chat/completions",
        data=json.dumps(body).encode(),
        headers=headers,
    )
    with urllib.request.urlopen(request, timeout=args.timeout) as response:
        reply = json.loads(response.read())
    choice = reply["choices"][0]
    return {
        "completion": choice["message"].get("content") or "",
        "finish_reason": choice.get("finish_reason"),
        "usage": reply.get("usage") or {},
    }


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
    ap.add_argument("--eval", type=Path, help="the eval set (default: $TUO_EVAL_DIR/dataset/eval_heldout.jsonl)")
    ap.add_argument("-o", "--out", type=Path, help="completions file (default: $TUO_EVAL_DIR/results/<model>[-primed].jsonl)")
    ap.add_argument("--resume", action="store_true", help="keep the tasks already in the output file")
    args = ap.parse_args()

    if EVAL_DIR is None and (args.eval is None or args.out is None):
        ap.error("the eval set is private: set TUO_EVAL_DIR, or pass both --eval and --out")
    eval_path = args.eval or EVAL_DIR / "dataset" / "eval_heldout.jsonl"
    if not eval_path.is_file():
        ap.error(f"no eval set at {eval_path}; run generate.py with TUO_EVAL_DIR set")
    out = args.out or EVAL_DIR / "results" / f"{slug(args.model)}{'-primed' if args.prime else ''}.jsonl"
    out = out.resolve()
    if out.is_relative_to(REPO):
        ap.error(f"{out} is inside the public repository; completions repeat the private tasks")
    if out.exists() and not args.resume:
        ap.error(f"{out} exists; pass --resume to continue it or remove it")

    tasks = load_eval(eval_path)
    extra = brief() if args.prime else ""
    done: set[str] = set()
    if args.resume and out.exists():
        done = {json.loads(line)["task"] for line in out.read_text().splitlines() if line.strip()}

    systems = {rec["system"] + (BRIEF_PREAMBLE + extra if extra else "") for rec in tasks.values()}
    meta = {
        "model": args.model,
        "endpoint": args.endpoint,
        "temperature": args.temperature,
        "seed": SEED,
        "primed": args.prime,
        "max_tokens": args.max_tokens,
        "reasoning_effort": args.reasoning_effort,
        "system_sha256": sorted(hashlib.sha256(s.encode()).hexdigest() for s in systems),
        "eval_sha256": hashlib.sha256(eval_path.read_bytes()).hexdigest(),
        "tasks": len(tasks),
        "started": datetime.now(timezone.utc).isoformat(timespec="seconds"),
    }
    out.parent.mkdir(parents=True, exist_ok=True)
    print(f"{args.model}{' (primed)' if args.prime else ''}: {len(tasks) - len(done)} of {len(tasks)} tasks -> {out}")

    with out.open("a") as sink:
        for index, rec in enumerate(tasks.values(), 1):
            if rec["task"] in done:
                continue
            system = rec["system"] + (BRIEF_PREAMBLE + extra if extra else "")
            started = time.monotonic()
            try:
                got = ask(args, system, rec["task"])
            except (urllib.error.URLError, TimeoutError) as error:
                # Recorded as an empty completion, which scores as a failure:
                # a task the model could not answer in time is not skipped.
                got = {"completion": "", "finish_reason": f"error: {error}", "usage": {}}
            seconds = round(time.monotonic() - started, 1)
            sink.write(json.dumps({"task": rec["task"], **got, "seconds": seconds}) + "\n")
            sink.flush()
            print(f"  [{index:3d}/{len(tasks)}] {rec['concept']:22s} {seconds:7.1f}s {got['finish_reason']}")

    meta["finished"] = datetime.now(timezone.utc).isoformat(timespec="seconds")
    out.with_suffix(".meta.json").write_text(json.dumps(meta, indent=2) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
