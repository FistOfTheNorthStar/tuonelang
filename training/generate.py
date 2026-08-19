#!/usr/bin/env python3
"""Compiler-validated fine-tuning data generator for tuonelang.

Every example this emits is verified by the *real* ``tuo`` compiler before it is
written — nothing is trusted on assertion, mirroring how the project's corpus and
benchmark harnesses refuse to record a metric the compiler did not produce.

Outputs (under ``training/dataset/``):

  * ``sft_oneshot.jsonl`` — task -> correct program (chat SFT format).
  * ``sft_repair.jsonl``  — task -> buggy program -> REAL compiler diagnostic ->
    corrected program (multi-turn, teaches the TDG feedback loop).
  * ``eval_heldout.jsonl`` — a held-out slice (by concept hash) never emitted
    into the SFT sets, for scoring Compile@1 / SpecPass@1 with the real compiler.
  * ``stats.json`` — coverage and validation counts.

Usage:
    python3 training/generate.py            # validate seeds, emit all datasets
    python3 training/generate.py --check    # validate only; emit nothing
    python3 training/generate.py --limit 5  # first N seeds (smoke test)

The generator shells out to ``cargo run -p tuo-cli`` by default; set
``TUO_BIN=/path/to/tuo`` to use a prebuilt binary (much faster). Build one with
``cargo build -p tuo-cli`` and point ``TUO_BIN`` at ``target/debug/tuo``.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import sys
import tempfile
from dataclasses import asdict
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent
OUT = HERE / "dataset"

sys.path.insert(0, str(HERE))
from breaks import BREAKS  # noqa: E402
from seeds import SEEDS, Seed  # noqa: E402

SYSTEM_PROMPT = (
    "You are a professional tuonelang programmer. tuonelang is a statically "
    "typed, memory-safe, natively compiled language built for Test Driven "
    "Generation (TDG): every function is accompanied by colocated executable "
    "`spec` blocks. Write only code that the tuonelang v0 compiler accepts and "
    "runs. Key rules: mutable bindings use `var` (not `let mut`); assignment is "
    "plain `=` (no `+=`); imports use `import` (not `use`); generics use square "
    "brackets `Option[Int]` (not `<>`); Option/Result payloads are named fields "
    "`Some { value: x }` / `Ok { value: x }` / `Err { error: e }`; every "
    "parameter has a mode (`take`/`in`/`mut`) and a type; there are no negative "
    "literals (`-5` is unary minus); comparisons do not chain; integer overflow "
    "traps; and there are no methods — call free functions like `area(r)`, never "
    "`r.area()`. Prefer the standard library's one obvious API per task."
)


def tuo_cmd() -> list[str]:
    """Return the base command for invoking the compiler."""
    prebuilt = os.environ.get("TUO_BIN")
    if prebuilt:
        return [prebuilt]
    return ["cargo", "run", "--quiet", "-p", "tuo-cli", "--"]


STDLIB_DIR = REPO / "crates" / "tuo-stdlib" / "src" / "std"


def stdlib_dep_paths(deps: tuple[str, ...]) -> list[str]:
    """Resolve stdlib module names (``"core"``) to their source-file paths.

    tuonelang's standard library is consumed as input: a program importing
    ``std::core`` is compiled together with ``std/core.tuo``. These committed
    catalog files are the single source of record, so the training data always
    reflects the exact stdlib the compiler ships.
    """
    paths = []
    for dep in deps:
        p = STDLIB_DIR / f"{dep}.tuo"
        if not p.is_file():
            raise FileNotFoundError(f"stdlib module source not found: {p}")
        paths.append(str(p))
    return paths


def _run(
    args: list[str], text: str, deps: tuple[str, ...] = ()
) -> subprocess.CompletedProcess[str]:
    """Write ``text`` to a temp ``.tuo`` file and run ``tuo <args> <file> <deps>``.

    ``deps`` names stdlib modules whose source is passed alongside the program,
    since the standard library is compiled as input rather than linked in.
    """
    with tempfile.NamedTemporaryFile(
        "w", suffix=".tuo", dir=str(OUT), delete=False
    ) as f:
        f.write(text)
        path = f.name
    try:
        return subprocess.run(
            tuo_cmd() + args + [path] + stdlib_dep_paths(deps),
            cwd=str(REPO),
            capture_output=True,
            text=True,
            timeout=180,
        )
    finally:
        os.unlink(path)


def check_ok(text: str, deps: tuple[str, ...] = ()) -> tuple[bool, str]:
    """True if ``tuo check`` accepts ``text``; else (False, stderr+stdout)."""
    p = _run(["check"], text, deps)
    return p.returncode == 0, (p.stderr + p.stdout).strip()


def spec_ok(text: str, deps: tuple[str, ...] = ()) -> tuple[bool, str]:
    """True if ``tuo spec`` runs every spec green."""
    p = _run(["spec"], text, deps)
    out = (p.stdout + p.stderr).strip()
    return p.returncode == 0 and "0 failed" in out, out


def run_exit(text: str, deps: tuple[str, ...] = ()) -> tuple[int | None, str]:
    """Compile+run natively; return the process exit byte (or None on failure)."""
    p = _run(["run"], text, deps)
    # `tuo run` propagates the program's own exit status. A codegen/link error is
    # a nonzero tuo error with a message on stderr; distinguish by stderr noise.
    return p.returncode, (p.stderr + p.stdout).strip()


def first_diagnostic(text: str, deps: tuple[str, ...] = ()) -> dict | None:
    """Run ``tuo --message-format=json check`` and return the first diagnostic."""
    with tempfile.NamedTemporaryFile(
        "w", suffix=".tuo", dir=str(OUT), delete=False
    ) as f:
        f.write(text)
        path = f.name
    try:
        p = subprocess.run(
            tuo_cmd() + ["--message-format=json", "check", path]
            + stdlib_dep_paths(deps),
            cwd=str(REPO),
            capture_output=True,
            text=True,
            timeout=180,
        )
    finally:
        os.unlink(path)
    for line in (p.stdout or "").splitlines():
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            envelope = json.loads(line)
        except json.JSONDecodeError:
            continue
        for event in envelope.get("events", []):
            if event.get("event") == "diagnostic":
                return event.get("diagnostic")
    return None


def render_diagnostic(diag: dict) -> str:
    """Render a compiler diagnostic the way a terminal/agent tool would show it."""
    code = diag.get("code", "?")
    sev = diag.get("severity", "error")
    msg = diag.get("message", "")
    span = diag.get("primary_span", {})
    lc = span.get("start_line_col", {})
    loc = f"{lc.get('line', '?')}:{lc.get('column', '?')}"
    label = diag.get("primary_label", "")
    line = f"{sev}[{code}]: {msg}"
    if label:
        line += f"\n  --> {loc}: {label}"
    help_text = diag.get("help")
    if help_text:
        line += f"\n  help: {help_text}"
    return line


def held_out(seed: Seed, ratio: int = 5) -> bool:
    """Deterministically hold out ~1/ratio of seeds for eval, stable per task."""
    h = int(hashlib.sha256(seed.task.encode()).hexdigest(), 16)
    return (h % ratio) == 0


def chat(system: str, user: str, assistant: str) -> dict:
    """A single-turn chat SFT record."""
    return {
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user},
            {"role": "assistant", "content": assistant},
        ]
    }


def repair_chat(system: str, user: str, bad: str, diag: str, fixed: str) -> dict:
    """A multi-turn repair record with a compiler-tool turn between the two
    assistant turns."""
    return {
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user},
            {"role": "assistant", "content": bad},
            {
                "role": "tool",
                "name": "tuo_compiler",
                "content": f"$ tuo check program.tuo\n{diag}",
            },
            {"role": "assistant", "content": fixed},
        ]
    }


def assistant_answer(seed: Seed) -> str:
    """The assistant's one-shot answer: the program, plus a short note if any."""
    body = "```tuo\n" + seed.solution.rstrip() + "\n```"
    if seed.stdlib_deps:
        deps = " ".join(f"std/{d}.tuo" for d in seed.stdlib_deps)
        body += (
            f"\n\nThis imports a standard-library module, so compile its source "
            f"alongside your program — the tuonelang stdlib is consumed as "
            f"input:\n\n```bash\ntuo verify program.tuo {deps}\n```"
        )
    if seed.notes:
        body += f"\n\n{seed.notes}"
    return body


