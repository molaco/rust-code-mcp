//! Vector database integration using Qdrant
//!
//! Provides vector search capabilities for code chunks using embeddings

pub mod config;

use crate::chunker::{ChunkId, CodeChunk};
use crate::embeddings::Embedding;
pub use config::{estimate_codebase_size, QdrantOptimizedConfig};
use qdrant_client::qdrant::vectors_config::Config;
use qdrant_client::qdrant::{
    CreateCollection, Distance, PointStruct, SearchPoints, VectorParams, VectorsConfig,
};
use qdrant_client::Qdrant as QdrantClient;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

/// Configuration for the vector store
#[derive(Debug, Clone)]
pub struct VectorStoreConfig {
    /// Qdrant server URL (e.g., "http://localhost:6333")
    pub url: String,
    /// Collection name
    pub collection_name: String,
    /// Vector dimensions (should match embedding model)
    pub vector_size: usize,
}

impl Default for VectorStoreConfig {
    fn default() -> Self {
        // Generate collection name from project directory to avoid mixing codebases
        let collection_name = std::env::current_dir()
            .ok()
            .and_then(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| {
                        // Sanitize name: replace hyphens and invalid chars with underscores
                        let sanitized = name.replace('-', "_").replace(' ', "_");
                        format!("code_chunks_{}", sanitized)
                    })
            })
            .unwrap_or_else(|| "code_chunks_default".to_string());

        Self {
            url: "http://localhost:6334".to_string(),  // gRPC port
            collection_name,
            vector_size: 384, // all-MiniLM-L6-v2
        }
    }
}

/// Vector database client for code search
#[derive(Clone)]
pub struct VectorStore {
    client: Arc<QdrantClient>,
    collection_name: String,
    vector_size: usize,
}

/// A search result from vector search
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub chunk_id: ChunkId,
    pub score: f32,
    pub chunk: CodeChunk,
}

impl VectorStore {
    /// Create a new vector store client
    pub async fn new(config: VectorStoreConfig) -> Result<Self, Box<dyn std::error::Error + Send>> {
        Self::new_with_optimization(config, None).await
    }

    /// Create a new vector store client with optimized HNSW configuration
    pub async fn new_with_optimization(
        config: VectorStoreConfig,
        optimized_config: Option<QdrantOptimizedConfig>,
    ) -> Result<Self, Box<dyn std::error::Error + Send>> {
        let client = QdrantClient::from_url(&config.url).build()
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;

        let mut store = Self {
            client: Arc::new(client),
            collection_name: config.collection_name,
            vector_size: config.vector_size,
        };

        // Ensure collection exists
        store.create_collection_if_not_exists(optimized_config).await?;

        Ok(store)
    }

    /// Check if collection exists and create it if not
    async fn create_collection_if_not_exists(
        &mut self,
        optimized_config: Option<QdrantOptimizedConfig>,
    ) -> Result<(), Box<dyn std::error::Error + Send>> {
        // Check if collection exists
        let collections = self.client.list_collections().await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;
        let exists = collections
            .collections
            .iter()
            .any(|c| c.name == self.collection_name);

        if exists {
            return Ok(());
        }

        // Use optimized config if provided, otherwise use defaults
        let (hnsw_config, optimizers_config) = if let Some(opt_config) = optimized_config {
            tracing::info!(
                "Creating collection '{}' with optimized HNSW config: m={}, ef_construct={}",
                self.collection_name,
                opt_config.hnsw_m,
                opt_config.hnsw_ef_construct
            );
            (
                Some(opt_config.hnsw_config()),
                Some(opt_config.optimizer_config()),
            )
        } else {
            // Default configuration (small codebase)
            tracing::info!(
                "Creating collection '{}' with default configuration",
                self.collection_name
            );
            (
                Some(qdrant_client::qdrant::HnswConfigDiff {
                    m: Some(16),
                    ef_construct: Some(100),
                    full_scan_threshold: Some(10000),
                    max_indexing_threads: Some(4),  // Reasonable default instead of 0 (auto)
                    on_disk: None,
                    payload_m: None,
                }),
                Some(qdrant_client::qdrant::OptimizersConfigDiff {
                    deleted_threshold: Some(0.2),
                    vacuum_min_vector_number: Some(1000),
                    default_segment_number: Some(0),
                    max_segment_size: None,
                    memmap_threshold: Some(50000),
                    indexing_threshold: Some(10000),
                    flush_interval_sec: Some(5),
                    max_optimization_threads: None,
                    deprecated_max_optimization_threads: None,
                }),
            )
        };

        // Create collection with optimal configuration for code search
        let create_collection = CreateCollection {
            collection_name: self.collection_name.clone(),
            vectors_config: Some(VectorsConfig {
                config: Some(Config::Params(VectorParams {
                    size: self.vector_size as u64,
                    distance: Distance::Cosine.into(),
                    on_disk: None,
                    hnsw_config: None,
                    quantization_config: None,
                    datatype: None,
                    multivector_config: None,
                })),
            }),
            hnsw_config,
            optimizers_config,
            ..Default::default()
        };

        self.client.create_collection(create_collection).await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;
        Ok(())
    }

