use crate::hypergraph::Hypergraph;

impl Hypergraph {
    /// Returns the number of nodes in the hypergraph
    pub fn count_nodes(&self) -> usize {
        self.nodes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hypergraph::nodes::tests::create_test_node;

    #[test]
    fn test_count_nodes() {
        let mut hg = Hypergraph::new();
        assert_eq!(hg.count_nodes(), 0);

        hg.add_node(create_test_node("fn1")).unwrap();
        assert_eq!(hg.count_nodes(), 1);

        hg.add_node(create_test_node("fn2")).unwrap();
        hg.add_node(create_test_node("fn3")).unwrap();
        assert_eq!(hg.count_nodes(), 3);
    }
}
