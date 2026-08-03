//! Integration tests for the corpus validation pipeline and its six-corpus
//! admission contracts.
//!
//! These drive the *real* compiler stages (through the public `tuo-corpus`
//! surface): a program is admitted only when the pipeline confirms it behaves as
//! its corpus requires. Native execution is passed `None` here — the crate
//! cannot link a binary on its own — so it is recorded as skipped; the CLI's
//! integration test exercises the injected native seam end to end.

use tuo_corpus::{
    Candidate, Category, Config, Feature, Origin, Rejection, SourceFile, StageStatus, admit,
    validate,
};
use tuo_source::SourceMap;

/// Load one program's text into a fresh map and validate it (no native seam).
fn validate_one(name: &str, text: &str) -> tuo_corpus::ValidationReport {
    let mut map = SourceMap::new();
    let file = map.intern_file(name);
    let source = map.add_source(file, text.to_string()).expect("adds source");
    validate(&map, &[source], Config::default(), None)
}

/// A correct, canonically-formatted single-function program with a passing spec.
const CORRECT: &str = "\
fn double(take x: Int) -> Int {
    x + x
}

spec double {
    then double(3) == 6;
    then double(0) == 0;
}
";

/// A correct program with a runnable `main` entry.
const CORRECT_MAIN: &str = "\
fn main() -> Int {
    0
}

spec main {
    then main() == 0;
}
";

/// A program with a parse error (a missing closing brace region): the parser
/// cannot make sense of it.
const SYNTAX_BROKEN: &str = "\
fn broken(take x: Int) -> Int {
    x +
}
";

/// A program that parses and resolves but does not type-check: it returns a
/// `Bool` where an `Int` is declared.
const TYPE_BROKEN: &str = "\
fn wrong() -> Int {
    true
}
";

/// A statically valid program whose spec is false.
const SPEC_BROKEN: &str = "\
fn double(take x: Int) -> Int {
    x + x
}

spec double {
    then double(3) == 7;
}
";

#[test]
fn a_correct_program_passes_every_applicable_stage() {
    let report = validate_one("correct.tuo", CORRECT);
    assert!(
        report.is_clean(),
        "correct program should be clean: {report:?}"
    );
    let r = &report.results;
    assert_eq!(r.format, StageStatus::Passed);
    assert_eq!(r.parse, StageStatus::Passed);
    assert_eq!(r.resolve, StageStatus::Passed);
    assert_eq!(r.type_check, StageStatus::Passed);
    assert_eq!(r.ownership, StageStatus::Passed);
    assert_eq!(r.mir_verify, StageStatus::Passed);
    assert_eq!(r.specs, StageStatus::Passed);
    // No native executor supplied and no `main`, so native execution is skipped.
    assert_eq!(r.native_execution, StageStatus::Skipped);
}

#[test]
fn features_are_detected_from_a_validated_program() {
    let report = validate_one("correct.tuo", CORRECT);
    let facts = report.facts.expect("facts for an accepted program");
    assert!(facts.features.contains(Feature::Function));
    assert!(facts.features.contains(Feature::Spec));
    assert!(facts.features.contains(Feature::Arithmetic));
    assert!(facts.features.contains(Feature::OwnershipModes)); // `take`
    assert!(!facts.features.contains(Feature::Enum));
    assert_eq!(facts.spec_count, 1);
}

#[test]
fn a_program_with_main_has_a_native_entry() {
    let report = validate_one("main.tuo", CORRECT_MAIN);
    assert!(report.is_clean());
    assert!(report.has_native_entry, "main() is a runnable entry");
    // Still skipped without an executor, but *applicable*.
    assert_eq!(report.results.native_execution, StageStatus::Skipped);
}

#[test]
fn a_syntax_error_fails_at_parse_and_skips_later_stages() {
    let report = validate_one("broken.tuo", SYNTAX_BROKEN);
    assert_eq!(report.results.parse, StageStatus::Failed);
    assert_eq!(report.results.resolve, StageStatus::Skipped);
    assert_eq!(report.results.type_check, StageStatus::Skipped);
    assert!(!report.is_clean());
}

#[test]
fn a_type_error_fails_at_type_check_not_earlier() {
    let report = validate_one("type.tuo", TYPE_BROKEN);
    assert_eq!(report.results.parse, StageStatus::Passed);
    assert_eq!(report.results.resolve, StageStatus::Passed);
    assert_eq!(report.results.type_check, StageStatus::Failed);
    assert_eq!(report.results.ownership, StageStatus::Skipped);
}

#[test]
fn a_false_spec_fails_at_the_specs_stage() {
    let report = validate_one("spec.tuo", SPEC_BROKEN);
    assert_eq!(report.results.type_check, StageStatus::Passed);
    assert_eq!(report.results.mir_verify, StageStatus::Passed);
    assert_eq!(report.results.specs, StageStatus::Failed);
}

