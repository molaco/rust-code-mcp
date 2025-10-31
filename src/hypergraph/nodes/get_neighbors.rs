use crate::hypergraph::{Hypergraph, NodeId, Result};
use std::collections::HashSet;

impl Hypergraph {
    /// Gets all neighbors of a node (nodes connected via hyperedges)
    ///
    /// # Arguments
    /// * `node_id` - The node to query
    ///
    /// # Returns
    /// Set of node IDs that share at least one hyperedge with the given node
    pub fn get_neighbors(&self, node_id: NodeId) -> Result<HashSet<NodeId>> {
        let edges = self.get_hyperedges_containing(node_id)?;

        let mut neighbors = HashSet::new();

        for edge_id in edges {
            let edge = self.get_hyperedge(edge_id)?;

            // Add all nodes from this hyperedge except the query node
            for &n in edge.sources.iter().chain(edge.targets.iter()) {
                if n != node_id {
                    neighbors.insert(n);
                }
            }
        }

        Ok(neighbors)
    }

    /// Gets neighbors in a specific direction (sources or targets)
    pub fn get_neighbors_from(&self, node_id: NodeId) -> Result<HashSet<NodeId>> {
        let edges = self.get_hyperedges_containing(node_id)?;

        let mut neighbors = HashSet::new();

        for edge_id in edges {
            let edge = self.get_hyperedge(edge_id)?;

            // If node_id is in sources, return targets
            if edge.sources.contains(&node_id) {
                neighbors.extend(edge.targets.iter());
            }
        }

        Ok(neighbors)
    }

    /// Gets nodes that point TO this node
    pub fn get_neighbors_to(&self, node_id: NodeId) -> Result<HashSet<NodeId>> {
        let edges = self.get_hyperedges_containing(node_id)?;

        let mut neighbors = HashSet::new();

        for edge_id in edges {
            let edge = self.get_hyperedge(edge_id)?;

            // If node_id is in targets, return sources
            if edge.targets.contains(&node_id) {
                neighbors.extend(edge.sources.iter());
            }
        }

        Ok(neighbors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hypergraph::{Hypergraph, HyperedgeType};
    use crate::hypergraph::nodes::tests::create_test_node;

    #[test]
    fn test_get_neighbors() {
        let mut hg = Hypergraph::new();

        let n1 = hg.add_node(create_test_node("fn1")).unwrap();
        let n2 = hg.add_node(create_test_node("fn2")).unwrap();
        let n3 = hg.add_node(create_test_node("fn3")).unwrap();

        hg.add_hyperedge([n1].into(), [n2, n3].into(), HyperedgeType::CallPattern, 1.0)
            .unwrap();

        let neighbors = hg.get_neighbors(n1).unwrap();
        assert_eq!(neighbors, [n2, n3].into_iter().collect());
    }

    #[test]
    fn test_get_neighbors_directional() {
        let mut hg = Hypergraph::new();

        let n1 = hg.add_node(create_test_node("fn1")).unwrap();
        let n2 = hg.add_node(create_test_node("fn2")).unwrap();
        let n3 = hg.add_node(create_test_node("fn3")).unwrap();

        // n1 → [n2, n3]
        hg.add_hyperedge([n1].into(), [n2, n3].into(), HyperedgeType::CallPattern, 1.0)
            .unwrap();

        let neighbors_from = hg.get_neighbors_from(n1).unwrap();
        assert_eq!(neighbors_from, [n2, n3].into_iter().collect());

        let neighbors_to = hg.get_neighbors_to(n2).unwrap();
        assert_eq!(neighbors_to, [n1].into_iter().collect());
    }
}
