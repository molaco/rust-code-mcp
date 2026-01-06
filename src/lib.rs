//! Rust Code MCP - Scalable code search for large Rust codebases
//!
//! Library modules for the MCP server

pub mod chunker;
pub mod config;
pub mod embeddings;
pub mod indexing;
pub mod mcp;
pub mod metadata_cache;
pub mod metrics;
pub mod monitoring;
pub mod parser;
pub mod schema;
pub mod search;
pub mod security;
pub mod tools;
pub mod vector_store;

#[cfg(feature = "ide")]
pub mod semantic;

// Will be added in later steps:
// pub mod watcher;
