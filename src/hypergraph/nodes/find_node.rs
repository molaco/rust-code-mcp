use crate::hypergraph::{Hypergraph, NodeId};

impl Hypergraph {
    /// Finds a node by its name
    ///
    /// # Arguments
    /// * `name` - The name of the node to find
    ///
    /// # Returns
    /// * `Some(NodeId)` - If a node with this name exists
    /// * `None` - If no node with this name exists
    pub fn find_node_by_name(&self, name: &str) -> Option<NodeId> {
        self.name_to_node.get(name).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hypergraph::nodes::tests::create_test_node;

    #[test]
    fn test_find_existing_node() {
        let mut hg = Hypergraph::new();
        let node = create_test_node("my_function");
        let node_id = hg.add_node(node).unwrap();

        let found = hg.find_node_by_name("my_function");
        assert_eq!(found, Some(node_id));
    }

    #[test]
    fn test_find_nonexistent_node() {
        let hg = Hypergraph::new();
        assert_eq!(hg.find_node_by_name("nonexistent"), None);
    }
}
