//! Indexing module - Unified pipeline for both Tantivy and Qdrant

pub mod merkle;
pub mod unified;

pub use unified::{IndexFileResult, IndexStats, UnifiedIndexer};
