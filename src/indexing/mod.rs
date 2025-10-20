//! Indexing module - Unified pipeline for both Tantivy and Qdrant

pub mod bulk;
pub mod merkle;
pub mod unified;

pub use bulk::{bulk_index_with_auto_mode, BulkIndexer, HnswConfig};
pub use unified::{IndexFileResult, IndexStats, UnifiedIndexer};