    /// Index a batch of chunks with their embeddings
    pub async fn upsert_chunks(
        &self,
        chunks_with_embeddings: Vec<(ChunkId, Embedding, CodeChunk)>,
    ) -> Result<(), Box<dyn std::error::Error + Send>> {
        if chunks_with_embeddings.is_empty() {
            return Ok(());
        }

        // Convert to Qdrant points
        let points: Vec<PointStruct> = chunks_with_embeddings
            .into_iter()
            .map(|(chunk_id, embedding, chunk)| {
                // Create payload with all chunk metadata
                let mut payload: HashMap<String, qdrant_client::qdrant::Value> = HashMap::new();

                // Store the full chunk as JSON for retrieval
                if let Ok(chunk_json) = serde_json::to_value(&chunk) {
                    if let serde_json::Value::Object(obj) = chunk_json {
                        for (key, value) in obj {
                            // Convert serde_json::Value to qdrant Value
                            if let Ok(json_str) = serde_json::to_string(&value) {
                                let qdrant_value = qdrant_client::qdrant::Value {
                                    kind: Some(qdrant_client::qdrant::value::Kind::StringValue(json_str)),
                                };
                                payload.insert(key, qdrant_value);
                            }
                        }
                    }
                }

                PointStruct {
                    id: Some(chunk_id.to_string().into()),
                    vectors: Some(embedding.into()),
                    payload,
                }
            })
            .collect();

        // Upsert in batches of 500 (optimal for Qdrant performance)
        for batch in points.chunks(500) {
            let upsert_points = qdrant_client::qdrant::UpsertPoints {
                collection_name: self.collection_name.clone(),
                points: batch.to_vec(),
                ..Default::default()
            };
            self.client.upsert_points(upsert_points).await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;
        }

        Ok(())
    }

