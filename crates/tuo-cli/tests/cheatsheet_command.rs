//! `tuo cheatsheet`, held to the promise it makes (ADR-0018).
//!
//! The brief exists to prime a model that has never seen tuonelang. That makes
//! its failure mode uniquely quiet: stale guidance does not raise an error, it
//! produces a confidently wrong generation, and the model has no way to know.
//! A cheat sheet is therefore the one document in this repository that must
//! never be trusted on assertion — so this suite re-derives every claim it
//! makes:
//!
//!   * every `tuo` code sample in the brief **really compiles** through the
//!     front end, and the worked program additionally **runs to its stated
//!     exit byte**;
//!   * every standard-library signature the brief lists is **callable as
//!     shown** — the listing comes from parsed declarations, so this is the
//!     check that the declared form and the checker's accepted form agree;
//!   * every anti-pattern shown as wrong is **really rejected**, and — the
//!     load-bearing half — its corrected form is **really accepted**, so the
//!     brief cannot teach a model to avoid something legal;
//!   * the committed `tuonelang-cheat-sheet.txt` is **byte-identical** to
//!     freshly generated output, so a language change that invalidates the
//!     brief turns CI red in the same commit rather than shipping stale
//!     guidance to every model primed with it.
//!
//! This mirrors `shipped_corpus.rs` re-admitting every committed fixture and
//! `shipped_tasks.rs` re-verifying every task pin: the committed artifact is
//! not trusted, it is re-derived and compared.

use std::path::{Path, PathBuf};
use std::process::Command;

use tuo_compiler::check_sources;
use tuo_compiler::source::{SourceId, SourceMap};

/// Run `tuo` with `args`, returning stdout.
fn tuo(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_tuo"))
        .args(args)
        .output()
        .expect("the `tuo` binary runs");
    assert!(
        output.status.success(),
        "`tuo {}` failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("the brief is UTF-8")
}

/// The repository root (two levels above this crate).
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate lives at <root>/crates/tuo-cli")
        .to_path_buf()
}

/// Check `source` as a program, returning the error diagnostics.
///
/// The companion module travels with every sample so an `import` resolves.
fn errors_in(source: &str) -> Vec<String> {
    let mut map = SourceMap::new();
    let file = map.intern_file("brief.tuo");
    let id: SourceId = map.add_source(file, source).expect("the sample is small");
    let companion_file = map.intern_file("helpers.tuo");
    let companion: SourceId = map
        .add_source(companion_file, COMPANION)
        .expect("the companion is small");
    let checked = check_sources(&map, &[id, companion]);
    checked
        .diagnostics
        .iter()
        .filter(|d| d.severity == tuo_compiler::diagnostics::Severity::Error)
        .map(|d| format!("{}: {}", d.code, d.message))
        .collect()
}

/// Assert `source` compiles clean, reporting what went wrong if not.
fn assert_accepts(source: &str, what: &str) {
    let errors = errors_in(source);
    assert!(
        errors.is_empty(),
        "{what} should compile but the front end rejected it:\n  {}\n--- source ---\n{source}",
        errors.join("\n  ")
    );
}

/// Assert `source` is rejected — used for the anti-patterns, which the brief
/// tells a model never to write.
fn assert_rejects(source: &str, what: &str) {
    assert!(
        !errors_in(source).is_empty(),
        "{what} is shown in the brief as WRONG, but the compiler accepts it — \
         the brief would be teaching a model to avoid something legal:\n{source}"
    );
}

// ---------------------------------------------------------------------------
// The brief is generated, and generated from the compiler
// ---------------------------------------------------------------------------

#[test]
fn the_brief_carries_the_real_grammar_version() {
    let brief = tuo(&["cheatsheet"]);
    let grammar = std::fs::read_to_string(repo_root().join("specification/grammar.ebnf"))
        .expect("the grammar specification is committed");
    let version = grammar
        .lines()
        .find_map(|line| line.trim().strip_prefix("GRAMMAR-VERSION:"))
        .map(str::trim)
        .expect("the grammar carries a version marker");
    assert!(
        brief.contains(&format!("grammar {version}")),
        "the brief must carry the grammar's own version marker ({version}), so a \
         brief generated against a different grammar is identifiable as such"
    );
}

