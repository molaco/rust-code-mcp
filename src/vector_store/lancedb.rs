//! LanceDB backend for vector storage
//!
//! Embedded vector database using Apache Arrow for columnar storage.
//! No external server required - direct file access.

use async_trait::async_trait;
use arrow_array::{
    Array, FixedSizeListArray, Float32Array, RecordBatch, RecordBatchIterator, StringArray,
};
use arrow_schema::{DataType, Field, Schema};
use futures::TryStreamExt;
use lancedb::connect;
use lancedb::index::scalar::BTreeIndexBuilder;
use lancedb::index::Index;
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::DistanceType;
use std::path::PathBuf;
use std::sync::Arc;

use crate::chunker::{ChunkId, CodeChunk};
use crate::embeddings::Embedding;
use super::error::VectorStoreError;
use super::traits::VectorStoreBackend;
use super::SearchResult;

const TABLE_NAME: &str = "vectors";

/// LanceDB backend for embedded vector storage
pub struct LanceDbBackend {
    db: lancedb::Connection,
    table_name: String,
    vector_dim: usize,
    /// Cached Arrow schema to avoid recreation on every batch
    schema: Arc<Schema>,
}

impl LanceDbBackend {
    /// Create a new LanceDB backend
    ///
    /// # Arguments
    /// * `path` - Directory for database storage
    /// * `vector_dim` - Embedding dimension (384 for all-MiniLM-L6-v2)
    pub async fn new(path: PathBuf, vector_dim: usize) -> Result<Self, VectorStoreError> {
        // Ensure directory exists
        std::fs::create_dir_all(&path).map_err(|e| {
            VectorStoreError::connection(format!("Failed to create directory: {}", e))
        })?;

        let db = connect(path.to_string_lossy().as_ref())
            .execute()
            .await
            .map_err(|e| VectorStoreError::connection(format!("Failed to connect: {}", e)))?;

        // Create and cache schema once
        let schema = Arc::new(Self::create_schema_for_dim(vector_dim));

        let backend = Self {
            db,
            table_name: TABLE_NAME.to_string(),
            vector_dim,
            schema,
        };

        // Ensure table exists
        backend.ensure_table_exists().await?;

        Ok(backend)
    }

