//! Indexing module - Unified pipeline for both Tantivy and vector store

pub(crate) mod backup;
mod consistency;
pub(crate) mod embedding_batcher;
pub mod error;
pub mod error_collection;
pub(crate) mod file_processor;
mod identity;
mod incremental;
pub mod incremental_service;
mod indexer_core;
mod merkle;
pub mod project_paths;
mod retry;
pub mod search;
mod tantivy_adapter;
mod unified;
mod unified_parallel;

pub(crate) use error::IndexingError;
pub use incremental::{get_snapshot_path, IncrementalIndexer};
pub use incremental_service::{
    index_project_incrementally, IncrementalIndexOutcome, IncrementalIndexRequest,
};
pub use merkle::{ChangeSet, FileSystemMerkle};
pub use project_paths::{
    collection_prefix, dir_hash, read_embedder_identity, IndexedProfilePaths,
    IndexingProjectPaths,
};
pub use search::open_bm25_search;
pub use tantivy_adapter::TantivyAdapter;
pub use unified::{IndexFileResult, IndexStats, UnifiedIndexer};
