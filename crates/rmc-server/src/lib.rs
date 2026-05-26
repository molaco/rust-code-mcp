//! `rmc-server` — MCP server cluster.
//!
//! Hosts the three modules that together implement the MCP server surface:
//! `tools` (the rmcp adapter endpoints), `mcp` (the `SyncManager` plus
//! project-path resolver), and `semantic` (the rust-analyzer IDE
//! wrapper used by analysis tools).

// lancedb 0.29's async stack can push Send inference for the sync-manager
// background task past the default recursion limit when the runtime owner
// spawns it.
#![recursion_limit = "512"]
#![warn(unreachable_pub, dead_code)]

pub mod tools;
pub mod mcp;
pub mod semantic;
