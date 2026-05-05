//! Rust Code MCP - Scalable code search for large Rust codebases
//!
//! Library modules for the MCP server

#![warn(unreachable_pub, dead_code)]

pub use rust_code_mcp_model::{ChunkContext, ChunkId, CodeChunk, Embedding, EMBEDDING_DIM};
pub use rust_code_mcp_syntax::{
    CallGraph, Chunker, Import, ParseResult, Range, RustParser, Symbol, SymbolKind, TypeReference,
    Visibility,
};

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

pub mod semantic;

pub mod graph;

// Will be added in later steps:
// pub mod watcher;
