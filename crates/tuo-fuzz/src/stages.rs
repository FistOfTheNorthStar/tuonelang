//! Per-stage invariant checkers — the single source of truth every fuzz target
//! and stable robustness test drives.
//!
//! Each `check_*` function takes an arbitrary input string, runs one pipeline
//! stage over it, and asserts that stage's invariants. A `cargo fuzz` target
//! (nightly, coverage-guided) and a `#[test]` sweep (stable CI, fixed-seed
//! corpus) call the *same* checker, so the two can never drift: the invariant is
//! written once. A checker panics (via `assert!`) only when an invariant is
//! violated — that panic is exactly the crash the fuzzer is built to surface.
//!
//! # The invariants, by stage
//!
//! - **lexer / syntax / parser / formatter / AST lowering / type check /
//!   ownership check** — *arbitrary source input must not crash the compiler*.
//!   Every one of these stage entry points is documented total (no panic, no
//!   `Result`): malformed input becomes error tokens, recovery islands, poison
//!   nodes, or diagnostics, never a panic. The checkers additionally assert each
//!   stage's structural contract (lexer/parser losslessness, formatter idempotence
//!   and meaning-preservation).
//! - **formatting is idempotent** and **valid formatted source remains
//!   parseable** — [`check_fmt`] formats, then re-formats and requires a fixed
//!   point, and re-parses the canonical text requiring the *same* diagnostics.
//! - **HIR → MIR lowering / MIR verifier** — lowering an accepted program yields
//!   MIR the verifier accepts; the verifier itself is total on any (even
//!   malformed) `Program`.
//! - **verified MIR must not trigger interpreter structural panics** —
//!   [`check_interp`] only ever hands the interpreter MIR that
//!   `Interpreter::new`'s mandatory verify gate accepted, and requires the run to
//!   terminate as a value or a structured trap. A `TrapKind::Internal` — the
//!   interpreter's own "this should have been impossible" signal — is treated as
//!   a finding, since verified MIR must never reach it.
//! - **differential execution must agree across engines where defined** — the
//!   randomized interpreter-vs-native agreement is a mandatory CI gate that
//!   already lives in `tuo-cli/tests/differential.rs` over the accepted-program
//!   generator; this crate does not duplicate the native build, and says so
//!   ([`super`] crate docs) rather than re-asserting a weaker version.

use tuo_ast::Ast;
use tuo_fmt::format_source;
use tuo_lexer::{TokenKind, lex};
use tuo_mir_interp::{Interpreter, Limits, TrapKind};
use tuo_parser::{ParseResult, parse};
use tuo_source::{SourceMap, SourceText};

/// Bounded limits so a fuzzed program cannot hang the sweep. Small relative to
/// the interpreter's defaults: a fuzzed program either finishes quickly or trips
/// a resource trap, both of which are fine outcomes.
fn fuzz_limits() -> Limits {
    Limits::default().max_depth(256).max_values(100_000)
}

/// Intern `text` and return the snapshot the stage entry points consume.
///
/// Returns `None` only when the input exceeds `SourceText`'s 4 GiB limit — a
/// correct rejection by construction, not a crash, so callers simply skip it.
/// (The corpus never generates inputs that large; this keeps the checkers total
/// even if a giant seed file is replayed.)
fn snapshot(map: &mut SourceMap, text: &str) -> Option<std::sync::Arc<SourceText>> {
    let file = map.intern_file("fuzz.tuo");
    let id = map.add_source(file, text).ok()?;
    Some(map.source(id).clone())
}

/// Lexer: lexing arbitrary UTF-8 never panics, and the token stream tiles the
/// input losslessly and ends in a zero-width EOF.
pub fn check_lexer(text: &str) {
    let mut map = SourceMap::new();
    let Some(source) = snapshot(&mut map, text) else {
        return;
    };
    let result = lex(&source);

    // The stream always ends with a zero-width EOF at the end of input.
    let (eof, rest) = result.tokens.split_last().expect("stream is never empty");
    assert_eq!(eof.kind, TokenKind::Eof, "input {text:?}");
    assert_eq!(eof.range().start().as_usize(), text.len(), "input {text:?}");
    assert!(eof.range().is_empty(), "EOF is zero-width: {text:?}");

    // Losslessness: non-EOF tokens tile the input exactly, in order, on char
    // boundaries, with no gaps, overlaps, or zero-width tokens.
    let mut cursor = 0;
    for token in rest {
        assert_eq!(token.range().start().as_usize(), cursor, "gap at {text:?}");
        assert!(!token.range().is_empty(), "zero-width token in {text:?}");
        cursor = token.range().end().as_usize();
        // Slicing on a non-char-boundary would panic — this asserts it can't.
        let _ = &text[token.range().start().as_usize()..token.range().end().as_usize()];
    }
    assert_eq!(cursor, text.len(), "coverage gap in {text:?}");

    // Error tokens and diagnostics accompany each other.
    let has_error_token = rest.iter().any(|t| t.kind == TokenKind::Error);
    assert_eq!(
        has_error_token,
        result.has_errors(),
        "error tokens and diagnostics disagree on {text:?}"
    );
}

