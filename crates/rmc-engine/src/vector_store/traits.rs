//! Vector store backend trait definition
//!
//! Defines the interface that all vector storage backends must implement.

use async_trait::async_trait;
use std::collections::{HashMap, HashSet};

use super::VectorSearchResult;
use super::VectorStoreLayout;
use super::VectorStoreMaintenance;
use super::error::VectorStoreError;
use crate::chunker::{ChunkId, CodeChunk};
use crate::embeddings::Embedding;

/// Trait for vector storage backends
///
/// Implementations must be Send + Sync for use with async runtimes.
/// All operations are async to support both embedded and remote backends.
#[async_trait]
pub trait VectorStoreBackend: Send + Sync {
    /// Append chunks with their embeddings, as one write.
    ///
    /// An append, not an upsert: the ids must not be in the store already.
    /// They never are — `ChunkId` is a fresh random UUID per chunking — and
    /// replacing a file's chunks is a delete by file path followed by this.
    /// An upsert keyed on such ids only ever paid for a join that matched
    /// nothing (60 ms per file against 4 for an append, measured 2026-10-29
    /// on a 0.93M-row store).
    async fn insert_chunks(
        &self,
        chunks_with_embeddings: Vec<(ChunkId, Embedding, CodeChunk)>,
    ) -> Result<(), VectorStoreError>;

    /// Search for similar chunks using a query vector
    async fn search(
        &self,
        query_vector: Embedding,
        limit: usize,
    ) -> Result<Vec<VectorSearchResult>, VectorStoreError>;

    /// The stored chunks of `chunk_ids`, keyed by id.
    ///
    /// An id the store does not hold is simply absent from the map: the
    /// BM25 index and the store are written one after the other, so an id
    /// one of them knows and the other does not is a normal moment between
    /// two writes, and the caller decides what it means.
    async fn chunks_by_ids(
        &self,
        chunk_ids: &[ChunkId],
    ) -> Result<HashMap<ChunkId, CodeChunk>, VectorStoreError>;

    /// Every stored chunk, handed to `sink` one record batch at a time.
    ///
    /// For rebuilding what is derived from the chunks (the BM25 index)
    /// without holding the whole collection in memory. Returns the number of
    /// chunks passed to `sink`; stops at the first error, from either side.
    async fn scan_chunks(
        &self,
        sink: &mut (dyn FnMut(Vec<CodeChunk>) -> anyhow::Result<()> + Send),
    ) -> anyhow::Result<usize>;

    /// Delete chunks by their IDs
    async fn delete_chunks(&self, chunk_ids: Vec<ChunkId>) -> Result<(), VectorStoreError>;

    /// Delete all chunks of these files as ONE write; an empty slice is no
    /// write at all.
    ///
    /// One write per sync instead of one per file: each write publishes a
    /// dataset version, and a per-file delete made a 36-file sync publish 36
    /// of them only for deletions.
    async fn delete_by_file_paths(&self, file_paths: &[String]) -> Result<(), VectorStoreError>;

    /// Version, fragments and index coverage of the table.
    async fn layout(&self) -> Result<VectorStoreLayout, VectorStoreError>;

    /// Get the total number of vectors in the store
    async fn count(&self) -> Result<usize, VectorStoreError>;

    /// Distinct source files that have at least one vector in the store.
    ///
    /// Coverage lives here and nowhere else: `count()` answers "how many
    /// vectors", which says nothing about whether a file was indexed at
    /// all. A run that dies halfway leaves both numbers plausible, and
    /// only the file set can be put next to the Merkle snapshot.
    async fn indexed_file_paths(&self) -> Result<HashSet<String>, VectorStoreError>;

    /// Clear all vectors (keep collection/table structure)
    async fn clear(&self) -> Result<(), VectorStoreError>;

    /// Reclaim disk space left behind by writes.
    ///
    /// The store is append-only: every upsert lands in a NEW fragment and
    /// every change publishes a NEW version, and neither the superseded
    /// rows nor the old manifests go away on their own. Nothing in the
    /// write path can do this incrementally — a fragment only becomes
    /// unreferenced once its rows have been rewritten elsewhere — so it is
    /// a separate pass, and without it on-disk size grows without bound
    /// against a row count that does not.
    ///
    /// The same pass extends the table's indices over the rows written
    /// since the last one: lance never does that on write, so an index that
    /// is not extended here covers only what existed when it was created.
    ///
    /// `prune_keep` is the age below which versions are left alone: it is
    /// the margin for a reader that opened the table before this pass
    /// started and is still walking it.
    async fn maintain(
        &self,
        prune_keep: std::time::Duration,
    ) -> Result<VectorStoreMaintenance, VectorStoreError>;

    /// Check if the backend is healthy/connected
    async fn health_check(&self) -> Result<(), VectorStoreError>;
}
