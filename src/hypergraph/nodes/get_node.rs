use crate::hypergraph::{Hypergraph, HyperNode, NodeId, Result, HypergraphError};

impl Hypergraph {
    /// Gets a node by its ID
    ///
    /// # Arguments
    /// * `node_id` - The ID of the node to retrieve
    ///
    /// # Returns
    /// * `Ok(&HyperNode)` - Reference to the node
    /// * `Err` - If the node ID doesn't exist
    pub fn get_node(&self, node_id: NodeId) -> Result<&HyperNode> {
        let internal = self.get_node_internal(node_id)?;

        self.nodes
            .get_index(internal)
            .map(|(node, _)| node)
            .ok_or(HypergraphError::InternalNodeNotFound(internal))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hypergraph::nodes::tests::create_test_node;

    #[test]
    fn test_get_node() {
        let mut hg = Hypergraph::new();
        let node = create_test_node("test");
        let node_id = hg.add_node(node.clone()).unwrap();

        let retrieved = hg.get_node(node_id).unwrap();
        assert_eq!(retrieved.name, "test");
    }

    #[test]
    fn test_get_nonexistent_node() {
        let hg = Hypergraph::new();
        let result = hg.get_node(NodeId(999));
        assert!(matches!(result, Err(HypergraphError::NodeNotFound(_))));
    }
}
