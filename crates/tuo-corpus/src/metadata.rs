//! The metadata recorded for every corpus entry.
//!
//! A corpus entry is only as trustworthy as what we know about it, so each one
//! carries a full [`EntryMetadata`] record: the language version it was
//! validated against, where its source came from, which language features it
//! uses, the exact per-stage validation results, a coarse complexity measure,
//! and — reusing the research harness's tokenizers — token counts under every
//! deterministic tokenizer we can measure.
//!
//! Everything here is a plain, serializable value. The metadata is *derived*
//! from a real validation run (see [`crate::pipeline`]); nothing in this module
//! runs the compiler, so it can be inspected, serialized, and diffed on its own.

use serde::{Deserialize, Serialize};

/// The tuonelang language version a corpus entry was validated against.
///
/// v0 has exactly one language edition (`2024`), but a corpus outlives any one
/// compiler build, so every entry records the version *string* it was validated
/// under. A consumer comparing two corpora can then detect entries validated by
/// an older toolchain rather than silently trusting them.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct LanguageVersion(pub String);

impl LanguageVersion {
    /// The language version this build validates against: the sole v0 edition.
    #[must_use]
    pub fn current() -> Self {
        Self("2024".to_string())
    }

    /// The version string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Where a corpus candidate came from.
///
/// The corpus admits programs from several sources; recording which one lets a
/// consumer weight or partition entries by provenance (for instance, to measure
/// how LLM-generated programs fare against hand-written ones). The category is
/// declared by whoever submits the candidate — the pipeline validates the
/// *program*, not the honesty of its stated origin.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// Written by a human.
    Human,
    /// Emitted by a deterministic program generator.
    Generator,
    /// Produced by an LLM.
    Llm,
    /// Adapted from an external benchmark task.
    TransformedBenchmark,
}

impl Origin {
    /// A short, stable machine identifier.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Generator => "generator",
            Self::Llm => "llm",
            Self::TransformedBenchmark => "transformed_benchmark",
        }
    }
}

/// A single language feature a program exercises.
///
/// The feature set is deliberately coarse — it names the v0 constructs a
/// program *uses*, so a consumer can query the corpus for coverage ("which
/// entries exercise enums?") without re-parsing every file. Features are
/// detected from the validated program: declaration kinds come from the
/// resolved symbol table, and constructs the symbol table does not surface
/// (imports, control flow, specs, arithmetic) are detected lexically. Detection
/// is conservative — a feature is reported only when its evidence is present.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Feature {
    /// Declares at least one function.
    Function,
    /// Declares at least one `struct`.
    Struct,
    /// Declares at least one `enum`.
    Enum,
    /// Declares at least one `interface`.
    Interface,
    /// Declares at least one module-level `const`.
    Const,
    /// Declares at least one colocated `spec`.
    Spec,
    /// Uses an `import` to bring a symbol into scope.
    Import,
    /// Declares more than one `module`.
    MultiModule,
    /// Uses a conditional (`if` / `else`).
    Conditional,
    /// Uses a `match` expression.
    Match,
    /// Uses a loop construct (`while` / `loop` / `for`).
    Loop,
    /// Uses a mutable binding (`var`).
    Mutability,
    /// Uses arithmetic on values.
    Arithmetic,
    /// References an ownership mode marker (`take` / `borrow` / `mut`).
    OwnershipModes,
}

impl Feature {
    /// A short, stable machine identifier.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Function => "function",
            Self::Struct => "struct",
            Self::Enum => "enum",
            Self::Interface => "interface",
            Self::Const => "const",
            Self::Spec => "spec",
            Self::Import => "import",
            Self::MultiModule => "multi_module",
            Self::Conditional => "conditional",
            Self::Match => "match",
            Self::Loop => "loop",
            Self::Mutability => "mutability",
            Self::Arithmetic => "arithmetic",
            Self::OwnershipModes => "ownership_modes",
        }
    }
}

/// A sorted, de-duplicated set of the features a program uses.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct FeatureSet(Vec<Feature>);

impl FeatureSet {
    /// An empty feature set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that `feature` is present (idempotent).
    pub fn insert(&mut self, feature: Feature) {
        if let Err(pos) = self.0.binary_search(&feature) {
            self.0.insert(pos, feature);
        }
    }

    /// Does the set contain `feature`?
    #[must_use]
    pub fn contains(&self, feature: Feature) -> bool {
        self.0.binary_search(&feature).is_ok()
    }

    /// The features, in a stable sorted order.
    #[must_use]
    pub fn as_slice(&self) -> &[Feature] {
        &self.0
    }

