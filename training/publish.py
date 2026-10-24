#!/usr/bin/env python3
"""Check and publish the generated training datasets.

The JSONL files ``generate.py`` writes are deterministic outputs of the seeds,
the harvested sources, and the compiler, so they are not committed: each
regeneration would be a large, noisy diff. The repository keeps
``training/dataset/stats.json`` and ``training/dataset/manifest.json`` (the
source commit and a SHA-256 per output), and the files themselves are a
GitHub release asset per published run.

Usage:
    python3 training/publish.py verify               # training/dataset matches its manifest
    python3 training/publish.py verify --dir DIR     # ...or downloaded files do
    python3 training/publish.py release --dry-run    # show what would be published
    python3 training/publish.py release              # create the GitHub release

To check a published dataset, regenerate it at the manifest's
``source_commit`` (with ``TUO_BIN`` built from that commit) and run
``verify``; or download the release's files and run ``verify --dir``.

``release`` refuses unless the files match the manifest, the manifest was
generated from a clean tree, and that commit is on ``origin/main``, so a
published dataset always names a commit anyone can check out. It uploads only
``PUBLIC_OUTPUTS`` and the manifest: the eval set is private and is never
uploaded, whatever ``TUO_EVAL_DIR`` holds.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent
OUT = HERE / "dataset"
MANIFEST = OUT / "manifest.json"

sys.path.insert(0, str(HERE))
from generate import PUBLIC_OUTPUTS  # noqa: E402


def load_manifest() -> dict:
    manifest = json.loads(MANIFEST.read_text())
    if manifest.get("schema") != 1:
        raise SystemExit(f"{MANIFEST}: unknown schema {manifest.get('schema')!r}")
    if sorted(manifest["files"]) != sorted(PUBLIC_OUTPUTS):
        raise SystemExit(f"{MANIFEST}: lists {sorted(manifest['files'])}, not {sorted(PUBLIC_OUTPUTS)}")
    return manifest


def mismatches(manifest: dict, directory: Path) -> list[str]:
    """Every file under ``directory`` that is missing or differs from the manifest."""
    found = []
    for name, entry in sorted(manifest["files"].items()):
        path = directory / name
        if not path.is_file():
            found.append(f"{name}: missing from {directory}")
            continue
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        if digest != entry["sha256"]:
            found.append(f"{name}: sha256 {digest}, manifest says {entry['sha256']}")
    return found


def git(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(["git", *args], cwd=REPO, capture_output=True, text=True)


def release_notes(manifest: dict) -> str:
    stats = json.loads((OUT / "stats.json").read_text())
    commit = manifest["source_commit"]
    lines = [
        f"Training datasets generated at {commit} by `{manifest['compiler']}`.",
        "",
        f"- `sft_oneshot.jsonl`: {stats['oneshot_examples']} one-shot examples "
        f"({stats['train_seeds']} seeds, {stats['harvested_kept']} harvested)",
        f"- `sft_repair.jsonl`: {stats['repair_examples']} repair transcripts",
        "",
        "Every program compiled through the real `tuo` at generation time and every "
        "diagnostic is the compiler's own output. To check these files, regenerate at "
        "that commit and compare:",
        "",
        "```",
        f"git checkout {commit}",
        "cargo build -p tuo-cli",
        'TUO_BIN="$(pwd)/target/debug/tuo" python3 training/generate.py',
        "python3 training/publish.py verify",
        "```",
    ]
    if manifest.get("eval"):
        lines += [
            "",
            f"The held-out eval set is private; its sha256 is `{manifest['eval']['sha256']}` "
            f"({manifest['eval']['tasks']} tasks).",
        ]
    return "\n".join(lines) + "\n"


def cmd_verify(args: argparse.Namespace) -> int:
    manifest = load_manifest()
    directory = args.dir or OUT
    found = mismatches(manifest, directory)
    for problem in found:
        print(f"  [FAIL] {problem}")
    if found:
        return 1
    print(f"{directory} matches the manifest (source {manifest['source_commit'][:12]}).")
    return 0


def cmd_release(args: argparse.Namespace) -> int:
    manifest = load_manifest()
    problems = mismatches(manifest, OUT)
    commit = manifest["source_commit"]
    if manifest["source_dirty"]:
        problems.append(
            "the manifest was generated from a tree with uncommitted changes; "
            "commit them and regenerate"
        )
    if git("fetch", "--quiet", "origin", "main").returncode != 0:
        problems.append("could not fetch origin/main")
    elif git("merge-base", "--is-ancestor", commit, "origin/main").returncode != 0:
        problems.append(f"source commit {commit[:12]} is not on origin/main")
    committed = git("show", "HEAD:training/dataset/manifest.json").stdout
    if committed != MANIFEST.read_text():
        problems.append("training/dataset/manifest.json differs from the committed one")
    if problems:
        for problem in problems:
            print(f"  [FAIL] {problem}")
        print("Refusing to publish.")
        return 1

    tag = f"dataset-{commit[:12]}"
    files = [str(OUT / name) for name in PUBLIC_OUTPUTS] + [str(MANIFEST)]
    command = [
        "gh", "release", "create", tag, *files,
        "--target", commit,
        "--title", f"Training datasets ({commit[:12]})",
        "--notes", release_notes(manifest),
    ]
    if args.dry_run:
        print("Would run:")
        print("  " + " ".join(command[:-1]) + " --notes <below>")
        print()
        print(release_notes(manifest))
        return 0
    return subprocess.run(command, cwd=REPO).returncode


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="command", required=True)
    verify = sub.add_parser("verify", help="check files against the manifest")
    verify.add_argument("--dir", type=Path, help="directory holding the files (default: training/dataset)")
    verify.set_defaults(run=cmd_verify)
    release = sub.add_parser("release", help="publish the files as a GitHub release")
    release.add_argument("--dry-run", action="store_true", help="print the release, create nothing")
    release.set_defaults(run=cmd_release)
    args = ap.parse_args()
    return args.run(args)


if __name__ == "__main__":
    raise SystemExit(main())
