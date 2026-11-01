//! Vector database integration using LanceDB
//!
//! Provides vector search capabilities for code chunks using embeddings.
//! Uses LanceDB as the embedded, zero-config backend.

mod error;
mod lancedb;
mod traits;

// Re-exports
pub use error::VectorStoreError;
pub use lancedb::LanceDbBackend;
pub use traits::VectorStoreBackend;

use crate::chunker::{ChunkId, CodeChunk};
use crate::embeddings::{Embedding, EmbeddingBackend};
use std::collections::{HashMap, HashSet};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

/// What one [`VectorStore::maintain`] pass actually reclaimed.
///
/// Every field is an OUTCOME, not an intent: a pass that ran and found
/// nothing to do reports zeros, which is how a caller tells "maintenance
/// is wired up and the store is already tidy" from "maintenance never
/// ran". Reporting only "ok" would collapse those two.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VectorStoreMaintenance {
    /// Fragments compaction merged away.
    pub fragments_removed: usize,
    /// Fragments compaction wrote in their place (always fewer, or the
    /// compaction was not worth running).
    pub fragments_added: usize,
    /// Superseded dataset versions the prune pass dropped.
    pub old_versions_removed: u64,
    /// Bytes the prune pass freed on disk.
    pub bytes_removed: u64,
}

/// How the store's table is laid out on disk right now — the numbers the
/// write path is supposed to keep small.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VectorStoreLayout {
    /// Current dataset version: every write (add, delete, compaction,
    /// index optimisation) publishes one.
    pub version: u64,
    /// Data fragments in the current version. Every `add` writes one.
    pub fragments: usize,
    /// Rows in the current version.
    pub rows: usize,
    /// Every index on the table.
    pub indices: Vec<VectorIndexCoverage>,
}

/// One index and how much of the table it actually covers.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VectorIndexCoverage {
    pub name: String,
    pub columns: Vec<String>,
    pub indexed_rows: usize,
    /// Rows a lookup through this index still has to scan for. An index
    /// that covers nothing is a full scan with extra steps.
    pub unindexed_rows: usize,
    /// Deltas the index is stored as: each index pass that found new rows
    /// adds one until maintenance merges them (`RMC_ANN_MAX_DELTAS`).
    pub deltas: usize,
}

/// A search result from vector search
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorSearchResult {
    pub chunk_id: ChunkId,
    pub score: f32,
    pub chunk: CodeChunk,
}

/// Configuration for the vector store
#[derive(Debug, Clone)]
pub enum VectorStoreConfig {
    /// Embedded LanceDB (default, zero-config)
    Embedded {
        /// Path to store the database
        path: PathBuf,
        /// Vector dimensions (depends on the active embedder backend;
        /// 1024 for the default Qwen3-Embedding-0.6B).
        vector_size: usize,
        /// Stable embedder identity string written to `metadata.json`
        /// next to the LanceDB table. Reopens with a different identity
        /// are rejected with [`VectorStoreError::VersionMismatch`].
        embedder_identity: String,
    },
}

impl Default for VectorStoreConfig {
    fn default() -> Self {
        // Use XDG cache directory
        let cache_dir = directories::ProjectDirs::from("", "", "rust-code-mcp")
            .map(|dirs| dirs.cache_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from(".cache/rust-code-mcp"));
        let backend = EmbeddingBackend::default();
        Self::Embedded {
            path: cache_dir.join("vectors"),
            vector_size: backend.dim(),
            embedder_identity: backend.identity(),
        }
    }
}

/// Vector store with swappable backend
///
/// This is the main entry point for vector operations.
/// It wraps a backend implementation and provides a unified API.
#[derive(Clone)]
pub struct VectorStore {
    backend: Arc<dyn VectorStoreBackend>,
}

impl VectorStore {
    /// Create with embedded backend (LanceDB).
    ///
    /// `embedder_identity` is the stable string returned by
    /// [`EmbeddingBackend::identity`] for the embedder that will produce
    /// vectors stored here. On reopen with a different identity, this
    /// returns [`VectorStoreError::VersionMismatch`] so the caller can
    /// refuse to corrupt the existing index.
    pub async fn new_embedded(
        path: PathBuf,
        vector_size: usize,
        embedder_identity: &str,
    ) -> Result<Self, VectorStoreError> {
        let backend = LanceDbBackend::new(path, vector_size, embedder_identity).await?;
        Ok(Self {
            backend: Arc::new(backend),
        })
    }

    /// Open an existing embedded backend without creating missing storage.
    ///
    /// This is intended for read-side probes such as health checks. Missing
    /// directories, tables, or metadata return [`VectorStoreError::NotFound`]
    /// instead of creating an empty vector store.
    pub async fn open_existing_embedded(
        path: PathBuf,
        vector_size: usize,
        embedder_identity: &str,
    ) -> Result<Self, VectorStoreError> {
        let backend = LanceDbBackend::open_existing(path, vector_size, embedder_identity).await?;
        Ok(Self {
            backend: Arc::new(backend),
        })
    }

    /// Create from config
    pub async fn from_config(config: VectorStoreConfig) -> Result<Self, VectorStoreError> {
        match config {
            VectorStoreConfig::Embedded {
                path,
                vector_size,
                embedder_identity,
            } => Self::new_embedded(path, vector_size, &embedder_identity).await,
        }
    }

    /// Create with default configuration
    pub async fn new_default() -> Result<Self, VectorStoreError> {
        Self::from_config(VectorStoreConfig::default()).await
    }

    // Delegate all methods to backend

    /// Append chunks with their embeddings — see
    /// [`VectorStoreBackend::insert_chunks`] for the contract on ids.
    pub async fn insert_chunks(
        &self,
        chunks_with_embeddings: Vec<(ChunkId, Embedding, CodeChunk)>,
    ) -> Result<(), VectorStoreError> {
        self.backend.insert_chunks(chunks_with_embeddings).await
    }

