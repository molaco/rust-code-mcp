//! Bi-directional mapping for stable indexes
//!
//! IndexMap/IndexSet use internal positions that shift when items are removed.
//! This maintains stable public IDs by mapping between internal and public indexes.

use std::collections::HashMap;
use std::fmt::Debug;
use std::hash::Hash;

/// Bi-directional hashmap: internal_index ↔ public_index
pub(crate) struct BiHashMap<Index>
where
    Index: Copy + Debug + Eq + Hash,
{
    /// Internal position → Public stable ID
    pub(crate) internal_to_public: HashMap<usize, Index>,

    /// Public stable ID → Internal position
    pub(crate) public_to_internal: HashMap<Index, usize>,
}

impl<Index> BiHashMap<Index>
where
    Index: Copy + Debug + Eq + Hash,
{
    /// Creates a new empty BiHashMap
    pub(crate) fn new() -> Self {
        Self {
            internal_to_public: HashMap::new(),
            public_to_internal: HashMap::new(),
        }
    }

    /// Insert a mapping: internal ↔ public
    pub(crate) fn insert(&mut self, internal_index: usize, public_index: Index) {
        self.internal_to_public.insert(internal_index, public_index);
        self.public_to_internal.insert(public_index, internal_index);
    }

    /// Get public ID from internal position
    pub(crate) fn get_public(&self, internal_index: usize) -> Option<Index> {
        self.internal_to_public.get(&internal_index).copied()
    }

    /// Get internal position from public ID
    pub(crate) fn get_internal(&self, public_index: Index) -> Option<usize> {
        self.public_to_internal.get(&public_index).copied()
    }

    /// Remove a mapping
    pub(crate) fn remove(&mut self, public_index: Index) {
        if let Some(internal) = self.public_to_internal.remove(&public_index) {
            self.internal_to_public.remove(&internal);
        }
    }

    /// Clear all mappings
    pub(crate) fn clear(&mut self) {
        self.internal_to_public.clear();
        self.public_to_internal.clear();
    }
}

impl<Index> Default for BiHashMap<Index>
where
    Index: Copy + Debug + Eq + Hash,
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hypergraph::indexes::NodeId;

    #[test]
    fn test_bidirectional_mapping() {
        let mut map = BiHashMap::new();

        map.insert(0, NodeId(100));
        map.insert(1, NodeId(101));

        assert_eq!(map.get_public(0), Some(NodeId(100)));
        assert_eq!(map.get_internal(NodeId(100)), Some(0));
    }

    #[test]
    fn test_remove() {
        let mut map = BiHashMap::new();
        map.insert(5, NodeId(500));

        map.remove(NodeId(500));

        assert_eq!(map.get_public(5), None);
        assert_eq!(map.get_internal(NodeId(500)), None);
    }
}
