//! Indexing module - Unified pipeline for both Tantivy and vector store

pub mod consistency;
pub(crate) mod embedding_batcher;
pub mod error;
pub mod error_collection;
pub(crate) mod file_processor;
pub mod identity;
pub mod incremental;
pub mod indexer_core;
pub mod merkle;
pub mod retry;
pub mod tantivy_adapter;
pub mod unified;
mod unified_parallel;

pub use consistency::{ConsistencyChecker, ConsistencyReport};
pub use error::IndexingError;
pub use error_collection::{ErrorCategory, ErrorCollector, ErrorDetail};
pub use incremental::{get_snapshot_path, IncrementalIndexer};
pub use indexer_core::{IndexerCore, ProcessedFile};
pub use retry::{retry_sync_with_backoff, retry_with_backoff};
pub use tantivy_adapter::TantivyAdapter;
pub use unified::{IndexFileResult, IndexStats, UnifiedIndexer};
