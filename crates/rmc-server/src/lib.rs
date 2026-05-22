//! `rmc-server` — MCP server cluster.
//!
//! Hosts the three modules that together implement the MCP server surface:
//! `tools` (the rmcp adapter endpoints), `mcp` (the `SyncManager` plus
//! project-path resolver), and `semantic` (the rust-analyzer IDE
//! wrapper used by analysis tools).

#![warn(unreachable_pub, dead_code)]

pub mod tools;
pub mod mcp;
pub mod semantic;
