//! Rust Code MCP - Scalable code search for large Rust codebases
//!
//! Library modules for the MCP server

#![warn(unreachable_pub, dead_code)]

pub use rmc_engine::{chunker, embeddings, parser, schema, search, vector_store};
pub use rmc_graph::graph;
pub use rmc_config::config;
pub use rmc_indexing::{indexing, monitoring, metadata_cache, metrics, security};
pub use rmc_server::{tools, mcp, semantic};
