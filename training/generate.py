#!/usr/bin/env python3
"""Compiler-validated fine-tuning data generator for tuonelang.

Every example this emits is verified by the *real* ``tuo`` compiler before it is
written — nothing is trusted on assertion, mirroring how the project's corpus and
benchmark harnesses refuse to record a metric the compiler did not produce.

Outputs (under ``training/dataset/``):

  * ``sft_oneshot.jsonl`` — task -> correct program (chat SFT format).
  * ``sft_repair.jsonl``  — task -> buggy program -> REAL compiler feedback ->
    corrected program (multi-turn, teaches the TDG feedback loop). The feedback
    is a ``tuo check`` diagnostic (parse, type, or ownership break) or a failing
    ``tuo spec`` report (a logic break or a seed's own mutant).
  * ``eval_heldout.jsonl`` — about a fifth of each concept's seeds, never
    emitted into the SFT sets, for scoring Compile@1 / SpecPass@1 / TestPass@1
    with the real compiler.
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
import re
import tempfile
from dataclasses import asdict
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent
OUT = HERE / "dataset"

sys.path.insert(0, str(HERE))
from breaks import BREAKS  # noqa: E402
from harvest import Harvested, harvest_all  # noqa: E402
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


def _run(args: list[str], text: str) -> subprocess.CompletedProcess[str]:
    """Write ``text`` to a temp ``.tuo`` file and run ``tuo <args> <file>``.

    Nothing is passed alongside the program: since ADR-0038 the toolchain loads
    every standard-library module a program names, so the data is generated
    the way a user compiles.
    """
    with tempfile.NamedTemporaryFile(
        "w", suffix=".tuo", dir=str(OUT), delete=False
    ) as f:
        f.write(text)
        path = f.name
    try:
        return subprocess.run(
            tuo_cmd() + args + [path],
            cwd=str(REPO),
            capture_output=True,
            text=True,
            timeout=180,
        )
    finally:
        os.unlink(path)


def check_ok(text: str) -> tuple[bool, str]:
    """True if ``tuo check`` accepts ``text``; else (False, stderr+stdout)."""
    p = _run(["check"], text)
    return p.returncode == 0, (p.stderr + p.stdout).strip()


def spec_ok(text: str) -> tuple[bool, str]:
    """True if ``tuo spec`` runs every spec green."""
    p = _run(["spec"], text)
    out = (p.stdout + p.stderr).strip()
    return p.returncode == 0 and "0 failed" in out, out


def run_exit(text: str) -> tuple[int | None, str]:
    """Compile+run natively; return the process exit byte (or None on failure)."""
    p = _run(["run"], text)
    # `tuo run` propagates the program's own exit status. A codegen/link error is
    # a nonzero tuo error with a message on stderr; distinguish by stderr noise.
    return p.returncode, (p.stderr + p.stdout).strip()


def first_diagnostic(text: str) -> dict | None:
    """Run ``tuo --message-format=json check`` and return the first diagnostic."""
    p = _run(["--message-format=json", "check"], text)
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


def spec_failure(text: str) -> str | None:
    """The human ``tuo spec`` report for a program that checks but fails a
    spec — the text a developer sees — or None if it does not fail that way."""
    p = _run(["spec"], text)
    out = (p.stdout + p.stderr).strip()
    # A real spec failure: a `FAILED` report and an ordinary failure exit — not
    # a front-end error, and not the process dying (a crash is not feedback a
    # model can learn to act on).
    if p.returncode != 1 or "FAILED " not in out or "error[" in out:
        return None
    # Temp-file paths and measured durations differ run to run; name the file
    # the way the transcript does and drop the timings, so the dataset is
    # reproducible.
    out = re.sub(r"/[^\s:()]*?/tmp[^\s:()]*?\.tuo", "program.tuo", out)
    out = re.sub(r", \d+(?:\.\d+)?(?:ns|µs|ms|s)\)", ")", out)
    return re.sub(r" in \d+(?:\.\d+)?(?:ns|µs|ms|s)$", "", out, flags=re.MULTILINE)


EFFECT_PATHS = (
    "std::io::",
    "std::fs::",
    "std::net::",
    "std::sync::",
    "std::process::",
    "std::time::now",
    "std::rt::",
    "std::crypto::random_byte",
    "std::crypto::nonce",
)


def infer_tier(solution: str) -> str:
    """``effect`` if the program calls an effectful module, else ``executable``.

    ``interpreter`` (checks and specs, outside the native subset) cannot be
    seen from the text; a seed declares it."""
    return "effect" if any(p in solution for p in EFFECT_PATHS) else "executable"


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


def split_by_concept(seeds: list[Seed], ratio: int = 5) -> tuple[list[Seed], list[Seed]]:
    """Hold out about 1/``ratio`` of *each concept's* seeds for eval.

    A global hash split left whole concepts out of the eval set and others
    over-represented, so a score said little about any one area. Within a
    concept the seeds are ordered by a stable hash of their task, and the
    first ``round(n / ratio)`` are held out — at least one when the concept has
    two or more seeds, none when it has one (its only example stays in
    training). Deterministic: the same seeds always give the same split.
    """
    by_concept: dict[str, list[Seed]] = {}
    for seed in seeds:
        by_concept.setdefault(seed.concept, []).append(seed)
    held: set[int] = set()
    for group in by_concept.values():
        if len(group) < 2:
            continue
        ordered = sorted(
            group, key=lambda s: hashlib.sha256(s.task.encode()).hexdigest()
        )
        for seed in ordered[: max(1, round(len(group) / ratio))]:
            held.add(id(seed))
    train = [s for s in seeds if id(s) not in held]
    eval_ = [s for s in seeds if id(s) in held]
    return train, eval_


def chat(system: str, user: str, assistant: str) -> dict:
    """A single-turn chat SFT record."""
    return {
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user},
            {"role": "assistant", "content": assistant},
        ]
    }


def repair_chat(
    system: str, user: str, bad: str, diag: str, fixed: str, command: str = "check"
) -> dict:
    """A multi-turn repair record with a compiler-tool turn between the two
    assistant turns. ``command`` is what produced ``diag`` (``check`` for a
    compile error, ``spec`` for a failing spec)."""
    return {
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user},
            {"role": "assistant", "content": bad},
            {
                "role": "tool",
                "name": "tuo_compiler",
                "content": f"$ tuo {command} program.tuo\n{diag}",
            },
            {"role": "assistant", "content": fixed},
        ]
    }


def assistant_answer(seed: Seed) -> str:
    """The assistant's one-shot answer: the program, plus a short note if any."""
    body = "```tuo\n" + seed.solution.rstrip() + "\n```"
    if seed.notes:
        body += f"\n\n{seed.notes}"
    return body


