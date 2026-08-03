//! The pluggable model-adapter seam.
//!
//! The harness benchmarks a *model* through a [`ModelAdapter`] — a trait a host
//! implements to connect a real code-generation model (an LLM behind an API, a
//! local runner, a deterministic generator). **No LLM provider is embedded in
//! this crate.** The harness owns the evaluation loop; the adapter owns turning a
//! prompt (and, on repair turns, the compiler's diagnostics) into tuonelang
//! source. This is the same shape as tuo-corpus's `NativeExecutor` and the
//! agent's `Formatter` seam: the capability the crate cannot perform is injected.
//!
//! An adapter never sees the compiler. It receives a [`Prompt`] and returns a
//! [`Generation`] — the source it produced plus the token count it reports. The
//! harness compiles that source itself, so a metric is only ever claimed when the
//! *real* compiler produced it.

use serde::{Deserialize, Serialize};

/// Metadata describing the model under test, recorded verbatim with every run so
/// a result is reproducible and attributable.
///
/// These are *data* the harness keeps alongside the results; the harness does not
/// interpret them. Only [`id`](ModelConfig::id) is load-bearing (it labels the
/// run); everything else is provenance a reviewer reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ModelConfig {
    /// A stable identifier for the model + configuration under test
    /// (e.g. `"claude-opus-4-8@t0"`). Labels the [`BenchmarkRun`](crate::BenchmarkRun).
    pub id: String,
    /// The provider/vendor, if meaningful (free-form, e.g. `"anthropic"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// The sampling temperature, recorded as a string so any value (or "n/a")
    /// round-trips without imposing a numeric model on every adapter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<String>,
    /// Arbitrary additional configuration keys (top-p, max tokens, a system
    /// prompt hash, …). Kept as ordered key/value pairs so the record is stable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra: Vec<ConfigEntry>,
}

impl ModelConfig {
    /// A config that only carries an id (the common case for a deterministic or
    /// local adapter).
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            ..Self::default()
        }
    }
}

/// One free-form model-configuration key/value pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigEntry {
    /// The configuration key.
    pub key: String,
    /// The configuration value, as a string.
    pub value: String,
}

/// What the harness asks the adapter to produce.
///
/// The `feedback` is empty on the first attempt and carries the compiler's
/// rendered diagnostics on each subsequent repair turn, so an adapter can
/// implement a repair policy without the harness prescribing one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt<'a> {
    /// The task's natural-language instruction.
    pub instruction: &'a str,
    /// The syntax variant being evaluated (its label), so an adapter that
    /// generates for a specific spelling can honor it. `None` for the task's
    /// default spelling.
    pub variant: Option<&'a str>,
    /// The prior generation being repaired, if this is a repair turn.
    pub previous_source: Option<&'a str>,
    /// The compiler's diagnostics on the previous generation, rendered to text,
    /// one per entry. Empty on the first attempt.
    pub feedback: &'a [String],
}

impl Prompt<'_> {
    /// Whether this is a repair turn (there is prior source and feedback).
    #[must_use]
    pub fn is_repair(&self) -> bool {
        self.previous_source.is_some()
    }
}

/// What an adapter returns for one turn: the source it generated and the number
/// of tokens it reports having emitted.
///
/// The token count is the model's own accounting (the harness cannot observe an
/// external model's tokenizer), recorded as data. When an adapter does not know
/// its token count it may report `0`; the harness never fabricates one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generation {
    /// The tuonelang source the model produced.
    pub source: String,
    /// The number of tokens the model reports it generated for this turn.
    pub generated_tokens: u64,
}

impl Generation {
    /// A generation that reports its own token count.
    #[must_use]
    pub fn new(source: impl Into<String>, generated_tokens: u64) -> Self {
        Self {
            source: source.into(),
            generated_tokens,
        }
    }
}

/// A model the harness can benchmark.
///
/// Implementations connect a real generator; this crate embeds none. An adapter
/// must be deterministic for the harness's results to be reproducible where the
/// model is — a non-deterministic model produces non-reproducible results, which
/// is a property of the model, not a defect of the harness. Implementations must
/// not panic: any failure to generate is a [`GenerationError`].
pub trait ModelAdapter {
    /// The model's configuration metadata, recorded with the run.
    fn config(&self) -> ModelConfig;

    /// Generate tuonelang source for one turn (initial or repair).
    ///
    /// # Errors
    ///
    /// Returns a [`GenerationError`] if the model could not produce output for
    /// this prompt (a transport failure, a refusal, a timeout). The harness
    /// records the error and moves on rather than aborting the whole run.
    fn generate(&self, prompt: &Prompt<'_>) -> Result<Generation, GenerationError>;
}

/// A model failed to generate for a prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationError {
    /// A short, human-readable reason.
    pub reason: String,
}

impl GenerationError {
    /// Build an error from a reason string.
    #[must_use]
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}
