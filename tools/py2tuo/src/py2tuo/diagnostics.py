"""Structured, machine-readable diagnostics for the Python -> tuonelang compiler.

The tool's most important job is *refusing precisely*. A translation it cannot
perform must be reported as an unsupported construct naming the exact Python
feature and the line it appeared on -- never emitted as tuonelang that does
something subtly different.

Codes are namespaced by stage, mirroring tuonelang's own convention
(``L``/``P``/``R``/``T``/... in the real compiler):

``PY``
    The input is not valid Python, or uses Python the subset excludes.
``TY``
    The annotations are missing, unsupported, or inconsistent.
``SEM``
    The construct is valid, annotated Python whose *semantics* have no
    faithful tuonelang translation.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum


class Severity(str, Enum):
    """How badly a diagnostic affects the translation."""

    ERROR = "error"
    WARNING = "warning"


@dataclass(frozen=True)
class Diagnostic:
    """One structured message about a Python source construct.

    ``line``/``column`` are 1-based and 0-based respectively, matching
    CPython's own ``ast`` node positions, so an editor can jump to them.
    """

    code: str
    message: str
    line: int
    column: int
    severity: Severity = Severity.ERROR
    notes: tuple[str, ...] = field(default_factory=tuple)

    def render(self, path: str) -> str:
        """Render as a human-readable, editor-clickable line."""
        head = f"{path}:{self.line}:{self.column + 1}: {self.severity.value}[{self.code}] {self.message}"
        return "\n".join([head, *(f"    note: {n}" for n in self.notes)])

    def to_json(self) -> dict[str, object]:
        """Render as a plain JSON-ready dictionary."""
        return {
            "code": self.code,
            "severity": self.severity.value,
            "message": self.message,
            "line": self.line,
            "column": self.column,
            "notes": list(self.notes),
        }


class CompileError(Exception):
    """Raised to abort translation with one structured diagnostic."""

    def __init__(self, diagnostic: Diagnostic) -> None:
        super().__init__(diagnostic.message)
        self.diagnostic = diagnostic


def error(code: str, message: str, node: object, *notes: str) -> CompileError:
    """Build a :class:`CompileError` positioned at a Python AST ``node``."""
    line = getattr(node, "lineno", 0)
    column = getattr(node, "col_offset", 0)
    return CompileError(Diagnostic(code, message, line, column, Severity.ERROR, tuple(notes)))


def unsupported(feature: str, node: object, *notes: str) -> CompileError:
    """Refuse a Python feature the subset deliberately excludes."""
    return error("PY0001", f"unsupported Python construct: {feature}", node, *notes)