    /// Create Arrow schema for vectors table (static version for initialization)
    fn create_schema_for_dim(vector_dim: usize) -> Schema {
        Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new(
                "vector",
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, true)),
                    vector_dim as i32,
                ),
                false,
            ),
            Field::new("chunk_json", DataType::Utf8, false),
            Field::new("file_path", DataType::Utf8, false),
        ])
    }

    /// Get cached Arrow schema for vectors table
    fn schema(&self) -> Arc<Schema> {
        self.schema.clone()
    }

    /// Ensure the vectors table exists
    async fn ensure_table_exists(&self) -> Result<(), VectorStoreError> {
        let tables = self
            .db
            .table_names()
            .execute()
            .await
            .map_err(|e| VectorStoreError::backend(format!("Failed to list tables: {}", e)))?;

        if !tables.contains(&self.table_name) {
            // Create empty table with cached schema
            let schema = self.schema();

            // Create empty arrays for initial table
            let id_array = StringArray::from(Vec::<String>::new());
            let vector_array = self.create_empty_vector_array();
            let chunk_json_array = StringArray::from(Vec::<String>::new());
            let file_path_array = StringArray::from(Vec::<String>::new());

            let batch = RecordBatch::try_new(
                schema.clone(),
                vec![
                    Arc::new(id_array),
                    Arc::new(vector_array),
                    Arc::new(chunk_json_array),
                    Arc::new(file_path_array),
                ],
            )
            .map_err(|e| VectorStoreError::backend(format!("Failed to create batch: {}", e)))?;

            let batches = RecordBatchIterator::new(vec![Ok(batch)], schema);

            let table = self.db
                .create_table(&self.table_name, Box::new(batches))
                .execute()
                .await
                .map_err(|e| VectorStoreError::backend(format!("Failed to create table: {}", e)))?;

            tracing::info!("Created LanceDB table: {}", self.table_name);

            // Create BTree index on id column for fast merge_insert lookups
            table
                .create_index(&["id"], Index::BTree(BTreeIndexBuilder::default()))
                .execute()
                .await
                .map_err(|e| {
                    VectorStoreError::backend(format!("Failed to create id index: {}", e))
                })?;

            tracing::info!("Created BTree index on 'id' column for fast upserts");
        }

        Ok(())
    }

    /// Create empty vector array for schema initialization
    fn create_empty_vector_array(&self) -> FixedSizeListArray {
        let values = Float32Array::from(Vec::<f32>::new());
        let field = Arc::new(Field::new("item", DataType::Float32, true));
        FixedSizeListArray::try_new(field, self.vector_dim as i32, Arc::new(values), None).unwrap()
    }

    /// Convert chunks to Arrow RecordBatch
    fn chunks_to_batch(
        &self,
        chunks: &[(ChunkId, Embedding, CodeChunk)],
    ) -> Result<RecordBatch, VectorStoreError> {
        let schema = self.schema();

        // Extract IDs
        let ids: Vec<String> = chunks.iter().map(|(id, _, _)| id.to_string()).collect();
        let id_array = StringArray::from(ids);

        // Extract vectors - flatten all vectors into a single array
        let flat_vectors: Vec<f32> = chunks
            .iter()
            .flat_map(|(_, embedding, _)| embedding.iter().copied())
            .collect();
        let values = Float32Array::from(flat_vectors);
        let field = Arc::new(Field::new("item", DataType::Float32, true));
        let vector_array =
            FixedSizeListArray::try_new(field, self.vector_dim as i32, Arc::new(values), None)
                .map_err(|e| {
                    VectorStoreError::serialization(format!("Failed to create vector array: {}", e))
                })?;

        // Serialize chunks to JSON
        let chunk_jsons: Result<Vec<String>, _> = chunks
            .iter()
            .map(|(_, _, chunk)| {
                serde_json::to_string(chunk).map_err(|e| {
                    VectorStoreError::serialization(format!("Failed to serialize chunk: {}", e))
                })
            })
            .collect();
        let chunk_json_array = StringArray::from(chunk_jsons?);

        // Extract file paths
        let file_paths: Vec<String> = chunks
            .iter()
            .map(|(_, _, chunk)| chunk.context.file_path.display().to_string())
            .collect();
        let file_path_array = StringArray::from(file_paths);

        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(id_array),
                Arc::new(vector_array),
                Arc::new(chunk_json_array),
                Arc::new(file_path_array),
            ],
        )
        .map_err(|e| VectorStoreError::backend(format!("Failed to create batch: {}", e)))
    }

    /// Get table, returns error if not exists
    async fn get_table(&self) -> Result<lancedb::Table, VectorStoreError> {
        self.db
            .open_table(&self.table_name)
            .execute()
            .await
            .map_err(|e| VectorStoreError::not_found(format!("Table not found: {}", e)))
    }
}

#[async_trait]
impl VectorStoreBackend for LanceDbBackend {
    async fn upsert_chunks(
        &self,
        chunks_with_embeddings: Vec<(ChunkId, Embedding, CodeChunk)>,
    ) -> Result<(), VectorStoreError> {
        if chunks_with_embeddings.is_empty() {
            return Ok(());
        }

        let num_chunks = chunks_with_embeddings.len();
        let table = self.get_table().await?;
        let batch = self.chunks_to_batch(&chunks_with_embeddings)?;
        let schema = batch.schema();
        let batches = RecordBatchIterator::new(vec![Ok(batch)], schema);

        // Use native merge_insert for atomic upsert (replaces delete-then-add pattern)
        // - when_matched_update_all: update existing rows with matching id
        // - when_not_matched_insert_all: insert new rows
        // This is a single atomic operation vs. two separate delete + add operations
        let mut merge_builder = table.merge_insert(&["id"]);
        merge_builder
            .when_matched_update_all(None)
            .when_not_matched_insert_all();
        merge_builder
            .execute(Box::new(batches))
            .await
            .map_err(|e| VectorStoreError::backend(format!("Failed to upsert chunks: {}", e)))?;

        tracing::debug!("Upserted {} chunks to LanceDB via merge_insert", num_chunks);
        Ok(())
    }

