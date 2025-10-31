//! Graph statistics and metrics

use crate::hypergraph::Hypergraph;

/// Statistics about the hypergraph
#[derive(Debug, Clone)]
pub struct Stats {
    /// Number of nodes
    pub node_count: usize,

    /// Number of hyperedges
    pub edge_count: usize,

    /// Average hyperedge order (nodes per edge)
    pub avg_order: f32,

    /// Maximum hyperedge order
    pub max_order: usize,

    /// Minimum hyperedge order
    pub min_order: usize,
}

impl Hypergraph {
    /// Computes statistics about the hypergraph
    pub fn stats(&self) -> Stats {
        let node_count = self.count_nodes();
        let edge_count = self.count_hyperedges();

        if edge_count == 0 {
            return Stats {
                node_count,
                edge_count: 0,
                avg_order: 0.0,
                max_order: 0,
                min_order: 0,
            };
        }

        let orders: Vec<usize> = (0..edge_count)
            .filter_map(|i| {
                let edge_id = crate::hypergraph::HyperedgeId(i);
                self.get_hyperedge(edge_id).ok().map(|e| e.order())
            })
            .collect();

        let total_order: usize = orders.iter().sum();
        let avg_order = total_order as f32 / edge_count as f32;
        let max_order = orders.iter().max().copied().unwrap_or(0);
        let min_order = orders.iter().min().copied().unwrap_or(0);

        Stats {
            node_count,
            edge_count,
            avg_order,
            max_order,
            min_order,
        }
    }
}

impl std::fmt::Display for Stats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Hypergraph Statistics:")?;
        writeln!(f, "  Nodes: {}", self.node_count)?;
        writeln!(f, "  Hyperedges: {}", self.edge_count)?;
        writeln!(f, "  Avg edge order: {:.2}", self.avg_order)?;
        writeln!(f, "  Max edge order: {}", self.max_order)?;
        writeln!(f, "  Min edge order: {}", self.min_order)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hypergraph::{Hypergraph, HyperedgeType};
    use crate::hypergraph::nodes::tests::create_test_node;

    #[test]
    fn test_stats_empty() {
        let hg = Hypergraph::new();
        let stats = hg.stats();
        assert_eq!(stats.node_count, 0);
        assert_eq!(stats.edge_count, 0);
    }

    #[test]
    fn test_stats_with_data() {
        let mut hg = Hypergraph::new();

        let n1 = hg.add_node(create_test_node("fn1")).unwrap();
        let n2 = hg.add_node(create_test_node("fn2")).unwrap();
        let n3 = hg.add_node(create_test_node("fn3")).unwrap();

        hg.add_hyperedge([n1].into(), [n2].into(), HyperedgeType::CallPattern, 1.0)
            .unwrap();
        hg.add_hyperedge([n1].into(), [n2, n3].into(), HyperedgeType::CallPattern, 1.0)
            .unwrap();

        let stats = hg.stats();
        assert_eq!(stats.node_count, 3);
        assert_eq!(stats.edge_count, 2);
        assert_eq!(stats.max_order, 3); // [n1] + [n2, n3]
        assert_eq!(stats.min_order, 2); // [n1] + [n2]
    }
}
