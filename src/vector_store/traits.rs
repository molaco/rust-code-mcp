//! Vector store backend trait definition
//!
//! Defines the interface that all vector storage backends must implement.

use async_trait::async_trait;

use crate::chunker::{ChunkId, CodeChunk};
use crate::embeddings::Embedding;
use super::error::VectorStoreError;
use super::SearchResult;

/// Trait for vector storage backends
///
/// Implementations must be Send + Sync for use with async runtimes.
/// All operations are async to support both embedded and remote backends.
#[async_trait]
pub trait VectorStoreBackend: Send + Sync {
    /// Insert or update chunks with their embeddings
    async fn upsert_chunks(
        &self,
        chunks_with_embeddings: Vec<(ChunkId, Embedding, CodeChunk)>,
    ) -> Result<(), VectorStoreError>;

    /// Search for similar chunks using a query vector
    async fn search(
        &self,
        query_vector: Embedding,
        limit: usize,
    ) -> Result<Vec<SearchResult>, VectorStoreError>;

    /// Delete chunks by their IDs
    async fn delete_chunks(
        &self,
        chunk_ids: Vec<ChunkId>,
    ) -> Result<(), VectorStoreError>;

    /// Delete all chunks from a specific file path
    async fn delete_by_file_path(
        &self,
        file_path: &str,
    ) -> Result<(), VectorStoreError>;

    /// Get the total number of vectors in the store
    async fn count(&self) -> Result<usize, VectorStoreError>;

    /// Clear all vectors (keep collection/table structure)
    async fn clear(&self) -> Result<(), VectorStoreError>;

    /// Check if the backend is healthy/connected
    async fn health_check(&self) -> Result<(), VectorStoreError>;
}