    /// Iterate the features in sorted order.
    pub fn iter(&self) -> impl Iterator<Item = Feature> + '_ {
        self.0.iter().copied()
    }

    /// The number of distinct features.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Is the set empty?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// A coarse complexity measure for a corpus entry.
///
/// These are cheap, syntactic counts — not a semantic complexity metric. They
/// exist so a consumer can bucket the corpus by size ("small programs" vs
/// "large ones") and track how validation cost scales, without imposing a
/// single scalar "difficulty" the numbers cannot honestly support.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct Complexity {
    /// Source bytes.
    pub bytes: usize,
    /// Non-blank source lines.
    pub lines: usize,
    /// Distinct top-level (module-level) declarations.
    pub items: usize,
    /// Colocated specs.
    pub specs: usize,
    /// Distinct language features exercised (`FeatureSet::len`).
    pub features: usize,
}

/// Token counts for one tokenizer.
///
/// The corpus records how each entry tokenizes under every deterministic,
/// offline tokenizer the research harness ships ([`tuo_bench::tokenizer`]), so
/// downstream LLM-reliability work has real token budgets to reason about. The
/// list is empty only if no tokenizer was available.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct TokenCount {
    /// The tokenizer's stable id (e.g. `"bytes"`, `"gpt-like"`).
    pub tokenizer: String,
    /// The number of tokens the source encodes to.
    pub tokens: usize,
}

/// The result of one validation stage.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageStatus {
    /// The stage ran and the program met its requirement.
    Passed,
    /// The stage ran and the program failed its requirement.
    Failed,
    /// The stage did not run because an earlier required stage failed, or
    /// because it does not apply to this entry (e.g. native execution for a
    /// program with no runnable entry).
    Skipped,
}

/// The per-stage outcome of validating one candidate.
///
/// This is the machine-readable record of the required validation pipeline —
/// format → parse → resolve → type check → ownership → MIR verify →
/// specs/tests → native execution. Each field is the [`StageStatus`] of that
/// stage for this entry. A `Skipped` stage carries no verdict; it neither
/// passed nor failed.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ValidationResults {
    /// Canonical formatting (`tuo fmt --check` clean).
    pub format: StageStatus,
    /// Parsing (no parse errors).
    pub parse: StageStatus,
    /// Name/path resolution (no resolution errors).
    pub resolve: StageStatus,
    /// Type checking (no type errors).
    pub type_check: StageStatus,
    /// Ownership checking (no ownership errors).
    pub ownership: StageStatus,
    /// MIR verification (every lowered function verifies).
    pub mir_verify: StageStatus,
    /// Colocated specs / tests (all pass).
    pub specs: StageStatus,
    /// Native execution where applicable (compiles and the entry runs).
    pub native_execution: StageStatus,
}

impl ValidationResults {
    /// The results before any stage has run: every stage skipped.
    #[must_use]
    pub fn pending() -> Self {
        Self {
            format: StageStatus::Skipped,
            parse: StageStatus::Skipped,
            resolve: StageStatus::Skipped,
            type_check: StageStatus::Skipped,
            ownership: StageStatus::Skipped,
            mir_verify: StageStatus::Skipped,
            specs: StageStatus::Skipped,
            native_execution: StageStatus::Skipped,
        }
    }

    /// Did any stage that ran fail?
    #[must_use]
    pub fn any_failed(&self) -> bool {
        self.stages()
            .iter()
            .any(|(_, status)| *status == StageStatus::Failed)
    }

    /// The stages that ran and passed, in pipeline order.
    #[must_use]
    pub fn passed_stages(&self) -> Vec<&'static str> {
        self.stages()
            .iter()
            .filter(|(_, status)| *status == StageStatus::Passed)
            .map(|(name, _)| *name)
            .collect()
    }

    /// The (name, status) of every stage, in pipeline order.
    #[must_use]
    pub fn stages(&self) -> [(&'static str, StageStatus); 8] {
        [
            ("format", self.format),
            ("parse", self.parse),
            ("resolve", self.resolve),
            ("type_check", self.type_check),
            ("ownership", self.ownership),
            ("mir_verify", self.mir_verify),
            ("specs", self.specs),
            ("native_execution", self.native_execution),
        ]
    }
}

/// The complete metadata record stored for one corpus entry.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct EntryMetadata {
    /// The language version this entry was validated against.
    pub language_version: LanguageVersion,
    /// Where the source came from.
    pub origin: Origin,
    /// The language features the program uses.
    pub features: FeatureSet,
    /// The per-stage validation results.
    pub validation: ValidationResults,
    /// A coarse complexity measure.
    pub complexity: Complexity,
    /// Token counts under every available tokenizer, in registration order.
    pub token_counts: Vec<TokenCount>,
}
