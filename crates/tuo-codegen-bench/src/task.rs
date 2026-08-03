//! Benchmark tasks: the version-controlled *inputs* to an evaluation, and the
//! guarantee that they are never changed silently.
//!
//! A [`BenchTask`] is what a model is asked to produce: a natural-language
//! instruction, the colocated `spec`s the generated program must satisfy, an
//! optional set of **held-out tests** (specs the model is *not* shown, used to
//! score TestPass@1 independently of the specs the model could have overfit to),
//! and optional [`SyntaxVariant`]s — comparable spellings of the same task so a
//! language-design decision can be evaluated empirically rather than by taste.
//!
//! # Tasks are never changed silently
//!
//! Every task carries a content [`digest`](BenchTask::digest) over its own
//! fields. A [`TaskSet`] records the digest of each task it contains; loading a
//! task set [verifies](TaskSet::verify_digests) that every task's stored digest
//! still matches its content, so an edit to a task's prompt or specs that was not
//! accompanied by a digest update is a *loud* error, not a quiet drift. This is
//! the same honesty discipline the corpus uses for its shipped fixtures: the
//! benchmark you run is provably the benchmark that was reviewed.

use serde::{Deserialize, Serialize};

/// The prompt file/entry name a generated program's single source is stored
/// under while it is compiled (tasks are single-module in v0).
pub(crate) const GENERATED_FILE: &str = "generated.tuo";

/// One comparable spelling of a task, for empirical language-design evaluation.
///
/// A variant does not change *what* the task asks for — only the surface syntax a
/// generation is steered toward and, optionally, the spec spelling used to score
/// it. Comparing metrics across a task's variants is how a syntax decision is
/// evaluated with data instead of opinion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyntaxVariant {
    /// A short, stable label for this spelling (e.g. `"fn"`, `"func"`).
    pub label: String,
    /// A human-readable note on what this variant changes and why it is worth
    /// comparing.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// The colocated specs written in this variant's spelling. When empty the
    /// task's default [`specs`](BenchTask::specs) are used, so a variant that
    /// only changes non-spec syntax need not restate them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub specs: Vec<String>,
}

/// A single benchmark task (an input).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BenchTask {
    /// Stable task identifier (e.g. `"double"`).
    pub id: String,
    /// Natural-language description of what to generate.
    pub instruction: String,
    /// The colocated `spec`(s) the generated program must satisfy, as tuonelang
    /// source. Appended to the model's generation to score SpecPass@1.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub specs: Vec<String>,
    /// Held-out tests: `spec`s the model is **not** shown, appended to the final
    /// accepted program to score TestPass@1. Kept separate from `specs` so a
    /// model cannot trivially satisfy the tests by copying them from the prompt.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tests: Vec<String>,
    /// Comparable syntax variants of this task. Empty means the task is evaluated
    /// only in its default spelling.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variants: Vec<SyntaxVariant>,
    /// Free-form tags (difficulty, category).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

impl BenchTask {
    /// The specs used to score a given variant: the variant's own specs when it
    /// provides them, else the task's default specs.
    #[must_use]
    pub fn specs_for<'a>(&'a self, variant: Option<&'a SyntaxVariant>) -> &'a [String] {
        match variant {
            Some(v) if !v.specs.is_empty() => &v.specs,
            _ => &self.specs,
        }
    }

    /// A content digest over every field that defines what this task *is*.
    ///
    /// Two tasks with the same instruction, specs, tests, and variants have the
    /// same digest; any change to those fields changes it. This is a
    /// change-detector (a self-contained FNV-1a-based fold), not a cryptographic
    /// commitment — its only job is to make a silent task edit detectable.
    #[must_use]
    pub fn digest(&self) -> String {
        let mut h = Fnv::new();
        h.field("id", &self.id);
        h.field("instruction", &self.instruction);
        for (i, s) in self.specs.iter().enumerate() {
            h.field(&format!("spec{i}"), s);
        }
        for (i, t) in self.tests.iter().enumerate() {
            h.field(&format!("test{i}"), t);
        }
        for (i, v) in self.variants.iter().enumerate() {
            h.field(&format!("variant{i}.label"), &v.label);
            h.field(&format!("variant{i}.note"), &v.note);
            for (j, s) in v.specs.iter().enumerate() {
                h.field(&format!("variant{i}.spec{j}"), s);
            }
        }
        h.hex()
    }
}

/// A version-controlled set of benchmark tasks, each pinned by its content
/// digest so the set cannot drift silently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSet {
    /// The harness schema version ([`crate::SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// Free-form description of the task set.
    pub description: String,
    /// The tasks and their pinned digests.
    pub tasks: Vec<PinnedTask>,
}

/// A task together with the digest that pins its content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PinnedTask {
    /// The recorded content digest of `task`. Must equal `task.digest()`.
    pub digest: String,
    /// The task itself.
    pub task: BenchTask,
}

/// A task's stored digest did not match its content — the task was changed
/// without updating its pin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DigestMismatch {
    /// The task whose pin is stale.
    pub task_id: String,
    /// The digest recorded in the file.
    pub recorded: String,
    /// The digest computed from the task's current content.
    pub actual: String,
}

