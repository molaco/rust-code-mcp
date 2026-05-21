//! Semantic code chunking with context enrichment
//!
//! Chunks code by symbols (functions, structs, etc.) and adds rich context
//! for better embedding and retrieval quality.

mod chunker;
mod split;
mod types;

pub use chunker::Chunker;
pub use types::{ChunkContext, ChunkId, ChunkSplitConfig, CodeChunk};
