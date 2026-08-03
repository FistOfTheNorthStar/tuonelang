//! Evaluating one generated program by *driving the real compiler*.
//!
//! Every `@1`-style metric this harness reports is earned by compiling the
//! model's output through the actual pipeline — never by an asserted boolean.
//! [`evaluate`] takes a generated source (plus the task's specs), assembles the
//! program the way a user would (generation + colocated specs), and runs:
//!
//! - the front end ([`tuo_compiler::check_sources`]) for **parse** and **check**,
//!   attributing failures to the right stage by diagnostic namespace exactly as
//!   the corpus pipeline does;
//! - the reference spec runner ([`tuo_spec::run`]) for **specs**;
//!
//! and records the **invented-symbol count** (undefined-name diagnostics — a
//! reference to a symbol that does not exist, i.e. a hallucination) and the
//! **wall-clock latency** of producing that compiler feedback. Held-out
//! **tests** are scored the same way, by appending them instead of the specs
//! (see [`evaluate_tests`]).

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use tuo_compiler::check_sources;
use tuo_diagnostics::{Diagnostic, Namespace, Severity};
use tuo_source::{SourceMap, SourceText};
use tuo_spec::{Limits, RunOutcome, Selection};

use crate::task::GENERATED_FILE;

/// The undefined-name diagnostic (`R0002`): a reference to a symbol that is not
/// in scope. Counting these is how the harness measures *invented symbols* — the
/// compiler is the authority on which names do not exist.
const UNDEFINED_NAME: (Namespace, u16) = (Namespace::Resolution, 2);

/// The per-stage outcome of compiling one generated program, plus the two
/// compiler-derived signals the harness records for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evaluation {
    /// Did the program parse (no lexical/parser errors)?
    pub parsed: bool,
    /// Did the program pass the whole front end (parse, resolve, type,
    /// ownership)?
    pub checked: bool,
    /// Did every colocated spec pass? Only meaningful when `checked`.
    pub specs_passed: bool,
    /// Count of undefined-name (`R0002`) diagnostics — invented symbols.
    pub invented_symbols: u32,
    /// The compiler's diagnostics, rendered one per line, for repair feedback.
    pub feedback: Vec<String>,
    /// The 1-based line numbers of the *generation* (not the appended specs)
    /// that carried an error diagnostic. This is the region the compiler pointed
    /// at, used to judge whether a subsequent repair edited only the flagged
    /// lines or also touched unrelated code.
    pub error_lines: BTreeSet<u32>,
    /// Wall-clock time to produce this feedback (front end + specs). Measured,
    /// not promised.
    pub latency: Duration,
}

impl Evaluation {
    /// A fully-passing evaluation (parsed, checked, specs passed, nothing
    /// invented).
    #[must_use]
    pub fn is_success(&self) -> bool {
        self.parsed && self.checked && self.specs_passed
    }
}

/// Assemble `generation + specs` into one program and evaluate it.
///
/// The specs are appended to the generated source exactly as a colocated spec
/// block would sit in the file, so the spec runner sees the program a developer
/// would write. `limits` bounds the spec sandbox.
#[must_use]
pub fn evaluate(generation: &str, specs: &[String], limits: Limits) -> Evaluation {
    let program = assemble(generation, specs);
    // The generation owns lines 1..=generation_lines; anything past that is the
    // appended spec text, which the model did not write.
    let generation_lines = u32::try_from(generation.lines().count()).unwrap_or(u32::MAX);
    let started = Instant::now();

    let mut map = SourceMap::new();
    let file = map.intern_file(GENERATED_FILE);
    let Ok(source) = map.add_source(file, program.clone()) else {
        // An interior NUL or similar makes the text unstorable; treat it as a
        // non-parsing program rather than panicking.
        return Evaluation {
            parsed: false,
            checked: false,
            specs_passed: false,
            invented_symbols: 0,
            feedback: vec!["source could not be interned".to_string()],
            error_lines: BTreeSet::new(),
            latency: started.elapsed(),
        };
    };
    let sources = [source];

    let check = check_sources(&map, &sources);
    let parsed = !has_error_in(&check.diagnostics, front_end_parse_namespaces());
    let checked = !check.has_errors();
    let invented_symbols = count_code(&check.diagnostics, UNDEFINED_NAME);
    let error_lines = error_lines_of(map.source(source), &check.diagnostics, generation_lines);
    let mut feedback = render_all(&check.diagnostics);

    // Specs only run on a program that checks; otherwise the spec runner would
    // just re-report the front-end errors.
    let specs_passed = if checked && !specs.is_empty() {
        match tuo_spec::run(&map, &sources, &Selection::All, limits) {
            RunOutcome::Ran(report) => report.passed(),
            RunOutcome::NotChecked(mut problems) => {
                feedback.extend(render_all(&problems));
                problems.clear();
                false
            }
        }
    } else {
        // No specs to falsify: a checked program vacuously "passes" its (empty)
        // spec set. An unchecked program cannot pass specs.
        checked
    };

    Evaluation {
        parsed,
        checked,
        specs_passed,
        invented_symbols,
        feedback,
        error_lines,
        latency: started.elapsed(),
    }
}

