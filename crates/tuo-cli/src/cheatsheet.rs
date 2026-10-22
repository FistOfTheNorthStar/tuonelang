//! The `tuo cheatsheet` command: a dense, context-injectable language brief
//! (ADR-0018).
//!
//! The brief exists for a reader that is **not** a human working through a
//! tutorial: a coding agent or a local model that needs the whole of tuonelang
//! in one paste before writing a program. [`REFERENCE.md`] is the document for
//! people; this is the document for context windows, and the difference is not
//! length but derivation.
//!
//! **Nothing here is authored as a claim about the language.** Every factual
//! section is assembled from a compiler-owned source:
//!
//!   * the syntax skeleton carries `grammar.ebnf`'s `GRAMMAR-VERSION`, so a
//!     brief generated against a different grammar is identifiable as such;
//!   * the standard-library surface is the **real** symbol table — the twelve
//!     `tuo-stdlib` catalog modules driven through `check_sources`, listing
//!     what `Resolution::symbols()` actually resolved, with the signature
//!     `TypeckResult::type_of` actually inferred, rendered by `Ty::render`.
//!     This is the same query behind `tuo package symbols` and the agent
//!     protocol's `visible_symbols_at`;
//!   * the runnable-core boundary and the anti-pattern list are fixed text,
//!     but every code sample in them is compiled by
//!     `tuo-cli/tests/cheatsheet_command.rs` — the wrong forms must really be
//!     rejected and the corrected forms must really be accepted.
//!
//! That test also pins the committed root copy byte-for-byte against fresh
//! output, so a language change that invalidates the brief turns CI red in the
//! same commit rather than silently shipping stale guidance to every model
//! primed with it. The failure mode this design exists to prevent is specific:
//! a stale brief does not produce a visible error, it produces a confidently
//! wrong generation.

use std::process::ExitCode;

use serde_json::json;
use tuo_compiler::ast::{Ast, FnDecl, Item};
use tuo_compiler::check_sources;
use tuo_compiler::resolve::Builtin;
use tuo_compiler::source::{SourceId, SourceMap};

use crate::output::OutputMode;
use crate::protocol::{Event, ProtocolCommand, Status};

/// The grammar specification, embedded so the emitted brief's version marker
/// cannot drift from the grammar the compiler was built against.
const GRAMMAR: &str = include_str!("../../../specification/grammar.ebnf");

/// `tuo cheatsheet`: emit the language brief on stdout.
///
/// Exit status: success unless the brief could not be assembled (which would
/// mean the stdlib catalog itself does not compile — a condition
/// `tuo-cli/tests/stdlib.rs` already forbids).
#[expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "this is the CLI presentation layer: stdout carries the brief, stderr the error"
)]
pub(crate) fn run(mode: OutputMode) -> ExitCode {
    let text = match render() {
        Ok(text) => text,
        Err(message) => {
            if mode.is_machine() {
                emit(mode, Status::Error, json!({ "message": message }));
            } else {
                eprintln!("error: {message}");
            }
            return ExitCode::FAILURE;
        }
    };

    if mode.is_machine() {
        // In a machine format stdout carries protocol output only, so the
        // brief travels as a field rather than as bare text.
        emit(
            mode,
            Status::Ok,
            json!({
                "kind": "cheatsheet",
                "grammar_version": grammar_version().unwrap_or("unknown"),
                "text": text,
            }),
        );
    } else {
        print!("{text}");
    }
    ExitCode::SUCCESS
}

/// Emit a single-item protocol exchange.
fn emit(mode: OutputMode, status: Status, payload: serde_json::Value) {
    let Some(mut emitter) = mode.emitter(ProtocolCommand::Cheatsheet) else {
        return;
    };
    let write = (|| -> std::io::Result<()> {
        emitter.emit(&Event::started(&[] as &[String]))?;
        emitter.emit(&Event::item(status, payload))?;
        emitter.emit(&Event::finished(status, json!({})))?;
        emitter.finish()
    })();
    if write.is_err() {
        mode.log("protocol: stdout write failed");
    }
}

