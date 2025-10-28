//! Indexing module - Unified pipeline for both Tantivy and Qdrant

pub mod bulk;
pub mod errors;
pub mod incremental;
pub mod merkle;
pub mod unified;

pub use bulk::{bulk_index_with_auto_mode, BulkIndexer, HnswConfig};
pub use errors::{ErrorCategory, ErrorCollector, ErrorDetail};
pub use incremental::{get_snapshot_path, IncrementalIndexer};
pub use unified::{IndexFileResult, IndexStats, UnifiedIndexer};