/// Parse (and the CST it builds): parsing never panics, and the tree covers the
/// input and reconstructs it byte-for-byte, however broken the input is.
pub fn check_parser(text: &str) {
    let result = parse_text(text);
    check_syntax_invariants(&result, text);
}

/// Syntax-tree operations: the CST's own losslessness contract, checked
/// independently of the parser so a `check_coverage`/`reconstruct` regression is
/// attributed to the syntax layer.
pub fn check_syntax(text: &str) {
    let result = parse_text(text);
    result
        .tree
        .check_coverage()
        .unwrap_or_else(|e| panic!("coverage violated on {text:?}: {e}"));
    assert_eq!(
        result.tree.reconstruct(text),
        text,
        "reconstruction differs on {text:?}"
    );
}

fn check_syntax_invariants(result: &ParseResult, text: &str) {
    result
        .tree
        .check_coverage()
        .unwrap_or_else(|e| panic!("coverage violated on {text:?}: {e}"));
    assert_eq!(
        result.tree.reconstruct(text),
        text,
        "reconstruction differs on {text:?}"
    );
}

/// Formatter: formatting never panics, never returns unverified output, is
/// idempotent, and its canonical output re-parses with the same diagnostics as
/// the input (meaning preservation).
pub fn check_fmt(text: &str) {
    let outcome = format_text(text);
    if !outcome.safe {
        // The conservative bail-out: allowed only when the formatter cannot
        // verify meaning preservation (e.g. lexically broken byte soup). It must
        // return the input untouched.
        assert_eq!(
            outcome.text, text,
            "unsafe outcome modified input: {text:?}"
        );
        assert!(
            !outcome.changed,
            "unsafe outcome claimed a change: {text:?}"
        );
        return;
    }

    // Idempotence: formatting the canonical text is a fixed point.
    let second = format_text(&outcome.text);
    assert!(second.safe, "second-pass self-check failed on {text:?}");
    assert_eq!(
        second.text, outcome.text,
        "formatting is not idempotent on {text:?}"
    );
    assert!(
        !second.changed,
        "re-formatting canonical text reported a change on {text:?}"
    );

    // Valid formatted source remains parseable, with the same diagnostics.
    let before = diagnostic_codes(&parse_text(text));
    let after = diagnostic_codes(&parse_text(&outcome.text));
    assert_eq!(
        before, after,
        "diagnostics changed across formatting on {text:?}"
    );
}

/// The whole front end (resolve → type check → ownership check) over one source:
/// none of these total stages panics on arbitrary input, and every reported
/// diagnostic has a well-formed span within the source.
pub fn check_front_end(text: &str) {
    let mut map = SourceMap::new();
    let file = map.intern_file("fuzz.tuo");
    let Ok(id) = map.add_source(file, text) else {
        return;
    };
    // The facade drives parse → resolve → types → ownership; being total, it
    // returns diagnostics rather than panicking. Simply reaching this point on
    // arbitrary input is the invariant.
    let check = tuo_compiler::check_sources(&map, &[id]);
    let _ = check.has_errors();
    // Every diagnostic span must lie within the source (a malformed span would
    // panic a downstream renderer that slices by it).
    let len = map.source(id).text().len();
    for diag in &check.diagnostics {
        let range = diag.primary_span.range();
        assert!(
            range.end().as_usize() <= len,
            "diagnostic span out of bounds on {text:?}"
        );
    }
}

/// AST lowering (build the typed AST views + lower to HIR) never panics on
/// arbitrary input: unresolved and malformed constructs become poison nodes, not
/// crashes.
pub fn check_ast_lowering(text: &str) {
    let mut map = SourceMap::new();
    let file = map.intern_file("fuzz.tuo");
    let Ok(id) = map.add_source(file, text) else {
        return;
    };
    let parse = parse(&map.source(id).clone());
    let source_text = map.source(id).text().to_owned();
    let asts = [Ast::new(&parse.tree, &source_text)];
    let resolution = tuo_resolve::resolve(&asts);
    let types = tuo_types::check(&asts, &resolution);
    let _ownership = tuo_ownership::check(&asts, &resolution, &types);
    // HIR lowering is documented total: poison in, poison out, never a panic.
    let _hir = tuo_hir::lower(&asts, &resolution);
}

/// HIR → MIR lowering and the MIR verifier over an *accepted* program: lowering
/// produces MIR the verifier accepts. On rejected input the checker returns
/// early — MIR is only defined for accepted programs — so this stresses the
/// happy path (lowering must not silently emit ill-formed MIR).
///
/// The verifier's *totality* on arbitrary `Program`s is exercised indirectly:
/// [`check_interp`] hands every lowered program to `Interpreter::new`, which runs
/// the verifier, and the verifier is documented never to panic.
pub fn check_mir(text: &str) {
    let Some(built) = lower_accepted(text) else {
        return;
    };
    let problems = tuo_mir::verify(&built.program, &built.types);
    assert!(
        problems.is_empty(),
        "lowered MIR of an accepted program failed verification on {text:?}: {problems:?}"
    );
}