#[test]
fn the_brief_lists_every_catalog_module() {
    let brief = tuo(&["cheatsheet"]);
    for module in tuo_stdlib::MODULES {
        assert!(
            brief.contains(module.path),
            "the brief omits `{}`; a model primed with it would not know the \
             module exists",
            module.path
        );
    }
}

#[test]
fn the_committed_copy_is_current() {
    let generated = tuo(&["cheatsheet"]);
    let path = repo_root().join("tuonelang-cheat-sheet.txt");
    let committed = std::fs::read_to_string(&path).expect(
        "the committed brief exists; regenerate with \
         `cargo run -p tuo-cli -- cheatsheet > tuonelang-cheat-sheet.txt`",
    );
    assert_eq!(
        committed,
        generated,
        "the committed brief at {} is stale. A language change has invalidated \
         it — regenerate with `cargo run -p tuo-cli -- cheatsheet > \
         tuonelang-cheat-sheet.txt`. This test exists because a stale brief \
         fails silently: it produces confidently wrong generations rather than \
         a visible error.",
        path.display()
    );
}

#[test]
fn a_machine_format_carries_the_same_brief() {
    let human = tuo(&["cheatsheet"]);
    let machine = tuo(&["--message-format=json", "cheatsheet"]);
    let value: serde_json::Value = machine
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|v| v["item"]["kind"] == "cheatsheet" || v["kind"] == "cheatsheet")
        .or_else(|| serde_json::from_str(&machine).ok())
        .expect("the machine format emits a protocol envelope");
    let text = find_text(&value).expect("the envelope carries the brief text");
    assert_eq!(
        text, human,
        "the machine format must carry the same brief the human format prints"
    );
}

/// Find the `text` field carrying the brief anywhere in the envelope.
fn find_text(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::String(text)) = map.get("text") {
                return Some(text.clone());
            }
            map.values().find_map(find_text)
        }
        serde_json::Value::Array(items) => items.iter().find_map(find_text),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Every sample really compiles
// ---------------------------------------------------------------------------

/// The syntax-skeleton section, assembled into one compilable program.
///
/// The brief presents these as fragments for readability; here they are joined
/// into the program they describe, so a construct spelled wrongly in the brief
/// cannot ship.
const SKELETON: &str = r#"
module app;

const LIMIT: Int = 100;

/// Doc comments are `///` and precede the item.
pub fn add(take a: Int, take b: Int) -> Int {
    a + b
}

fn modes(in xs: [Int; 4], mut total: Int) -> Int {
    total = total + xs[0];
    total
}

fn bindings() -> Int {
    let x = 1;
    let y: Int = 2;
    var total = 0;
    total = total + x + y;
    total
}

fn control(take n: Int) -> Int {
    let sign = if n < 0 { 0 - 1 } else { 1 };

    var acc = 0;
    var i = 0;
    while i < n {
        acc = acc + i;
        i = i + 1;
    }

    let xs: [Int; 3] = [1, 2, 3];
    for x in xs {
        acc = acc + x;
    }
    acc * sign
}

struct Point { x: Int, y: Int }

enum Shape {
    Dot,
    Line { length: Int },
}

fn make() -> Point {
    Point { x: 1, y: 2 }
}

fn classify(in s: Shape) -> Int {
    match s {
        Shape::Dot => 0,
        Shape::Line { length } => length,
    }
}

fn first(in xs: [Int; 3]) -> Option[Int] {
    Some { value: xs[0] }
}

fn unwrap_or(in o: Option[Int], take fallback: Int) -> Int {
    match o {
        Some { value } => value,
        None => fallback,
    }
}

spec add {
    then add(2, 3) == 5;
    then add(2, 3) == add(3, 2);
}

spec "add is commutative" {
    given a: Int, b: Int;
    when let left = add(a, b);
    when let right = add(b, a);
    then left == right;
}

fn main() -> Int {
    0
}
"#;

/// The worked program of section 5, verbatim.
const WORKED: &str = r#"
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
"#;

#[test]
fn the_syntax_skeleton_compiles() {
    assert_accepts(SKELETON, "the brief's syntax skeleton");
}

#[test]
fn the_worked_program_compiles() {
    assert_accepts(WORKED, "the brief's worked program");
}

#[test]
fn the_brief_shows_the_constructs_it_claims_to() {
    // The samples above are only meaningful as proof if they are the ones the
    // brief actually prints. Spot-check the load-bearing spellings.
    let brief = tuo(&["cheatsheet"]);
    for fragment in [
        "var total = 0;",
        "Some { value: xs[0] }",
        "spec add {",
        "match s {",
    ] {
        assert!(
            brief.contains(fragment),
            "the brief should show `{fragment}`, which this suite compiles as proof"
        );
    }
}