    /// Search for similar chunks using a query vector
    pub async fn search(
        &self,
        query_vector: Embedding,
        limit: usize,
    ) -> Result<Vec<SearchResult>, Box<dyn std::error::Error + Send>> {
        let search_points = SearchPoints {
            collection_name: self.collection_name.clone(),
            vector: query_vector,
            limit: limit as u64,
            with_payload: Some(true.into()),
            ..Default::default()
        };

        let response = self.client.search_points(search_points).await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;

        // Convert Qdrant results to SearchResult
        let results: Result<Vec<SearchResult>, Box<dyn std::error::Error + Send>> = response
            .result
            .into_iter()
            .map(|point| {
                // Extract chunk_id
                let chunk_id_str = match &point.id {
                    Some(id) => match id.point_id_options.as_ref().unwrap() {
                        qdrant_client::qdrant::point_id::PointIdOptions::Uuid(uuid) => uuid.clone(),
                        qdrant_client::qdrant::point_id::PointIdOptions::Num(_) => {
                            return Err(Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, "Expected UUID point ID")) as Box<dyn std::error::Error + Send>)
                        }
                    },
                    None => return Err(Box::new(std::io::Error::new(std::io::ErrorKind::NotFound, "Missing point ID")) as Box<dyn std::error::Error + Send>),
                };

                let chunk_id = ChunkId::from_string(&chunk_id_str)
                    .map_err(|e| Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{:?}", e))) as Box<dyn std::error::Error + Send>)?;

                // Deserialize chunk from payload
                // Convert Qdrant values back to serde_json::Value
                let mut json_map = serde_json::Map::new();
                for (key, value) in point.payload {
                    if let Some(qdrant_client::qdrant::value::Kind::StringValue(s)) = value.kind {
                        // Parse the JSON string back to value
                        if let Ok(parsed_value) = serde_json::from_str(&s) {
                            json_map.insert(key, parsed_value);
                        }
                    }
                }

                let chunk: CodeChunk = serde_json::from_value(serde_json::Value::Object(json_map))
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;

                Ok(SearchResult {
                    chunk_id,
                    score: point.score,
                    chunk,
                })
            })
            .collect();

