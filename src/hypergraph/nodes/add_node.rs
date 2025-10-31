use crate::hypergraph::{
    Hypergraph, HyperNode, NodeId, Result, HypergraphError,
};
use ahash::RandomState;
use indexmap::IndexSet;

impl Hypergraph {
    /// Adds a node to the hypergraph
    ///
    /// # Arguments
    /// * `node` - The node to add (wraps a Symbol)
    ///
    /// # Returns
    /// * `Ok(NodeId)` - Stable ID for the added node
    /// * `Err` - If a node with the same name already exists
    ///
    /// # Example
    /// ```ignore
    /// let node = HyperNode {
    ///     id: NodeId(0),  // Will be reassigned
    ///     name: "parse_file".into(),
    ///     file_path: PathBuf::from("src/parser.rs"),
    ///     line_start: 42,
    ///     line_end: 100,
    ///     symbol: my_symbol,
    /// };
    /// let node_id = hg.add_node(node)?;
    /// ```
    pub fn add_node(&mut self, mut node: HyperNode) -> Result<NodeId> {
        // Check for duplicate name
        if self.name_to_node.contains_key(&node.name) {
            return Err(HypergraphError::NodeNameExists(node.name));
        }

        // Generate stable ID
        let node_id = NodeId(self.nodes_count);
        self.nodes_count += 1;

        // Update node with stable ID
        node.id = node_id;

        // Insert into IndexMap
        let (internal_index, _) = self.nodes.insert_full(
            node.clone(),
            IndexSet::with_hasher(RandomState::new()),
        );

        // Update mappings
        self.nodes_mapping.insert(internal_index, node_id);
        self.name_to_node.insert(node.name.clone(), node_id);

        Ok(node_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hypergraph::nodes::tests::create_test_node;

    #[test]
    fn test_add_node() {
        let mut hg = Hypergraph::new();
        let node = create_test_node("test_fn");

        let node_id = hg.add_node(node).unwrap();
        assert_eq!(node_id, NodeId(0));
    }

    #[test]
    fn test_duplicate_name() {
        let mut hg = Hypergraph::new();
        let node1 = create_test_node("duplicate");
        let node2 = create_test_node("duplicate");

        hg.add_node(node1).unwrap();
        let result = hg.add_node(node2);

        assert!(matches!(result, Err(HypergraphError::NodeNameExists(_))));
    }

    #[test]
    fn test_stable_ids() {
        let mut hg = Hypergraph::new();

        let id1 = hg.add_node(create_test_node("fn1")).unwrap();
        let id2 = hg.add_node(create_test_node("fn2")).unwrap();
        let id3 = hg.add_node(create_test_node("fn3")).unwrap();

        assert_eq!(id1, NodeId(0));
        assert_eq!(id2, NodeId(1));
        assert_eq!(id3, NodeId(2));
    }
}