    async fn search(
        &self,
        query_vector: Embedding,
        limit: usize,
    ) -> Result<Vec<SearchResult>, VectorStoreError> {
        let table = self.get_table().await?;

        let results = table
            .vector_search(query_vector)
            .map_err(|e| VectorStoreError::query(format!("Failed to create search: {}", e)))?
            .distance_type(DistanceType::Cosine)
            .limit(limit)
            .execute()
            .await
            .map_err(|e| VectorStoreError::query(format!("Search failed: {}", e)))?;

        let batches: Vec<RecordBatch> = results
            .try_collect()
            .await
            .map_err(|e| VectorStoreError::query(format!("Failed to collect results: {}", e)))?;

        let mut search_results = Vec::new();

        for batch in batches {
            let id_col = batch
                .column_by_name("id")
                .ok_or_else(|| VectorStoreError::query("Missing id column"))?;
            let id_array = id_col
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| VectorStoreError::query("Invalid id column type"))?;

            let chunk_json_col = batch
                .column_by_name("chunk_json")
                .ok_or_else(|| VectorStoreError::query("Missing chunk_json column"))?;
            let chunk_json_array = chunk_json_col
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| VectorStoreError::query("Invalid chunk_json column type"))?;

            // LanceDB adds _distance column for search results
            let distance_col = batch
                .column_by_name("_distance")
                .ok_or_else(|| VectorStoreError::query("Missing _distance column"))?;
            let distance_array = distance_col
                .as_any()
                .downcast_ref::<Float32Array>()
                .ok_or_else(|| VectorStoreError::query("Invalid _distance column type"))?;

