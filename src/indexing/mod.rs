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

pub(crate) use error::IndexingError;
pub use incremental::{get_snapshot_path, IncrementalIndexer};
pub use tantivy_adapter::TantivyAdapter;
pub use unified::{IndexFileResult, IndexStats, UnifiedIndexer};