/// The 1-based line numbers within the generation (lines `1..=generation_lines`)
/// that carried an error diagnostic. Diagnostics anchored in the appended spec
/// text are excluded, since they are not on code the model wrote.
fn error_lines_of(
    text: &SourceText,
    diagnostics: &[Diagnostic],
    generation_lines: u32,
) -> BTreeSet<u32> {
    let mut lines = BTreeSet::new();
    for d in diagnostics {
        if d.severity != Severity::Error {
            continue;
        }
        if let Ok(loc) = text.line_col(d.primary_span.range().start()) {
            // `line_col` is 0-based; present as 1-based lines.
            let line = loc.line.saturating_add(1);
            if line <= generation_lines {
                lines.insert(line);
            }
        }
    }
    lines
}

/// Score the held-out **tests**: assemble `generation + tests` and report
/// whether every test passes (the program must also still check).
#[must_use]
pub fn evaluate_tests(generation: &str, tests: &[String], limits: Limits) -> bool {
    if tests.is_empty() {
        // No held-out tests: TestPass is not applicable and is reported as such
        // by the caller; here we answer the narrow question "did the tests pass",
        // which is vacuously true.
        return true;
    }
    evaluate(generation, tests, limits).specs_passed
}

/// Append spec blocks to a generation to form the program the compiler sees.
fn assemble(generation: &str, specs: &[String]) -> String {
    let mut program = generation.to_string();
    for spec in specs {
        if !program.ends_with('\n') {
            program.push('\n');
        }
        program.push('\n');
        program.push_str(spec);
    }
    program
}

/// The namespaces whose errors mean "did not parse".
fn front_end_parse_namespaces() -> [Namespace; 2] {
    [Namespace::Lexical, Namespace::Parser]
}

/// Whether any error diagnostic falls in one of `namespaces`.
fn has_error_in(diagnostics: &[Diagnostic], namespaces: [Namespace; 2]) -> bool {
    diagnostics
        .iter()
        .any(|d| d.severity == Severity::Error && namespaces.contains(&d.code.namespace()))
}

/// Count error diagnostics with a specific `(namespace, number)` code.
fn count_code(diagnostics: &[Diagnostic], code: (Namespace, u16)) -> u32 {
    let (ns, number) = code;
    u32::try_from(
        diagnostics
            .iter()
            .filter(|d| {
                d.severity == Severity::Error
                    && d.code.namespace() == ns
                    && d.code.number() == number
            })
            .count(),
    )
    .unwrap_or(u32::MAX)
}

/// Render each error/warning to a stable one-line `CODE: message` string for
/// repair feedback.
fn render_all(diagnostics: &[Diagnostic]) -> Vec<String> {
    diagnostics
        .iter()
        .filter(|d| matches!(d.severity, Severity::Error | Severity::Warning))
        .map(|d| format!("{}: {}", d.code, d.message))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOUBLE: &str = "fn double(take x: Int) -> Int {\n    x + x\n}\n";
    const GOOD_SPEC: &str = "spec double {\n    then double(3) == 6;\n}\n";
    const BAD_SPEC: &str = "spec double {\n    then double(3) == 7;\n}\n";
    const PARSE_BROKEN: &str = "fn double(take x: Int) -> Int {\n    x +\n}\n";
    const TYPE_BROKEN: &str = "fn double(take x: Int) -> Int {\n    true\n}\n";
    const INVENTED: &str = "fn double(take x: Int) -> Int {\n    frobnicate(x)\n}\n";

    fn limits() -> Limits {
        Limits::default()
    }

    #[test]
    fn a_correct_program_with_a_true_spec_succeeds() {
        let e = evaluate(DOUBLE, &[GOOD_SPEC.to_string()], limits());
        assert!(e.parsed && e.checked && e.specs_passed);
        assert!(e.is_success());
        assert_eq!(e.invented_symbols, 0);
    }

    #[test]
    fn a_false_spec_checks_but_fails_specs() {
        let e = evaluate(DOUBLE, &[BAD_SPEC.to_string()], limits());
        assert!(e.checked);
        assert!(!e.specs_passed);
        assert!(!e.is_success());
    }

    #[test]
    fn a_parse_error_does_not_parse() {
        let e = evaluate(PARSE_BROKEN, &[GOOD_SPEC.to_string()], limits());
        assert!(!e.parsed);
        assert!(!e.checked);
        assert!(!e.feedback.is_empty());
    }

    #[test]
    fn a_type_error_parses_but_does_not_check() {
        let e = evaluate(TYPE_BROKEN, &[GOOD_SPEC.to_string()], limits());
        assert!(e.parsed);
        assert!(!e.checked);
    }

    #[test]
    fn an_undefined_name_is_counted_as_an_invented_symbol() {
        let e = evaluate(INVENTED, &[], limits());
        assert!(!e.checked);
        assert!(
            e.invented_symbols >= 1,
            "frobnicate is an invented symbol: {e:?}"
        );
    }

    #[test]
    fn held_out_tests_are_scored_independently() {
        // The generation passes the shown spec and also the held-out test.
        let test = "spec double {\n    then double(10) == 20;\n}\n".to_string();
        assert!(evaluate_tests(DOUBLE, &[test], limits()));
        // A wrong implementation fails the held-out test.
        let wrong = "fn double(take x: Int) -> Int {\n    x + 1\n}\n";
        let test = "spec double {\n    then double(10) == 20;\n}\n".to_string();
        assert!(!evaluate_tests(wrong, &[test], limits()));
    }
}
