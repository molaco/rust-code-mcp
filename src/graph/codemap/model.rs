//! Serializable data model for the codemap MCP tool.
//!
//! These types are the public shape returned by `build_codemap`. They are
//! re-exported from `graph::codemap` via `pub use model::*` in `mod.rs` so
//! external paths like `crate::graph::codemap::Codemap` continue to resolve.

use serde::{Deserialize, Serialize};

use crate::graph::ids::NodeId;
use crate::graph::model::{ItemKind, NodeKind};
use crate::graph::ModuleTreeNode;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Codemap {
    pub prompt: String,
    pub snapshot_id: String,
    pub generated_at_unix: u64,
    pub seeds: Vec<NodeId>,
    pub nodes: Vec<CodemapNode>,
    pub edges: Vec<CodemapEdge>,
    pub hierarchy: ModuleTreeNode,
    pub stats: CodemapStats,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodemapNode {
    pub id: NodeId,
    pub qualified_name: String,
    pub kind: NodeKind,
    pub item_kind: Option<ItemKind>,
    pub file: Option<String>,
    pub span: Option<(u32, u32)>,
    /// 1-indexed source line of the node's `span.0` byte offset. Matches the
    /// `ChunkContext.line_start` convention. `None` when the file isn't on
    /// disk, when the span is absent, or when the line→byte table lookup
    /// fails for any reason.
    pub line: Option<u32>,
    pub relevance: f32,
    pub is_seed: bool,
    pub snippet: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodemapEdge {
    pub from: NodeId,
    pub to: NodeId,
    pub kind: EdgeKind,
    /// Edge multiplicity. v1: always 1, since the raw-ID graph adapters
    /// (callees_of / referrers_of) deduplicate by NodeId and the BFS
    /// dedups re-visits. Future versions may carry call-site multiplicity
    /// once the adapters expose counts.
    pub weight: u32,
}

/// Edge kind. Marked `#[non_exhaustive]` so future variants
/// (`Implements`, `Inherits`, …) are not semver-breaking — `EdgeKind`
/// is part of the MCP tool's serialized JSON output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum EdgeKind {
    Calls,
    Uses,
    Imports,
    Contains,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodemapStats {
    pub seed_count: usize,
    pub node_count: usize,
    pub edge_count: usize,
    pub embedded_nodes: usize,
    pub embeddings_computed: usize,
    pub total_ms: u64,
}

/// Caller-tunable knobs. The MCP tool layer translates JSON params into this.
#[derive(Debug, Clone)]
pub struct CodemapOptions {
    pub max_nodes: usize,
    pub depth: u8,
    pub top_k_seeds: usize,
    pub max_incoming_per_node: usize,
    pub embedding_policy: EmbeddingPolicy,
    pub include_snippets: bool,
}

impl Default for CodemapOptions {
    fn default() -> Self {
        Self {
            max_nodes: 80,
            depth: 3,
            top_k_seeds: 20,
            max_incoming_per_node: 8,
            embedding_policy: EmbeddingPolicy::NoRerank,
            include_snippets: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddingPolicy {
    NoRerank,
    UseCachedOnly,
    ComputeMissing,
}
