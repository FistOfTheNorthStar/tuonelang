# ADR-0038: The standard library is in scope — a host loads the modules a program names

- **Status:** accepted (2026-10-16 — landed with this ADR; see Resolution)
- **Date:** 2026-10-16

## Context

The standard library is twenty-three modules written in tuonelang (`tuo-stdlib`).
The architecture has always said it is **consumed as input**: the compiler
facade never links it (`dependency_policy.rs` forbids `tuo-compiler →
tuo-stdlib`), and a host that wants it loads the sources into its own
`SourceMap`. What no host did was load it **for a user's program**. The only
program that ever saw the library was the library's own test suite, which
loads each module by hand.

So the library could not be called. A single-file program naming any catalog
function was refused at resolution:

```tuo
module app;

fn main() -> Int {
    std::core::abs(0 - 7)
}
```

```
error[R0002]: no `core` in `std`
```

The examples worked around it by **copying** modules into their own sources
(`examples/cli-stats/src/std_io.tuo` is `std::io`, pinned byte-for-byte
against the catalog). Nothing else could.

This went unnoticed for as long as it did because the surfaces that describe
the library never compile a user's call to it:

- `tuo cheatsheet` (ADR-0018) lists all 471 public catalog functions under the
  heading *"the real exported surface"*, tells the reader *"if a function you
  want is not listed, it does not exist"*, and proves each listed signature
  callable — by loading the module beside the call, exactly as no host did.
- The codegen benchmark scores generations through `check_sources` on the
  generated file alone. A model primed with the brief that wrote
  `std::core::abs(x)` was marked down for an **invented symbol** (`R0002`)
  when it had called a real function exactly as documented.

The brief's builtin operations (`std::array::push`, `std::string::len`,
`std::map::insert`, `std::str::byte_at`, …) had the opposite problem: they
resolve everywhere, but the brief listed none of them, so a primed model was
shown `std::array::len(xs)` as the one spelling of length and nowhere told
what else `std::array` holds.

A second wall stood behind the first. Loading `std::collections` beside a
program made `tuo run` refuse it:

```
error: cannot build (codegen): `pair` uses a type the Cranelift backend does
not lower yet: generic struct layout needs monomorphization, deferred in v0
```

The program never calls `pair`. The native path lowered **every** function it
was handed, so one generic helper in a module the program used refused the
build, while the interpreter — which executes only what is called — ran it.

## Decision

1. **A host loads the catalog modules a program names.** Every host that
   compiles a user's program — `tuo check`/`spec`/`verify`/`build`/`run` on
   files, their package-mode forms, the agent session behind `tuo agent` and
   the `tuo` MCP tools, and the codegen benchmark's harness — adds, after the
   user's sources, each catalog module whose path appears in the program text
   (`std::core::abs(x)`, `import std::io;`) together with its declared
   dependencies. No `import` is required, and no copy is made. The selection
   is one function, `tuo_stdlib::referenced_by`, so every host agrees on what a
   program means.

2. **The library stays input.** The facade still does not depend on the
   catalog; the forbidden edge stays. What changed is that the hosts — which
   were always allowed to load it — now do. `tuo_stdlib::DEPENDENCIES` (moved
   from the stdlib test, which still proves it exact and acyclic) is how a host
   knows that `std::hkdf` brings `std::crypto`, `std::bits`, and `std::ct`.

3. **The scan is lexical and generous.** A path inside a comment or string
   also loads its module. Loading a module that goes unused costs some checking
   time and nothing else: the catalog checks with zero diagnostics, and (item 5)
   nothing unused is compiled natively. Missing a module would turn a correct
   call into an error; loading one too many cannot.

4. **A program that carries its own copy keeps it.** A module the program
   declares (`module std::io;`) is not loaded again, so the examples that copy
   a module still compile, and a deliberate local edit of a library module is
   not silently shadowed or duplicated.

5. **Only the library functions a program reaches are compiled natively.** A
   program-level MIR pass, `tuo_mir::retain_reachable`, keeps everything the
   user wrote plus whatever `main` and those functions reach through direct
   calls or by naming them as function values, and drops the loaded library's
   other functions before the backend runs. A function no execution can enter
   has no observable behavior, so this changes nothing a program can see; it
   makes the native path accept what the interpreter already ran. The user's
   own functions stay roots on purpose: a function they wrote outside the
   native subset is still refused whether `main` calls it or not — the
   contract the refusal suites pin — and `constant_time.rs` still finds every
   function it disassembles. (A first cut pruned everything unreachable from
   `main`; those suites caught it.)

6. **The library's specs are its own.** A spec run executes only the specs
   written in the user's files (`tuo_spec::run_scoped`); a loaded library
   module is checked with the program but its specs are not re-run on the
   program's behalf, and `tuo package symbols` reports only what the package's
   own modules declare.

7. **The brief describes the builtins.** `tuo cheatsheet` lists every
   non-`std::rt` builtin operation with its parameters' names, modes, and
   types (`fn push(mut xs: Array[T], take value: T) -> ()`). The modes come
   from `Builtin::param_modes` — the table the ownership checker enforces —
   and the existence, modules, and names from `Builtin::ALL`; only the type
   spellings are written out, and `cheatsheet_command.rs` compiles a call
   through every listed line, so a spelling that drifted from the checker
   fails the suite. The brief now also says, plainly, that a catalog function
   is called by its full path from any program.

## Consequences

- The brief's promise — the listed surface is real — is now true of a user's
  program, not only of the library's own test harness.
- A benchmark generation that calls a documented catalog function is scored on
  what it does, not marked down for a name that exists.
- `R0002` on `std::x` now means the name really is not a catalog module or a
  builtin.
- Checking a program that names `std::tls` loads eight modules; it is slower
  than checking one that does not. This is the cost the generous scan pays,
  and it is paid only by programs that name the module.
- Native builds no longer contain unreachable functions; binaries of programs
  with dead code get smaller.

### Not yet covered

These hosts still compile only what they are given, and are follow-ups:

- `tuo corpus validate` — its format stage and feature extraction run over
  every source, so the library would need to be excluded from both before it
  can be included in the program.
- `tuo debug ast|hir|mir` — developer dumps with unstable output.
- `tuo-lsp` — it has no transport yet; when it gets one it should load the
  library as the agent session does.

## Resolution

Landed with this ADR:

- `tuo_stdlib::referenced_by`, `DEPENDENCIES`, and `dependencies`, with unit
  tests (dependency closure, builtin-only paths load nothing, a declared module
  is not loaded twice, `std::coreutils` is not `std::core`).
- `tuo_spec::run_scoped`; `tuo_mir::retain_reachable`.
- The CLI's `stdlib_input` module, wired into the file and package loaders,
  `verify --affected-by` (whose incremental session is given the same program,
  so the selection is computed over what runs), and `package symbols`.
- The agent session loads the modules its open documents name and scopes its
  spec runs to them; the codegen harness loads them and scopes both its specs
  and its repair feedback to the generated file.
- `tuo cheatsheet`'s builtin block, pinned by
  `every_builtin_is_callable_as_listed` (confirmed by sabotage to fail on a
  wrong return type) and `a_program_calls_the_library_without_copying_it`.
