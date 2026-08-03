//! The compiler-validated corpus pipeline for tuonelang.
//!
//! tuonelang's corpus is a body of programs the compiler has *actually*
//! validated — never a bag of files someone asserts are good. This crate owns
//! the pipeline that earns that trust and the metadata that records it.
//!
//! # What "validated" means
//!
//! A candidate program is admitted only after clearing the required, ordered
//! gauntlet, driving the **real** compiler stages at each step:
//!
//! ```text
//! format → parse → resolve → type check → ownership → MIR verify →
//!     specs/tests → native execution (where applicable)
//! ```
//!
//! The stages short-circuit at the first failure and every later stage is
//! recorded as skipped, so a validation record is an honest account of exactly
//! how far a program got. Native execution — the one stage needing a concrete
//! backend and the `cc` linker — is a host-injected [`NativeExecutor`] seam, so
//! this crate stays free of any backend while still recording whether a program
//! runs.
//!
//! # The six corpora
//!
//! Candidates come from four [`Origin`]s — humans, generators, LLMs, and
//! transformed benchmark tasks — and are sorted into six [`Category`]s, each
//! with its own admission contract:
//!
//! - **correct programs** — pass the whole pipeline;
//! - **syntax-error repair** — fail at parse;
//! - **type-error repair** — fail at type check;
//! - **ownership-error repair** — fail at ownership;
//! - **failing-spec repair** — statically valid, a spec fails;
//! - **repository-level changes** — a multi-file change validated as a whole.
//!
//! Crucially, a repair program is admitted only if it fails **at exactly the
//! stage its category names** (see [`corpus::admit`]) — so a "type-error" entry
//! is a real type error, not a mislabeled parse error — and, when a corrected
//! program is attached, only if that fix validates clean. The corpus stores real
//! (broken → repaired) pairs, never an unverified claim.
//!
//! # Metadata
//!
//! Every admitted entry carries a full [`EntryMetadata`] record: the language
//! version it was validated against, its source [`Origin`], the language
//! [`Feature`]s it uses, the per-stage [`ValidationResults`], a coarse
//! [`Complexity`] measure, and token counts under every deterministic tokenizer
//! the research harness ships (reused from [`tuo_bench`], so there is one
//! measurement of record).
//!
//! # Honesty
//!
//! Nothing here asserts a capability the compiler lacks. Native execution is
//! only claimed when a host actually ran the program; feature detection is exact
//! for declarations and conservative (token-based) for everything else; and a
//! corpus entry's category is *proven* by the pipeline, not trusted from its
//! label.

mod corpus;
mod features;
mod metadata;
mod pipeline;
mod tokens;

pub use corpus::{
    AdmittedEntry, Candidate, Category, Rejection, SourceFile, admit, expected_namespace,
};
pub use features::{ProgramFacts, extract_facts};
pub use metadata::{
    Complexity, EntryMetadata, Feature, FeatureSet, LanguageVersion, Origin, StageStatus,
    TokenCount, ValidationResults,
};
pub use pipeline::{Config, ENTRY, NativeExecutor, NativeRun, ValidationReport, validate};
pub use tokens::{count, count_builtin};