impl TaskSet {
    /// Build a task set, computing each task's pin from its content. Use this
    /// when authoring a set programmatically; the pins it writes are correct by
    /// construction.
    #[must_use]
    pub fn pinned(description: impl Into<String>, tasks: Vec<BenchTask>) -> Self {
        let tasks = tasks
            .into_iter()
            .map(|task| PinnedTask {
                digest: task.digest(),
                task,
            })
            .collect();
        Self {
            schema_version: crate::SCHEMA_VERSION,
            description: description.into(),
            tasks,
        }
    }

    /// Verify that every task's stored digest matches its content.
    ///
    /// # Errors
    ///
    /// Returns the first [`DigestMismatch`] found. A caller loading a committed
    /// task set should treat this as fatal: it means a task was edited without
    /// re-pinning, i.e. the benchmark changed silently.
    pub fn verify_digests(&self) -> Result<(), DigestMismatch> {
        for pinned in &self.tasks {
            let actual = pinned.task.digest();
            if actual != pinned.digest {
                return Err(DigestMismatch {
                    task_id: pinned.task.id.clone(),
                    recorded: pinned.digest.clone(),
                    actual,
                });
            }
        }
        Ok(())
    }

    /// The tasks, digest-verified.
    ///
    /// # Errors
    ///
    /// Returns a [`DigestMismatch`] if any pin is stale (see
    /// [`verify_digests`](TaskSet::verify_digests)).
    pub fn tasks(&self) -> Result<Vec<&BenchTask>, DigestMismatch> {
        self.verify_digests()?;
        Ok(self.tasks.iter().map(|p| &p.task).collect())
    }

    /// Parse a task set from JSON.
    ///
    /// # Errors
    ///
    /// Returns the underlying [`serde_json::Error`] on malformed input.
    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }

    /// Serialize to pretty JSON for a committed task file.
    ///
    /// # Errors
    ///
    /// Returns the underlying [`serde_json::Error`] if serialization fails.
    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

/// A tiny self-contained FNV-1a fold, used only as a task change-detector.
///
/// It is deliberately not a cryptographic hash: the benchmark's integrity
/// guarantee is "a reviewed task cannot be edited without the pin also changing",
/// which any collision-unlikely fold over the content satisfies. Keeping it
/// in-crate avoids a dependency purely for a digest.
struct Fnv {
    state: u64,
}

impl Fnv {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    fn new() -> Self {
        Self {
            state: Self::OFFSET,
        }
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.state ^= u64::from(b);
            self.state = self.state.wrapping_mul(Self::PRIME);
        }
    }

    /// Fold a named field, with a separator so `("ab","c")` and `("a","bc")`
    /// differ.
    fn field(&mut self, name: &str, value: &str) {
        self.write(name.as_bytes());
        self.write(&[0x1f]);
        self.write(value.as_bytes());
        self.write(&[0x1e]);
    }

    fn hex(&self) -> String {
        format!("{:016x}", self.state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task() -> BenchTask {
        BenchTask {
            id: "double".into(),
            instruction: "Write a function `double` that doubles an Int.".into(),
            specs: vec!["spec double {\n    then double(3) == 6;\n}\n".into()],
            tests: vec!["spec double {\n    then double(10) == 20;\n}\n".into()],
            variants: vec![],
            tags: vec!["arithmetic".into()],
        }
    }

    #[test]
    fn digest_is_stable_and_content_sensitive() {
        let t = task();
        let d = t.digest();
        assert_eq!(d, t.clone().digest(), "digest is deterministic");

        let mut edited = t.clone();
        edited.instruction.push_str(" (edited)");
        assert_ne!(d, edited.digest(), "editing content changes the digest");
    }

    #[test]
    fn pinned_set_verifies_and_detects_a_silent_edit() {
        let set = TaskSet::pinned("t", vec![task()]);
        set.verify_digests().expect("freshly pinned set verifies");

        // Simulate a silent edit: change the task but keep the old pin.
        let mut tampered = set.clone();
        tampered.tasks[0].task.instruction.push_str(" tampered");
        let err = tampered
            .verify_digests()
            .expect_err("stale pin is detected");
        assert_eq!(err.task_id, "double");
    }

    #[test]
    fn variant_specs_fall_back_to_task_specs() {
        let mut t = task();
        t.variants = vec![
            SyntaxVariant {
                label: "default".into(),
                note: String::new(),
                specs: vec![],
            },
            SyntaxVariant {
                label: "explicit".into(),
                note: "restates the spec".into(),
                specs: vec!["spec double {\n    then double(1) == 2;\n}\n".into()],
            },
        ];
        assert_eq!(t.specs_for(Some(&t.variants[0])), t.specs.as_slice());
        assert_eq!(
            t.specs_for(Some(&t.variants[1])),
            t.variants[1].specs.as_slice()
        );
        assert_eq!(t.specs_for(None), t.specs.as_slice());
    }

    #[test]
    fn task_set_round_trips_through_json() {
        let set = TaskSet::pinned("demo", vec![task()]);
        let json = set.to_json_pretty().unwrap();
        let parsed = TaskSet::from_json(&json).unwrap();
        assert_eq!(set, parsed);
        assert_eq!(parsed.tasks().unwrap().len(), 1);
    }
}
