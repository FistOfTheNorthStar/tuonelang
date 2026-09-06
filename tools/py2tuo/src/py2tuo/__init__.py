"""py2tuo -- a Python-subset to tuonelang compiler.

This is a developer tool, not part of the shipped tuonelang compiler. It
translates a **statically typed, annotated subset** of Python into tuonelang
source and verifies the result with the real `tuo` binary.

The public entry point is :func:`translate`.
"""

from __future__ import annotations

import ast
from pathlib import Path

from .compiler import Compiler
from .diagnostics import CompileError, Diagnostic, Severity

__all__ = ["CompileError", "Diagnostic", "Severity", "translate", "module_name_for"]

__version__ = "0.1.0"


def module_name_for(path: str | Path) -> str:
    """Derive a legal tuonelang module name from a Python file path."""
    stem = Path(path).stem
    cleaned = "".join(c if c.isalnum() else "_" for c in stem).strip("_").lower()
    if not cleaned or not cleaned[0].isalpha():
        cleaned = f"m_{cleaned}" if cleaned else "generated"
    return cleaned


def translate(source: str, *, module: str, filename: str = "<input>") -> str:
    """Translate Python ``source`` into tuonelang source text.

    Raises :class:`CompileError` with a positioned diagnostic if the input
    falls outside the translatable subset.
    """
    try:
        tree = ast.parse(source, filename=filename)
    except SyntaxError as exc:
        raise CompileError(
            Diagnostic(
                code="PY0000",
                message=f"input is not valid Python: {exc.msg}",
                line=exc.lineno or 0,
                column=(exc.offset or 1) - 1,
            )
        ) from exc

    return Compiler(module).compile_module(tree, source=source)
