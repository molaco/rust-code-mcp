pub mod project_paths;

// Phase 1: Modular structure
mod endpoints;
mod params;
mod router;

// Hypergraph (Layer 7): MCP tools backed by the persisted graph snapshot.
mod graph;

pub use router::SearchToolRouter;
pub use router::SearchToolRouter as SearchTool;
pub use endpoints::index::{index_codebase, IndexCodebaseParams};
