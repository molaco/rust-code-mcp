//! Indexing module - Unified pipeline for both Tantivy and Qdrant

pub mod bulk;
pub mod consistency;
pub mod errors;
pub mod incremental;
pub mod merkle;
pub mod retry;
pub mod unified;

pub use bulk::{bulk_index_with_auto_mode, BulkIndexer, HnswConfig};
pub use consistency::{ConsistencyChecker, ConsistencyReport};
pub use errors::{ErrorCategory, ErrorCollector, ErrorDetail};
pub use incremental::{get_snapshot_path, IncrementalIndexer};
pub use retry::{retry_sync_with_backoff, retry_with_backoff};
pub use unified::{IndexFileResult, IndexStats, UnifiedIndexer};
