"""The Python -> tuonelang translator.

Scope
-----
This translates a **statically typed, annotated subset** of Python. That
restriction is not a limitation to be lifted later; it is the reason the tool
can be trusted. Python is dynamically typed, garbage collected, and has
arbitrary-precision integers, while tuonelang is statically typed, ownership
checked, and traps on integer overflow. A total translation does not exist, so
the tool translates the overlap exactly and refuses everything else with a
positioned diagnostic naming the construct.

Guarantees
----------
* Every emitted construct is inside tuonelang's **runnable core** -- the tier
  that `tuo run` executes, not merely what `tuo check` accepts.
* Nothing is emitted on a guess. A construct that cannot be translated
  faithfully raises :class:`~py2tuo.diagnostics.CompileError`.
* The output is verified by the **real** `tuo` binary (see ``verify.py``),
  never asserted to be correct by this module.
"""

from __future__ import annotations

import ast
from dataclasses import dataclass, field

from .diagnostics import CompileError, Diagnostic, error, unsupported
from .emit import Writer
from .types import (
    BOOL,
    FLOAT,
    INT,
    STR,
    STRING,
    UNIT,
    Type,
    array_of,
    element_of,
    is_map,
    map_parts,
    translate_annotation,
)

#: Python binary operators that map onto a tuonelang operator directly.
_BINOPS: dict[type[ast.operator], str] = {
    ast.Add: "+",
    ast.Sub: "-",
    ast.Mult: "*",
    ast.BitAnd: "&",
    ast.BitOr: "|",
    ast.BitXor: "^",
    ast.LShift: "<<",
    ast.RShift: ">>",
}

_COMPARES: dict[type[ast.cmpop], str] = {
    ast.Eq: "==",
    ast.NotEq: "!=",
    ast.Lt: "<",
    ast.LtE: "<=",
    ast.Gt: ">",
    ast.GtE: ">=",
}

#: Reserved words in tuonelang that a Python identifier must not collide with.
_RESERVED = {
    "fn", "let", "var", "const", "if", "else", "while", "for", "in", "match",
    "struct", "enum", "impl", "module", "import", "pub", "spec", "given",
    "when", "then", "take", "mut", "return", "loop", "as", "true", "false",
}


#: Prefix for compiler-generated helper functions, chosen so it cannot collide
#: with a translated Python identifier (which may not start with `__`).
_HELPER_PREFIX = "py_"

#: Self-contained tuonelang implementations of the Python builtins that would
#: otherwise require a stdlib source module to be supplied alongside the output.
_HELPER_SOURCE: dict[str, str] = {
    "abs": """/// `abs` -- the magnitude of an integer.
fn py_abs(take n: Int) -> Int {
    if n < 0 { 0 - n } else { n }
}
""",
    "min": """/// `min` -- the smaller of two integers.
fn py_min(take a: Int, take b: Int) -> Int {
    if a < b { a } else { b }
}
""",
    "max": """/// `max` -- the larger of two integers.
fn py_max(take a: Int, take b: Int) -> Int {
    if a > b { a } else { b }
}
""",
    "sum": """/// `sum` -- the total of every element.
fn py_sum(in xs: Array[Int]) -> Int {
    var acc = 0;
    var i = 0;
    while i < std::array::len(xs) {
        acc = acc + std::array::get(xs, i);
        i = i + 1;
    }
    acc
}
""",
    "reversed": """/// `reversed` -- a new array in the opposite order.
fn py_reversed(in xs: Array[Int]) -> Array[Int] {
    var out = std::array::empty();
    var i = std::array::len(xs);
    while i > 0 {
        i = i - 1;
        std::array::push(out, std::array::get(xs, i));
    }
    out
}
""",
    "sorted": """/// `sorted` -- a new array in ascending order (insertion sort).
fn py_sorted(in xs: Array[Int]) -> Array[Int] {
    var out = std::array::empty();
    var i = 0;
    while i < std::array::len(xs) {
        let value = std::array::get(xs, i);
        var j = 0;
        var placed = false;
        var next = std::array::empty();
        while j < std::array::len(out) {
            let current = std::array::get(out, j);
            if !placed && value < current {
                std::array::push(next, value);
                placed = true;
            }
            std::array::push(next, current);
            j = j + 1;
        }
        if !placed {
            std::array::push(next, value);
        }
        out = next;
        i = i + 1;
    }
    out
}
""",
}


@dataclass
class Local:
    """A translated local binding."""

    name: str
    ty: Type
    mutable: bool


@dataclass
class FunctionSig:
    """A translated function signature, used to type-check call sites."""

    name: str
    params: list[tuple[str, str, Type]]  # (mode, name, type)
    ret: Type


@dataclass
class Scope:
    """Local bindings visible while translating one function body.

    ``mutable_names`` is the pre-scanned set of names the body assigns to more
    than once, which decides `let` versus `var` at binding time.
    """

    locals: dict[str, Local] = field(default_factory=dict)
    mutable_names: set[str] = field(default_factory=set)

    def get(self, name: str) -> Local | None:
        return self.locals.get(name)

    def declare(self, local: Local) -> None:
        self.locals[local.name] = local


