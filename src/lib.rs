//! Rust Code MCP - Scalable code search for large Rust codebases
//!
//! Library modules for the MCP server

#![warn(unreachable_pub, dead_code)]

pub use rmc_engine::chunker;
pub use rmc_config::config;
pub use rmc_engine::embeddings;
pub use rmc_indexing::indexing;
pub mod mcp;
pub use rmc_indexing::metadata_cache;
pub use rmc_indexing::metrics;
pub use rmc_indexing::monitoring;
pub use rmc_engine::parser;
pub use rmc_engine::schema;
pub use rmc_engine::search;
pub use rmc_indexing::security;
pub mod tools;
pub use rmc_engine::vector_store;

pub mod semantic;

pub use rmc_graph::graph;

// Will be added in later steps:
// pub mod watcher;