/// Assemble the complete brief.
///
/// Public to the crate so the test suite can compare freshly generated output
/// against the committed copy without going through the process boundary.
pub(crate) fn render() -> Result<String, String> {
    let mut out = String::new();
    out.push_str(&header());
    out.push_str(SYNTAX);
    out.push_str(&stdlib_surface()?);
    out.push_str(RUNNABLE_CORE);
    out.push_str(ANTI_PATTERNS);
    out.push_str(WORKED_EXAMPLE);
    out.push_str(FOOTER);
    Ok(out)
}

/// The `GRAMMAR-VERSION: X.Y` marker carried in `grammar.ebnf`.
fn grammar_version() -> Option<&'static str> {
    GRAMMAR
        .lines()
        .find_map(|line| line.trim().strip_prefix("GRAMMAR-VERSION:"))
        .map(str::trim)
}

fn header() -> String {
    format!(
        "\
================================================================================
tuonelang — language brief (grammar {version}, edition 2024)
================================================================================

Generated by `tuo cheatsheet` from the compiler's own sources: the syntax
version below comes from the grammar specification, and every standard-library
signature is the one the type checker actually inferred. Nothing here is
hand-asserted, and the repository's tests fail if this text drifts from what
the compiler accepts.

Read this before writing tuonelang. It is complete enough to write correct
programs from; it is not a tutorial. If a construct is not mentioned here, it
probably does not exist — prefer asking the compiler (`tuo check`) over
guessing.

",
        version = grammar_version().unwrap_or("unknown")
    )
}

/// Section 1 — the canonical spelling of every construct.
///
/// Each sample is compiled by the test suite, so a construct spelled wrongly
/// here cannot ship.
const SYNTAX: &str = r#"--------------------------------------------------------------------------------
1. SYNTAX SKELETON — the one spelling of each construct
--------------------------------------------------------------------------------

There is deliberately ONE way to write each construct. No overloading, no
default arguments, no variadics, no syntactic sugar. If you are choosing
between two spellings, the more explicit one is the real one.

  module app;                      // every file declares its module, first
  import util::helpers;            // `import`, NEVER `use`
                                   // (the named module must be part of the
                                   //  program you compile — a package
                                   //  dependency or another file)

  const LIMIT: Int = 100;          // const REQUIRES a type annotation

  /// Doc comments are `///` and precede the item.
  pub fn add(take a: Int, take b: Int) -> Int {
      a + b                        // last expression, no semicolon, is the value
  }

  fn modes(in xs: [Int; 4], mut total: Int) -> Int {
      // Every parameter needs BOTH a mode and a type.
      //   take = ownership moves in     (consumed scalars, owned values)
      //   in   = shared read-only borrow for the call
      //   mut  = exclusive mutable borrow for the call
      total = total + xs[0];
      total
  }

  fn bindings() -> Int {
      let x = 1;                   // immutable, type inferred
      let y: Int = 2;              // optional annotation
      var total = 0;               // mutable — `var`, NEVER `let mut`
      total = total + x + y;       // plain `=`; there is NO `+=`
      total
  }

  fn control(take n: Int) -> Int {
      // `if` is an expression; both arms must have the same type.
      let sign = if n < 0 { 0 - 1 } else { 1 };

      var acc = 0;
      var i = 0;
      while i < n {                // no parentheses around the condition
          acc = acc + i;
          i = i + 1;
      }

      let xs: [Int; 3] = [1, 2, 3];
      for x in xs {                // bounded iteration over an array
          acc = acc + x;
      }
      acc * sign
  }

  struct Point { x: Int, y: Int }  // fields are `name: Type`, comma-separated

  enum Shape {
      Dot,                         // a unit variant
      Line { length: Int },        // a variant with NAMED fields
  }

  fn make() -> Point {
      Point { x: 1, y: 2 }         // struct literals name every field
  }

  fn classify(in s: Shape) -> Int {
      match s {                    // `match` is an expression and must be
          Shape::Dot => 0,         // exhaustive (T0007 otherwise)
          Shape::Line { length } => length,
      }
  }

  // Option/Result payloads are NAMED FIELDS, not positional.
  fn first(in xs: [Int; 3]) -> Option[Int] {
      Some { value: xs[0] }        // NOT `Some(xs[0])`
  }

  fn unwrap_or(in o: Option[Int], take fallback: Int) -> Int {
      match o {
          Some { value } => value,
          None => fallback,
      }
  }

  // Generic arguments use SQUARE brackets: Option[Int], never Option<Int>.

  /// A `spec` is a colocated executable test, named for the function it
  /// covers. `then` asserts; `==` compares SCALARS (Int, Bool, Str).
  spec add {
      then add(2, 3) == 5;
      then add(2, 3) == add(3, 2);
  }

  /// A spec may instead carry a description string, and the full form adds
  /// `given` inputs and `when` steps.
  spec "add is commutative" {
      given a: Int, b: Int;
      when let left = add(a, b);
      when let right = add(b, a);
      then left == right;
  }

  fn main() -> Int {               // `main` is nullary; its Int IS the exit status
      0
  }