            for i in 0..batch.num_rows() {
                let id_str = id_array.value(i);
                let chunk_json = chunk_json_array.value(i);
                let distance = distance_array.value(i);

                let chunk_id = ChunkId::from_string(id_str).map_err(|e| {
                    VectorStoreError::serialization(format!("Invalid chunk ID: {:?}", e))
                })?;

                let chunk: CodeChunk = serde_json::from_str(chunk_json).map_err(|e| {
                    VectorStoreError::serialization(format!("Failed to deserialize chunk: {}", e))
                })?;

                // Convert cosine distance to similarity score
                // Cosine distance range is [0, 2], where 0 = identical, 2 = opposite
                // Convert to similarity [0, 1] (standard cosine similarity range)
                let score = 1.0 - (distance / 2.0);

                search_results.push(SearchResult {
                    chunk_id,
                    score,
                    chunk,
                });
            }
        }

        Ok(search_results)
    }

    async fn delete_chunks(&self, chunk_ids: Vec<ChunkId>) -> Result<(), VectorStoreError> {
        if chunk_ids.is_empty() {
            return Ok(());
        }

        let table = self.get_table().await?;

        // Build filter for deletion
        let ids: Vec<String> = chunk_ids.iter().map(|id| format!("'{}'", id.to_string())).collect();
        let filter = format!("id IN ({})", ids.join(", "));

        table
            .delete(&filter)
            .await
            .map_err(|e| VectorStoreError::backend(format!("Failed to delete chunks: {}", e)))?;

        tracing::debug!("Deleted {} chunks from LanceDB", chunk_ids.len());
        Ok(())
    }

    async fn delete_by_file_path(&self, file_path: &str) -> Result<(), VectorStoreError> {
        let table = self.get_table().await?;

        // Escape single quotes in file path
        let escaped_path = file_path.replace('\'', "''");
        let filter = format!("file_path = '{}'", escaped_path);

        table
            .delete(&filter)
            .await
            .map_err(|e| VectorStoreError::backend(format!("Failed to delete by path: {}", e)))?;

        tracing::debug!("Deleted chunks for file: {}", file_path);
        Ok(())
    }

    async fn count(&self) -> Result<usize, VectorStoreError> {
        let table = self.get_table().await?;

        let count = table
            .count_rows(None)
            .await
            .map_err(|e| VectorStoreError::query(format!("Failed to count rows: {}", e)))?;

        Ok(count)
    }

    async fn clear(&self) -> Result<(), VectorStoreError> {
        let table = self.get_table().await?;

        // Delete all rows
        table
            .delete("1=1")
            .await
            .map_err(|e| VectorStoreError::backend(format!("Failed to clear table: {}", e)))?;

        tracing::info!("Cleared all vectors from LanceDB table");
        Ok(())
    }

    async fn health_check(&self) -> Result<(), VectorStoreError> {
        // Try to list tables as a health check
        self.db
            .table_names()
            .execute()
            .await
            .map_err(|e| VectorStoreError::connection(format!("Health check failed: {}", e)))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunker::ChunkContext;
    use tempfile::TempDir;

    fn create_test_chunk(id: ChunkId, content: &str, file_path: &str) -> CodeChunk {
        CodeChunk {
            id,
            content: content.to_string(),
            context: ChunkContext {
                file_path: PathBuf::from(file_path),
                module_path: vec!["crate".to_string()],
                symbol_name: "test_func".to_string(),
                symbol_kind: "function".to_string(),
                docstring: None,
                imports: vec![],
                outgoing_calls: vec![],
                line_start: 1,
                line_end: 10,
            },
            overlap_prev: None,
            overlap_next: None,
        }
    }

    #[tokio::test]
    async fn test_lancedb_creation() {
        let temp_dir = TempDir::new().unwrap();
        let backend = LanceDbBackend::new(temp_dir.path().to_path_buf(), 384).await;
        assert!(backend.is_ok());
    }

    #[tokio::test]
    async fn test_lancedb_upsert_and_count() {
        let temp_dir = TempDir::new().unwrap();
        let backend = LanceDbBackend::new(temp_dir.path().to_path_buf(), 4)
            .await
            .unwrap();

        let chunk_id = ChunkId::new();
        let chunk = create_test_chunk(chunk_id, "fn test() {}", "test.rs");
        let embedding = vec![0.1, 0.2, 0.3, 0.4];

        backend
            .upsert_chunks(vec![(chunk_id, embedding, chunk)])
            .await
            .unwrap();

        let count = backend.count().await.unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_lancedb_delete() {
        let temp_dir = TempDir::new().unwrap();
        let backend = LanceDbBackend::new(temp_dir.path().to_path_buf(), 4)
            .await
            .unwrap();

        let chunk_id = ChunkId::new();
        let chunk = create_test_chunk(chunk_id, "fn test() {}", "test.rs");
        let embedding = vec![0.1, 0.2, 0.3, 0.4];

        backend
            .upsert_chunks(vec![(chunk_id, embedding, chunk)])
            .await
            .unwrap();

        assert_eq!(backend.count().await.unwrap(), 1);

        backend.delete_chunks(vec![chunk_id]).await.unwrap();

        assert_eq!(backend.count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_lancedb_delete_by_file_path() {
        let temp_dir = TempDir::new().unwrap();
        let backend = LanceDbBackend::new(temp_dir.path().to_path_buf(), 4)
            .await
            .unwrap();

        let chunk1_id = ChunkId::new();
        let chunk1 = create_test_chunk(chunk1_id, "fn test1() {}", "file1.rs");

        let chunk2_id = ChunkId::new();
        let chunk2 = create_test_chunk(chunk2_id, "fn test2() {}", "file2.rs");

        backend
            .upsert_chunks(vec![
                (chunk1_id, vec![0.1, 0.2, 0.3, 0.4], chunk1),
                (chunk2_id, vec![0.5, 0.6, 0.7, 0.8], chunk2),
            ])
            .await
            .unwrap();

        assert_eq!(backend.count().await.unwrap(), 2);

        backend.delete_by_file_path("file1.rs").await.unwrap();

        assert_eq!(backend.count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_lancedb_clear() {
        let temp_dir = TempDir::new().unwrap();
        let backend = LanceDbBackend::new(temp_dir.path().to_path_buf(), 4)
            .await
            .unwrap();

        let chunk_id = ChunkId::new();
        let chunk = create_test_chunk(chunk_id, "fn test() {}", "test.rs");
        let embedding = vec![0.1, 0.2, 0.3, 0.4];

        backend
            .upsert_chunks(vec![(chunk_id, embedding, chunk)])
            .await
            .unwrap();

        assert_eq!(backend.count().await.unwrap(), 1);

        backend.clear().await.unwrap();

        assert_eq!(backend.count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_lancedb_health_check() {
        let temp_dir = TempDir::new().unwrap();
        let backend = LanceDbBackend::new(temp_dir.path().to_path_buf(), 4)
            .await
            .unwrap();

        let result = backend.health_check().await;
        assert!(result.is_ok());
    }
}
