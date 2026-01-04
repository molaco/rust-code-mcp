//! Qdrant adapter for semantic vector indexing
//!
//! This module encapsulates all Qdrant-specific operations, providing a clean
//! interface for indexing and searching code chunks using vector embeddings.
//!
//! ## Overview
//!
//! The `QdrantAdapter` provides:
//! - **Batch vector indexing**: Efficient upsert of embeddings with metadata
//! - **File-based deletion**: Remove all vectors for modified/deleted files
//! - **Collection management**: Clear and count operations
//! - **Vector store access**: Clone-able store for concurrent searches
//!
//! ## Architecture
//!
//! ```text
//! QdrantAdapter
//!     └─ VectorStore (wraps Arc<QdrantClient>)
//!         ├─ Async batched upserts
//!         └─ Concurrent-safe operations
//! ```
//!
//! ## Refactoring Notes
//!
//! This module was extracted during Phase 2 refactoring to separate concerns:
//! - Moved from `unified.rs` (1047 LOC → 743 LOC)
//! - Encapsulates all Qdrant-specific logic
//! - Reduces coupling with Tantivy and chunking components
//!
//! ## Examples
//!
//! ```rust,no_run
//! use file_search_mcp::indexing::qdrant_adapter::QdrantAdapter;
//! use file_search_mcp::vector_store::{VectorStore, VectorStoreConfig};
//!
//! # async fn example() -> anyhow::Result<()> {
//! // Create vector store
//! let config = VectorStoreConfig {
//!     url: "http://localhost:6334".to_string(),
//!     collection_name: "code_chunks".to_string(),
//!     vector_size: 384,
//! };
//! let vector_store = VectorStore::new(config).await?;
//!
//! // Create adapter
//! let adapter = QdrantAdapter::new(vector_store);
//!
//! // Index chunks with embeddings
//! // adapter.index_chunks(chunks, embeddings).await?;
//!
//! // Get count
//! let count = adapter.count().await?;
//! # Ok(())
//! # }
//! ```

use crate::chunker::{ChunkId, CodeChunk};
use crate::embeddings::Embedding;
use crate::vector_store::VectorStore;
use anyhow::Result;
use std::path::Path;

/// Adapter for Qdrant vector indexing operations
pub struct QdrantAdapter {
    /// Vector store instance
    vector_store: VectorStore,
}

impl QdrantAdapter {
    /// Create a new QdrantAdapter
    pub fn new(vector_store: VectorStore) -> Self {
        Self { vector_store }
    }

    /// Index chunks with their embeddings to Qdrant
    pub async fn index_chunks(
        &self,
        chunks: Vec<CodeChunk>,
        embeddings: Vec<Embedding>,
    ) -> Result<()> {
        if chunks.len() != embeddings.len() {
            anyhow::bail!(
                "Chunk and embedding count mismatch: {} chunks, {} embeddings",
                chunks.len(),
                embeddings.len()
            );
        }

        let chunk_data: Vec<(ChunkId, Embedding, CodeChunk)> = chunks
            .into_iter()
            .zip(embeddings.into_iter())
            .map(|(chunk, embedding)| (chunk.id, embedding, chunk))
            .collect();

        self.vector_store
            .upsert_chunks(chunk_data)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to index to Qdrant: {}", e))?;

        Ok(())
    }

    /// Delete all chunks for a specific file
    pub async fn delete_file_chunks(&self, file_path: &Path) -> Result<()> {
        let file_path_str = file_path.to_string_lossy().to_string();

        self.vector_store
            .delete_by_file_path(&file_path_str)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to delete from Qdrant: {}", e))?;

        tracing::debug!("Deleted Qdrant chunks for file: {}", file_path_str);
        Ok(())
    }

    /// Clear entire collection
    pub async fn clear_collection(&self) -> Result<()> {
        self.vector_store
            .clear_collection()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to clear Qdrant collection: {}", e))?;
        Ok(())
    }

    /// Get count of vectors in the collection
    pub async fn count(&self) -> Result<usize> {
        self.vector_store
            .count()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to count Qdrant vectors: {}", e))
    }

    /// Get cloned vector store for searching
    pub fn vector_store_cloned(&self) -> VectorStore {
        self.vector_store.clone()
    }
}

#[cfg(all(test, feature = "qdrant"))]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Requires Qdrant server running
    async fn test_qdrant_adapter_creation() {
        let config = crate::vector_store::QdrantConfig {
            url: "http://localhost:6333".to_string(),
            collection_name: "test_adapter".to_string(),
            vector_size: 384,
        };

        let vector_store = VectorStore::new(config).await;
        assert!(vector_store.is_ok(), "Failed to create VectorStore: {:?}", vector_store.err());

        let adapter = QdrantAdapter::new(vector_store.unwrap());

        // Test count
        let count = adapter.count().await;
        assert!(count.is_ok(), "Failed to count vectors: {:?}", count.err());
    }
}