def validate_seed(seed: Seed, idx: int) -> list[str]:
    """Return a list of failure messages (empty if the seed is fully valid)."""
    problems: list[str] = []
    ok, out = check_ok(seed.solution)
    if not ok:
        problems.append(f"check failed:\n{out}")
        return problems  # no point running spec on an unchecked program
    ok, out = spec_ok(seed.solution)
    if not ok:
        problems.append(f"spec failed:\n{out}")
    if seed.runnable:
        code, out = run_exit(seed.solution)
        if code != seed.run_exit:
            problems.append(
                f"run exit was {code}, expected {seed.run_exit}\n{out}"
            )
    inferred = infer_tier(seed.solution)
    if seed.tier and seed.tier != "interpreter" and seed.tier != inferred:
        problems.append(f"declared tier `{seed.tier}`, but the solution is `{inferred}`")
    if seed.difficulty not in range(0, 6):
        problems.append(f"difficulty {seed.difficulty} is outside 0..=5")
    if seed.heldout_specs:
        ok, out = spec_ok(seed.solution.rstrip() + "\n\n" + seed.heldout_specs)
        if not ok:
            problems.append(f"held-out specs fail on the solution:\n{out}")
    for n, mutant in enumerate(seed.mutants):
        ok, out = check_ok(mutant)
        if not ok:
            problems.append(f"mutant #{n} does not check (it must, and fail a spec):\n{out}")
        elif spec_failure(mutant) is None:
            problems.append(f"mutant #{n} passes every spec, so no spec catches it")
    return problems


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--check", action="store_true", help="validate only")
    ap.add_argument("--limit", type=int, default=0, help="first N seeds only")
    ap.add_argument(
        "--no-harvest",
        action="store_true",
        help="skip the repository harvest (seeds only)",
    )
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

    # ---- harvest additional examples from validated repository source ---
    # The seed library is the coverage skeleton; these supply volume from code
    # the repository already keeps green. Each is compiled through the real
    # compiler here, exactly like a seed — an item that does not compile
    # standalone is dropped, and the count is recorded in stats.json.
    harvested: list[Harvested] = []
    harvest_dropped = 0
    harvest_by_origin: dict[str, int] = {}
    if not args.no_harvest:
        candidates = harvest_all(REPO)
        print(f"\nHarvesting {len(candidates)} examples from repository source...")
        for item in candidates:
            ok, _ = check_ok(item.solution)
            if not ok:
                harvest_dropped += 1
                continue
            ok, _ = spec_ok(item.solution)
            if not ok:
                harvest_dropped += 1
                continue
            harvested.append(item)
            harvest_by_origin[item.origin] = (
                harvest_by_origin.get(item.origin, 0) + 1
            )
        kept = len(harvested)
        print(
            f"  {kept} kept, {harvest_dropped} dropped "
            f"(did not compile/spec standalone): "
            + ", ".join(f"{k}={v}" for k, v in sorted(harvest_by_origin.items()))
        )

    # ---- emit datasets -------------------------------------------------
    train, eval_ = split_by_concept(valid)

    sft_path = OUT / "sft_oneshot.jsonl"
    repair_path = OUT / "sft_repair.jsonl"
    eval_path = OUT / "eval_heldout.jsonl"

    n_oneshot = 0
    with sft_path.open("w") as f:
        for seed in train:
            rec = chat(SYSTEM_PROMPT, seed.task, assistant_answer(seed))
            f.write(json.dumps(rec) + "\n")
            n_oneshot += 1
        # Harvested records join the one-shot set only. They are deliberately
        # kept OUT of the eval split so scoring is never contaminated by
        # material that also appears verbatim in the repository.
        for item in harvested:
            body = "```tuo\n" + item.solution.rstrip() + "\n```"
            if item.notes:
                body += f"\n\n{item.notes}"
            rec = chat(SYSTEM_PROMPT, item.task, body)
            f.write(json.dumps(rec) + "\n")
            n_oneshot += 1

    n_repair = 0
    repair_by_break: dict[str, int] = {}
    repair_by_kind: dict[str, int] = {}
    with repair_path.open("w") as f:

        def emit_repair(seed: Seed, broken: str, diag: str, command: str, label: str) -> None:
            nonlocal n_repair
            user = (
                seed.task
                + "\n\n(You may write a first attempt; the tuonelang "
                "compiler will give you feedback to repair against.)"
            )
            rec = repair_chat(
                SYSTEM_PROMPT,
                user,
                "```tuo\n" + broken.rstrip() + "\n```",
                diag,
                "```tuo\n" + seed.solution.rstrip() + "\n```",
                command,
            )
            f.write(json.dumps(rec) + "\n")
            n_repair += 1
            repair_by_break[label] = repair_by_break.get(label, 0) + 1

        for seed in train:
            for brk in BREAKS:
                broken = brk.apply(seed.solution)
                if broken == seed.solution:
                    continue  # break not applicable to this seed
                ok, _ = check_ok(broken)
                if brk.kind == "spec":
                    # The program must still check and must fail a spec: the
                    # repair is to the logic, shown by the real spec report.
                    if not ok:
                        continue
                    report = spec_failure(broken)
                    if report is None:
                        continue  # every spec still passes; nothing to repair
                    emit_repair(seed, broken, report, "spec", brk.name)
                    repair_by_kind["spec"] = repair_by_kind.get("spec", 0) + 1
                    continue
                if ok:
                    continue  # break did not actually break it; skip silently
                diag = first_diagnostic(broken)
                if diag is None:
                    continue  # no structured diagnostic captured; skip
                code = diag.get("code", "?")
                stage = {"L": "parse", "P": "parse", "R": "resolve", "T": "type", "O": "ownership"}
                kind = stage.get(code[:1], "other")
                emit_repair(seed, broken, render_diagnostic(diag), "check", brk.name)
                repair_by_kind[kind] = repair_by_kind.get(kind, 0) + 1
            # A seed's own mutants are hand-written wrong logic, verified in
            # `validate_seed` to check and to fail a spec.
            for mutant in seed.mutants:
                report = spec_failure(mutant)
                if report is not None:
                    emit_repair(seed, mutant, report, "spec", "seed-mutant")
                    repair_by_kind["spec"] = repair_by_kind.get("spec", 0) + 1

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
                "tier": seed.tier or infer_tier(seed.solution),
                "difficulty": seed.difficulty,
                "heldout_specs": seed.heldout_specs,
            }
            f.write(json.dumps(rec) + "\n")
            n_eval += 1

    concepts: dict[str, int] = {}
    tiers: dict[str, int] = {}
    for s in valid:
        concepts[s.concept] = concepts.get(s.concept, 0) + 1
        tier = s.tier or infer_tier(s.solution)
        tiers[tier] = tiers.get(tier, 0) + 1
    eval_by_concept: dict[str, int] = {}
    for s in eval_:
        eval_by_concept[s.concept] = eval_by_concept.get(s.concept, 0) + 1

    stats = {
        "seeds_total": len(valid),
        "train_seeds": len(train),
        "eval_seeds": len(eval_),
        "oneshot_examples": n_oneshot,
        "repair_examples": n_repair,
        "eval_examples": n_eval,
        "repair_by_break": repair_by_break,
        "repair_by_kind": repair_by_kind,
        "concepts": concepts,
        "eval_by_concept": eval_by_concept,
        "tiers": tiers,
        "harvested_kept": len(harvested),
        "harvested_dropped": harvest_dropped,
        "harvested_by_origin": harvest_by_origin,
    }
    (OUT / "stats.json").write_text(json.dumps(stats, indent=2) + "\n")

    print("\nEmitted:")
    print(f"  {sft_path.relative_to(REPO)}   ({n_oneshot} one-shot examples)")
    print(f"  {repair_path.relative_to(REPO)}    ({n_repair} repair transcripts)")
    print(f"  {eval_path.relative_to(REPO)}  ({n_eval} held-out eval tasks)")
    print(f"  {(OUT / 'stats.json').relative_to(REPO)}")
    print(f"\nConcept coverage: {len(concepts)} concepts")
    if harvested:
        print(
            f"Harvested examples included: {len(harvested)} "
            f"({harvest_dropped} dropped by the compile gate)"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
