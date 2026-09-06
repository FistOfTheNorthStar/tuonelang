"""Command-line front end for the Python -> tuonelang compiler.

Subcommands mirror the tuonelang CLI's own discipline: a command appears only
once it does what its name says, and machine output is a separate, explicit
format rather than scraped from the human one.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from . import __version__, module_name_for, translate
from .diagnostics import CompileError
from .verify import check_source, find_tuo, format_source, run_tuo


def build_parser() -> argparse.ArgumentParser:
    """Construct the argument parser."""
    parser = argparse.ArgumentParser(
        prog="py2tuo",
        description="Compile a typed subset of Python into tuonelang source.",
    )
    parser.add_argument("--version", action="version", version=f"py2tuo {__version__}")
    parser.add_argument(
        "--message-format",
        choices=("human", "json"),
        default="human",
        help="human (default) or a machine-readable JSON envelope",
    )
    parser.add_argument("--tuo", help="path to the `tuo` binary (default: $TUO_BIN, PATH, ./target)")

    sub = parser.add_subparsers(dest="command", required=True)

    build = sub.add_parser("build", help="translate a Python file to tuonelang")
    build.add_argument("input", help="the Python source file")
    build.add_argument("-o", "--output", help="write to this path instead of stdout")
    build.add_argument("--module", help="module name (default: derived from the filename)")
    build.add_argument(
        "--no-format",
        action="store_true",
        help="skip `tuo fmt` canonicalisation of the generated source",
    )
    build.add_argument(
        "--check",
        action="store_true",
        help="additionally run `tuo check` and fail if the output is rejected",
    )

    verify = sub.add_parser(
        "verify",
        help="translate, then prove the output through the real compiler (check + specs)",
    )
    verify.add_argument("input", help="the Python source file")
    verify.add_argument("--module", help="module name (default: derived from the filename)")
    verify.add_argument("--run", action="store_true", help="also `tuo run` it and report the exit status")

    sub.add_parser("supported", help="describe the translatable Python subset")
    return parser


def main(argv: list[str] | None = None) -> int:
    """Entry point. Returns the process exit status."""
    parser = build_parser()
    args = parser.parse_args(argv)

    if args.command == "supported":
        print(SUBSET_DOC.strip())
        return 0
    if args.command == "build":
        return _build(args)
    if args.command == "verify":
        return _verify(args)
    parser.error(f"unknown command {args.command}")
    return 2


def _translate_file(args: argparse.Namespace) -> tuple[str, str]:
    """Read and translate the input file, returning (source, module name)."""
    path = Path(args.input)
    text = path.read_text(encoding="utf-8")
    module = args.module or module_name_for(path)
    return translate(text, module=module, filename=str(path)), module


def _emit_error(exc: CompileError, path: str, machine: bool) -> int:
    """Report a compile error in the selected format."""
    if machine:
        json.dump(
            {"status": "error", "file": path, "diagnostics": [exc.diagnostic.to_json()]},
            sys.stdout,
            indent=2,
        )
        sys.stdout.write("\n")
    else:
        print(exc.diagnostic.render(path), file=sys.stderr)
    return 1


def _build(args: argparse.Namespace) -> int:
    """Run the `build` subcommand."""
    machine = args.message_format == "json"
    try:
        source, module = _translate_file(args)
    except CompileError as exc:
        return _emit_error(exc, args.input, machine)

    tuo = find_tuo(args.tuo)
    notes: list[str] = []

    if not args.no_format:
        if tuo:
            formatted, result = format_source(source, tuo=tuo, stem=module)
            if result.ok:
                source = formatted
            else:
                notes.append(f"tuo fmt failed, emitting unformatted source: {result.output}")
        else:
            notes.append("tuo binary not found; emitting unformatted source")

    if args.check:
        if not tuo:
            print("error: --check requires the `tuo` binary", file=sys.stderr)
            return 2
        result = check_source(source, tuo=tuo, stem=module)
        if not result.ok:
            if machine:
                json.dump(
                    {"status": "rejected", "file": args.input, "tuo_output": result.output},
                    sys.stdout,
                    indent=2,
                )
                sys.stdout.write("\n")
            else:
                print("generated tuonelang was rejected by `tuo check`:", file=sys.stderr)
                print(result.output, file=sys.stderr)
            return 1

    if args.output:
        Path(args.output).write_text(source, encoding="utf-8")

    if machine:
        json.dump(
            {
                "status": "ok",
                "file": args.input,
                "module": module,
                "output": args.output,
                "source": None if args.output else source,
                "notes": notes,
            },
            sys.stdout,
            indent=2,
        )
        sys.stdout.write("\n")
    else:
        for note in notes:
            print(f"note: {note}", file=sys.stderr)
        if args.output:
            print(f"wrote {args.output}", file=sys.stderr)
        else:
            sys.stdout.write(source)
    return 0


def _verify(args: argparse.Namespace) -> int:
    """Run the `verify` subcommand: translate and prove through `tuo`."""
    machine = args.message_format == "json"
    try:
        source, module = _translate_file(args)
    except CompileError as exc:
        return _emit_error(exc, args.input, machine)

    tuo = find_tuo(args.tuo)
    if not tuo:
        print(
            "error: the `tuo` binary was not found; build it with "
            "`cargo build -p tuo-cli` or set $TUO_BIN",
            file=sys.stderr,
        )
        return 2

    import tempfile

    stages: list[dict[str, object]] = []
    status = 0
    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory) / f"{module}.tuo"
        path.write_text(source, encoding="utf-8")

        commands = [("check", ["check", str(path)]), ("spec", ["spec", str(path)])]
        if args.run:
            commands.append(("run", ["run", str(path)]))

        for name, command in commands:
            result = run_tuo(tuo, command)
            # `run` reports the program's own exit status, which is a result,
            # not a failure -- only check/spec failing means the translation
            # was bad.
            failed = not result.ok and name != "run"
            stages.append(
                {
                    "stage": name,
                    "status": result.status,
                    "ok": not failed,
                    "output": result.output,
                }
            )
            if failed:
                status = 1
                break

    if machine:
        json.dump(
            {"status": "ok" if status == 0 else "failed", "module": module, "stages": stages},
            sys.stdout,
            indent=2,
        )
        sys.stdout.write("\n")
    else:
        for stage in stages:
            mark = "ok" if stage["ok"] else "FAILED"
            print(f"tuo {stage['stage']}: {mark} (exit {stage['status']})")
            if stage["output"]:
                for line in str(stage["output"]).splitlines():
                    print(f"    {line}")
    return status


SUBSET_DOC = """
py2tuo translates a statically typed, annotated subset of Python.