        results
    }

    /// Delete chunks by their IDs
    pub async fn delete_chunks(
        &self,
        chunk_ids: Vec<ChunkId>,
    ) -> Result<(), Box<dyn std::error::Error + Send>> {
        if chunk_ids.is_empty() {
            return Ok(());
        }

        let point_ids: Vec<qdrant_client::qdrant::PointId> = chunk_ids
            .into_iter()
            .map(|id| id.to_string().into())
            .collect();

        let delete_points = qdrant_client::qdrant::DeletePoints {
            collection_name: self.collection_name.clone(),
            points: Some(qdrant_client::qdrant::PointsSelector {
                points_selector_one_of: Some(
                    qdrant_client::qdrant::points_selector::PointsSelectorOneOf::Points(
                        qdrant_client::qdrant::PointsIdsList {
                            ids: point_ids,
                        },
                    ),
                ),
            }),
            ..Default::default()
        };

        self.client.delete_points(delete_points).await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;

        Ok(())
    }

    /// Delete all chunks from a specific file
    ///
    /// This is needed for incremental indexing when files are modified or deleted
    pub async fn delete_by_file_path(
        &self,
        file_path: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send>> {
        use qdrant_client::qdrant::{Condition, Filter};

        // Create a filter for the file_path field in payload
        let filter = Filter {
            must: vec![Condition {
                condition_one_of: Some(
                    qdrant_client::qdrant::condition::ConditionOneOf::Field(
                        qdrant_client::qdrant::FieldCondition {
                            key: "context.file_path".to_string(), // JSON path to file_path
                            r#match: Some(qdrant_client::qdrant::Match {
                                match_value: Some(
                                    qdrant_client::qdrant::r#match::MatchValue::Text(
                                        file_path.to_string(),
                                    ),
                                ),
                            }),
                            ..Default::default()
                        },
                    ),
                ),
            }],
            ..Default::default()
        };

        let delete_points = qdrant_client::qdrant::DeletePoints {
            collection_name: self.collection_name.clone(),
            points: Some(qdrant_client::qdrant::PointsSelector {
                points_selector_one_of: Some(
                    qdrant_client::qdrant::points_selector::PointsSelectorOneOf::Filter(filter),
                ),
            }),
            ..Default::default()
        };

        self.client
            .delete_points(delete_points)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;

        Ok(())
    }

    /// Get the total number of points in the collection
    pub async fn count(&self) -> Result<usize, Box<dyn std::error::Error + Send>> {
        let info = self
            .client
            .collection_info(&self.collection_name)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;
        Ok(info.result.and_then(|r| r.points_count).unwrap_or(0) as usize)
    }

    /// Delete the entire collection
    pub async fn delete_collection(&self) -> Result<(), Box<dyn std::error::Error + Send>> {
        self.client.delete_collection(&self.collection_name).await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;
        Ok(())
    }

    /// Clear all points from the collection (without deleting the collection itself)
    ///
    /// This is useful for force reindexing while preserving collection configuration
    pub async fn clear_collection(&self) -> Result<(), Box<dyn std::error::Error + Send>> {
        use qdrant_client::qdrant::Filter;

        // Create a filter that matches all points (using a filter that always evaluates to true)
        // We delete all points by not specifying any filter conditions, which matches everything
        let delete_points = qdrant_client::qdrant::DeletePoints {
            collection_name: self.collection_name.clone(),
            points: Some(qdrant_client::qdrant::PointsSelector {
                points_selector_one_of: Some(
                    qdrant_client::qdrant::points_selector::PointsSelectorOneOf::Filter(
                        Filter::default() // Empty filter matches all points
                    ),
                ),
            }),
            ..Default::default()
        };

        self.client
            .delete_points(delete_points)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunker::{ChunkContext, CodeChunk};
    use crate::embeddings::EmbeddingGenerator;
    use std::path::PathBuf;

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
    #[ignore] // Requires running Qdrant server
    async fn test_vector_store_creation() {
        let config = VectorStoreConfig::default();
        let store = VectorStore::new(config).await;
        assert!(store.is_ok());
    }

    #[tokio::test]
    #[ignore] // Requires running Qdrant server and embedding model
    async fn test_upsert_and_search() {
        let config = VectorStoreConfig {
            collection_name: "test_collection".to_string(),
            ..Default::default()
        };

        let store = VectorStore::new(config).await.unwrap();

        // Create test data
        let chunk_id = ChunkId::new();
        let chunk = create_test_chunk(chunk_id, "fn test() { println!(\"hello\"); }");

        // Generate embedding
        let generator = EmbeddingGenerator::new().unwrap();
        let formatted = chunk.format_for_embedding();
        let embedding = generator.embed(&formatted).unwrap();

        // Upsert
        store
            .upsert_chunks(vec![(chunk_id, embedding.clone(), chunk.clone())])
            .await
            .unwrap();

        // Search
        let results = store.search(embedding, 5).await.unwrap();

        assert!(!results.is_empty());
        assert_eq!(results[0].chunk_id, chunk_id);
        assert!(results[0].score > 0.9); // Should be very similar to itself

        // Cleanup
        store.delete_collection().await.unwrap();
    }

    #[tokio::test]
    #[ignore] // Requires running Qdrant server
    async fn test_delete_chunks() {
        let config = VectorStoreConfig {
            collection_name: "test_delete_collection".to_string(),
            ..Default::default()
        };

        let store = VectorStore::new(config).await.unwrap();

        // Create and insert test data
        let chunk_id = ChunkId::new();
        let chunk = create_test_chunk(chunk_id, "fn delete_test() {}");
        let embedding = vec![0.1; 384]; // Dummy embedding

        store
            .upsert_chunks(vec![(chunk_id, embedding.clone(), chunk)])
            .await
            .unwrap();

        // Verify it exists
        let count_before = store.count().await.unwrap();
        assert!(count_before > 0);

        // Delete
        store.delete_chunks(vec![chunk_id]).await.unwrap();

        // Verify deletion
        // Note: Count might not immediately reflect deletion due to async processing
        let results = store.search(embedding, 10).await.unwrap();
        assert!(!results.iter().any(|r| r.chunk_id == chunk_id));

        // Cleanup
        store.delete_collection().await.unwrap();
    }

    #[test]
    fn test_chunk_serialization() {
        let chunk_id = ChunkId::new();
        let chunk = create_test_chunk(chunk_id, "fn serialize_test() {}");

        // Serialize
        let json = serde_json::to_string(&chunk).unwrap();
        assert!(json.contains("serialize_test"));

        // Deserialize
        let deserialized: CodeChunk = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, chunk.id);
        assert_eq!(deserialized.content, chunk.content);
        assert_eq!(
            deserialized.context.symbol_name,
            chunk.context.symbol_name
        );
    }
}
