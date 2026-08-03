//! The six compiler-validated corpora and their admission rules.
//!
//! The corpus is not one bucket of programs; it is six, each with a *different*
//! validation contract. A candidate is admitted to a corpus only when the
//! pipeline confirms the program behaves as that corpus requires — a correct
//! program must pass everything, and a repair program must fail *at the stage
//! its category names* (and no earlier), so a "type-error" entry genuinely is a
//! type error and not a parse error mislabeled.
//!
//! | Corpus | Contract |
//! |--------|----------|
//! | [`Category::Correct`] | passes the whole pipeline |
//! | [`Category::SyntaxRepair`] | fails at parse (a lexical/parse error) |
//! | [`Category::TypeRepair`] | parses and resolves, fails at type check |
//! | [`Category::OwnershipRepair`] | type-checks, fails at ownership |
//! | [`Category::SpecRepair`] | statically valid, a spec fails |
//! | [`Category::RepositoryChange`] | a multi-file change, validated as a whole |
//!
//! A **repair** entry may additionally carry a `fixed` program: the corrected
//! source that the same pipeline validates *clean*. When present, admission also
//! confirms the fix works, so the corpus stores real (broken → repaired) pairs a
//! consumer can train or evaluate against, never an unverified "fix".

use tuo_diagnostics::Namespace;
use tuo_source::{SourceId, SourceMap};

use crate::metadata::{EntryMetadata, LanguageVersion, Origin, StageStatus};
use crate::pipeline::{Config, NativeExecutor, ValidationReport, validate};
use crate::tokens;

/// Which corpus a program belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Category {
    /// Programs that pass the entire validation pipeline.
    Correct,
    /// Programs with a syntax error, for syntax-error repair.
    SyntaxRepair,
    /// Programs that parse and resolve but fail type checking, for type-error
    /// repair.
    TypeRepair,
    /// Programs that type-check but fail ownership checking, for
    /// ownership-error repair.
    OwnershipRepair,
    /// Statically valid programs with a failing spec, for failing-spec repair.
    SpecRepair,
    /// Repository-level changes (multiple files validated together).
    RepositoryChange,
}

impl Category {
    /// A short, stable machine identifier / directory name.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Correct => "correct",
            Self::SyntaxRepair => "syntax_repair",
            Self::TypeRepair => "type_repair",
            Self::OwnershipRepair => "ownership_repair",
            Self::SpecRepair => "spec_repair",
            Self::RepositoryChange => "repository_change",
        }
    }

    /// All six categories, in a stable order.
    #[must_use]
    pub const fn all() -> [Self; 6] {
        [
            Self::Correct,
            Self::SyntaxRepair,
            Self::TypeRepair,
            Self::OwnershipRepair,
            Self::SpecRepair,
            Self::RepositoryChange,
        ]
    }

    /// Does this category hold *broken* programs (i.e. is it a repair corpus)?
    #[must_use]
    pub const fn is_repair(self) -> bool {
        !matches!(self, Self::Correct | Self::RepositoryChange)
    }
}

/// One source file in a candidate program (a candidate may span several).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SourceFile {
    /// The file's logical name (used for module resolution and diagnostics).
    pub name: String,
    /// The file's text.
    pub text: String,
}

impl SourceFile {
    /// A source file with the given name and text.
    pub fn new(name: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            text: text.into(),
        }
    }
}

/// A program submitted for admission to a corpus.
///
/// A candidate is a set of source files, a declared category, and a stated
/// origin. For a repair candidate, `fixed` optionally carries the corrected
/// program the pipeline should validate clean.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Candidate {
    /// The corpus this candidate is submitted to.
    pub category: Category,
    /// Where the source came from.
    pub origin: Origin,
    /// The program's source files (at least one).
    pub files: Vec<SourceFile>,
    /// For a repair candidate: the corrected program, if supplied.
    pub fixed: Option<Vec<SourceFile>>,
}

