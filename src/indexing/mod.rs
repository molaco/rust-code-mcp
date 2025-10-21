//! Indexing module - Unified pipeline for both Tantivy and Qdrant

pub mod bulk;
pub mod incremental;
pub mod merkle;
pub mod unified;

pub use bulk::{bulk_index_with_auto_mode, BulkIndexer, HnswConfig};
pub use incremental::IncrementalIndexer;
pub use unified::{IndexFileResult, IndexStats, UnifiedIndexer};
