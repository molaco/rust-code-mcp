//! Indexing module - Unified pipeline for both Tantivy and vector store

pub mod consistency;
pub mod errors;
pub mod incremental;
pub mod indexer_core;
pub mod merkle;
pub mod retry;
pub mod tantivy_adapter;
pub mod unified;

pub use consistency::{ConsistencyChecker, ConsistencyReport};
pub use errors::{ErrorCategory, ErrorCollector, ErrorDetail};
pub use incremental::{get_snapshot_path, IncrementalIndexer};
pub use indexer_core::{IndexerCore, ProcessedFile};
pub use retry::{retry_sync_with_backoff, retry_with_backoff};
pub use tantivy_adapter::TantivyAdapter;
pub use unified::{IndexFileResult, IndexStats, UnifiedIndexer};
