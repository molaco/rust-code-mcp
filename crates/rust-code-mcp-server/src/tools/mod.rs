pub mod clear_cache_tool;
pub mod health_tool;
pub mod index_tool;
pub mod project_paths;
pub mod search_tool;

// Phase 1: Modular structure
pub mod indexing_tools;
pub mod query_tools;
pub mod analysis_tools;
pub mod search_tool_router;

// Hypergraph (Layer 7): MCP tools backed by the persisted graph snapshot.
pub mod graph_tools;
