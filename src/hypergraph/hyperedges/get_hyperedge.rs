use crate::hypergraph::{Hypergraph, Hyperedge, HyperedgeId, NodeId, Result, HypergraphError};
use std::collections::HashSet;

impl Hypergraph {
    /// Gets a hyperedge by its ID, reconstructing full data
    ///
    /// # Arguments
    /// * `edge_id` - The ID of the hyperedge to retrieve
    ///
    /// # Returns
    /// * `Ok(Hyperedge)` - The hyperedge with all data
    /// * `Err` - If the hyperedge ID doesn't exist
    pub fn get_hyperedge(&self, edge_id: HyperedgeId) -> Result<Hyperedge> {
        let internal = self.get_hyperedge_internal(edge_id)?;

        let key = self
            .hyperedges
            .get_index(internal)
            .ok_or(HypergraphError::InternalHyperedgeNotFound(internal))?;

        // Convert internal indexes back to public NodeIds
        let sources: HashSet<NodeId> = key
            .sources
            .iter()
            .filter_map(|&internal| self.nodes_mapping.get_public(internal))
            .collect();

        let targets: HashSet<NodeId> = key
            .targets
            .iter()
            .filter_map(|&internal| self.nodes_mapping.get_public(internal))
            .collect();

        Ok(Hyperedge {
            id: edge_id,
            sources,
            targets,
            edge_type: key.edge_type.clone(),
            weight: key.weight.into(),
        })
    }

    /// Gets the source nodes of a hyperedge
    pub fn get_sources(&self, edge_id: HyperedgeId) -> Result<HashSet<NodeId>> {
        let edge = self.get_hyperedge(edge_id)?;
        Ok(edge.sources)
    }

    /// Gets the target nodes of a hyperedge
    pub fn get_targets(&self, edge_id: HyperedgeId) -> Result<HashSet<NodeId>> {
        let edge = self.get_hyperedge(edge_id)?;
        Ok(edge.targets)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hypergraph::{Hypergraph, HyperedgeType};
    use crate::hypergraph::nodes::tests::create_test_node;

    #[test]
    fn test_get_hyperedge() {
        let mut hg = Hypergraph::new();

        let n1 = hg.add_node(create_test_node("fn1")).unwrap();
        let n2 = hg.add_node(create_test_node("fn2")).unwrap();

        let edge_id = hg
            .add_hyperedge([n1].into(), [n2].into(), HyperedgeType::CallPattern, 2.5)
            .unwrap();

        let edge = hg.get_hyperedge(edge_id).unwrap();
        assert_eq!(edge.sources, [n1].into());
        assert_eq!(edge.targets, [n2].into());
        assert_eq!(edge.weight, 2.5);
    }

    #[test]
    fn test_get_sources_targets() {
        let mut hg = Hypergraph::new();

        let n1 = hg.add_node(create_test_node("fn1")).unwrap();
        let n2 = hg.add_node(create_test_node("fn2")).unwrap();
        let n3 = hg.add_node(create_test_node("fn3")).unwrap();

        let edge_id = hg
            .add_hyperedge([n1].into(), [n2, n3].into(), HyperedgeType::CallPattern, 1.0)
            .unwrap();

        let sources = hg.get_sources(edge_id).unwrap();
        let targets = hg.get_targets(edge_id).unwrap();

        assert_eq!(sources, [n1].into());
        assert_eq!(targets, [n2, n3].into());
    }
}