Operators: + - * / % on numbers; == != < <= > >= for comparison; && || ! for
Bool; & | ^ ~ << >> on integers (ADR-0019). Bitwise precedence is the
conventional one — ~ then << >> then & then ^ then | — and all six are
integers-only (never Float, never Bool: use && || ! there). `>>` is
arithmetic on a signed type and logical on an unsigned one, and a shift
amount outside 0..width TRAPS rather than wrapping, so `x << 64` is an abort,
not `x`. Note `|` does double duty: alternation inside a match pattern,
bitwise-or everywhere else. There is NO compound assignment (`+=`, `|=`), and
comparisons DO NOT chain (`a < b < c` is a parse error). Integer overflow
TRAPS: `+ - *` never wrap. That is right for arithmetic and WRONG for a
checksum or hash, which is *defined* modulo 2^w: `a + b` on two large values
aborts the program rather than wrapping. When you want wraparound, say so —
`std::int::wrapping_add(a, b)`, `wrapping_sub`, and `wrapping_mul` work on
any integer type (`U32` for a 32-bit hash, `U64` or `Int` for 64-bit), and
`std::int::rotate_left(x, n)` / `rotate_right` rotate by any amount (reduced
mod the width; unlike a shift, never a trap). When overflow is an input
error rather than intended, `std::int::checked_add(a, b)` (and
`checked_sub`, `checked_mul`) returns `None` exactly where `+` would trap.
None of these ever traps. They are free functions: `a.wrapping_add(b)` is
method syntax and is refused.

"#;

/// Section 3 — the boundary between what checks and what runs.
const RUNNABLE_CORE: &str = r#"--------------------------------------------------------------------------------
3. WHAT RUNS — the boundary that matters most
--------------------------------------------------------------------------------

tuonelang has two tiers, and confusing them is the most common way to write a
program that looks right and is refused. `tuo check` accepts the full v0
language. `tuo run` / `tuo build` / `tuo spec` execute the RUNNABLE CORE.

Runs everywhere (check, interpreter, and native):
  * Int / Bool / Char / Float arithmetic, comparison, `as` casts
  * if / match / while / for / loop, calls, recursion
  * structs, enums, Option[T], Result[T, E]
  * fixed arrays [T; N] with checked indexing and bounded `for`
  * borrow-mode (`in` / `mut`) calls
  * `Str` values: literals, `==`, and the `std::str` byte operations
  * owned `String` and growable `Array[T]` (`std::string` / `std::array`)
  * `Map[K, V]` (`std::map`): an Int or Str key, an Int, Bool, Float, or Str
    value, insertion-ordered
  * `std::json` — parse / render / arena accessors, all pure
  * first-class NON-CAPTURING function values: a bare top-level `fn` name is a
    value of type `fn(mode T, ...) -> R`, called indirectly
  * generic FUNCTIONS, `fn ident[T](take x: T) -> T`, compiled once per type
    they are called at. Inside the body `T` is owned, never `Copy`: move each
    `T` value exactly once. A generic is called, never passed as a value
    (T0015)
  * `Box[T]`, the one heap wrapper with values: `std::boxed::new(v)` boxes,
    `std::boxed::get(b)` copies the pointee out, `std::boxed::into(b)` moves it
    out. A recursive type holds itself behind one: `next: Option[Box[Node]]`.
    Matching a value reached through an `in` parameter LENDS its non-Copy
    parts (read them, pass them `in`, never move them); `get(b)` passed to an
    `in` parameter lends the pointee without copying. So a tree is walked as
    `fn walk(in n: Node)` with `walk(std::boxed::get(child))`

