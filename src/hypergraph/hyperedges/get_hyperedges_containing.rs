use crate::hypergraph::{Hypergraph, Hyperedge, HyperedgeId, NodeId, Result};

impl Hypergraph {
    /// Gets all hyperedges that contain a given node
    ///
    /// # Arguments
    /// * `node_id` - The node to query
    ///
    /// # Returns
    /// Vector of hyperedge IDs containing this node
    pub fn get_hyperedges_containing(&self, node_id: NodeId) -> Result<Vec<HyperedgeId>> {
        let internal = self.get_node_internal(node_id)?;

        let edge_ids = self
            .nodes
            .get_index(internal)
            .map(|(_, edge_set)| {
                edge_set
                    .iter()
                    .filter_map(|&internal_edge| self.hyperedges_mapping.get_public(internal_edge))
                    .collect()
            })
            .unwrap_or_default();

        Ok(edge_ids)
    }

    /// Gets full hyperedge data for edges containing a node
    pub fn get_full_hyperedges_containing(&self, node_id: NodeId) -> Result<Vec<Hyperedge>> {
        let edge_ids = self.get_hyperedges_containing(node_id)?;

        edge_ids
            .into_iter()
            .map(|id| self.get_hyperedge(id))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hypergraph::{Hypergraph, HyperedgeType};
    use crate::hypergraph::nodes::tests::create_test_node;

    #[test]
    fn test_get_hyperedges_containing() {
        let mut hg = Hypergraph::new();

        let n1 = hg.add_node(create_test_node("fn1")).unwrap();
        let n2 = hg.add_node(create_test_node("fn2")).unwrap();
        let n3 = hg.add_node(create_test_node("fn3")).unwrap();

        let e1 = hg
            .add_hyperedge([n1].into(), [n2].into(), HyperedgeType::CallPattern, 1.0)
            .unwrap();
        let e2 = hg
            .add_hyperedge([n1].into(), [n3].into(), HyperedgeType::CallPattern, 1.0)
            .unwrap();

        let edges = hg.get_hyperedges_containing(n1).unwrap();
        assert_eq!(edges.len(), 2);
        assert!(edges.contains(&e1));
        assert!(edges.contains(&e2));
    }

    #[test]
    fn test_node_in_no_edges() {
        let mut hg = Hypergraph::new();
        let n1 = hg.add_node(create_test_node("fn1")).unwrap();

        let edges = hg.get_hyperedges_containing(n1).unwrap();
        assert_eq!(edges.len(), 0);
    }
}
