//! Hypergraph data structure for code analysis
//!
//! A directed many-to-many hypergraph where:
//! - Nodes wrap parsed code symbols
//! - Hyperedges represent relationships (contains, calls, implements, etc.)
//! - Sources → Targets captures directionality

pub mod bi_hash_map;
pub mod builder;
pub mod errors;
pub mod indexes;
pub mod stats;
pub mod types;

pub mod hyperedges;
pub mod nodes;

// Phase 3: Visualization
pub mod layout;
pub mod viz;
mod viz_ui;

// Re-exports
pub use builder::{HypergraphBuilder, HypergraphConfig};
pub use errors::{HypergraphError, Result};
pub use indexes::{HyperedgeId, NodeId};
pub use stats::Stats;
pub use types::{HyperNode, Hyperedge, HyperedgeType, NodeType};

// Re-export visualization function
pub use viz::visualize;

use ahash::RandomState;
use bi_hash_map::BiHashMap;
use indexmap::{IndexMap, IndexSet};
use std::collections::HashMap;
use types::HyperedgeKey;

/// Type alias for IndexMap with ahash
type AIndexMap<K, V> = IndexMap<K, V, RandomState>;

/// Type alias for IndexSet with ahash
type AIndexSet<T> = IndexSet<T, RandomState>;

/// Main hypergraph data structure
pub struct Hypergraph {
    /// Nodes stored with reverse index to hyperedges
    /// Key: HyperNode, Value: Set of internal hyperedge indexes containing this node
    nodes: AIndexMap<HyperNode, AIndexSet<usize>>,

    /// Hyperedges stored as unique keys
    hyperedges: AIndexSet<HyperedgeKey>,

    /// Stable node ID mapping: internal index ↔ public NodeId
    nodes_mapping: BiHashMap<NodeId>,

    /// Stable hyperedge ID mapping: internal index ↔ public HyperedgeId
    hyperedges_mapping: BiHashMap<HyperedgeId>,

    /// Fast name lookup: node name → NodeId
    name_to_node: HashMap<String, NodeId>,

    /// Counter for generating stable node IDs
    nodes_count: usize,

    /// Counter for generating stable hyperedge IDs
    hyperedges_count: usize,
}

impl Hypergraph {
    /// Creates a new empty hypergraph
    pub fn new() -> Self {
        Self::with_capacity(0, 0)
    }

    /// Creates a new hypergraph with pre-allocated capacity
    pub fn with_capacity(nodes: usize, hyperedges: usize) -> Self {
        Self {
            nodes: AIndexMap::with_capacity_and_hasher(nodes, RandomState::new()),
            hyperedges: AIndexSet::with_capacity_and_hasher(hyperedges, RandomState::new()),
            nodes_mapping: BiHashMap::new(),
            hyperedges_mapping: BiHashMap::new(),
            name_to_node: HashMap::with_capacity(nodes),
            nodes_count: 0,
            hyperedges_count: 0,
        }
    }

    /// Clears all nodes and hyperedges while keeping capacity
    pub fn clear(&mut self) {
        self.nodes.clear();
        self.hyperedges.clear();
        self.nodes_mapping.clear();
        self.hyperedges_mapping.clear();
        self.name_to_node.clear();
        self.nodes_count = 0;
        self.hyperedges_count = 0;
    }

    /// Helper: Get internal index from public NodeId
    pub(crate) fn get_node_internal(&self, node_id: NodeId) -> Result<usize> {
        self.nodes_mapping
            .get_internal(node_id)
            .ok_or(HypergraphError::NodeNotFound(node_id))
    }

    /// Helper: Get internal index from public HyperedgeId
    pub(crate) fn get_hyperedge_internal(&self, edge_id: HyperedgeId) -> Result<usize> {
        self.hyperedges_mapping
            .get_internal(edge_id)
            .ok_or(HypergraphError::HyperedgeNotFound(edge_id))
    }

    /// Helper: Get multiple internal indexes from NodeIds
    pub(crate) fn get_nodes_internal(
        &self,
        node_ids: &std::collections::HashSet<NodeId>,
    ) -> Result<Vec<usize>> {
        node_ids
            .iter()
            .map(|&id| self.get_node_internal(id))
            .collect()
    }
}

impl Default for Hypergraph {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Hypergraph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hypergraph")
            .field("nodes", &self.nodes_count)
            .field("hyperedges", &self.hyperedges_count)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_hypergraph() {
        let hg = Hypergraph::new();
        assert_eq!(hg.nodes_count, 0);
        assert_eq!(hg.hyperedges_count, 0);
    }

    #[test]
    fn test_with_capacity() {
        let hg = Hypergraph::with_capacity(100, 200);
        assert!(hg.nodes.capacity() >= 100);
        assert!(hg.hyperedges.capacity() >= 200);
    }
}