#[test]
fn correct_corpus_admits_a_clean_program() {
    let candidate = Candidate::single(Category::Correct, Origin::Human, "correct.tuo", CORRECT);
    let entry = admit(&candidate, Config::default(), None).expect("admitted");
    assert_eq!(entry.category, Category::Correct);
    assert_eq!(entry.metadata.origin, Origin::Human);
    assert_eq!(entry.metadata.language_version.as_str(), "2024");
    // A correct single-file entry is stored canonically.
    assert!(!entry.files.is_empty());
    // Token counts came from the harness's built-in tokenizers.
    assert!(!entry.metadata.token_counts.is_empty());
}

#[test]
fn correct_corpus_rejects_a_broken_program() {
    let candidate = Candidate::single(
        Category::Correct,
        Origin::Human,
        "broken.tuo",
        SYNTAX_BROKEN,
    );
    let rejection = admit(&candidate, Config::default(), None).expect_err("rejected");
    assert!(
        matches!(rejection, Rejection::NotClean { stage: "parse" }),
        "{rejection:?}"
    );
}

#[test]
fn syntax_repair_corpus_admits_a_parse_error() {
    let candidate = Candidate::single(
        Category::SyntaxRepair,
        Origin::Llm,
        "broken.tuo",
        SYNTAX_BROKEN,
    );
    let entry = admit(&candidate, Config::default(), None).expect("admitted");
    assert_eq!(entry.category, Category::SyntaxRepair);
    assert_eq!(entry.metadata.validation.parse, StageStatus::Failed);
}

#[test]
fn type_repair_corpus_rejects_a_program_that_fails_at_the_wrong_stage() {
    // A parse error submitted to the *type*-repair corpus is mislabeled.
    let candidate = Candidate::single(
        Category::TypeRepair,
        Origin::Llm,
        "broken.tuo",
        SYNTAX_BROKEN,
    );
    let rejection = admit(&candidate, Config::default(), None).expect_err("rejected");
    assert!(
        matches!(
            rejection,
            Rejection::WrongFailureStage {
                expected: "type_check",
                actual: "parse"
            }
        ),
        "{rejection:?}"
    );
}

#[test]
fn type_repair_corpus_admits_a_real_type_error_with_a_working_fix() {
    let candidate = Candidate::single(Category::TypeRepair, Origin::Human, "type.tuo", TYPE_BROKEN)
        .with_fix(vec![SourceFile::new(
            "type.tuo",
            "fn wrong() -> Int {\n    0\n}\n",
        )]);
    let entry = admit(&candidate, Config::default(), None).expect("admitted with a valid fix");
    assert_eq!(entry.category, Category::TypeRepair);
    assert_eq!(entry.metadata.validation.type_check, StageStatus::Failed);
}

#[test]
fn a_repair_with_a_non_fixing_fix_is_rejected() {
    // The "fix" is still a type error, so it does not repair anything.
    let candidate = Candidate::single(Category::TypeRepair, Origin::Llm, "type.tuo", TYPE_BROKEN)
        .with_fix(vec![SourceFile::new("type.tuo", TYPE_BROKEN)]);
    let rejection = admit(&candidate, Config::default(), None).expect_err("rejected");
    assert!(
        matches!(
            rejection,
            Rejection::FixNotClean {
                stage: "type_check"
            }
        ),
        "{rejection:?}"
    );
}

#[test]
fn spec_repair_corpus_admits_a_failing_spec() {
    let candidate = Candidate::single(
        Category::SpecRepair,
        Origin::Generator,
        "spec.tuo",
        SPEC_BROKEN,
    );
    let entry = admit(&candidate, Config::default(), None).expect("admitted");
    assert_eq!(entry.metadata.validation.specs, StageStatus::Failed);
    // Everything up to specs passed.
    assert_eq!(entry.metadata.validation.type_check, StageStatus::Passed);
}

#[test]
fn a_repair_that_is_actually_clean_is_rejected() {
    // A correct program submitted to a repair corpus is not broken at all.
    let candidate = Candidate::single(
        Category::OwnershipRepair,
        Origin::Human,
        "correct.tuo",
        CORRECT,
    );
    let rejection = admit(&candidate, Config::default(), None).expect_err("rejected");
    assert!(
        matches!(rejection, Rejection::ExpectedFailureButClean),
        "{rejection:?}"
    );
}

#[test]
fn every_category_has_a_stable_id() {
    // The ids are the on-disk directory names; they must be stable and unique.
    let ids: Vec<&str> = Category::all().iter().map(|c| c.id()).collect();
    let mut unique = ids.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(ids.len(), unique.len(), "category ids must be unique");
}
