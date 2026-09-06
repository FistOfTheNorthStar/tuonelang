"""Verification of generated tuonelang through the **real** compiler.

A transpiler that only claims its output is valid is worth very little. This
module runs the actual `tuo` binary over the generated source, so a claim of
correctness is always backed by the compiler's own verdict.

The tool never parses tuonelang itself and never second-guesses a diagnostic:
whatever `tuo` says is the answer.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import tempfile
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class ToolResult:
    """The outcome of one `tuo` invocation."""

    command: list[str]
    status: int
    stdout: str
    stderr: str

    @property
    def ok(self) -> bool:
        """Whether the invocation succeeded."""
        return self.status == 0

    @property
    def output(self) -> str:
        """Combined output, for reporting a failure."""
        return (self.stdout + self.stderr).strip()


def find_tuo(explicit: str | None = None) -> str | None:
    """Locate the `tuo` binary.

    Search order: an explicit path, ``$TUO_BIN``, ``PATH``, then the repository's
    own build outputs, preferring release over debug.
    """
    for candidate in (explicit, os.environ.get("TUO_BIN")):
        if candidate and Path(candidate).is_file():
            return str(Path(candidate).resolve())

    found = shutil.which("tuo")
    if found:
        return found

    root = Path(__file__).resolve().parents[4]
    for build in ("release", "debug"):
        candidate_path = root / "target" / build / "tuo"
        if candidate_path.is_file():
            return str(candidate_path)
    return None


def run_tuo(tuo: str, args: list[str], *, timeout: float = 120.0) -> ToolResult:
    """Invoke `tuo` with ``args`` and capture its result."""
    command = [tuo, *args]
    try:
        completed = subprocess.run(
            command,
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
        )
    except subprocess.TimeoutExpired:
        return ToolResult(command, 124, "", f"timed out after {timeout}s")
    return ToolResult(command, completed.returncode, completed.stdout, completed.stderr)


def check_source(source: str, *, tuo: str, stem: str = "generated") -> ToolResult:
    """Run `tuo check` over generated source held in a temporary file."""
    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory) / f"{stem}.tuo"
        path.write_text(source, encoding="utf-8")
        return run_tuo(tuo, ["check", str(path)])


def format_source(source: str, *, tuo: str, stem: str = "generated") -> tuple[str, ToolResult]:
    """Run `tuo fmt` over generated source, returning the canonical text.

    The formatter is the authority on tuonelang's canonical shape, so the tool
    emits merely-correct source and lets `tuo fmt` make it canonical. If
    formatting fails the original text is returned unchanged along with the
    failing result, so the caller can still show the user what was generated.
    """
    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory) / f"{stem}.tuo"
        path.write_text(source, encoding="utf-8")
        result = run_tuo(tuo, ["fmt", str(path)])
        if not result.ok:
            return source, result
        return path.read_text(encoding="utf-8"), result