The subset exists because a total translation does not. Python is dynamically
typed, garbage collected, and has arbitrary-precision integers; tuonelang is
statically typed, ownership checked, and traps on integer overflow. Rather than
guess, the tool refuses anything it cannot translate faithfully.

TRANSLATED
  * module-level `def`s, fully annotated (every parameter and the return type)
  * int / bool / float / str / None, list[T], Optional[T]
  * arithmetic + - * // % ** and bitwise & | ^ ~ << >>
  * comparisons, including Python's chained form (a < b < c), and and/or/not
  * if / elif / else, while, for over range(..) or a list
  * assignment, augmented assignment (x += 1 -> x = x + 1), xs[i] = v
  * conditional expressions (a if c else b)
  * calls to functions defined in the same module, and passing a function by name
  * builtins: len, abs, min, max, sum, print, str, int, float, sorted
  * string concatenation (a + b) via std::string::concat
  * dict[str, int] and dict[int, int] -- the two map shapes tuonelang v0 has:
    d[k] = v, d.get(k, default), k in d, len(d), d.keys(), and `= {}`

REFUSED, with a positioned diagnostic naming the construct
  * classes, decorators, nested functions, lambdas, comprehensions
  * exceptions (try/except/raise) -- return Result[T, E] and match instead
  * async/await -- tuonelang concurrency is structured fork-join only
  * set/tuple, slicing, f-strings, *args/**kwargs, default arguments
  * dict shapes other than the two above; d[k] (raises KeyError, and tuonelang
    has no exceptions -- write d.get(k, default)); .values()/.items()/.pop()
  * negative indices: xs[-1] is the LAST element in Python and a trap here
  * `is`, `in`, truthiness, method-call syntax, imports of real libraries
  * unannotated parameters, mixed Int/Float arithmetic, `/` true division
  * integer literals wider than 64 bits

Everything emitted lies inside tuonelang's runnable core -- the tier `tuo run`
executes -- and `py2tuo verify` proves it by driving the real compiler.
"""
