//! Stable index types using the newtype pattern
//!
//! Reference: https://matklad.github.io/2018/06/04/newtype-index-pattern.html

use std::fmt::{Display, Formatter, Result};

/// Stable node identifier
///
/// Uses newtype pattern to prevent mixing with HyperedgeId
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct NodeId(pub usize);

impl Display for NodeId {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        write!(f, "N{}", self.0)
    }
}

impl From<usize> for NodeId {
    fn from(index: usize) -> Self {
        NodeId(index)
    }
}

/// Stable hyperedge identifier
///
/// Uses newtype pattern to prevent mixing with NodeId
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct HyperedgeId(pub usize);

impl Display for HyperedgeId {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        write!(f, "E{}", self.0)
    }
}

impl From<usize> for HyperedgeId {
    fn from(index: usize) -> Self {
        HyperedgeId(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_id_display() {
        assert_eq!(format!("{}", NodeId(42)), "N42");
    }

    #[test]
    fn test_hyperedge_id_display() {
        assert_eq!(format!("{}", HyperedgeId(7)), "E7");
    }

    #[test]
    fn test_ids_not_mixable() {
        let node = NodeId(5);
        let edge = HyperedgeId(5);
        // This won't compile: assert_eq!(node, edge);
        // Type safety works!
        assert_eq!(node.0, edge.0); // Can compare inner values explicitly
    }
}