    /// Search for similar chunks using a query vector
    pub async fn search(
        &self,
        query_vector: Embedding,
        limit: usize,
    ) -> Result<Vec<VectorSearchResult>, VectorStoreError> {
        self.backend.search(query_vector, limit).await
    }

    /// The stored chunks of `chunk_ids` — see
    /// [`VectorStoreBackend::chunks_by_ids`].
    pub async fn chunks_by_ids(
        &self,
        chunk_ids: &[ChunkId],
    ) -> Result<HashMap<ChunkId, CodeChunk>, VectorStoreError> {
        self.backend.chunks_by_ids(chunk_ids).await
    }

    /// Every stored chunk, batch by batch — see
    /// [`VectorStoreBackend::scan_chunks`].
    pub async fn scan_chunks(
        &self,
        sink: &mut (dyn FnMut(Vec<CodeChunk>) -> anyhow::Result<()> + Send),
    ) -> anyhow::Result<usize> {
        self.backend.scan_chunks(sink).await
    }

    /// Delete chunks by their IDs
    pub async fn delete_chunks(&self, chunk_ids: Vec<ChunkId>) -> Result<(), VectorStoreError> {
        self.backend.delete_chunks(chunk_ids).await
    }

    /// Delete all chunks of these files, in one write — see
    /// [`VectorStoreBackend::delete_by_file_paths`].
    pub async fn delete_by_file_paths(
        &self,
        file_paths: &[String],
    ) -> Result<(), VectorStoreError> {
        self.backend.delete_by_file_paths(file_paths).await
    }

    /// Version, fragments and index coverage of the table — see
    /// [`VectorStoreBackend::layout`].
    pub async fn layout(&self) -> Result<VectorStoreLayout, VectorStoreError> {
        self.backend.layout().await
    }

    /// Get the total number of vectors in the store
    pub async fn count(&self) -> Result<usize, VectorStoreError> {
        self.backend.count().await
    }

    /// Distinct source files that have at least one vector in the store.
    ///
    /// The "actual" side of the coverage check — see
    /// [`VectorStoreBackend::indexed_file_paths`].
    pub async fn indexed_file_paths(&self) -> Result<HashSet<String>, VectorStoreError> {
        self.backend.indexed_file_paths().await
    }

    /// Clear all vectors (keep collection/table structure)
    pub async fn clear_collection(&self) -> Result<(), VectorStoreError> {
        self.backend.clear().await
    }

    /// Reclaim disk space left behind by writes — see
    /// [`VectorStoreBackend::maintain`].
    pub async fn maintain(
        &self,
        prune_keep: std::time::Duration,
    ) -> Result<VectorStoreMaintenance, VectorStoreError> {
        self.backend.maintain(prune_keep).await
    }

    /// Delete the collection (alias for clear_collection for backward compatibility)
    ///
    /// Note: For LanceDB, this clears the table rather than deleting it entirely.
    pub async fn delete_collection(&self) -> Result<(), VectorStoreError> {
        self.backend.clear().await
    }

    /// Check if the backend is healthy/connected
    pub async fn health_check(&self) -> Result<(), VectorStoreError> {
        self.backend.health_check().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunker::ChunkContext;
    use tempfile::TempDir;

    fn create_test_chunk(id: ChunkId, content: &str) -> CodeChunk {
        CodeChunk {
            id,
            content: content.to_string(),
            context: ChunkContext {
                file_path: PathBuf::from("test.rs"),
                module_path: vec!["crate".to_string()],
                symbol_name: "test_function".to_string(),
                symbol_kind: "function".to_string(),
                docstring: Some("A test function".to_string()),
                imports: vec!["std::collections::HashMap".to_string()],
                outgoing_calls: vec!["helper_function".to_string()],
                parent_symbol_name: None,
                split_part: None,
                split_total: None,
                line_start: 10,
                line_end: 20,
            },
        }
    }

    #[tokio::test]
    async fn test_embedded_vector_store() {
        let temp_dir = TempDir::new().unwrap();
        let store =
            VectorStore::new_embedded(temp_dir.path().to_path_buf(), 4, "test-embedder:v1")
                .await
                .unwrap();

        // Test basic operations
        let chunk_id = ChunkId::new();
        let chunk = create_test_chunk(chunk_id, "fn test() {}");
        let embedding = vec![0.1, 0.2, 0.3, 0.4];

        store
            .insert_chunks(vec![(chunk_id, embedding.clone(), chunk)])
            .await
            .unwrap();

        let count = store.count().await.unwrap();
        assert_eq!(count, 1);

        // Search
        let results = store.search(embedding, 5).await.unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].chunk_id, chunk_id);

        // Clear
        store.clear_collection().await.unwrap();
        assert_eq!(store.count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_open_existing_embedded_vector_store() {
        let temp_dir = TempDir::new().unwrap();
        VectorStore::new_embedded(temp_dir.path().to_path_buf(), 4, "test-embedder:v1")
            .await
            .unwrap();

        let store =
            VectorStore::open_existing_embedded(
                temp_dir.path().to_path_buf(),
                4,
                "test-embedder:v1",
            )
            .await
            .unwrap();

        assert_eq!(store.count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_default_config() {
        let config = VectorStoreConfig::default();
        match config {
            VectorStoreConfig::Embedded {
                path,
                vector_size,
                embedder_identity,
            } => {
                assert!(path.to_string_lossy().contains("vectors"));
                assert_eq!(vector_size, 1024); // Qwen3-Embedding-0.6B
                assert!(embedder_identity.contains("Qwen3-Embedding-0.6B"));
            }
        }
    }
}