Native only — NOT available inside a spec (the spec sandbox is pure, and a
spec that touches an effect is refused with R0007):
  * `std::io` print / println / read_line
  * `std::process` exit / arg_count / arg
  * `std::time` now
  * `std::fs` read / write / exists / remove
  * `std::net` TCP and UDP sockets, including bounded-wait timeouts and IPv6
  * `std::sync` par_map (structured fork-join), channels, mutexes

DOES NOT EXIST — do not write these, they are refused, never mis-compiled:
  * capturing closures (function values must capture nothing)
  * `Shared` / `Weak` heap-wrapper VALUES, and `Box` as a growable `Array`
    element or map value (a `Box` field, local, or `Option` payload is fine)
  * `Box::new(x)`, `box x`, `*b` — the constructor is `std::boxed::new(x)` and
    there is no dereference operator
  * method-call syntax: there are FREE FUNCTIONS only. Write `len(s)`, never
    `s.len()` (refused with T0024, which offers the free call). `interface` /
    `impl` declarations type-check but lower to nothing (warned with T0026).
  * detached / unstructured thread spawn — concurrency is fork-join only
  * recursive struct/enum types without an indirection (refused with T0016)

Concurrency is ONE model, uniformly: structured fork-join via
`std::sync::par_map`, which forks, runs, and joins before returning. Channels
and mutexes exist for communication. There is no async/await, no actor system,
and no detached spawn — do not mix in a paradigm from another language.

"#;

/// Section 4 — the mistakes models actually make.
///
/// This list is the forward reading of `training/breaks.py`, which injects
/// exactly these errors to generate repair-training data. One list, two
/// consumers: the generator breaks a program with it, the brief warns against
/// it, and neither can drift from the other without the test suite noticing.
const ANTI_PATTERNS: &str = r#"--------------------------------------------------------------------------------
4. ANTI-PATTERNS — habits from other languages that do not compile
--------------------------------------------------------------------------------

These are the mistakes models actually make writing tuonelang, each with the
form that compiles. Every left-hand form is rejected outright — including
method-call syntax, which still PARSES, so the error (T0024) can point at the
free function to call instead. Write the free function.

  WRONG                          RIGHT
  ---------------------------    ---------------------------------------------
  use util::helpers;             import util::helpers;
  Option<Int>                    Option[Int]
  Some(x)                        Some { value: x }
  let mut total = 0;             var total = 0;
  total += 1;                    total = total + 1;
  fn f(x: Int)                   fn f(take x: Int)
  fn f() -> Int { true }         fn f() -> Bool { true }
  if a < b < c                   if a < b && b < c
  xs.len()                       std::array::len(xs)
  a.wrapping_add(b)              std::int::wrapping_add(a, b)

Further traps, in the order they usually bite:

  * A trailing `;` on a block's last expression turns it into a `()` statement,
    which usually surfaces as a T0001 type mismatch at the return.
  * Function signatures are NEVER inferred. Write every parameter type and the
    return type.
  * A call resolves to exactly one function by name — no overloading. If two
    things need different behavior, they need different names.
  * Specs compare scalars: extract an Int or Bool before `==` rather than
    comparing structs or enums directly.
  * Literal positions (array lengths) need a non-negative literal; `-5` is
    unary minus applied to `5`, which is an expression, not a literal.
  * Indexing takes a `Usize`: `xs[i as Usize]`. An indexed write `xs[i] = v`
    works on a `var` array (fixed or growable) when the element is `Copy`
    (Int, Float, Bool, Str); for a `String` or other owning element, use
    `std::array::set(xs, i, v)`. A buffer of known size is
    `std::array::filled(n, value)` (one allocation) or `[value; N]` (on the
    stack), not `n` pushes onto `std::array::empty()`.

