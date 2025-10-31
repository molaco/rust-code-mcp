use crate::hypergraph::{Hypergraph, NodeId, Result, HypergraphError};

impl Hypergraph {
    /// Removes a node from the hypergraph
    ///
    /// # Arguments
    /// * `node_id` - The ID of the node to remove
    ///
    /// # Returns
    /// * `Ok(())` - If removal succeeded
    /// * `Err` - If node doesn't exist or is still in use by hyperedges
    ///
    /// # Note
    /// This function will fail if the node is part of any hyperedges.
    /// Remove all hyperedges containing this node first.
    pub fn remove_node(&mut self, node_id: NodeId) -> Result<()> {
        let internal = self.get_node_internal(node_id)?;

        // Check if node is in use by any hyperedges
        if let Some((_, edge_set)) = self.nodes.get_index(internal) {
            if !edge_set.is_empty() {
                return Err(HypergraphError::NodeInUse(node_id, edge_set.len()));
            }
        }

        // Get node name for cleanup
        let node_name = self.nodes
            .get_index(internal)
            .map(|(node, _)| node.name.clone())
            .ok_or(HypergraphError::InternalNodeNotFound(internal))?;

        // Remove from IndexMap
        self.nodes.shift_remove_index(internal);

        // Remove from mappings
        self.nodes_mapping.remove(node_id);
        self.name_to_node.remove(&node_name);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hypergraph::{Hypergraph, HyperedgeType};
    use crate::hypergraph::nodes::tests::create_test_node;

    #[test]
    fn test_remove_unused_node() {
        let mut hg = Hypergraph::new();
        let n1 = hg.add_node(create_test_node("fn1")).unwrap();

        hg.remove_node(n1).unwrap();
        assert_eq!(hg.count_nodes(), 0);
    }

    #[test]
    fn test_remove_node_in_use() {
        let mut hg = Hypergraph::new();

        let n1 = hg.add_node(create_test_node("fn1")).unwrap();
        let n2 = hg.add_node(create_test_node("fn2")).unwrap();

        hg.add_hyperedge([n1].into(), [n2].into(), HyperedgeType::CallPattern, 1.0)
            .unwrap();

        let result = hg.remove_node(n1);
        assert!(matches!(result, Err(HypergraphError::NodeInUse(_, 1))));
    }
}