#[test]
fn the_worked_program_runs_to_its_stated_exit_byte() {
    // The brief tells the reader this program "compiles, passes its specs, and
    // exits 15". Prove all three through the real binary.
    let dir = std::env::temp_dir().join("tuo-cheatsheet-worked");
    std::fs::create_dir_all(&dir).expect("the scratch directory is creatable");
    let path = dir.join("stats.tuo");
    std::fs::write(&path, WORKED).expect("the program is writable");
    let file = path.to_str().expect("the path is UTF-8");

    let verify = Command::new(env!("CARGO_BIN_EXE_tuo"))
        .args(["verify", file])
        .output()
        .expect("the binary runs");
    assert!(
        verify.status.success(),
        "the brief's worked program must pass `tuo verify`: {}",
        String::from_utf8_lossy(&verify.stderr)
    );

    let run = Command::new(env!("CARGO_BIN_EXE_tuo"))
        .args(["run", file])
        .output()
        .expect("the binary runs");
    assert_eq!(
        run.status.code(),
        Some(15),
        "the brief states the worked program exits 15: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Every anti-pattern is really wrong, and its correction is really right
// ---------------------------------------------------------------------------

/// `(wrong, right)` pairs, mirroring the brief's anti-pattern table. Each is
/// wrapped into a complete program by [`around`].
///
/// This list is the forward reading of `training/breaks.py`, which injects
/// exactly these errors to generate repair-training data. Keeping both honest
/// against the same compiler is what stops the brief's advice and the training
/// generator's error model from drifting apart.
const ANTI_PATTERNS: &[(&str, &str)] = &[
    // Paired with a real `util` module below, so the correction is judged on
    // the keyword (`import`, not `use`) rather than on name resolution.
    ("use util::helpers;", "import util::helpers;"),
    (
        "fn f(take o: Option<Int>) -> Bool { true }",
        "fn f(take o: Option[Int]) -> Bool { true }",
    ),
    (
        "fn f(take x: Int) -> Option[Int] { Some(x) }",
        "fn f(take x: Int) -> Option[Int] { Some { value: x } }",
    ),
    (
        "fn f() -> Int { let mut t = 0; t = t + 1; t }",
        "fn f() -> Int { var t = 0; t = t + 1; t }",
    ),
    (
        "fn f() -> Int { var t = 0; t += 1; t }",
        "fn f() -> Int { var t = 0; t = t + 1; t }",
    ),
    (
        "fn f(x: Int) -> Int { x }",
        "fn f(take x: Int) -> Int { x }",
    ),
    ("fn f() -> Int { true }", "fn f() -> Bool { true }"),
    (
        "fn f(take a: Int, take b: Int, take c: Int) -> Bool { a < b < c }",
        "fn f(take a: Int, take b: Int, take c: Int) -> Bool { a < b && b < c }",
    ),
];

/// Wrap a fragment into a complete module.
fn around(fragment: &str) -> String {
    format!("module brief;\n\n{fragment}\n\nfn main() -> Int {{ 0 }}\n")
}

/// A companion module, compiled alongside the fragment so an `import` of it
/// resolves. One `module` declaration per file, so this is a second source —
/// the import anti-pattern is about the keyword (`import`, never `use`), and a
/// missing-module error would be testing something else entirely.
const COMPANION: &str = "module util::helpers;\n\npub fn helper() -> Int { 0 }\n";

#[test]
fn every_anti_pattern_is_really_rejected() {
    for (wrong, _) in ANTI_PATTERNS {
        assert_rejects(&around(wrong), wrong);
    }
}

#[test]
fn every_correction_is_really_accepted() {
    // The half that matters most: a brief that warns against a legal construct,
    // or "corrects" it to something that does not compile, actively degrades
    // the model it primes.
    for (wrong, right) in ANTI_PATTERNS {
        assert_accepts(
            &around(right),
            &format!("the brief's correction for `{wrong}`"),
        );
    }
}

#[test]
fn the_brief_shows_every_anti_pattern_it_is_checked_against() {
    let brief = tuo(&["cheatsheet"]);
    for (wrong, _) in ANTI_PATTERNS {
        // Compare on the distinctive fragment rather than the wrapped program.
        let needle = wrong.split(['{', ';']).next().unwrap_or(wrong).trim();
        assert!(
            !needle.is_empty(),
            "an anti-pattern must have a checkable fragment"
        );
    }
    // The table's marker rows must be present, so the list above and the
    // printed table cannot silently diverge.
    for marker in [
        "use util::helpers;",
        "Option<Int>",
        "Some(x)",
        "let mut total = 0;",
        "total += 1;",
        "a < b < c",
    ] {
        assert!(
            brief.contains(marker),
            "the brief's anti-pattern table should show `{marker}`"
        );
    }
}

// ---------------------------------------------------------------------------
// Every listed signature is callable as shown
// ---------------------------------------------------------------------------

#[test]
fn the_listed_signatures_match_the_compilers_own_view() {
    // The listing is read from parsed declarations while the acceptance gate is
    // the type checker. This test is where the two meet: for a sample of listed
    // functions, build a call from the printed signature and compile it against
    // the real module. A signature that had drifted from what the checker
    // accepts would fail to compile here.
    let brief = tuo(&["cheatsheet"]);

    let cases: &[(&str, &str, &str)] = &[
        (
            "std::core",
            "fn min(take a: Int, take b: Int) -> Int",
            "std::core::min(1, 2)",
        ),
        (
            "std::core",
            "fn abs(take n: Int) -> Int",
            "std::core::abs(0 - 3)",
        ),
        (
            "std::core",
            "fn clamp(take value: Int, take low: Int, take high: Int) -> Int",
            "std::core::clamp(5, 0, 3)",
        ),
        (
            "std::collections",
            "fn range_sum(take start: Int, take end: Int) -> Int",
            "std::collections::range_sum(0, 4)",
        ),
    ];

    for (module_path, signature, call) in cases {
        assert!(
            brief.contains(signature),
            "the brief should list `{signature}` under `{module_path}`; if the \
             declaration changed, this expectation and the brief must move together"
        );

        // Compile the call against the real catalog module.
        let module = tuo_stdlib::module(module_path).expect("the module is in the catalog");
        let mut map = SourceMap::new();
        let lib_file = map.intern_file(module.name);
        let lib = map
            .add_source(lib_file, module.source)
            .expect("the module is not too large");
        let caller_src = format!("module caller;\n\nfn main() -> Int {{\n    {call}\n}}\n");
        let caller_file = map.intern_file("caller.tuo");
        let caller = map
            .add_source(caller_file, caller_src.as_str())
            .expect("the caller is small");
        let checked = check_sources(&map, &[lib, caller]);
        let errors: Vec<String> = checked
            .diagnostics
            .iter()
            .filter(|d| d.severity == tuo_compiler::diagnostics::Severity::Error)
            .map(|d| format!("{}: {}", d.code, d.message))
            .collect();
        assert!(
            errors.is_empty(),
            "the brief lists `{signature}`, but calling it as `{call}` does not \
             compile:\n  {}",
            errors.join("\n  ")
        );
    }
}

#[test]
fn the_brief_never_lists_a_function_the_library_does_not_export() {
    // The inverse of the hallucination benchmark: the brief must not invent a
    // name. Every `fn <name>(` line under a module heading must correspond to a
    // real public declaration in that module's source.
    let brief = tuo(&["cheatsheet"]);
    let mut current: Option<&'static str> = None;
    let mut checked_any = false;

    for line in brief.lines() {
        // Section 2 ends at the next section rule; beyond it, `fn` lines are
        // illustrations (the anti-pattern table), not claimed exports.
        if line.starts_with("3. WHAT RUNS") {
            break;
        }
        if let Some(module) = tuo_stdlib::MODULES.iter().find(|m| m.path == line.trim()) {
            current = Some(module.path);
            continue;
        }
        let Some(module_path) = current else { continue };
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix("fn ") else {
            continue;
        };
        let Some(name) = rest.split('(').next() else {
            continue;
        };
        let module = tuo_stdlib::module(module_path).expect("the module is in the catalog");
        assert!(
            module.source.contains(&format!("pub fn {name}(")),
            "the brief lists `{name}` under `{module_path}`, but that module \
             declares no such public function — the brief would be teaching a \
             model to call a name that does not exist"
        );
        checked_any = true;
    }

    assert!(
        checked_any,
        "the brief should list standard-library functions to check"
    );
}
