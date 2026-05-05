//! Indexing module - Unified pipeline for both Tantivy and vector store

#![warn(unreachable_pub, dead_code)]

pub mod backup;
pub mod consistency;
pub(crate) mod embedding_batcher;
pub mod error;
pub mod errors;
pub(crate) mod file_processor;
pub mod incremental;
pub mod indexer_core;
pub mod merkle;
pub mod metadata_cache;
pub mod metrics;
pub mod retry;
pub mod security;
pub mod unified;
mod config;

pub use backup::BackupManager;
pub use config::{IndexerConfig, IndexerCoreConfig};
pub use consistency::{ConsistencyChecker, ConsistencyReport};
pub use error::IndexingError;
pub use errors::{ErrorCategory, ErrorCollector, ErrorDetail};
pub use incremental::{get_snapshot_path, IncrementalIndexer};
pub use indexer_core::{IndexerCore, ProcessedFile};
pub use metadata_cache::{FileMetadata, FileStat, MetadataCache};
pub use metrics::{IndexingMetrics, PhaseTimer};
pub use retry::{retry_sync_with_backoff, retry_with_backoff};
pub use rust_code_mcp_bm25::{Bm25Search, ChunkSchema, TantivyAdapter, TantivyConfig};
pub use security::SensitiveFileFilter;
pub use unified::{IndexFileResult, IndexStats, UnifiedIndexer};

pub mod tantivy_adapter {
    pub use rust_code_mcp_bm25::TantivyAdapter;
}
