use crate::hypergraph::{
    Hypergraph, HyperedgeId, HyperedgeType, NodeId, Result, HypergraphError,
    types::HyperedgeKey,
};
use std::collections::HashSet;

impl Hypergraph {
    /// Adds a directed many-to-many hyperedge to the hypergraph
    ///
    /// # Arguments
    /// * `sources` - Source nodes (where edge originates)
    /// * `targets` - Target nodes (where edge points to)
    /// * `edge_type` - Type of relationship
    /// * `weight` - Edge weight (default 1.0)
    ///
    /// # Returns
    /// * `Ok(HyperedgeId)` - Stable ID for the added hyperedge
    /// * `Err` - If nodes don't exist or hyperedge is empty
    ///
    /// # Example
    /// ```ignore
    /// // main_fn calls helper1 and helper2
    /// let edge_id = hg.add_hyperedge(
    ///     [main_fn].into(),
    ///     [helper1, helper2].into(),
    ///     HyperedgeType::CallPattern,
    ///     1.0,
    /// )?;
    /// ```
    pub fn add_hyperedge(
        &mut self,
        sources: HashSet<NodeId>,
        targets: HashSet<NodeId>,
        edge_type: HyperedgeType,
        weight: f32,
    ) -> Result<HyperedgeId> {
        // Validate: at least one source or target
        if sources.is_empty() && targets.is_empty() {
            return Err(HypergraphError::EmptyHyperedge);
        }

        // Convert public NodeIds to internal indexes
        let source_internals = self.get_nodes_internal(&sources)?;
        let target_internals = self.get_nodes_internal(&targets)?;

        // Create hyperedge key (sorted for uniqueness)
        let key = HyperedgeKey::new(
            source_internals.clone(),
            target_internals.clone(),
            edge_type,
            weight,
        );

        // Check for duplicate
        if self.hyperedges.contains(&key) {
            return Err(HypergraphError::HyperedgeAlreadyExists);
        }

        // Insert into IndexSet
        let (internal_index, _) = self.hyperedges.insert_full(key);

        // Update reverse index: each node → hyperedges containing it
        for &internal in source_internals.iter().chain(target_internals.iter()) {
            if let Some((_, edge_set)) = self.nodes.get_index_mut(internal) {
                edge_set.insert(internal_index);
            } else {
                return Err(HypergraphError::InternalNodeNotFound(internal));
            }
        }

        // Generate stable ID
        let edge_id = HyperedgeId(self.hyperedges_count);
        self.hyperedges_count += 1;

        // Update mapping
        self.hyperedges_mapping.insert(internal_index, edge_id);

        Ok(edge_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hypergraph::nodes::tests::create_test_node;
    use crate::hypergraph::Hypergraph;

    #[test]
    fn test_add_hyperedge() {
        let mut hg = Hypergraph::new();

        let n1 = hg.add_node(create_test_node("fn1")).unwrap();
        let n2 = hg.add_node(create_test_node("fn2")).unwrap();
        let n3 = hg.add_node(create_test_node("fn3")).unwrap();

        let edge_id = hg
            .add_hyperedge(
                [n1].into(),
                [n2, n3].into(),
                HyperedgeType::CallPattern,
                1.0,
            )
            .unwrap();

        assert_eq!(edge_id, HyperedgeId(0));
    }

    #[test]
    fn test_empty_hyperedge() {
        let mut hg = Hypergraph::new();

        let result = hg.add_hyperedge(
            HashSet::new(),
            HashSet::new(),
            HyperedgeType::CallPattern,
            1.0,
        );

        assert!(matches!(result, Err(HypergraphError::EmptyHyperedge)));
    }

    #[test]
    fn test_nonexistent_node() {
        let mut hg = Hypergraph::new();

        let result = hg.add_hyperedge(
            [NodeId(999)].into(),
            HashSet::new(),
            HyperedgeType::CallPattern,
            1.0,
        );

        assert!(matches!(result, Err(HypergraphError::NodeNotFound(_))));
    }

    #[test]
    fn test_many_to_many() {
        let mut hg = Hypergraph::new();

        let n1 = hg.add_node(create_test_node("fn1")).unwrap();
        let n2 = hg.add_node(create_test_node("fn2")).unwrap();
        let n3 = hg.add_node(create_test_node("fn3")).unwrap();
        let n4 = hg.add_node(create_test_node("fn4")).unwrap();

        // Functions fn1 and fn2 both call fn3 and fn4
        let edge_id = hg
            .add_hyperedge(
                [n1, n2].into(),
                [n3, n4].into(),
                HyperedgeType::CallPattern,
                1.0,
            )
            .unwrap();

        assert_eq!(edge_id, HyperedgeId(0));
    }
}