impl Candidate {
    /// A single-file candidate.
    #[must_use]
    pub fn single(
        category: Category,
        origin: Origin,
        name: impl Into<String>,
        text: impl Into<String>,
    ) -> Self {
        Self {
            category,
            origin,
            files: vec![SourceFile::new(name, text)],
            fixed: None,
        }
    }

    /// Attach a corrected program to a repair candidate (builder style).
    #[must_use]
    pub fn with_fix(mut self, fixed: Vec<SourceFile>) -> Self {
        self.fixed = Some(fixed);
        self
    }
}

/// Why a candidate was rejected from the corpus it was submitted to.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Rejection {
    /// A `Correct` candidate did not pass every stage.
    NotClean {
        /// The stage that failed.
        stage: &'static str,
    },
    /// A repair candidate was expected to fail at one stage but the pipeline
    /// found it clean (nothing failed) — it is not a broken program at all.
    ExpectedFailureButClean,
    /// A repair candidate failed, but at the wrong stage: its real failure is
    /// mislabeled. Carries where it actually failed vs. where the category
    /// requires it to fail.
    WrongFailureStage {
        /// The stage the category requires the failure at.
        expected: &'static str,
        /// The stage the program actually failed at.
        actual: &'static str,
    },
    /// A repair candidate carried a `fixed` program that did not itself
    /// validate clean — the "repair" does not actually repair.
    FixNotClean {
        /// The stage the supposed fix failed at.
        stage: &'static str,
    },
    /// A candidate had no source files.
    Empty,
}

/// A candidate that passed admission: its category, origin, canonical program,
/// and the full metadata record.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AdmittedEntry {
    /// The corpus it was admitted to.
    pub category: Category,
    /// The complete metadata record.
    pub metadata: EntryMetadata,
    /// The (canonical, for `Correct`) source files stored for this entry.
    pub files: Vec<SourceFile>,
    /// The validation report produced during admission.
    pub report: ValidationReport,
}

/// Admit `candidate` to its corpus, or explain why it cannot be.
///
/// Runs the full validation pipeline over the candidate's files and checks the
/// outcome against the category's contract:
///
/// - [`Category::Correct`] and [`Category::RepositoryChange`] must be **clean**
///   (every stage that ran passed).
/// - each repair category must **fail at exactly its named stage** (and not
///   earlier). If a `fixed` program is present, it must validate clean.
///
/// `native` is the host's native-execution seam; pass `None` to skip native
/// execution (it is then recorded as [`StageStatus::Skipped`], never assumed).
pub fn admit(
    candidate: &Candidate,
    config: Config,
    native: Option<&dyn NativeExecutor>,
) -> Result<AdmittedEntry, Rejection> {
    if candidate.files.is_empty() {
        return Err(Rejection::Empty);
    }

    let (map, sources) = load(&candidate.files);
    let report = validate(&map, &sources, config, native);

    match candidate.category {
        Category::Correct | Category::RepositoryChange => {
            if let Some(stage) = first_failed_stage(&report) {
                return Err(Rejection::NotClean { stage });
            }
        }
        repair => {
            check_repair_contract(repair, &report)?;
            if let Some(fixed) = &candidate.fixed {
                let (fix_map, fix_sources) = load(fixed);
                let fix_report = validate(&fix_map, &fix_sources, config, native);
                if let Some(stage) = first_failed_stage(&fix_report) {
                    return Err(Rejection::FixNotClean { stage });
                }
            }
        }
    }

    let metadata = build_metadata(candidate, &map, &sources, &report);
    // A `Correct` entry is stored in its canonical form; a repair entry is
    // stored verbatim (its whole point is the un-canonical, broken text).
    let files = canonical_files(candidate, &report);
    Ok(AdmittedEntry {
        category: candidate.category,
        metadata,
        files,
        report,
    })
}

/// Confirm a repair candidate fails at exactly the stage its category requires.
fn check_repair_contract(category: Category, report: &ValidationReport) -> Result<(), Rejection> {
    let expected = required_failure_stage(category);
    match first_failed_stage(report) {
        None => Err(Rejection::ExpectedFailureButClean),
        Some(actual) if actual == expected => Ok(()),
        Some(actual) => Err(Rejection::WrongFailureStage { expected, actual }),
    }
}

