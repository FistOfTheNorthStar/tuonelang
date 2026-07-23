//! The interpreter sandbox: resource limits and trace configuration.
//!
//! The interpreter is a **sandbox** by construction. It has no host access —
//! no filesystem, no network, no process spawning, no clock, no randomness —
//! and it never will: MIR has no instructions for any of those, so there is
//! nothing to gate. What *is* configurable is how much computation a run may
//! consume before it is aborted deterministically, so an untrusted or
//! non-terminating program cannot hang or exhaust the host. Every limit is a
//! hard ceiling that produces a structured [`crate::TrapKind`] abort, never a
//! panic.

/// Resource ceilings for one interpreter run. Each limit, when exceeded,
/// aborts the program with the corresponding [`crate::TrapKind`] rather than
/// running unbounded.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Limits {
    /// Maximum number of instructions executed (statements and terminators).
    /// Guarantees termination of any run. Exceeding it traps with
    /// [`crate::TrapKind::OutOfFuel`].
    pub fuel: u64,
    /// Maximum call-stack depth (number of nested calls, including the entry
    /// function). Exceeding it traps with [`crate::TrapKind::RecursionLimit`].
    pub max_call_depth: u32,
    /// Maximum number of live values across all stack frames — a
    /// backend-independent proxy for a memory budget (there is no host heap
    /// to measure). Exceeding it traps with [`crate::TrapKind::MemoryBudget`].
    pub max_live_values: u64,
}

impl Default for Limits {
    /// Generous defaults suitable for conformance tests and small programs.
    fn default() -> Self {
        Self {
            fuel: 10_000_000,
            max_call_depth: 1_024,
            max_live_values: 10_000_000,
        }
    }
}

impl Limits {
    /// Limits with the given instruction fuel and otherwise-default ceilings.
    #[must_use]
    pub fn with_fuel(fuel: u64) -> Self {
        Self {
            fuel,
            ..Self::default()
        }
    }

    /// Set the maximum call depth (builder style).
    #[must_use]
    pub fn max_depth(mut self, depth: u32) -> Self {
        self.max_call_depth = depth;
        self
    }

    /// Set the maximum number of live values (builder style).
    #[must_use]
    pub fn max_values(mut self, values: u64) -> Self {
        self.max_live_values = values;
        self
    }
}