class Compiler:
    """Translates one Python module into one tuonelang module."""

    def __init__(self, module_name: str) -> None:
        self.module_name = module_name
        self.signatures: dict[str, FunctionSig] = {}
        self.diagnostics: list[Diagnostic] = []
        self._imports: set[str] = set()
        # Statements that a translated expression needs emitted before the
        # statement containing it (a list literal, for instance, is built by a
        # sequence of pushes). Drained by `_statement`.
        self._prelude: list[str] = []
        # Names of the generated helper functions the output needs (see
        # `_helper_call`), emitted after the translated functions.
        self._helpers: set[str] = set()

    # -- entry point ----------------------------------------------------

    def compile_module(self, tree: ast.Module, *, source: str) -> str:
        """Translate a parsed Python module into tuonelang source text."""
        functions = self._collect_functions(tree)

        # Two passes: signatures first, so a call to a function defined later
        # in the file resolves. Python allows forward references at runtime;
        # tuonelang resolves the whole module, so order does not matter there
        # either -- but this compiler needs the types.
        for fn in functions:
            sig = self._signature(fn)
            if sig.name in self.signatures:
                raise error(
                    "PY0002",
                    f"function `{sig.name}` is defined more than once",
                    fn,
                    "tuonelang has no overloading: a call resolves to exactly one function by name",
                )
            self.signatures[sig.name] = sig

        bodies = [self._function(fn) for fn in functions]

        header = Writer()
        header.line(f"module {self.module_name};")
        for imported in sorted(self._imports):
            header.line(f"import {imported};")
        header.line()

        helpers = [_HELPER_SOURCE[name] for name in sorted(self._helpers)]
        return header.render() + "\n".join([*bodies, *helpers])

    def _collect_functions(self, tree: ast.Module) -> list[ast.FunctionDef]:
        """Gather translatable top-level functions, refusing anything else."""
        functions: list[ast.FunctionDef] = []
        for stmt in tree.body:
            if isinstance(stmt, ast.FunctionDef):
                functions.append(stmt)
            elif isinstance(stmt, ast.AsyncFunctionDef):
                raise unsupported(
                    "async function",
                    stmt,
                    "tuonelang has no async/await; concurrency is structured "
                    "fork-join via std::sync::par_map",
                )
            elif isinstance(stmt, ast.ClassDef):
                raise unsupported(
                    "class definition",
                    stmt,
                    "tuonelang v0 has free functions only -- `impl` bodies parse "
                    "but are not lowered, so a class cannot be translated faithfully",
                )
            elif isinstance(stmt, (ast.Import, ast.ImportFrom)):
                # `from typing import Optional` and friends are erased: the
                # annotations they enable are handled by the type mapping.
                self._check_import(stmt)
            elif isinstance(stmt, ast.Expr) and isinstance(stmt.value, ast.Constant):
                continue  # module docstring
            elif isinstance(stmt, (ast.Assign, ast.AnnAssign)):
                raise unsupported(
                    "module-level variable",
                    stmt,
                    "translate module state as a `const` by hand, or move it into a function",
                )
            elif isinstance(stmt, ast.If) and self._is_main_guard(stmt):
                continue  # `if __name__ == "__main__":` is erased
            else:
                raise unsupported("top-level statement", stmt)

        if not functions:
            raise unsupported("module with no functions", tree.body[0] if tree.body else ast.parse(""))
        return functions

    def _check_import(self, stmt: ast.Import | ast.ImportFrom) -> None:
        """Allow only the typing imports the annotation subset needs."""
        module = stmt.module if isinstance(stmt, ast.ImportFrom) else None
        if module == "typing" or (
            isinstance(stmt, ast.Import) and all(a.name == "typing" for a in stmt.names)
        ):
            return
        raise unsupported(
            "import",
            stmt,
            "only `typing` imports are recognised; a Python library has no "
            "tuonelang equivalent, so importing one cannot be translated",
        )

    @staticmethod
    def _is_main_guard(stmt: ast.If) -> bool:
        """Detect `if __name__ == "__main__":`."""
        test = stmt.test
        return (
            isinstance(test, ast.Compare)
            and isinstance(test.left, ast.Name)
            and test.left.id == "__name__"
            and len(test.ops) == 1
            and isinstance(test.ops[0], ast.Eq)
        )

    # -- signatures -----------------------------------------------------

    def _signature(self, fn: ast.FunctionDef) -> FunctionSig:
        """Translate a `def` header into a tuonelang signature."""
        self._reject_exotic_params(fn)
        name = self._identifier(fn.name, fn)
        ret = translate_annotation(fn.returns, owner=fn)

        params: list[tuple[str, str, Type]] = []
        mutated = _mutated_names(fn)
        for arg in fn.args.args:
            ty = translate_annotation(arg.annotation, owner=arg)
            mode = self._parameter_mode(ty, arg.arg in mutated)
            # A borrowed string parameter is `Str`, the read-only view; an
            # owned `String` parameter would force the caller to give up
            # ownership, which a Python call never implies.
            if mode == "in" and ty == STRING:
                ty = STR
            params.append((mode, self._identifier(arg.arg, arg), ty))

        return FunctionSig(name=name, params=params, ret=ret)

    @staticmethod
    def _parameter_mode(ty: Type, mutated: bool) -> str:
        """Choose the tuonelang parameter mode for a translated parameter.

        Python passes object references and mutates in place; tuonelang makes
        that explicit. A parameter the body assigns to needs `mut`; a scalar
        that is only read is cheapest as `take`; anything else is borrowed `in`.
        """
        if mutated:
            return "mut"
        return "take" if ty.is_scalar else "in"

    @staticmethod
    def _reject_exotic_params(fn: ast.FunctionDef) -> None:
        """Refuse parameter forms tuonelang has no spelling for."""
        args = fn.args
        if args.vararg or args.kwarg:
            raise unsupported(
                "*args / **kwargs",
                fn,
                "tuonelang has no variadics: every function takes a fixed parameter list",
            )
        if args.defaults or args.kw_defaults:
            raise unsupported(
                "default argument value",
                fn,
                "tuonelang has no default arguments; pass every argument explicitly",
            )
        if args.kwonlyargs or args.posonlyargs:
            raise unsupported("keyword-only or positional-only parameter", fn)
        if fn.decorator_list:
            raise unsupported("decorator", fn.decorator_list[0])

    @staticmethod
    def _fresh(base: str, scope: Scope) -> str:
        """A name not already bound in ``scope``, for compiler-introduced locals."""
        if base not in scope.locals:
            return base
        counter = 2
        while f"{base}{counter}" in scope.locals:
            counter += 1
        return f"{base}{counter}"

    def _identifier(self, name: str, node: ast.AST) -> str:
        """Validate a Python identifier as a tuonelang identifier."""
        if name in _RESERVED:
            raise error(
                "PY0003",
                f"`{name}` is a reserved word in tuonelang",
                node,
                "rename the Python identifier",
            )
        if name.startswith("__"):
            raise error("PY0003", f"dunder name `{name}` cannot be translated", node)
        return name

    # -- function bodies ------------------------------------------------

    def _function(self, fn: ast.FunctionDef) -> str:
        """Translate one function definition."""
        sig = self.signatures[fn.name]
        scope = Scope(mutable_names=_mutated_names(fn) | self._mut_argument_names(fn))
        for mode, pname, ty in sig.params:
            scope.declare(Local(pname, ty, mutable=(mode == "mut")))

        writer = Writer()
        doc = ast.get_docstring(fn)
        if doc:
            for docline in doc.strip().splitlines():
                writer.line(f"/// {docline.strip()}".rstrip())

        rendered = ", ".join(f"{mode} {pname}: {ty}" for mode, pname, ty in sig.params)
        header = f"pub fn {sig.name}({rendered}) -> {sig.ret}"

        body = [s for s in fn.body if not _is_docstring(s)]
        with writer.block(header):
            self._block(body, scope, writer, sig)

        return writer.render()

    def _mut_argument_names(self, fn: ast.FunctionDef) -> set[str]:
        """Locals passed where a callee takes a `mut` parameter.

        tuonelang refuses a `mut` borrow of a `let` binding (O0004), so such a
        local must be emitted as `var`. Callee signatures are only known after
        the signature pass, which is why this is separate from the syntactic
        mutation pre-scan in :func:`_mutated_names`.
        """
        names: set[str] = set()
        for node in ast.walk(fn):
            if not isinstance(node, ast.Call) or not isinstance(node.func, ast.Name):
                continue
            sig = self.signatures.get(node.func.id)
            if sig is None:
                continue
            for arg, (mode, _, _) in zip(node.args, sig.params, strict=False):
                if mode == "mut" and isinstance(arg, ast.Name):
                    names.add(arg.id)
        return names

    def _block(
        self,
        body: list[ast.stmt],
        scope: Scope,
        writer: Writer,
        sig: FunctionSig,
        *,
        tail: bool = True,
    ) -> None:
        """Translate a statement list into the body of a tuonelang block.

        ``tail`` marks whether this list is in the function's value position.
        Only then may its final `return` become tuonelang's bare tail
        expression; anywhere else a `return` needs the explicit keyword, or the
        early-exit would silently become a discarded block value.
        """
        if not body:
            if sig.ret != UNIT:
                raise error("TY0006", "function body is empty but returns a value", sig_node(sig))
            return

        for index, stmt in enumerate(body):
            last = tail and index == len(body) - 1
            self._statement(stmt, scope, writer, sig, tail=last)

    def _statement(
        self,
        stmt: ast.stmt,
        scope: Scope,
        writer: Writer,
        sig: FunctionSig,
        *,
        tail: bool,
    ) -> None:
        """Translate one Python statement, emitting any expression prelude first.

        Translating an expression may need statements of its own (building a
        list literal, for example). Those accumulate in ``self._prelude`` and
        are flushed ahead of the statement that needed them.
        """
        buffered = Writer()
        self._prelude = []
        self._statement_inner(stmt, scope, buffered, sig, tail=tail)
        for line in self._prelude:
            writer.line(line)
        self._prelude = []
        for line in buffered.render().splitlines():
            writer.line(line)

    def _statement_inner(
        self,
        stmt: ast.stmt,
        scope: Scope,
        writer: Writer,
        sig: FunctionSig,
        *,
        tail: bool,
    ) -> None:
        """Translate one Python statement (see :meth:`_statement`)."""
        if isinstance(stmt, ast.Return):
            self._return(stmt, scope, writer, sig, tail=tail)
        elif isinstance(stmt, ast.AnnAssign):
            self._ann_assign(stmt, scope, writer)
        elif isinstance(stmt, ast.Assign):
            self._assign(stmt, scope, writer)
        elif isinstance(stmt, ast.AugAssign):
            self._aug_assign(stmt, scope, writer)
        elif isinstance(stmt, ast.If):
            self._if(stmt, scope, writer, sig, tail=tail)
        elif isinstance(stmt, ast.While):
            self._while(stmt, scope, writer, sig)
        elif isinstance(stmt, ast.For):
            self._for(stmt, scope, writer, sig)
        elif isinstance(stmt, ast.Expr):
            self._expr_statement(stmt, scope, writer)
        elif isinstance(stmt, ast.Pass):
            pass
        elif isinstance(stmt, (ast.Break, ast.Continue)):
            writer.line("break;" if isinstance(stmt, ast.Break) else "continue;")
        elif isinstance(stmt, (ast.Try, ast.Raise)):
            raise unsupported(
                "exception handling",
                stmt,
                "tuonelang has no exceptions: return Result[T, E] and match on it",
            )
        elif isinstance(stmt, ast.With):
            raise unsupported("`with` statement", stmt)
        elif isinstance(stmt, (ast.Global, ast.Nonlocal)):
            raise unsupported("global/nonlocal binding", stmt)
        elif isinstance(stmt, (ast.FunctionDef, ast.AsyncFunctionDef)):
            raise unsupported(
                "nested function",
                stmt,
                "tuonelang function values must capture nothing; define the "
                "function at module level instead",
            )
        elif isinstance(stmt, ast.Assert):
            raise unsupported(
                "assert statement",
                stmt,
                "translate assertions into a colocated `spec` block by hand",
            )
        else:
            raise unsupported(type(stmt).__name__, stmt)

    def _return(
        self,
        stmt: ast.Return,
        scope: Scope,
        writer: Writer,
        sig: FunctionSig,
        *,
        tail: bool,
    ) -> None:
        """Translate `return`.

        A trailing return becomes tuonelang's tail expression (no semicolon,
        which is what makes it the block's value). A `return` anywhere else
        needs the explicit `return` keyword.
        """
        if stmt.value is None:
            if sig.ret != UNIT:
                raise error("TY0007", f"bare `return` in a function returning `{sig.ret}`", stmt)
            if not tail:
                writer.line("return;")
            return

        value, ty = self._expression(stmt.value, scope)
        self._require_assignable(ty, sig.ret, stmt, "return value")
        if tail:
            writer.line(value)
        else:
            writer.line(f"return {value};")

    def _ann_assign(self, stmt: ast.AnnAssign, scope: Scope, writer: Writer) -> None:
        """Translate an annotated assignment `x: int = e`."""
        if not isinstance(stmt.target, ast.Name):
            raise unsupported("annotated assignment to a non-name", stmt)
        if stmt.value is None:
            raise error("PY0004", "declaration without an initialiser", stmt,
                        "tuonelang bindings are always initialised")
        declared = translate_annotation(stmt.annotation, owner=stmt)

        # `d: dict[str, int] = {}` -- the annotation supplies the key/value
        # types an empty literal cannot carry on its own.
        if is_map(declared) and isinstance(stmt.value, ast.Dict) and not stmt.value.keys:
            name = self._identifier(stmt.target.id, stmt)
            # A map is only useful when entries can be added, and `insert` needs
            # a `mut` place, so an empty-map binding is always `var`.
            writer.line(f"var {name}: {declared} = std::map::empty();")
            scope.declare(Local(name, declared, mutable=True))
            return

        value, ty = self._expression(stmt.value, scope)
        self._require_assignable(ty, declared, stmt, "initialiser")
        name = self._identifier(stmt.target.id, stmt)
        self._bind(name, declared, value, scope, writer, annotate=True)

    def _assign(self, stmt: ast.Assign, scope: Scope, writer: Writer) -> None:
        """Translate a plain assignment: a new binding or a reassignment."""
        if len(stmt.targets) != 1:
            raise unsupported("chained assignment", stmt)
        target = stmt.targets[0]

        if isinstance(target, ast.Name):
            value, ty = self._expression(stmt.value, scope)
            name = self._identifier(target.id, stmt)
            existing = scope.get(name)
            if existing is None:
                self._bind(name, ty, value, scope, writer, annotate=False)
            else:
                self._require_assignable(ty, existing.ty, stmt, f"assignment to `{name}`")
                if not existing.mutable:
                    # Python rebinds freely; tuonelang needs `var`. The
                    # mutability pre-scan should have caught this.
                    raise error(
                        "SEM0001",
                        f"`{name}` is reassigned but was not declared mutable",
                        stmt,
                    )
                writer.line(f"{name} = {value};")
            return

        if isinstance(target, ast.Subscript):
            self._subscript_assign(target, stmt.value, scope, writer)
            return

        if isinstance(target, ast.Tuple):
            raise unsupported(
                "tuple unpacking",
                stmt,
                "tuonelang v0 has no tuple type; assign each name separately",
            )
        if isinstance(target, ast.Attribute):
            raise unsupported("attribute assignment", stmt)
        raise unsupported("assignment target", stmt)

    def _subscript_assign(
        self,
        target: ast.Subscript,
        value_node: ast.expr,
        scope: Scope,
        writer: Writer,
    ) -> None:
        """Translate `xs[i] = v` into `std::array::set(mut xs, i, v)`."""
        container, container_ty = self._expression(target.value, scope)

        if is_map(container_ty):
            self._map_assign(target, value_node, container, container_ty, scope, writer)
            return

        element = element_of(container_ty)
        if element is None:
            raise error(
                "TY0008",
                f"cannot index-assign into `{container_ty}`",
                target,
                "only Array[T] and Map[K, V] support indexed assignment",
            )
        self._reject_negative_index(target.slice)
        index, index_ty = self._expression(target.slice, scope)
        self._require_assignable(index_ty, INT, target, "index")
        value, value_ty = self._expression(value_node, scope)
        self._require_assignable(value_ty, element, target, "assigned element")

        if not isinstance(target.value, ast.Name):
            raise unsupported("index-assignment to a temporary", target)
        local = scope.get(target.value.id)
        if local is not None and not local.mutable:
            raise error("SEM0001", f"`{local.name}` is mutated but not declared mutable", target)

        writer.line(f"std::array::set({container}, {index}, {value});")

    def _aug_assign(self, stmt: ast.AugAssign, scope: Scope, writer: Writer) -> None:
        """Translate `x += e` -- tuonelang has no compound assignment."""
        if not isinstance(stmt.target, ast.Name):
            raise unsupported("augmented assignment to a non-name", stmt)
        op = _BINOPS.get(type(stmt.op))
        if op is None:
            raise unsupported(f"augmented operator {type(stmt.op).__name__}", stmt)

        name = self._identifier(stmt.target.id, stmt)
        local = scope.get(name)
        if local is None:
            raise error("PY0005", f"`{name}` is not defined", stmt)
        if not local.mutable:
            raise error("SEM0001", f"`{name}` is mutated but not declared mutable", stmt)
        value, ty = self._expression(stmt.value, scope)
        self._require_assignable(ty, local.ty, stmt, "augmented assignment")
        writer.line(f"{name} = {name} {op} {value};")

    def _bind(
        self,
        name: str,
        ty: Type,
        value: str,
        scope: Scope,
        writer: Writer,
        *,
        annotate: bool,
    ) -> None:
        """Emit a `let`/`var` binding and record it in scope."""
        mutable = name in scope.mutable_names
        keyword = "var" if mutable else "let"
        suffix = f": {ty}" if annotate else ""
        writer.line(f"{keyword} {name}{suffix} = {value};")
        scope.declare(Local(name, ty, mutable))

    def _if(
        self,
        stmt: ast.If,
        scope: Scope,
        writer: Writer,
        sig: FunctionSig,
        *,
        tail: bool,
    ) -> None:
        """Translate an `if` statement.

        When the statement is the function's last, and both arms end in a
        value-producing `return`, the whole `if` is the block's value and its
        arms emit tail expressions. Otherwise every `return` inside is an early
        exit and must use the explicit `return` keyword -- emitting a bare tail
        expression there would silently discard the value.
        """
        test, test_ty = self._expression(stmt.test, scope)
        self._require_assignable(test_ty, BOOL, stmt, "if condition")

        body = [s for s in stmt.body if not isinstance(s, ast.Pass)]
        orelse = [s for s in stmt.orelse if not isinstance(s, ast.Pass)]

        # Both arms must produce a value for the `if` to *be* the block's value.
        arms_are_values = (
            tail
            and bool(orelse)
            and _ends_in_value(body)
            and _ends_in_value(orelse)
        )

        if not orelse:
            with writer.block(f"if {test}") as inner:
                self._block(body, _child(scope), inner, sig, tail=arms_are_values)
            return

        # `if ... { } else { }` -- the closing brace of the `then` arm opens the
        # `else` arm, so the writer emits `} else {` as one line.
        with writer.block(f"if {test}", close="} else {") as inner:
            self._block(body, _child(scope), inner, sig, tail=arms_are_values)
        writer.indent()
        if len(orelse) == 1 and isinstance(orelse[0], ast.If):
            # `elif` -- translate the nested `if` as the sole else-arm statement.
            self._statement(orelse[0], _child(scope), writer, sig, tail=arms_are_values)
        else:
            self._block(orelse, _child(scope), writer, sig, tail=arms_are_values)
        writer.dedent()
        writer.line("}")

    def _while(self, stmt: ast.While, scope: Scope, writer: Writer, sig: FunctionSig) -> None:
        """Translate a `while` loop."""
        if stmt.orelse:
            raise unsupported("`while ... else`", stmt)
        test, test_ty = self._expression(stmt.test, scope)
        self._require_assignable(test_ty, BOOL, stmt, "while condition")
        with writer.block(f"while {test}") as inner:
            self._block(stmt.body, scope, inner, sig, tail=False)

    def _for(self, stmt: ast.For, scope: Scope, writer: Writer, sig: FunctionSig) -> None:
        """Translate a `for` loop over `range(...)` or an array."""
        if stmt.orelse:
            raise unsupported("`for ... else`", stmt)
        if not isinstance(stmt.target, ast.Name):
            raise unsupported("destructuring `for` target", stmt)
        name = self._identifier(stmt.target.id, stmt)

        iterable = stmt.iter
        if isinstance(iterable, ast.Call) and isinstance(iterable.func, ast.Name) and iterable.func.id == "range":
            self._for_range(name, iterable, stmt, scope, writer, sig)
            return

        value, ty = self._expression(iterable, scope)
        element = element_of(ty)
        if element is None:
            raise error(
                "TY0009",
                f"cannot iterate over `{ty}`",
                stmt,
                "tuonelang iterates arrays and `range`-style counted loops only",
            )

        # tuonelang's `for` CONSUMES the array it iterates, so it is only legal
        # over a value the loop owns. Python's `for` borrows -- the list is
        # still usable afterwards -- so the faithful translation is the indexed
        # `while`, which reads through the borrow without moving out of it.
        # (Emitting `for` here would be rejected with O0003 "cannot move out of
        # `in xs`", and, worse, would misrepresent Python's semantics.)
        index_name = self._fresh(f"{name}_i", scope)
        inner_scope = _child(scope)
        inner_scope.declare(Local(index_name, INT, mutable=True))
        inner_scope.declare(Local(name, element, mutable=False))

        writer.line(f"var {index_name} = 0;")
        with writer.block(f"while {index_name} < std::array::len({value})") as inner:
            inner.line(f"let {name} = std::array::get({value}, {index_name});")
            self._block(stmt.body, inner_scope, inner, sig, tail=False)
            inner.line(f"{index_name} = {index_name} + 1;")

    def _for_range(
        self,
        name: str,
        call: ast.Call,
        stmt: ast.For,
        scope: Scope,
        writer: Writer,
        sig: FunctionSig,
    ) -> None:
        """Translate `for i in range(a, b)` into a counted `while` loop.

        tuonelang's `for` iterates arrays; a numeric range is therefore lowered
        to the explicit counted loop, which is exactly what the language's own
        idiom is.
        """
        if call.keywords:
            raise unsupported("keyword arguments to range()", call)
        args = call.args
        if len(args) == 1:
            start, start_ty = "0", INT
            stop, stop_ty = self._expression(args[0], scope)
        elif len(args) == 2:
            start, start_ty = self._expression(args[0], scope)
            stop, stop_ty = self._expression(args[1], scope)
        else:
            raise unsupported(
                "range() with a step",
                call,
                "translate a stepped range into an explicit while loop",
            )
        self._require_assignable(start_ty, INT, call, "range start")
        self._require_assignable(stop_ty, INT, call, "range stop")

        inner_scope = _child(scope)
        inner_scope.declare(Local(name, INT, mutable=True))
        writer.line(f"var {name} = {start};")
        with writer.block(f"while {name} < {stop}") as inner:
            self._block(stmt.body, inner_scope, inner, sig, tail=False)
            inner.line(f"{name} = {name} + 1;")

    def _expr_statement(self, stmt: ast.Expr, scope: Scope, writer: Writer) -> None:
        """Translate an expression used as a statement (a call for effect)."""
        if isinstance(stmt.value, ast.Constant):
            return  # stray docstring
        value, ty = self._expression(stmt.value, scope)
        writer.line(f"{value};" if ty == UNIT else f"let _ = {value};")

    # -- expressions ----------------------------------------------------

    def _expression(self, node: ast.expr, scope: Scope) -> tuple[str, Type]:
        """Translate an expression, returning its rendered text and type."""
        if isinstance(node, ast.Constant):
            return self._constant(node)
        if isinstance(node, ast.Name):
            return self._name(node, scope)
        if isinstance(node, ast.BinOp):
            return self._binop(node, scope)
        if isinstance(node, ast.UnaryOp):
            return self._unaryop(node, scope)
        if isinstance(node, ast.BoolOp):
            return self._boolop(node, scope)
        if isinstance(node, ast.Compare):
            return self._compare(node, scope)
        if isinstance(node, ast.IfExp):
            return self._ifexp(node, scope)
        if isinstance(node, ast.Call):
            return self._call(node, scope)
        if isinstance(node, ast.Subscript):
            return self._index(node, scope)
        if isinstance(node, ast.List):
            return self._list(node, scope)
        if isinstance(node, (ast.ListComp, ast.SetComp, ast.DictComp, ast.GeneratorExp)):
            raise unsupported(
                "comprehension",
                node,
                "translate into an explicit loop, or use std::collections::map_into "
                "with a named top-level function",
            )
        if isinstance(node, ast.Lambda):
            raise unsupported(
                "lambda",
                node,
                "tuonelang function values must capture nothing; define a named "
                "top-level function and pass it by name",
            )
        if isinstance(node, ast.JoinedStr):
            raise unsupported(
                "f-string",
                node,
                "build strings with std::str::builder / push / push_int",
            )
        if isinstance(node, ast.Attribute):
            raise unsupported(
                "attribute access",
                node,
                "tuonelang v0 has free functions only: write len(xs), never xs.len()",
            )
        if isinstance(node, ast.Tuple):
            raise unsupported("tuple expression", node, "tuonelang v0 has no tuple type")
        if isinstance(node, ast.Dict):
            return self._dict_literal(node, scope)
        raise unsupported(type(node).__name__, node)

    def _constant(self, node: ast.Constant) -> tuple[str, Type]:
        """Translate a literal."""
        value = node.value
        if isinstance(value, bool):
            return ("true" if value else "false", BOOL)
        if isinstance(value, int):
            if value < 0:
                # Unary minus is an expression, not part of the literal.
                return (f"(0 - {abs(value)})", INT)
            if value.bit_length() > 63:
                raise error(
                    "SEM0002",
                    "integer literal does not fit in a 64-bit Int",
                    node,
                    "Python integers are arbitrary precision; tuonelang's Int is I64 "
                    "and traps on overflow. Use std::bignum for wider values.",
                )
            return (str(value), INT)
        if isinstance(value, float):
            rendered = repr(value)
            if "." not in rendered and "e" not in rendered and "E" not in rendered:
                rendered += ".0"
            return (rendered, FLOAT)
        if isinstance(value, str):
            return (_string_literal(value, node), STR)
        if value is None:
            raise unsupported("`None` literal", node, "use Option[T] with None/Some { value: .. }")
        raise unsupported(f"literal of type {type(value).__name__}", node)

    def _name(self, node: ast.Name, scope: Scope) -> tuple[str, Type]:
        """Translate a variable reference."""
        local = scope.get(node.id)
        if local is not None:
            return (local.name, local.ty)
        if node.id in self.signatures:
            # A bare function name is a first-class function value.
            sig = self.signatures[node.id]
            params = ", ".join(f"{mode} {ty}" for mode, _, ty in sig.params)
            return (sig.name, Type(f"fn({params}) -> {sig.ret}"))
        if node.id in {"True", "False"}:
            return (node.id.lower(), BOOL)
        raise error(
            "PY0005",
            f"`{node.id}` is not defined",
            node,
            "only parameters, locals, and module-level functions are in scope",
        )

    def _binop(self, node: ast.BinOp, scope: Scope) -> tuple[str, Type]:
        """Translate a binary operation."""
        left, left_ty = self._expression(node.left, scope)
        right, right_ty = self._expression(node.right, scope)

        if isinstance(node.op, ast.Div):
            raise unsupported(
                "`/` true division",
                node,
                "Python's `/` yields a float even for ints. Write `//` for integer "
                "division, or annotate the operands as float.",
            )
        if isinstance(node.op, ast.Pow):
            self._require_assignable(left_ty, INT, node, "pow base")
            self._require_assignable(right_ty, INT, node, "pow exponent")
            self._imports.add("std::math")
            return (f"std::math::pow({left}, {right})", INT)
        if isinstance(node.op, ast.FloorDiv):
            op = "/"
        elif isinstance(node.op, ast.Mod):
            op = "%"
        else:
            mapped = _BINOPS.get(type(node.op))
            if mapped is None:
                raise unsupported(f"operator {type(node.op).__name__}", node)
            op = mapped

        # Python's `+` on strings is concatenation. tuonelang has no `+`
        # operator on strings, but `std::string::concat(Str, Str) -> String` is
        # exactly that operation, so the translation is faithful.
        if left_ty.is_str_like or right_ty.is_str_like:
            if not (isinstance(node.op, ast.Add) and left_ty.is_str_like and right_ty.is_str_like):
                raise error(
                    "TY0019",
                    f"cannot apply `{type(node.op).__name__}` to `{left_ty}` and `{right_ty}`",
                    node,
                    "only `+` (concatenation) is defined on strings",
                )
            left_view = self._as_str(left, left_ty)
            right_view = self._as_str(right, right_ty)
            return (f"std::string::concat({left_view}, {right_view})", STRING)
        if op in {"&", "|", "^", "<<", ">>"}:
            self._require_assignable(left_ty, INT, node, "bitwise operand")
            self._require_assignable(right_ty, INT, node, "bitwise operand")
            return (f"({left} {op} {right})", INT)

        result = self._numeric_result(left_ty, right_ty, node)
        return (f"({left} {op} {right})", result)

    @staticmethod
    def _as_str(rendered: str, ty: Type) -> str:
        """Render a string value as the borrowed `Str` a builtin expects.

        `std::string::concat` takes two `Str` views; an owned `String` reaches
        one through `std::string::as_str`, which ADR-0010 lowers as a zero-copy
        two-word view.
        """
        return f"std::string::as_str({rendered})" if ty == STRING else rendered

    def _numeric_result(self, left: Type, right: Type, node: ast.AST) -> Type:
        """The result type of an arithmetic operation, refusing mixed numerics."""
        if left == right and left in (INT, FLOAT):
            return left
        if {left, right} == {INT, FLOAT}:
            raise error(
                "TY0010",
                "mixed Int and Float arithmetic",
                node,
                "tuonelang has no implicit numeric conversion; cast explicitly with `as`",
            )
        raise error("TY0010", f"arithmetic on `{left}` and `{right}`", node)

    def _unaryop(self, node: ast.UnaryOp, scope: Scope) -> tuple[str, Type]:
        """Translate a unary operation."""
        operand, ty = self._expression(node.operand, scope)
        if isinstance(node.op, ast.USub):
            if ty not in (INT, FLOAT):
                raise error("TY0011", f"cannot negate `{ty}`", node)
            # tuonelang has no unary minus on an arbitrary expression in v0's
            # idiom; `0 - x` is the explicit form the cheatsheet uses.
            return (f"(0 - {operand})", ty) if ty == INT else (f"(0.0 - {operand})", ty)
        if isinstance(node.op, ast.Not):
            self._require_assignable(ty, BOOL, node, "`not` operand")
            return (f"(!{operand})", BOOL)
        if isinstance(node.op, ast.Invert):
            self._require_assignable(ty, INT, node, "`~` operand")
            return (f"(~{operand})", INT)
        if isinstance(node.op, ast.UAdd):
            return (operand, ty)
        raise unsupported(f"unary {type(node.op).__name__}", node)

    def _boolop(self, node: ast.BoolOp, scope: Scope) -> tuple[str, Type]:
        """Translate `and` / `or`."""
        op = "&&" if isinstance(node.op, ast.And) else "||"
        parts = []
        for value in node.values:
            rendered, ty = self._expression(value, scope)
            self._require_assignable(ty, BOOL, value, f"`{op}` operand")
            parts.append(rendered)
        return ("(" + f" {op} ".join(parts) + ")", BOOL)

    def _compare(self, node: ast.Compare, scope: Scope) -> tuple[str, Type]:
        """Translate a comparison, expanding Python's chained form."""
        if len(node.ops) > 1:
            # `a < b < c` is legal Python and a parse error in tuonelang;
            # expand it into the explicit conjunction rather than refusing.
            operands = [node.left, *node.comparators]
            parts = []
            for index, op in enumerate(node.ops):
                mapped = self._compare_op(op, node)
                left, left_ty = self._expression(operands[index], scope)
                right, right_ty = self._expression(operands[index + 1], scope)
                self._require_comparable(left_ty, right_ty, node)
                parts.append(f"({left} {mapped} {right})")
            return ("(" + " && ".join(parts) + ")", BOOL)

        if isinstance(node.ops[0], (ast.In, ast.NotIn)):
            membership = self._membership(node, scope)
            if membership is not None:
                return membership

        op = self._compare_op(node.ops[0], node)
        left, left_ty = self._expression(node.left, scope)
        right, right_ty = self._expression(node.comparators[0], scope)
        self._require_comparable(left_ty, right_ty, node)
        return (f"({left} {op} {right})", BOOL)

    @staticmethod
    def _compare_op(op: ast.cmpop, node: ast.AST) -> str:
        """Map a Python comparison operator, refusing the ones with no analogue."""
        mapped = _COMPARES.get(type(op))
        if mapped is not None:
            return mapped
        if isinstance(op, (ast.Is, ast.IsNot)):
            raise unsupported(
                "`is` / `is not`",
                node,
                "tuonelang has no object identity; compare values with `==`",
            )
        if isinstance(op, (ast.In, ast.NotIn)):
            raise unsupported(
                "`in` / `not in`",
                node,
                "on a dict this is `std::map::contains_key`, which the translator "
                "emits for `k in d` when `d` is a dict; on a list or string, use "
                "std::collections::contains(xs, needle) or std::str::contains(s, needle)",
            )
        raise unsupported(f"comparison {type(op).__name__}", node)

    def _membership(self, node: ast.Compare, scope: Scope) -> tuple[str, Type] | None:
        """Translate `k in d` / `k not in d` when the right side is a dict."""
        container, container_ty = self._expression(node.comparators[0], scope)
        if not is_map(container_ty):
            return None

        parts = map_parts(container_ty)
        assert parts is not None
        key_ty, _ = parts
        key, actual_key = self._expression(node.left, scope)
        self._require_assignable(actual_key, key_ty, node, "dict key")

        call = f"std::map::contains_key({container}, {key})"
        if isinstance(node.ops[0], ast.NotIn):
            return (f"(!{call})", BOOL)
        return (call, BOOL)

    def _require_comparable(self, left: Type, right: Type, node: ast.AST) -> None:
        """Require two compared operands to have a common type."""
        if left == right or (left.is_str_like and right.is_str_like):
            return
        raise error("TY0012", f"cannot compare `{left}` with `{right}`", node)

    def _ifexp(self, node: ast.IfExp, scope: Scope) -> tuple[str, Type]:
        """Translate `a if cond else b` -- tuonelang's `if` is an expression."""
        test, test_ty = self._expression(node.test, scope)
        self._require_assignable(test_ty, BOOL, node, "conditional expression test")
        then, then_ty = self._expression(node.body, scope)
        other, other_ty = self._expression(node.orelse, scope)
        if then_ty != other_ty:
            raise error(
                "TY0013",
                f"conditional branches have different types: `{then_ty}` vs `{other_ty}`",
                node,
                "both arms of a tuonelang `if` expression must have the same type",
            )
        return (f"if {test} {{ {then} }} else {{ {other} }}", then_ty)

    def _index(self, node: ast.Subscript, scope: Scope) -> tuple[str, Type]:
        """Translate `xs[i]`."""
        container, container_ty = self._expression(node.value, scope)

        if is_map(container_ty):
            return self._map_index(node, container, container_ty, scope)

        element = element_of(container_ty)
        if element is None:
            raise error("TY0008", f"cannot index `{container_ty}`", node)
        if isinstance(node.slice, ast.Slice):
            raise unsupported(
                "slicing",
                node,
                "tuonelang has no slice syntax; loop over the range you need",
            )
        self._reject_negative_index(node.slice)
        index, index_ty = self._expression(node.slice, scope)
        self._require_assignable(index_ty, INT, node, "index")
        return (f"std::array::get({container}, {index})", element)

    def _dict_literal(self, node: ast.Dict, scope: Scope) -> tuple[str, Type]:
        """Translate a dict literal.

        Only `{}` is translated, and only where an annotation fixes the type:
        a non-empty literal would need its key/value types inferred from the
        elements, and the two supported map shapes make that ambiguous more
        often than it is useful. `{}` plus assignments is the shape that
        actually occurs in translatable Python.
        """
        if node.keys:
            raise unsupported(
                "non-empty dict literal",
                node,
                "write `{}` with an annotation (d: dict[str, int] = {}) and "
                "assign the entries",
            )
        _ = scope
        raise error(
            "TY0018",
            "cannot infer the key and value types of an empty dict",
            node,
            "annotate the binding, e.g. `counts: dict[str, int] = {}`",
        )

    def _dict_method(self, node: ast.Call, scope: Scope) -> tuple[str, Type] | None:
        """Translate the dict methods that have a faithful tuonelang spelling.

        `d.get(k, default)` is the important one: it is Python's own way of
        saying "collapse the absent case", which is exactly what tuonelang's
        `Option[V]` needs. `d.keys()` maps onto `std::map::keys`. Every other
        method is left to the general method-call refusal.
        """
        attribute = node.func
        assert isinstance(attribute, ast.Attribute)
        receiver, receiver_ty = self._expression(attribute.value, scope)
        if not is_map(receiver_ty):
            return None

        parts = map_parts(receiver_ty)
        assert parts is not None
        key_ty, value_ty = parts

        if attribute.attr == "get" and len(node.args) == 2:
            key, actual_key = self._expression(node.args[0], scope)
            self._require_assignable(actual_key, key_ty, node, "dict key")
            default, actual_default = self._expression(node.args[1], scope)
            self._require_assignable(actual_default, value_ty, node, "dict default")
            # `match` is an expression, so the Option collapses inline.
            return (
                f"match std::map::get({receiver}, {key}) {{ "
                f"Some {{ value }} => value, None => {default}, }}",
                value_ty,
            )

        if attribute.attr == "get" and len(node.args) == 1:
            raise unsupported(
                "`d.get(k)` without a default",
                node,
                "it yields None in Python and Option[V] in tuonelang, which this "
                "subset has no way to hand back -- pass a default: d.get(k, 0)",
            )

        if attribute.attr == "keys" and not node.args:
            # Python's d.keys() is a lazy VIEW: not subscriptable, and it
            # tracks later mutations of the dict. std::map::keys returns a
            # snapshot Array, so only the materialising `list(d.keys())`
            # spelling is translated -- the bare view would hand the generated
            # program a different object than the Python has.
            raise unsupported(
                "`d.keys()`",
                node,
                "Python's d.keys() is a lazy view -- not subscriptable, and it "
                "reflects later changes to the dict. Write `list(d.keys())`, "
                "which is the snapshot std::map::keys returns.",
            )

        if attribute.attr in {"values", "items", "pop", "update", "setdefault"}:
            raise unsupported(
                f"`dict.{attribute.attr}()`",
                node,
                "std::map exposes get / insert / remove / contains_key / len / keys; "
                f"`{attribute.attr}` has no counterpart",
            )
        return None

    def _map_index(
        self,
        node: ast.Subscript,
        container: str,
        container_ty: Type,
        scope: Scope,
    ) -> tuple[str, Type]:
        """Translate `d[k]` on a map.

        Python's `d[k]` raises `KeyError` when the key is absent; tuonelang's
        `std::map::get` returns `Option[V]`, and tuonelang has no exceptions.
        There is no faithful translation of the raising form, so it is refused
        and the user is pointed at `d.get(k, default)` -- which IS translatable,
        because a default is exactly what `Option` needs to collapse.
        """
        _ = container, scope
        parts = map_parts(container_ty)
        assert parts is not None
        raise error(
            "SEM0005",
            "`d[k]` on a dict has no faithful translation",
            node,
            "Python raises KeyError when the key is absent, and tuonelang has "
            "no exceptions -- its std::map::get returns Option[V] instead",
            "write `d.get(k, default)`, which translates exactly",
        )

    def _map_assign(
        self,
        target: ast.Subscript,
        value_node: ast.expr,
        container: str,
        container_ty: Type,
        scope: Scope,
        writer: Writer,
    ) -> None:
        """Translate `d[k] = v` into `std::map::insert`."""
        parts = map_parts(container_ty)
        assert parts is not None
        key_ty, value_ty = parts

        self._reject_negative_index(target.slice)
        key, actual_key_ty = self._expression(target.slice, scope)
        self._require_assignable(actual_key_ty, key_ty, target, "dict key")
        value, actual_value_ty = self._expression(value_node, scope)
        self._require_assignable(actual_value_ty, value_ty, target, "dict value")

        if not isinstance(target.value, ast.Name):
            raise unsupported("index-assignment to a temporary dict", target)
        local = scope.get(target.value.id)
        if local is not None and not local.mutable:
            raise error("SEM0001", f"`{local.name}` is mutated but not declared mutable", target)

        # `insert` returns the displaced Option[V], which Python's `d[k] = v`
        # discards.
        writer.line(f"let _ = std::map::insert({container}, {key}, {value});")

    @staticmethod
    def _reject_negative_index(index: ast.expr) -> None:
        """Refuse an index that is statically negative.

        Python's `xs[-1]` means the LAST element; tuonelang indexing is
        unsigned-in-effect and traps out of bounds. Translating it literally
        would turn a working Python program into one that aborts at runtime, so
        the divergence is reported at compile time where it can be fixed.

        Only a syntactically negative literal is caught -- a negative value
        arriving through a variable still traps at runtime, which is the safe
        direction (tuonelang aborts rather than reading the wrong element).
        """
        negative_literal = (
            isinstance(index, ast.UnaryOp)
            and isinstance(index.op, ast.USub)
            and isinstance(index.operand, ast.Constant)
            and isinstance(index.operand.value, int)
            and index.operand.value > 0
        )
        subtracted = (
            isinstance(index, ast.BinOp)
            and isinstance(index.op, ast.Sub)
            and isinstance(index.left, ast.Constant)
            and index.left.value == 0
            and isinstance(index.right, ast.Constant)
            and isinstance(index.right.value, int)
            and index.right.value > 0
        )
        if negative_literal or subtracted:
            raise error(
                "SEM0004",
                "negative index has no tuonelang equivalent",
                index,
                "Python's xs[-1] is the LAST element; tuonelang indexing traps "
                "out of bounds. Write xs[len(xs) - 1] instead.",
            )

    def _list(self, node: ast.List, scope: Scope) -> tuple[str, Type]:
        """Translate a list literal into a growable `Array[T]`.

        tuonelang has no literal syntax for the growable array, so the literal
        becomes an `empty()` plus a `push` per element. Those are compiler
        builtins rather than stdlib source, which keeps generated output
        self-contained: it compiles on its own, with no stdlib file to supply.

        Because a literal is an expression but the construction is a statement
        sequence, the elements are hoisted into a pending prelude that the
        enclosing statement emits first.
        """
        if not node.elts:
            raise error(
                "TY0014",
                "cannot infer the element type of an empty list",
                node,
                "annotate the binding, e.g. `xs: list[int] = []`",
            )

        rendered: list[str] = []
        element_ty: Type | None = None
        for item in node.elts:
            text, ty = self._expression(item, scope)
            if element_ty is None:
                element_ty = ty
            elif element_ty != ty:
                raise error("TY0015", f"list mixes `{element_ty}` and `{ty}`", item)
            rendered.append(text)

        assert element_ty is not None
        temp = self._fresh("lit", scope)
        self._prelude.append(f"var {temp} = std::array::empty();")
        for text in rendered:
            self._prelude.append(f"std::array::push({temp}, {text});")
        scope.declare(Local(temp, array_of(element_ty), mutable=True))
        return (temp, array_of(element_ty))

    def _call(self, node: ast.Call, scope: Scope) -> tuple[str, Type]:
        """Translate a call to a builtin or to a translated function."""
        if node.keywords:
            raise unsupported("keyword argument", node, "tuonelang calls are positional")
        if any(isinstance(a, ast.Starred) for a in node.args):
            raise unsupported("argument unpacking", node)

        if isinstance(node.func, ast.Attribute):
            translated = self._dict_method(node, scope)
            if translated is not None:
                return translated
            raise unsupported(
                "method call",
                node,
                "tuonelang v0 has free functions only: write len(xs), never xs.len()",
            )
        if not isinstance(node.func, ast.Name):
            raise unsupported("indirect call expression", node)

        name = node.func.id
        builtin = self._builtin_call(name, node, scope)
        if builtin is not None:
            return builtin

        sig = self.signatures.get(name)
        if sig is None:
            raise error(
                "PY0005",
                f"call to undefined function `{name}`",
                node,
                "only functions defined in this module can be called; a Python "
                "library call has no tuonelang equivalent",
            )
        if len(node.args) != len(sig.params):
            raise error(
                "TY0016",
                f"`{name}` takes {len(sig.params)} argument(s), {len(node.args)} given",
                node,
            )

        rendered: list[str] = []
        for arg, (mode, _, param_ty) in zip(node.args, sig.params, strict=True):
            text, ty = self._expression(arg, scope)
            self._require_assignable(ty, param_ty, arg, f"argument to `{name}`")
            _ = mode  # a call site never spells the mode; the compiler infers it
            rendered.append(text)
        return (f"{name}({', '.join(rendered)})", sig.ret)

    def _builtin_call(
        self, name: str, node: ast.Call, scope: Scope
    ) -> tuple[str, Type] | None:
        """Translate a Python builtin onto its stdlib counterpart, if any."""
        args = node.args

        def one() -> tuple[str, Type]:
            if len(args) != 1:
                raise error("TY0016", f"`{name}` takes exactly 1 argument", node)
            return self._expression(args[0], scope)

        if name == "len":
            value, ty = one()
            if is_map(ty):
                return (f"std::map::len({value})", INT)
            if element_of(ty) is not None:
                return (f"std::array::len({value})", INT)
            if ty.is_str_like:
                return (f"std::string::len({value})", INT)
            raise error("TY0017", f"`len` is not defined for `{ty}`", node)

        if name == "abs":
            value, ty = one()
            self._require_assignable(ty, INT, node, "`abs` argument")
            return (self._helper_call("abs", [value]), INT)

        if name in {"min", "max"}:
            if len(args) != 2:
                raise unsupported(
                    f"`{name}` with {len(args)} arguments",
                    node,
                    "std::core::min/max take exactly two Int arguments",
                )
            left, left_ty = self._expression(args[0], scope)
            right, right_ty = self._expression(args[1], scope)
            self._require_assignable(left_ty, INT, node, f"`{name}` argument")
            self._require_assignable(right_ty, INT, node, f"`{name}` argument")
            return (self._helper_call(name, [left, right]), INT)

        if name == "sum":
            value, ty = one()
            if element_of(ty) != INT:
                raise error("TY0017", f"`sum` expects list[int], got `{ty}`", node)
            return (self._helper_call("sum", [value]), INT)

        if name == "print":
            if len(args) != 1:
                raise unsupported(
                    "`print` with multiple arguments",
                    node,
                    "std::io::println takes exactly one Str",
                )
            value, ty = self._expression(args[0], scope)
            if not ty.is_str_like:
                raise error(
                    "TY0017",
                    f"`print` expects a string, got `{ty}`",
                    node,
                    "convert with std::str::to_string first",
                )
            self._imports.add("std::io")
            return (f"let _ = std::io::println({value})", UNIT)

        if name == "str":
            value, ty = one()
            self._require_assignable(ty, INT, node, "`str` argument")
            self._imports.add("std::str")
            return (f"std::str::to_string({value})", STRING)

        if name == "int":
            value, ty = one()
            if ty == FLOAT:
                return (f"({value} as Int)", INT)
            if ty == INT:
                return (value, INT)
            raise unsupported("`int()` on a string", node, "use std::str::parse_int, which returns Result")

        if name == "float":
            value, ty = one()
            if ty == INT:
                return (f"({value} as Float)", FLOAT)
            if ty == FLOAT:
                return (value, FLOAT)
            raise unsupported("`float()` on a string", node, "use std::str::parse_float")

        if name == "bool":
            raise unsupported(
                "`bool()` coercion",
                node,
                "tuonelang has no truthiness: compare explicitly, e.g. `n != 0`",
            )
        if name == "range":
            raise unsupported(
                "`range` outside a `for` header",
                node,
                "range is translated only as the iterable of a for loop",
            )
        if name == "sorted":
            value, ty = one()
            if element_of(ty) != INT:
                raise error("TY0017", "`sorted` expects list[int], got `" + str(ty) + "`", node)
            return (self._helper_call("sorted", [value]), array_of(INT))

        if name == "reversed":
            raise unsupported(
                "`reversed`",
                node,
                "Python's reversed() returns a lazy iterator, not a list -- it is "
                "not subscriptable and is consumed by one pass, which Array[T] "
                "does not model. Write `list(reversed(xs))` instead.",
            )

        if name == "list":
            # `list(reversed(xs))` is the one materialising form worth having:
            # it is how Python spells "an actual list, reversed".
            if len(args) != 1:
                raise error("TY0016", "`list` takes exactly 1 argument", node)
            inner = args[0]
            if (
                isinstance(inner, ast.Call)
                and isinstance(inner.func, ast.Name)
                and inner.func.id == "reversed"
                and len(inner.args) == 1
            ):
                value, ty = self._expression(inner.args[0], scope)
                if element_of(ty) != INT:
                    raise error("TY0017", "`reversed` expects list[int], got `" + str(ty) + "`", node)
                return (self._helper_call("reversed", [value]), array_of(INT))
            if (
                isinstance(inner, ast.Call)
                and isinstance(inner.func, ast.Attribute)
                and inner.func.attr == "keys"
                and not inner.args
            ):
                receiver, receiver_ty = self._expression(inner.func.value, scope)
                parts = map_parts(receiver_ty)
                if parts is not None:
                    return (f"std::map::keys({receiver})", array_of(parts[0]))
            raise unsupported(
                "`list()` conversion",
                node,
                "only `list(reversed(xs))` and `list(d.keys())` are translated; "
                "a list literal or an explicit loop expresses the rest",
            )
        return None

    def _helper_call(self, name: str, args: list[str]) -> str:
        """Emit a call to a generated helper, requesting its definition.

        Python builtins such as `abs` and `sum` correspond to functions in
        tuonelang's stdlib *source* modules, which a program must be compiled
        alongside. Generating a small private helper from builtins instead keeps
        the output a single self-contained file.
        """
        self._helpers.add(name)
        return f"{_HELPER_PREFIX}{name}({', '.join(args)})"

    # -- helpers --------------------------------------------------------

    def _require_assignable(self, actual: Type, expected: Type, node: ast.AST, context: str) -> None:
        """Require ``actual`` to be usable where ``expected`` is wanted."""
        if actual == expected:
            return
        if expected.is_str_like and actual.is_str_like:
            return
        raise error(
            "TY0005",
            f"type mismatch in {context}: expected `{expected}`, found `{actual}`",
            node,
        )