/// The MIR interpreter over verified MIR: `Interpreter::new` runs the mandatory
/// verify gate, so any program it accepts is verified MIR — and running verified
/// MIR must terminate as a value or a structured trap, never a Rust panic and
/// never `TrapKind::Internal` (the interpreter's own impossible-state signal).
pub fn check_interp(text: &str) {
    let Some(built) = lower_accepted(text) else {
        return;
    };
    // Only verified MIR reaches `run`: the gate rejects anything else with
    // diagnostics rather than a panic.
    let Ok(interp) = Interpreter::new(&built.program, &built.types) else {
        return;
    };
    let interp = interp.with_limits(fuzz_limits());

    // Run each nullary function that exists, so the interpreter is exercised on
    // whatever the program actually lowered. A parameterized entry is skipped
    // (we have no well-typed arguments to synthesize here).
    for function in &built.program.functions {
        if !function.params.is_empty() {
            continue;
        }
        // Running verified MIR must terminate as a value or a structured trap;
        // an `Internal` trap of any message is the interpreter signalling an
        // impossible state, which verified MIR must never reach.
        if let Err(err) = interp.run(&function.name, Vec::new()) {
            assert!(
                !matches!(err.kind, TrapKind::Internal(_)),
                "verified MIR produced an Internal trap ({}) on {text:?}",
                err.message
            );
        }
    }
}

/// A lowered, front-end-clean program plus the types it was checked against —
/// everything the MIR/interpreter checkers need.
struct Lowered {
    program: tuo_mir::Program,
    types: tuo_types::TypeckResult,
}

/// Lower `text` to MIR, but only if it passes the whole front end. Returns
/// `None` for any input the front end rejects (the overwhelming majority of
/// fuzz inputs) — those are exercised by the front-end checker, not here.
fn lower_accepted(text: &str) -> Option<Lowered> {
    let mut map = SourceMap::new();
    let file = map.intern_file("fuzz.tuo");
    let id = map.add_source(file, text).ok()?;
    let check = tuo_compiler::check_sources(&map, &[id]);
    if check.has_errors() {
        return None;
    }
    let parse = parse(&map.source(id).clone());
    let source_text = map.source(id).text().to_owned();
    let asts = [Ast::new(&parse.tree, &source_text)];
    let hir = tuo_hir::lower(&asts, &check.resolution);
    let program = tuo_mir::lower(&hir, &check.resolution, &check.types);
    Some(Lowered {
        program,
        types: check.types,
    })
}

// --- small shared helpers ---------------------------------------------------

fn parse_text(text: &str) -> ParseResult {
    let mut map = SourceMap::new();
    let file = map.intern_file("fuzz.tuo");
    let id = map.add_source(file, text).expect("input fits");
    parse(&map.source(id).clone())
}

fn format_text(text: &str) -> tuo_fmt::FormatOutcome {
    let mut map = SourceMap::new();
    let file = map.intern_file("fuzz.tuo");
    let id = map.add_source(file, text).expect("input fits");
    format_source(&map.source(id).clone())
}

/// The sorted set of diagnostic codes a parse reports (lexical + parser),
/// used to compare "same errors before and after formatting".
fn diagnostic_codes(result: &ParseResult) -> Vec<String> {
    let mut codes: Vec<String> = result
        .all_diagnostics()
        .iter()
        .map(|d| d.code.to_string())
        .collect();
    codes.sort_unstable();
    codes
}

/// A named stage checker: a stable stage name paired with the function that
/// runs that stage's invariants over one input.
pub type NamedChecker = (&'static str, fn(&str));

/// Every checker in this module, keyed by a stable stage name. The stable
/// robustness sweep and the regression replayer iterate this so a new stage is
/// covered everywhere by adding one entry.
#[must_use]
pub fn all_checkers() -> Vec<NamedChecker> {
    vec![
        ("lexer", check_lexer as fn(&str)),
        ("syntax", check_syntax),
        ("parser", check_parser),
        ("fmt", check_fmt),
        ("front-end", check_front_end),
        ("ast-lowering", check_ast_lowering),
        ("mir", check_mir),
        ("interp", check_interp),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every checker is total on a well-formed program (no false-positive
    /// panic).
    #[test]
    fn checkers_accept_a_valid_program() {
        let src = "fn main() -> Int {\n    let x = 2;\n    x + x\n}\n\nspec main {\n    then main() == 4;\n}\n";
        for (name, check) in all_checkers() {
            check(src);
            let _ = name;
        }
    }

    /// Every checker is total on trivially broken input.
    #[test]
    fn checkers_survive_broken_input() {
        for broken in ["", "fn", "}{)(", "fn f( -> { let", "🦀🦀🦀", "\0\0\0"] {
            for (_name, check) in all_checkers() {
                check(broken);
            }
        }
    }
}