"#;

/// Section 5 — one complete program, compiled and run by the test suite.
const WORKED_EXAMPLE: &str = r#"--------------------------------------------------------------------------------
5. A COMPLETE PROGRAM
--------------------------------------------------------------------------------

This program compiles, passes its specs, and exits 15. It is compiled and run
by the repository's test suite, so it is known to work.

  module stats;

  /// Sum the values of a fixed batch.
  pub fn total(in xs: [Int; 5]) -> Int {
      var sum = 0;
      for x in xs {
          sum = sum + x;
      }
      sum
  }

  /// The largest value, or the fallback for an empty range.
  pub fn largest(in xs: [Int; 5], take fallback: Int) -> Int {
      var best = fallback;
      for x in xs {
          if x > best {
              best = x;
          }
      }
      best
  }

  spec total {
      then total([1, 2, 3, 4, 5]) == 15;
      then total([0, 0, 0, 0, 0]) == 0;
  }

  spec largest {
      then largest([1, 4, 2, 5, 3], 0) == 5;
      then largest([0, 0, 0, 0, 0], 7) == 7;
  }

  fn main() -> Int {
      let batch: [Int; 5] = [1, 2, 3, 4, 5];
      total(batch)
  }

Workflow — ask the compiler rather than guessing:

  tuo check  program.tuo    parse, resolve, type-check, ownership-check
  tuo spec   program.tuo    run the colocated specs
  tuo verify program.tuo    both of the above
  tuo run    program.tuo    compile natively and run (exit status = main's Int)
  tuo fmt    program.tuo    canonical formatting, zero configuration

Diagnostics are structured and machine-readable: add
`--message-format=json` to any of these for a versioned envelope carrying each
diagnostic's code, span, notes, and machine-applicable suggestions. Diagnostic
codes are namespaced by stage: L (lexical), P (parser), R (resolution),
T (types), O (ownership), M (MIR), S (spec), C (codegen).

"#;

const FOOTER: &str = r#"--------------------------------------------------------------------------------
When in doubt: write the explicit form, use a free function, add the parameter
mode, and run `tuo check`. The compiler refuses what it cannot compile — it
never mis-compiles — so a program that checks is a program that means what it
says.
================================================================================
"#;

/// Section 2 — the real standard-library surface.
///
/// Two compiler facts combine here. The **acceptance gate** is
/// `check_sources`: the twelve catalog modules are driven through the real
/// front end, and if they do not compile the brief refuses to describe them at
/// all (a condition `tuo-cli/tests/stdlib.rs` independently forbids). The
/// **listing** is then read from the parsed declarations — each public `fn` as
/// its author wrote it, parameter names and type spellings included.
///
/// Reading declarations rather than rendering `TypeckResult` types is
/// deliberate, and is the honest choice for this audience. `Ty::render`
/// normalizes to the checker's canonical spelling (`Int` becomes its underlying
/// `I64`) and carries no parameter names, so a model reading it would see
/// `fn abs(take I64) -> I64` — correct, but not the form a programmer writes
/// and missing the names that say what to pass. The declared form is what the
/// caller must actually type. The two cannot silently disagree: the checker
/// accepted these exact declarations, and `cheatsheet_command.rs` compiles a
/// call against every listed signature.
fn stdlib_surface() -> Result<String, String> {
    let mut map = SourceMap::new();
    let mut sources: Vec<SourceId> = Vec::new();
    for module in tuo_stdlib::MODULES {
        let file = map.intern_file(module.name);
        let id = map
            .add_source(file, module.source)
            .map_err(|_| format!("stdlib module `{}` is too large", module.path))?;
        sources.push(id);
    }

    // The gate: describe the library only if the compiler accepts it.
    let checked = check_sources(&map, &sources);
    if checked.has_errors() {
        return Err(
            "the standard library catalog does not compile; cannot report its symbols".to_owned(),
        );
    }

    let mut out = String::from(
        r#"--------------------------------------------------------------------------------
2. STANDARD LIBRARY — the real exported surface
--------------------------------------------------------------------------------

Every signature below is a declaration the compiler accepted in this build,
written exactly as the caller must type it. If a function you want is not
listed, it does not exist — do not invent a plausible name. There is exactly
one obvious API per task, and the library is FREE FUNCTIONS ONLY
(`std::array::len(xs)`, never `xs.len()`).

Call any of them by its full path from any program — `std::core::abs(x)`,
`std::io::println("hi")` — with no `import` and no copy of the module: the
toolchain loads every standard-library module a program names.

`Int` is an alias for `I64` and `Float` for `F64`; both spellings are legal,
and these declarations use the one the library authors chose.

"#,
    );
    out.push_str(&builtin_surface());
    out.push_str(LIBRARY_MODULES);

    for module in tuo_stdlib::MODULES {
        let parse = tuo_compiler::parser::parse(map.source(sources[index_of(module.path)]));
        let ast = Ast::new(&parse.tree, module.source);
        let mut entries: Vec<String> = Vec::new();
        for item in ast.file().items() {
            let Item::Fn(decl) = item else { continue };
            if !decl.is_pub() {
                continue;
            }
            let Some(name) = decl.name() else { continue };
            entries.push(declaration(name, decl));
        }
        if entries.is_empty() {
            continue;
        }
        out.push_str(&format!("{}\n", module.path));
        for entry in entries {
            out.push_str(&format!("  {entry}\n"));
        }
        out.push('\n');
    }
    Ok(out)
}

/// The operations built into the compiler — the heap types, `Str`'s byte
/// access, `Box`, and the float square root — which have no module source to
/// read a declaration from.
///
/// Every line is the compiler's own: which builtins exist and their modules
/// ([`Builtin::ALL`]), and each one's caller-facing declaration
/// ([`Builtin::declaration`] — modes from the table the ownership checker
/// enforces, shapes from [`Builtin::surface`]). `cheatsheet_command.rs`
/// compiles a call through every line, so a shape that drifted from the
/// checker fails the suite.
fn builtin_surface() -> String {
    let mut out = String::from(
        "Built into the compiler (no module file; always available). `T` is the
element type — Int, Float, Bool, Str, String, or a struct or enum of those; a
map's key `K` is Int or Str and its value `V` is Int, Bool, Float, or Str. In
`std::int`, `T` is any integer type (both operands share it) and a rotation's
amount `N` is any integer type of its own.

",
    );
    // One block per module, in the order modules first appear in `ALL`.
    let mut modules: Vec<&[&str]> = Vec::new();
    for builtin in Builtin::ALL {
        if builtin.surface().is_some() && !modules.contains(&builtin.module_path()) {
            modules.push(builtin.module_path());
        }
    }
    for module in modules {
        out.push_str(&format!("{}\n", module.join("::")));
        for builtin in Builtin::ALL.iter().filter(|b| b.module_path() == module) {
            if let Some(declaration) = builtin.declaration() {
                out.push_str(&format!("  {declaration}\n"));
            }
        }
        out.push('\n');
    }
    out
}

/// The line separating the builtins from the catalog's modules (a reader, and
/// the brief's own tests, can tell the two apart by it).
pub(crate) const LIBRARY_MODULES: &str = "The library's own modules, written in tuonelang:\n\n";

/// The catalog index of the module with this path.
fn index_of(path: &str) -> usize {
    tuo_stdlib::MODULES
        .iter()
        .position(|m| m.path == path)
        .expect("the path comes from the catalog itself")
}

/// Render one public function as its declaration line: every parameter's mode,
/// name, and written type, plus the return type.
fn declaration(name: &str, decl: FnDecl<'_>) -> String {
    let params: Vec<String> = decl
        .params()
        .map(|param| {
            let mode = param.mode().unwrap_or("take");
            let param_name = param.name().unwrap_or("_");
            let ty = param.ty().map_or("?", |ty| ty.text());
            format!("{mode} {param_name}: {ty}")
        })
        .collect();
    // An omitted return type means `()`, which the brief states explicitly
    // rather than leaving the reader to infer.
    let ret = decl.return_type().map_or("()", |ty| ty.text());
    format!("fn {name}({}) -> {ret}", params.join(", "))
}
