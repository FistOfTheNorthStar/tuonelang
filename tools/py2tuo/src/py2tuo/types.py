"""The Python-annotation -> tuonelang-type mapping.

Python is dynamically typed; tuonelang is not. Every translated function must
therefore be fully annotated, and each annotation must name a type this mapping
covers. Anything else is refused here rather than guessed at.

The mapping is deliberately small and total: a name either maps to a tuonelang
type spelling that the target compiler accepts, or it is an error. There is no
"fall back to Int" case -- a wrong guess would produce a program that compiles
and computes the wrong thing, which is the one outcome worse than refusing.
"""

from __future__ import annotations

import ast
from dataclasses import dataclass

from .diagnostics import CompileError, error


@dataclass(frozen=True)
class Type:
    """A tuonelang type, carried as the spelling a caller must type."""

    spelling: str

    def __str__(self) -> str:
        return self.spelling

    @property
    def is_scalar(self) -> bool:
        """Whether the type is a `Copy` scalar (passed with `take`)."""
        return self.spelling in {"Int", "Bool", "Float", "Char"}

    @property
    def is_str_like(self) -> bool:
        """Whether the type is one of tuonelang's two string types."""
        return self.spelling in {"Str", "String"}


INT = Type("Int")
BOOL = Type("Bool")
FLOAT = Type("Float")
STR = Type("Str")
STRING = Type("String")
UNIT = Type("()")

#: Python annotation names that map directly onto a tuonelang scalar.
_SCALARS: dict[str, Type] = {
    "int": INT,
    "bool": BOOL,
    "float": FLOAT,
    # `str` becomes the OWNED String: a Python str is an owned value, and an
    # owned String is what a function can return. Borrowed `Str` appears only
    # in parameter position, chosen by the parameter-mode rules.
    "str": STRING,
    "None": UNIT,
}


#: The only two map shapes tuonelang v0 has an operation surface for
#: (ADR-0011). Any other key/value pair is a `T0001` in the target compiler,
#: so it is refused here where the message can name the Python type.
SUPPORTED_MAPS: dict[tuple[str, str], str] = {
    ("Str", "Int"): "Map[Str, Int]",
    ("Int", "Int"): "Map[Int, Int]",
}


def map_of(key: Type, value: Type) -> Type:
    """The `Map[K, V]` type, for a supported key/value pair."""
    return Type(f"Map[{key}, {value}]")


def is_map(ty: Type) -> bool:
    """Whether ``ty`` is a map type."""
    return ty.spelling.startswith("Map[") and ty.spelling.endswith("]")


def map_parts(ty: Type) -> tuple[Type, Type] | None:
    """The (key, value) types of a `Map[K, V]`, or ``None``."""
    if not is_map(ty):
        return None
    inner = ty.spelling[len("Map[") : -1]
    key, _, value = inner.partition(", ")
    return (Type(key), Type(value))


def array_of(element: Type) -> Type:
    """The growable `Array[T]` type over ``element``."""
    return Type(f"Array[{element}]")


def element_of(ty: Type) -> Type | None:
    """The element type of an `Array[T]`, or ``None`` if ``ty`` is not one."""
    if ty.spelling.startswith("Array[") and ty.spelling.endswith("]"):
        return Type(ty.spelling[len("Array[") : -1])
    return None


def translate_annotation(node: ast.expr | None, *, owner: ast.AST) -> Type:
    """Translate a Python annotation expression into a tuonelang :class:`Type`.

    ``owner`` is the node blamed when the annotation is missing entirely, so a
    missing return type points at the ``def`` rather than at nothing.
    """
    if node is None:
        raise error(
            "TY0001",
            "missing type annotation",
            owner,
            "tuonelang never infers a signature: annotate every parameter and the return type",
            "example: def add(a: int, b: int) -> int:",
        )

    # `x: int`, `x: str`, ...
    if isinstance(node, ast.Name):
        mapped = _SCALARS.get(node.id)
        if mapped is not None:
            return mapped
        raise error(
            "TY0002",
            f"unsupported type annotation `{node.id}`",
            node,
            "supported: int, bool, float, str, None, list[T], Optional[T]",
        )

    # `x: None` is spelled as a constant, not a Name.
    if isinstance(node, ast.Constant) and node.value is None:
        return UNIT

    # `list[int]`, `Optional[int]`, `list[list[int]]`, ...
    if isinstance(node, ast.Subscript):
        return _translate_generic(node)

    raise error(
        "TY0002",
        "unsupported type annotation",
        node,
        "supported: int, bool, float, str, None, list[T], Optional[T]",
    )


def _translate_generic(node: ast.Subscript) -> Type:
    """Translate a subscripted annotation such as ``list[int]``."""
    base = node.value
    if not isinstance(base, ast.Name):
        raise error("TY0002", "unsupported generic type annotation", node)

    name = base.id
    if name in {"list", "List"}:
        element = translate_annotation(node.slice, owner=node)
        if element == UNIT:
            raise error("TY0003", "`list[None]` has no tuonelang equivalent", node)
        return array_of(element)

    if name in {"Optional"}:
        inner = translate_annotation(node.slice, owner=node)
        return Type(f"Option[{inner}]")

    if name in {"dict", "Dict"}:
        return _translate_dict(node)

    if name in {"tuple", "Tuple", "set", "Set"}:
        raise error(
            "TY0004",
            f"`{name}` has no tuonelang equivalent",
            node,
            "tuonelang v0 has no tuple or set type; use a struct or Array[T]",
        )

    raise error("TY0002", f"unsupported generic type `{name}`", node)


def _translate_dict(node: ast.Subscript) -> Type:
    """Translate `dict[K, V]`, refusing the pairs tuonelang has no surface for.

    tuonelang v0 supports exactly `Map[Str, Int]` and `Map[Int, Int]`
    (ADR-0011). Those two cover the counting/indexing dicts that motivate most
    translatable Python; every other pair is refused here rather than at the
    tuonelang layer, so the message can name the *Python* type the user wrote.
    """
    slice_node = node.slice
    if not isinstance(slice_node, ast.Tuple) or len(slice_node.elts) != 2:
        raise error(
            "TY0004",
            "`dict` needs both a key and a value type",
            node,
            "write dict[str, int] or dict[int, int]",
        )

    key = translate_annotation(slice_node.elts[0], owner=node)
    value = translate_annotation(slice_node.elts[1], owner=node)
    # A `str` KEY is the borrowed `Str`: a map key is compared, never owned.
    key_spelling = "Str" if key.is_str_like else key.spelling

    if (key_spelling, value.spelling) not in SUPPORTED_MAPS:
        written = ast.unparse(node)
        raise error(
            "TY0004",
            f"`{written}` has no tuonelang equivalent",
            node,
            "tuonelang v0 has exactly two map shapes: Map[Str, Int] and "
            "Map[Int, Int] (ADR-0011)",
            "translate as dict[str, int] or dict[int, int], or use a struct "
            "when the value type is fixed and known",
        )
    return map_of(Type(key_spelling), value)


def unify(left: Type, right: Type, node: ast.AST, context: str) -> Type:
    """Require two types to match, or raise a positioned type error.

    ``Str`` and ``String`` unify to ``Str`` because a borrowed view compares
    equal to an owned string in tuonelang's own string operations.
    """
    if left == right:
        return left
    if left.is_str_like and right.is_str_like:
        return STR
    raise CompileError(
        error("TY0005", f"type mismatch in {context}: `{left}` vs `{right}`", node).diagnostic
    )