def validate_seed(seed: Seed, idx: int) -> list[str]:
    """Return a list of failure messages (empty if the seed is fully valid)."""
    problems: list[str] = []
    ok, out = check_ok(seed.solution, seed.stdlib_deps)
    if not ok:
        problems.append(f"check failed:\n{out}")
        return problems  # no point running spec on an unchecked program
    ok, out = spec_ok(seed.solution, seed.stdlib_deps)
    if not ok:
        problems.append(f"spec failed:\n{out}")
    if seed.runnable:
        code, out = run_exit(seed.solution, seed.stdlib_deps)
        if code != seed.run_exit:
            problems.append(
                f"run exit was {code}, expected {seed.run_exit}\n{out}"
            )
    return problems


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--check", action="store_true", help="validate only")
    ap.add_argument("--limit", type=int, default=0, help="first N seeds only")
    args = ap.parse_args()

    OUT.mkdir(parents=True, exist_ok=True)
    seeds = SEEDS[: args.limit] if args.limit else SEEDS

    print(f"Validating {len(seeds)} seeds through the real tuo compiler...")
    valid: list[Seed] = []
    failures = 0
    for i, seed in enumerate(seeds):
        problems = validate_seed(seed, i)
        if problems:
            failures += 1
            print(f"  [FAIL] seed #{i} ({seed.concept}): {seed.task[:60]}")
            for p in problems:
                print("         " + p.replace("\n", "\n         "))
        else:
            valid.append(seed)
            tag = "run" if seed.runnable else "spec"
            print(f"  [ ok ] seed #{i} ({seed.concept}) [{tag}]")

    print(f"\n{len(valid)}/{len(seeds)} seeds valid, {failures} failed.")
    if failures:
        print("Refusing to emit datasets while any seed fails to compile.")
        return 1
    if args.check:
        print("--check: validation passed, emitting nothing.")
        return 0

    # ---- emit datasets -------------------------------------------------
    train = [s for s in valid if not held_out(s)]
    eval_ = [s for s in valid if held_out(s)]

    sft_path = OUT / "sft_oneshot.jsonl"
    repair_path = OUT / "sft_repair.jsonl"
    eval_path = OUT / "eval_heldout.jsonl"

    n_oneshot = 0
    with sft_path.open("w") as f:
        for seed in train:
            rec = chat(SYSTEM_PROMPT, seed.task, assistant_answer(seed))
            f.write(json.dumps(rec) + "\n")
            n_oneshot += 1

    n_repair = 0
    repair_by_break: dict[str, int] = {}
    with repair_path.open("w") as f:
        for seed in train:
            for brk in BREAKS:
                broken = brk.apply(seed.solution)
                if broken == seed.solution:
                    continue  # break not applicable to this seed
                ok, _ = check_ok(broken, seed.stdlib_deps)
                if ok:
                    continue  # break did not actually break it; skip silently
                diag = first_diagnostic(broken, seed.stdlib_deps)
                if diag is None:
                    continue  # no structured diagnostic captured; skip
                rendered = render_diagnostic(diag)
                user = (
                    seed.task
                    + "\n\n(You may write a first attempt; the tuonelang "
                    "compiler will give you feedback to repair against.)"
                )
                rec = repair_chat(
                    SYSTEM_PROMPT,
                    user,
                    "```tuo\n" + broken.rstrip() + "\n```",
                    rendered,
                    "```tuo\n" + seed.solution.rstrip() + "\n```",
                )
                f.write(json.dumps(rec) + "\n")
                n_repair += 1
                repair_by_break[brk.name] = repair_by_break.get(brk.name, 0) + 1

    n_eval = 0
    with eval_path.open("w") as f:
        for seed in eval_:
            rec = {
                "task": seed.task,
                "concept": seed.concept,
                "system": SYSTEM_PROMPT,
                "reference_solution": seed.solution,
                "runnable": seed.runnable,
                "run_exit": seed.run_exit if seed.runnable else None,
                "stdlib_deps": list(seed.stdlib_deps),
            }
            f.write(json.dumps(rec) + "\n")
            n_eval += 1

    concepts: dict[str, int] = {}
    for s in valid:
        concepts[s.concept] = concepts.get(s.concept, 0) + 1

    stats = {
        "seeds_total": len(valid),
        "train_seeds": len(train),
        "eval_seeds": len(eval_),
        "oneshot_examples": n_oneshot,
        "repair_examples": n_repair,
        "eval_examples": n_eval,
        "repair_by_break": repair_by_break,
        "concepts": concepts,
    }
    (OUT / "stats.json").write_text(json.dumps(stats, indent=2) + "\n")

    print("\nEmitted:")
    print(f"  {sft_path.relative_to(REPO)}   ({n_oneshot} one-shot examples)")
    print(f"  {repair_path.relative_to(REPO)}    ({n_repair} repair transcripts)")
    print(f"  {eval_path.relative_to(REPO)}  ({n_eval} held-out eval tasks)")
    print(f"  {(OUT / 'stats.json').relative_to(REPO)}")
    print(f"\nConcept coverage: {len(concepts)} concepts")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
