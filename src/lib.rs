//! Rust Code MCP - Scalable code search for large Rust codebases
//!
//! Library modules for the MCP server

#![warn(unreachable_pub, dead_code)]

pub use rmc_engine::chunker;
pub mod config;
pub use rmc_engine::embeddings;
pub mod indexing;
pub mod mcp;
pub mod metadata_cache;
pub mod metrics;
pub mod monitoring;
pub use rmc_engine::parser;
pub use rmc_engine::schema;
pub use rmc_engine::search;
pub mod security;
pub mod tools;
pub use rmc_engine::vector_store;

pub mod semantic;

pub mod graph;

// Will be added in later steps:
// pub mod watcher;
