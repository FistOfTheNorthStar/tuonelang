#!/usr/bin/env python3
"""Run the test suite without pytest.

`pytest` is the nicer runner, but requiring it makes a standard-library-only
tool awkward to check out and run. This driver executes the same test
functions, so there is one suite rather than two that can drift.

    python3 tools/py2tuo/run_tests.py
"""

from __future__ import annotations

import sys
import traceback
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE / "src"))
sys.path.insert(0, str(HERE / "tests"))


def main() -> int:
    """Execute every test in the suite, reporting failures."""
    try:
        import test_translate as suite
    except ImportError as exc:  # pragma: no cover - import guard
        print(f"could not import the test module: {exc}", file=sys.stderr)
        return 2

    if suite.TUO is None:
        print("note: the `tuo` binary was not found -- compiler-backed tests are skipped")

    passed = failed = skipped = 0
    failures: list[tuple[str, str]] = []

    for name in sorted(dir(suite)):
        if not name.startswith("test_"):
            continue
        function = getattr(suite, name)
        # Expand the parametrised cases by hand, mirroring the pytest marks.
        if name.endswith(("_translates_without_error", "_source_is_accepted", "_agrees_with_python")):
            cases = [(key,) for key in sorted(suite.PROGRAMS)]
        elif name.endswith("_positioned_diagnostic"):
            cases = [(key,) for key in sorted(suite.REFUSED)]
        else:
            cases = [()]

        needs_compiler = "accepted" in name or "agrees" in name or "formatted" in name
        for case in cases:
            label = f"{name}{case if case else ''}"
            if needs_compiler and suite.TUO is None:
                skipped += 1
                continue
            try:
                function(*case)
            except Exception:  # noqa: BLE001 - a test failure of any kind
                failed += 1
                failures.append((label, traceback.format_exc()))
            else:
                passed += 1

    for label, trace in failures:
        print(f"\nFAILED {label}\n{trace}", file=sys.stderr)

    summary = f"{passed} passed, {failed} failed"
    if skipped:
        summary += f", {skipped} skipped (no `tuo` binary)"
    print(summary)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
