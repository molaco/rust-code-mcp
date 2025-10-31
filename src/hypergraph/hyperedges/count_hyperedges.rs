use crate::hypergraph::Hypergraph;

impl Hypergraph {
    /// Returns the number of hyperedges in the hypergraph
    pub fn count_hyperedges(&self) -> usize {
        self.hyperedges.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hypergraph::{Hypergraph, HyperedgeType};
    use crate::hypergraph::nodes::tests::create_test_node;

    #[test]
    fn test_count_hyperedges() {
        let mut hg = Hypergraph::new();
        assert_eq!(hg.count_hyperedges(), 0);

        let n1 = hg.add_node(create_test_node("fn1")).unwrap();
        let n2 = hg.add_node(create_test_node("fn2")).unwrap();

        hg.add_hyperedge([n1].into(), [n2].into(), HyperedgeType::CallPattern, 1.0)
            .unwrap();
        assert_eq!(hg.count_hyperedges(), 1);
    }
}
