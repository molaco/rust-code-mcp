//! Vector database integration with swappable backends
//!
//! Provides vector search capabilities for code chunks using embeddings.
//! Supports multiple backends:
//! - LanceDB (default): Embedded, zero-config
//! - Qdrant (optional): Remote server, feature-gated

#[cfg(feature = "qdrant")]
pub mod config;
pub mod error;
pub mod lancedb;
pub mod traits;

#[cfg(feature = "qdrant")]
pub mod qdrant;

// Re-exports
pub use error::VectorStoreError;
pub use lancedb::LanceDbBackend;
pub use traits::VectorStoreBackend;

#[cfg(feature = "qdrant")]
pub use qdrant::{QdrantBackend, QdrantConfig};

#[cfg(feature = "qdrant")]
pub use config::{estimate_codebase_size, QdrantOptimizedConfig};

use crate::chunker::{ChunkId, CodeChunk};
use crate::embeddings::Embedding;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

/// A search result from vector search
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
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
        /// Vector dimensions (384 for all-MiniLM-L6-v2)
        vector_size: usize,
    },
    /// Remote Qdrant server
    #[cfg(feature = "qdrant")]
    Qdrant {
        /// Server URL
        url: String,
        /// Collection name
        collection_name: String,
        /// Vector dimensions
        vector_size: usize,
        /// Optional HNSW optimization config
        optimized_config: Option<QdrantOptimizedConfig>,
    },
}

impl Default for VectorStoreConfig {
    fn default() -> Self {
        // Use XDG cache directory
        let cache_dir = directories::ProjectDirs::from("", "", "rust-code-mcp")
            .map(|dirs| dirs.cache_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from(".cache/rust-code-mcp"));

        Self::Embedded {
            path: cache_dir.join("vectors"),
            vector_size: 384, // all-MiniLM-L6-v2
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
    /// Create with default embedded backend (LanceDB)
    pub async fn new_embedded(path: PathBuf, vector_size: usize) -> Result<Self, VectorStoreError> {
        let backend = LanceDbBackend::new(path, vector_size).await?;
        Ok(Self {
            backend: Arc::new(backend),
        })
    }

    /// Create with Qdrant backend
    #[cfg(feature = "qdrant")]
    pub async fn new_qdrant(config: QdrantConfig) -> Result<Self, VectorStoreError> {
        let backend = QdrantBackend::new(config).await?;
        Ok(Self {
            backend: Arc::new(backend),
        })
    }

    /// Create with Qdrant backend with optimization
    #[cfg(feature = "qdrant")]
    pub async fn new_qdrant_optimized(
        config: QdrantConfig,
        optimized_config: Option<QdrantOptimizedConfig>,
    ) -> Result<Self, VectorStoreError> {
        let backend = QdrantBackend::new_with_optimization(config, optimized_config).await?;
        Ok(Self {
            backend: Arc::new(backend),
        })
    }

    /// Create from config
    pub async fn from_config(config: VectorStoreConfig) -> Result<Self, VectorStoreError> {
        match config {
            VectorStoreConfig::Embedded { path, vector_size } => {
                Self::new_embedded(path, vector_size).await
            }
            #[cfg(feature = "qdrant")]
            VectorStoreConfig::Qdrant {
                url,
                collection_name,
                vector_size,
                optimized_config,
            } => {
                let qdrant_config = QdrantConfig {
                    url,
                    collection_name,
                    vector_size,
                };
                Self::new_qdrant_optimized(qdrant_config, optimized_config).await
            }
        }
    }

    /// Create with default configuration
    pub async fn new_default() -> Result<Self, VectorStoreError> {
        Self::from_config(VectorStoreConfig::default()).await
    }

    // Delegate all methods to backend

    /// Insert or update chunks with their embeddings
    pub async fn upsert_chunks(
        &self,
        chunks_with_embeddings: Vec<(ChunkId, Embedding, CodeChunk)>,
    ) -> Result<(), VectorStoreError> {
        self.backend.upsert_chunks(chunks_with_embeddings).await
    }

    /// Search for similar chunks using a query vector
    pub async fn search(
        &self,
        query_vector: Embedding,
        limit: usize,
    ) -> Result<Vec<SearchResult>, VectorStoreError> {
        self.backend.search(query_vector, limit).await
    }

    /// Delete chunks by their IDs
    pub async fn delete_chunks(&self, chunk_ids: Vec<ChunkId>) -> Result<(), VectorStoreError> {
        self.backend.delete_chunks(chunk_ids).await
    }

    /// Delete all chunks from a specific file path
    pub async fn delete_by_file_path(&self, file_path: &str) -> Result<(), VectorStoreError> {
        self.backend.delete_by_file_path(file_path).await
    }

    /// Get the total number of vectors in the store
    pub async fn count(&self) -> Result<usize, VectorStoreError> {
        self.backend.count().await
    }

    /// Clear all vectors (keep collection/table structure)
    pub async fn clear_collection(&self) -> Result<(), VectorStoreError> {
        self.backend.clear().await
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

// Backward compatibility: Keep the old API signature for gradual migration
// These will be removed in a future version

/// Legacy Qdrant configuration (for backward compatibility)
///
/// DEPRECATED: Use `QdrantConfig` instead
#[cfg(feature = "qdrant")]
pub type LegacyVectorStoreConfig = QdrantConfig;

#[cfg(feature = "qdrant")]
impl VectorStore {
    /// Legacy constructor for backward compatibility
    ///
    /// DEPRECATED: Use `new_qdrant` or `new_embedded` instead
    pub async fn new(config: QdrantConfig) -> Result<Self, Box<dyn std::error::Error + Send>> {
        Self::new_qdrant(config)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)
    }

    /// Legacy constructor with optimization for backward compatibility
    ///
    /// DEPRECATED: Use `new_qdrant_optimized` instead
    pub async fn new_with_optimization(
        config: QdrantConfig,
        optimized_config: Option<QdrantOptimizedConfig>,
    ) -> Result<Self, Box<dyn std::error::Error + Send>> {
        Self::new_qdrant_optimized(config, optimized_config)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)
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
                line_start: 10,
                line_end: 20,
            },
            overlap_prev: None,
            overlap_next: None,
        }
    }

    #[tokio::test]
    async fn test_embedded_vector_store() {
        let temp_dir = TempDir::new().unwrap();
        let store = VectorStore::new_embedded(temp_dir.path().to_path_buf(), 4)
            .await
            .unwrap();

        // Test basic operations
        let chunk_id = ChunkId::new();
        let chunk = create_test_chunk(chunk_id, "fn test() {}");
        let embedding = vec![0.1, 0.2, 0.3, 0.4];

        store
            .upsert_chunks(vec![(chunk_id, embedding.clone(), chunk)])
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
    async fn test_default_config() {
        let config = VectorStoreConfig::default();
        match config {
            VectorStoreConfig::Embedded { path, vector_size } => {
                assert!(path.to_string_lossy().contains("vectors"));
                assert_eq!(vector_size, 384);
            }
            #[cfg(feature = "qdrant")]
            _ => panic!("Expected embedded config as default"),
        }
    }
}