def _child(scope: Scope) -> Scope:
    """A nested scope that inherits the enclosing bindings.

    tuonelang blocks scope their own bindings; sharing the dictionary would let
    an inner binding leak outward, so the child gets a copy.
    """
    return Scope(locals=dict(scope.locals), mutable_names=scope.mutable_names)


def _string_literal(value: str, node: ast.AST) -> str:
    """Render a Python string as a tuonelang string literal.

    Only the escapes tuonelang's lexer defines are emitted; a character needing
    any other escape is refused rather than passed through, since an escape the
    target does not recognise would change the string's meaning.
    """
    escapes = {"\\": "\\\\", '"': '\\"', "\n": "\\n", "\t": "\\t", "\r": "\\r"}
    out: list[str] = []
    for char in value:
        if char in escapes:
            out.append(escapes[char])
        elif " " <= char <= "~":
            out.append(char)
        else:
            raise error(
                "SEM0006",
                f"string literal contains a character with no tuonelang escape: {char!r}",
                node,
                "tuonelang string literals are byte strings over the escapes "
                "\\\\, \\\", \\n, \\t, \\r",
            )
    return '"' + "".join(out) + '"'


def _is_docstring(stmt: ast.stmt) -> bool:
    """Whether a statement is a bare string expression."""
    return isinstance(stmt, ast.Expr) and isinstance(stmt.value, ast.Constant) and isinstance(stmt.value.value, str)


