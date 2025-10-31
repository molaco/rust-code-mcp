//! Error types for hypergraph operations

use thiserror::Error;
use crate::hypergraph::indexes::{NodeId, HyperedgeId};

/// All possible hypergraph errors
#[derive(Debug, Error)]
pub enum HypergraphError {
    // Node errors
    #[error("NodeId {0} not found")]
    NodeNotFound(NodeId),

    #[error("Node name '{0}' already exists")]
    NodeNameExists(String),

    #[error("Internal node index {0} not found")]
    InternalNodeNotFound(usize),

    // Hyperedge errors
    #[error("HyperedgeId {0} not found")]
    HyperedgeNotFound(HyperedgeId),

    #[error("Hyperedge must have at least one source or target node")]
    EmptyHyperedge,

    #[error("Internal hyperedge index {0} not found")]
    InternalHyperedgeNotFound(usize),

    #[error("Hyperedge {0} has no source nodes")]
    NoSourceNodes(HyperedgeId),

    #[error("Hyperedge {0} has no target nodes")]
    NoTargetNodes(HyperedgeId),

    // Operation errors
    #[error("Cannot remove node {0}: it is part of {1} hyperedges")]
    NodeInUse(NodeId, usize),

    #[error("Hyperedge already exists with these parameters")]
    HyperedgeAlreadyExists,
}

/// Convenient Result type for hypergraph operations
pub type Result<T> = std::result::Result<T, HypergraphError>;
