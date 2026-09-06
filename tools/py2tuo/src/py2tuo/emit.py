"""Rendering of tuonelang source text.

The compiler builds tuonelang source through this tiny writer rather than by
concatenating strings ad hoc, so indentation and block structure are uniform.
Output is intended to be fed straight to `tuo fmt`, which is the canonical
authority on formatting -- this writer only needs to be correct, not canonical.
"""

from __future__ import annotations


class Writer:
    """An indentation-aware line buffer."""

    def __init__(self) -> None:
        self._lines: list[str] = []
        self._depth = 0

    def line(self, text: str = "") -> None:
        """Append one line at the current indentation."""
        self._lines.append("" if not text else ("    " * self._depth) + text)

    def block(self, header: str, *, close: str = "}") -> "_Block":
        """Open a `header { ... }` block as a context manager.

        ``close`` is the closing line, so an `if` whose `else` follows can emit
        `} else {` instead of a bare `}` without the caller manipulating the
        indentation by hand.
        """
        return _Block(self, header, close)

    def indent(self) -> None:
        """Increase the indentation level."""
        self._depth += 1

    def dedent(self) -> None:
        """Decrease the indentation level."""
        self._depth = max(0, self._depth - 1)

    def render(self) -> str:
        """Render the buffered lines as a single source string."""
        body = "\n".join(self._lines).rstrip("\n")
        return body + "\n" if body else ""


class _Block:
    """Context manager emitting `header {` ... `}`."""

    def __init__(self, writer: Writer, header: str, close: str = "}") -> None:
        self._writer = writer
        self._header = header
        self._close = close

    def __enter__(self) -> Writer:
        self._writer.line(f"{self._header} {{")
        self._writer.indent()
        return self._writer

    def __exit__(self, *exc: object) -> None:
        self._writer.dedent()
        self._writer.line(self._close)
