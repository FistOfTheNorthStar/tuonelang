//! Places: the locations the ownership checker tracks.
//!
//! A place is a local binding or parameter (its **root** symbol) plus a
//! finite field path (`specification/ownership.md` §1). v0 has no
//! dereference operator, so paths never go through a wrapper, and index
//! expressions are not places.

use std::fmt;

use tuo_resolve::SymbolId;

/// One tracked place: a root binding/parameter and a field path rooted in
/// it (`p`, `p.left`, `msg.payload.buf`).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Place {
    /// The root binding or parameter.
    pub root: SymbolId,
    /// The field path from the root (field names or tuple indices), outermost
    /// first. Empty for the root itself.
    pub path: Vec<String>,
}

impl Place {
    /// The whole place of a root binding or parameter.
    #[must_use]
    pub fn root(root: SymbolId) -> Self {
        Self {
            root,
            path: Vec::new(),
        }
    }

    /// The place one field further down.
    #[must_use]
    pub fn child(&self, field: &str) -> Self {
        let mut path = self.path.clone();
        path.push(field.to_owned());
        Self {
            root: self.root,
            path,
        }
    }

    /// Is `self` a (non-strict) prefix of `other`?
    #[must_use]
    pub fn is_prefix_of(&self, other: &Self) -> bool {
        self.root == other.root
            && self.path.len() <= other.path.len()
            && other.path[..self.path.len()] == self.path[..]
    }

    /// Do the two places overlap — is one a prefix of the other
    /// (`specification/ownership.md` §1)?
    #[must_use]
    pub fn overlaps(&self, other: &Self) -> bool {
        self.is_prefix_of(other) || other.is_prefix_of(self)
    }
}

/// Renders a place given its root's name (`p`, `p.left`).
pub(crate) struct DisplayPlace<'a> {
    pub root_name: &'a str,
    pub place: &'a Place,
}

impl fmt::Display for DisplayPlace<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.root_name)?;
        for field in &self.place.path {
            write!(f, ".{field}")?;
        }
        Ok(())
    }
}
