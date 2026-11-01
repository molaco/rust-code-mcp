//! Indexing module - Unified pipeline for both Tantivy and vector store

pub(crate) mod backup;
mod consistency;
pub(crate) mod embedding_batcher;
pub mod error;
pub(crate) mod error_collection;
pub(crate) mod file_processor;
mod identity;
mod incremental;
pub mod incremental_service;
pub mod index_stats;
mod indexer_core;
mod merkle;
pub mod project_paths;
mod retry;
pub mod search;
mod tantivy_adapter;
mod traversal;
mod unified;
mod unified_parallel;

pub(crate) use error::IndexingError;
pub use incremental::{IncrementalIndexer, ReaderExclusion, get_snapshot_path};
pub use incremental_service::{
    IncrementalIndexOutcome, IncrementalIndexRequest, index_project_incrementally,
};
pub use merkle::{ChangeSet, FileSystemMerkle};
pub use project_paths::{
    IndexedProfilePaths, IndexingProjectPaths, collection_prefix, dir_hash, read_embedder_identity,
};
pub use search::open_bm25_search;
pub use tantivy_adapter::TantivyAdapter;
// Exactly one tree walk is exported: rmc-server stats the same files as the
// indexer, and both sides must agree EXACTLY on what a project file is, otherwise
// the semantic cache considers fresh what the index has already reindexed.
// For the same reason the nested-project boundary is exported too: where this
// project ends must be understood the same way by everyone asking about its files.
pub use traversal::{NESTED_ROOT_MARKER, collect_project_rust_files, is_nested_project_root};
pub use unified::{
    IndexFileResult, IndexStats, UnifiedIndexer, VectorMaintenance,
    rebuild_bm25_from_existing_store,
};