def _ends_in_value(body: list[ast.stmt]) -> bool:
    """Whether a statement list ends in a value-producing `return`.

    A trailing if/else counts when both of its arms do, since that is exactly
    the shape tuonelang expresses as a block-valued `if`.
    """
    if not body:
        return False
    last = body[-1]
    if isinstance(last, ast.Return):
        return last.value is not None
    if isinstance(last, ast.If) and last.orelse:
        return _ends_in_value(last.body) and _ends_in_value(last.orelse)
    return False


def _mutated_names(fn: ast.FunctionDef) -> set[str]:
    """Names the function body assigns to after their initial binding.

    Python rebinds freely; tuonelang distinguishes `let` from `var` and needs
    `mut` on a mutated parameter. This pre-scan decides which names need the
    mutable spelling, so the emitted binding is right the first time.
    """
    seen: set[str] = set()
    mutated: set[str] = set()

    for arg in fn.args.args:
        seen.add(arg.arg)

    for node in ast.walk(fn):
        if isinstance(node, ast.AugAssign) and isinstance(node.target, ast.Name):
            mutated.add(node.target.id)
        elif isinstance(node, ast.Assign):
            for target in node.targets:
                if isinstance(target, ast.Name):
                    if target.id in seen:
                        mutated.add(target.id)
                    seen.add(target.id)
                elif isinstance(target, ast.Subscript) and isinstance(target.value, ast.Name):
                    mutated.add(target.value.id)
        elif isinstance(node, ast.AnnAssign) and isinstance(node.target, ast.Name):
            if node.target.id in seen:
                mutated.add(node.target.id)
            seen.add(node.target.id)
        elif isinstance(node, ast.For) and isinstance(node.target, ast.Name):
            seen.add(node.target.id)
    return mutated


def sig_node(sig: FunctionSig) -> ast.AST:
    """A placeholder node for diagnostics raised without a Python position."""
    node = ast.Pass()
    node.lineno = 0
    node.col_offset = 0
    return node
