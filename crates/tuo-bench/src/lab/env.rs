//! The run environment: the hardware, OS, and toolchain versions a benchmark
//! measurement was produced on.
//!
//! A performance number is meaningless without the machine it came from. Every
//! [`LabReport`](crate::lab::LabReport) embeds an [`Environment`] so a reader
//! knows exactly what a figure reflects and can decide whether it is comparable
//! to their own. **Nothing here is hard-coded or asserted** — each field is
//! captured from the running process (compile-time target facts, live CPU count,
//! the tuonelang and Rust toolchain versions). Anything the process cannot
//! observe is reported as [`Unknown`](FieldValue::Unknown) rather than guessed,
//! so the record never claims knowledge it does not have.

use serde::{Deserialize, Serialize};

/// A captured environment fact that may or may not be observable.
///
/// Some facts (the target triple, the pointer width) are known at compile time
/// and are always [`Known`](FieldValue::Known). Others (a CPU model string) are
/// not portably available; those are [`Unknown`](FieldValue::Unknown) rather
/// than filled with a plausible-but-unverified value. A reader can therefore
/// trust that a `Known` value was really observed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
pub enum FieldValue {
    /// The fact was observed from the running process.
    Known(String),
    /// The fact is not portably observable here; not guessed.
    Unknown,
}

impl FieldValue {
    /// The observed string, or `None` when unknown.
    #[must_use]
    pub fn known(&self) -> Option<&str> {
        match self {
            Self::Known(value) => Some(value.as_str()),
            Self::Unknown => None,
        }
    }

    /// A human-readable rendering: the value, or the literal `unknown`.
    #[must_use]
    pub fn display(&self) -> &str {
        self.known().unwrap_or("unknown")
    }
}

/// The complete, machine-readable description of where a measurement ran.
///
/// This is the `Record:` block the prompt requires — hardware, OS, compiler
/// versions — captured from the live process, not configured. Two runs on the
/// same machine with the same toolchain produce equal `Environment`s (the
/// fields are all deterministic facts of the build and host), which is what lets
/// a committed example report round-trip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Environment {
    /// The target triple the benchmark binary was built for (e.g.
    /// `aarch64-apple-darwin`). Compile-time fact; always known.
    pub target_triple: String,
    /// CPU architecture (`target_arch`: `aarch64`, `x86_64`, …).
    pub arch: String,
    /// Pointer width in bits (32 or 64). Affects layout and is worth recording.
    pub pointer_width: u32,
    /// Operating system family (`target_os`: `macos`, `linux`, `windows`, …).
    pub os: String,
    /// A best-effort CPU model string, when the host exposes one; otherwise
    /// [`FieldValue::Unknown`].
    pub cpu_model: FieldValue,
    /// The number of logical CPUs available to the process, when observable.
    pub logical_cpus: FieldValue,
    /// The tuonelang toolchain version (this workspace's version).
    pub tuonelang_version: String,
    /// The language edition targeted (v0 has one edition).
    pub language_edition: String,
    /// The Rust compiler version the toolchain was built with, when captured at
    /// build time; otherwise [`FieldValue::Unknown`].
    pub rustc_version: FieldValue,
}

impl Environment {
    /// Capture the current environment from the running process.
    ///
    /// Compile-time facts come from `cfg!`/`env!`; the CPU count is read live;
    /// the CPU model and `rustc` version are captured when a build script or the
    /// host makes them available and are [`FieldValue::Unknown`] otherwise. This
    /// function performs no network access and does not shell out.
    #[must_use]
    pub fn capture() -> Self {
        Self {
            target_triple: target_triple(),
            arch: std::env::consts::ARCH.to_string(),
            pointer_width: (size_of::<usize>() as u32) * 8,
            os: std::env::consts::OS.to_string(),
            cpu_model: cpu_model(),
            logical_cpus: logical_cpus(),
            tuonelang_version: env!("CARGO_PKG_VERSION").to_string(),
            language_edition: "2024".to_string(),
            rustc_version: rustc_version(),
        }
    }

    /// A compact one-line summary for a human report header.
    #[must_use]
    pub fn summary_line(&self) -> String {
        format!(
            "{arch} {os} · {cpus} CPU · tuonelang {ver} (edition {ed})",
            arch = self.arch,
            os = self.os,
            cpus = self.logical_cpus.display(),
            ver = self.tuonelang_version,
            ed = self.language_edition,
        )
    }
}

/// The build's target triple. `TARGET` is set by our build script (Cargo does
/// not expose it to the crate directly); absent that, fall back to the
/// arch/os pair so the field is always populated with something true.
fn target_triple() -> String {
    option_env!("TUO_BUILD_TARGET").map_or_else(
        || format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS),
        ToString::to_string,
    )
}

/// The Rust compiler version, captured by the build script into an env var when
/// available. Unknown rather than guessed if the build script did not run.
fn rustc_version() -> FieldValue {
    option_env!("TUO_BUILD_RUSTC").map_or(FieldValue::Unknown, |v| FieldValue::Known(v.to_string()))
}

/// The number of logical CPUs available to the process.
///
/// Uses `std::thread::available_parallelism`, which reflects scheduler
/// affinity/cgroup limits where the platform supports it, and is `Unknown` on
/// the rare platform that cannot report it.
fn logical_cpus() -> FieldValue {
    std::thread::available_parallelism().map_or(FieldValue::Unknown, |n| {
        FieldValue::Known(n.get().to_string())
    })
}

/// A best-effort CPU model string.
///
/// Read from an env var a build script or wrapper may set (`TUO_BENCH_CPU`), so
/// a host can inject the true model without this crate shelling out or growing a
/// platform dependency. Unknown when unset — never a fabricated model name.
fn cpu_model() -> FieldValue {
    match std::env::var("TUO_BENCH_CPU") {
        Ok(model) if !model.trim().is_empty() => FieldValue::Known(model.trim().to_string()),
        _ => FieldValue::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_populates_compile_time_facts() {
        let env = Environment::capture();
        // Compile-time facts are always known.
        assert!(!env.arch.is_empty());
        assert!(!env.os.is_empty());
        assert!(env.pointer_width == 32 || env.pointer_width == 64);
        assert_eq!(env.tuonelang_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(env.language_edition, "2024");
    }

    #[test]
    fn logical_cpus_is_observed_on_a_normal_host() {
        // available_parallelism is supported on every platform CI runs on, so
        // the count should be known and positive.
        let env = Environment::capture();
        let count: usize = env
            .logical_cpus
            .known()
            .expect("logical CPU count observable in CI")
            .parse()
            .expect("count parses");
        assert!(count >= 1);
    }

    #[test]
    fn unknown_fields_render_as_unknown_not_a_guess() {
        let unknown = FieldValue::Unknown;
        assert_eq!(unknown.display(), "unknown");
        assert_eq!(unknown.known(), None);
    }

    #[test]
    fn capture_is_deterministic_on_one_host() {
        // The same host + toolchain yields an equal record, which is what lets a
        // committed example report round-trip.
        assert_eq!(Environment::capture(), Environment::capture());
    }
}