/// The stage a repair category requires the program to fail at.
fn required_failure_stage(category: Category) -> &'static str {
    match category {
        Category::SyntaxRepair => "parse",
        Category::TypeRepair => "type_check",
        Category::OwnershipRepair => "ownership",
        Category::SpecRepair => "specs",
        // The clean categories have no required failure stage.
        Category::Correct | Category::RepositoryChange => unreachable!(),
    }
}

/// The first stage (in pipeline order) that failed, if any.
fn first_failed_stage(report: &ValidationReport) -> Option<&'static str> {
    report
        .results
        .stages()
        .into_iter()
        .find(|(_, status)| *status == StageStatus::Failed)
        .map(|(name, _)| name)
}

/// Build the metadata record for an admitted candidate.
fn build_metadata(
    candidate: &Candidate,
    map: &SourceMap,
    sources: &[SourceId],
    report: &ValidationReport,
) -> EntryMetadata {
    let features = report
        .facts
        .as_ref()
        .map(|facts| facts.features.clone())
        .unwrap_or_default();
    let complexity = report
        .facts
        .as_ref()
        .map(|facts| facts.complexity(map, sources))
        .unwrap_or_else(|| {
            // A program that never reached the front end still gets an honest
            // size measure (bytes/lines), with zero item/spec/feature counts.
            use crate::metadata::Complexity;
            let mut bytes = 0;
            let mut lines = 0;
            for &id in sources {
                let text = map.source(id).text();
                bytes += text.len();
                lines += text.lines().filter(|l| !l.trim().is_empty()).count();
            }
            Complexity {
                bytes,
                lines,
                items: 0,
                specs: 0,
                features: 0,
            }
        });

    // Token counts are measured over the concatenated program text so a
    // multi-file entry has one budget. Reuses the harness's tokenizers.
    let joined = candidate
        .files
        .iter()
        .map(|f| f.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let token_counts = tokens::count_builtin(&joined);

    EntryMetadata {
        language_version: LanguageVersion::current(),
        origin: candidate.origin,
        features,
        validation: report.results,
        complexity,
        token_counts,
    }
}

/// The files an admitted entry is stored with. `Correct` entries are stored in
/// canonical form (the formatter's output for the first file, when available);
/// everything else is stored verbatim.
fn canonical_files(candidate: &Candidate, report: &ValidationReport) -> Vec<SourceFile> {
    if candidate.category == Category::Correct
        && candidate.files.len() == 1
        && let Some(canonical) = &report.canonical_source
    {
        return vec![SourceFile::new(
            candidate.files[0].name.clone(),
            canonical.clone(),
        )];
    }
    candidate.files.clone()
}

/// Load a set of source files into a fresh [`SourceMap`], returning the map and
/// the source ids in file order.
fn load(files: &[SourceFile]) -> (SourceMap, Vec<SourceId>) {
    let mut map = SourceMap::new();
    let mut sources = Vec::with_capacity(files.len());
    for file in files {
        let id = map.intern_file(&file.name);
        // A duplicate file name or unreadable text would be a caller error; the
        // pipeline treats an add failure as an empty source so validation still
        // runs (and fails) deterministically rather than panicking.
        if let Ok(source) = map.add_source(id, file.text.clone()) {
            sources.push(source);
        }
    }
    (map, sources)
}

/// The diagnostic namespace a repair category's failure should carry, for a
/// consumer that wants to cross-check a stored entry's diagnostics against its
/// category. `None` for the clean categories.
#[must_use]
pub fn expected_namespace(category: Category) -> Option<Namespace> {
    Some(match category {
        Category::SyntaxRepair => Namespace::Parser,
        Category::TypeRepair => Namespace::Type,
        Category::OwnershipRepair => Namespace::Ownership,
        Category::SpecRepair => Namespace::Specification,
        Category::Correct | Category::RepositoryChange => return None,
    })
}
