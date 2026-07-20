//! Stable semantic identifiers.
//!
//! Resolution never hands out AST pointers: every declared entity gets a
//! [`SymbolId`] and every module a [`ModuleId`], both plain arena indices
//! into the [`crate::Resolution`] they came from. Within one resolved
//! program snapshot the assignment is **deterministic** — file order, then
//! source order — so equal inputs always produce equal IDs.

use std::fmt;

/// The identity of one declared entity (function, struct, local, …) in a
/// resolved program.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct SymbolId(u32);

impl SymbolId {
    /// Construct from a raw arena index.
    #[must_use]
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    /// The raw arena index.
    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self.0
    }
}

impl fmt::Display for SymbolId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sym{}", self.0)
    }
}

/// The identity of one module in a resolved program.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ModuleId(u32);

impl ModuleId {
    /// Construct from a raw arena index.
    #[must_use]
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    /// The raw arena index.
    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self.0
    }
}

impl fmt::Display for ModuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "mod{}", self.0)
    }
}
