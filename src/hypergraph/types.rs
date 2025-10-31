//! Core hypergraph data types

use crate::hypergraph::indexes::{NodeId, HyperedgeId};
use crate::parser::Symbol;
use ordered_float::OrderedFloat;
use std::collections::HashSet;
use std::fmt::{Display, Formatter, Result as FmtResult};
use std::path::PathBuf;

/// A node in the hypergraph (wraps Symbol from parser)
#[derive(Debug, Clone, PartialEq)]
pub struct HyperNode {
    pub id: NodeId,
    pub name: String,
    pub file_path: PathBuf,
    pub line_start: usize,
    pub line_end: usize,
    pub symbol: Symbol,  // Reuse existing parser Symbol
}

impl Display for HyperNode {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "{}:{}", self.file_path.display(), self.name)
    }
}

impl std::hash::Hash for HyperNode {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
        self.name.hash(state);
    }
}

impl Eq for HyperNode {}

/// Types of hyperedges for code analysis
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum HyperedgeType {
    /// File/module contains symbols
    /// Example: {file} → {fn1, fn2, struct1}
    ModuleContainment,

    /// Functions call other functions
    /// Example: {caller} → {callee1, callee2}
    CallPattern,

    /// Types implement a trait
    /// Example: {trait} → {Type1, Type2}
    TraitImpl { trait_name: String },

    /// Struct is composed of field types
    /// Example: {Struct} → {FieldType1, FieldType2}
    TypeComposition { struct_name: String },

    /// File imports modules
    /// Example: {file} → {dep1, dep2}
    ImportCluster,
}

impl Display for HyperedgeType {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        match self {
            HyperedgeType::ModuleContainment => write!(f, "ModuleContainment"),
            HyperedgeType::CallPattern => write!(f, "CallPattern"),
            HyperedgeType::TraitImpl { trait_name } => write!(f, "TraitImpl({})", trait_name),
            HyperedgeType::TypeComposition { struct_name } => write!(f, "TypeComposition({})", struct_name),
            HyperedgeType::ImportCluster => write!(f, "ImportCluster"),
        }
    }
}

/// A directed many-to-many hyperedge
#[derive(Debug, Clone, PartialEq)]
pub struct Hyperedge {
    pub id: HyperedgeId,
    pub sources: HashSet<NodeId>,   // Source nodes
    pub targets: HashSet<NodeId>,   // Target nodes
    pub edge_type: HyperedgeType,
    pub weight: f32,
}

impl Hyperedge {
    /// Returns the order (total number of nodes) of this hyperedge
    pub fn order(&self) -> usize {
        self.sources.len() + self.targets.len()
    }

    /// Returns all node IDs in this hyperedge
    pub fn all_nodes(&self) -> HashSet<NodeId> {
        self.sources.union(&self.targets).copied().collect()
    }
}

/// Internal key for storing hyperedges in IndexSet
/// Combines all fields to ensure uniqueness
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct HyperedgeKey {
    /// Sorted source node internal indexes
    pub(crate) sources: Vec<usize>,

    /// Sorted target node internal indexes
    pub(crate) targets: Vec<usize>,

    pub(crate) edge_type: HyperedgeType,

    /// OrderedFloat makes f32 hashable
    pub(crate) weight: OrderedFloat<f32>,
}

impl HyperedgeKey {
    pub(crate) fn new(
        sources: Vec<usize>,
        targets: Vec<usize>,
        edge_type: HyperedgeType,
        weight: f32,
    ) -> Self {
        let mut sources = sources;
        let mut targets = targets;
        sources.sort_unstable();
        targets.sort_unstable();

        Self {
            sources,
            targets,
            edge_type,
            weight: OrderedFloat(weight),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hyperedge_order() {
        let edge = Hyperedge {
            id: HyperedgeId(0),
            sources: [NodeId(1), NodeId(2)].into_iter().collect(),
            targets: [NodeId(3), NodeId(4), NodeId(5)].into_iter().collect(),
            edge_type: HyperedgeType::CallPattern,
            weight: 1.0,
        };

        assert_eq!(edge.order(), 5);
    }

    #[test]
    fn test_hyperedge_type_display() {
        assert_eq!(
            format!("{}", HyperedgeType::TraitImpl { trait_name: "Display".into() }),
            "TraitImpl(Display)"
        );
    }
}
