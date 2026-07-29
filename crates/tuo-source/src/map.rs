//! The source map: interned files and their snapshot history.

use std::collections::HashMap;
use std::sync::Arc;

use crate::{FileId, Revision, SourceId, SourceText, SourceTooLarge};

/// Owner of all [`FileId`]s and [`SourceId`]s in a compilation session.
///
/// Files are interned by name; each file then accumulates an append-only
/// history of immutable text snapshots, one per [`Revision`]. Old snapshots
/// stay valid and addressable — a span anchored to an old [`SourceId`] can
/// still be resolved after the file changes, which is the property later
/// incremental compilation builds on.
#[derive(Debug, Default)]
pub struct SourceMap {
    file_names: Vec<String>,
    file_ids: HashMap<String, FileId>,
    sources: Vec<Arc<SourceText>>,
    /// Latest snapshot per file, indexed by `FileId`.
    latest: Vec<Option<SourceId>>,
}

impl SourceMap {
    /// An empty source map.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Intern a file name, returning the same [`FileId`] for the same name.
    ///
    /// The name is an opaque presentation string to this crate (path
    /// normalization is the caller's concern).
    pub fn intern_file(&mut self, name: &str) -> FileId {
        if let Some(&id) = self.file_ids.get(name) {
            return id;
        }
        let id = FileId::from_raw(
            u32::try_from(self.file_names.len()).expect("file count overflowed u32"),
        );
        self.file_names.push(name.to_owned());
        self.file_ids.insert(name.to_owned(), id);
        self.latest.push(None);
        id
    }

    /// The interned name of `file`.
    ///
    /// # Panics
    ///
    /// Panics if `file` was not produced by this map.
    #[must_use]
    pub fn file_name(&self, file: FileId) -> &str {
        &self.file_names[file.as_raw() as usize]
    }

    /// The [`FileId`] a file `name` was interned under, if this map has seen it.
    /// The read-only counterpart of [`SourceMap::intern_file`] — it never
    /// creates a file, so a consumer holding a map by shared reference (a
    /// tooling query, a language server) can resolve a document name to its id
    /// without mutating the map.
    #[must_use]
    pub fn file_id(&self, name: &str) -> Option<FileId> {
        self.file_ids.get(name).copied()
    }

    /// Add a new immutable text snapshot for `file` at the file's next
    /// revision, returning its identity.
    ///
    /// # Errors
    ///
    /// Returns [`SourceTooLarge`] if the text exceeds the 32-bit offset space.
    ///
    /// # Panics
    ///
    /// Panics if `file` was not produced by this map.
    pub fn add_source(
        &mut self,
        file: FileId,
        text: impl Into<Arc<str>>,
    ) -> Result<SourceId, SourceTooLarge> {
        let revision = match self.latest[file.as_raw() as usize] {
            Some(prev) => self.source(prev).revision().next(),
            None => Revision::INITIAL,
        };
        let id = SourceId::from_raw(
            u32::try_from(self.sources.len()).expect("source count overflowed u32"),
        );
        let snapshot = SourceText::new(id, file, revision, text.into())?;
        self.sources.push(Arc::new(snapshot));
        self.latest[file.as_raw() as usize] = Some(id);
        Ok(id)
    }

    /// The snapshot identified by `id`.
    ///
    /// # Panics
    ///
    /// Panics if `id` was not produced by this map (use
    /// [`SourceMap::get_source`] to probe).
    #[must_use]
    pub fn source(&self, id: SourceId) -> &Arc<SourceText> {
        &self.sources[id.as_raw() as usize]
    }

    /// The snapshot identified by `id`, or `None` if this map never issued it.
    #[must_use]
    pub fn get_source(&self, id: SourceId) -> Option<&Arc<SourceText>> {
        self.sources.get(id.as_raw() as usize)
    }

    /// The most recent snapshot of `file`, or `None` if no text was added yet.
    ///
    /// # Panics
    ///
    /// Panics if `file` was not produced by this map.
    #[must_use]
    pub fn latest(&self, file: FileId) -> Option<SourceId> {
        self.latest[file.as_raw() as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interning_is_idempotent() {
        let mut map = SourceMap::new();
        let a = map.intern_file("main.tuo");
        let b = map.intern_file("main.tuo");
        let c = map.intern_file("lib.tuo");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(map.file_name(a), "main.tuo");
    }

    #[test]
    fn file_id_looks_up_without_creating() {
        let mut map = SourceMap::new();
        let a = map.intern_file("main.tuo");
        assert_eq!(map.file_id("main.tuo"), Some(a));
        // An unseen name is not created — the read-only lookup returns None.
        assert_eq!(map.file_id("absent.tuo"), None);
    }

    #[test]
    fn revisions_advance_and_old_snapshots_survive() {
        let mut map = SourceMap::new();
        let file = map.intern_file("main.tuo");
        assert_eq!(map.latest(file), None);

        let v0 = map.add_source(file, "fn a() {}").expect("fits");
        let v1 = map.add_source(file, "fn b() {}").expect("fits");
        assert_ne!(v0, v1);
        assert_eq!(map.latest(file), Some(v1));

        // The old snapshot is still addressable and unchanged.
        assert_eq!(map.source(v0).text(), "fn a() {}");
        assert_eq!(map.source(v0).revision(), Revision::INITIAL);
        assert_eq!(map.source(v1).revision(), Revision::INITIAL.next());
        assert_eq!(map.source(v1).file(), file);
    }

    #[test]
    fn get_source_probes_without_panicking() {
        let map = SourceMap::new();
        assert!(map.get_source(SourceId::from_raw(0)).is_none());
    }
}
