//! Unified indexing pipeline that populates both Tantivy (BM25) and vector store
//!
//! This module coordinates indexing operations by delegating to specialized adapters:
//! - TantivyAdapter: BM25 indexing operations
//! - VectorStore: Vector indexing operations (LanceDB embedded backend)
//! - IndexerCore: Core file processing and embedding generation

use crate::chunker::{ChunkId, CodeChunk};
use crate::embeddings::EmbeddingGenerator;
use crate::indexing::errors::{categorize_error, ErrorCollector, ErrorDetail};
use crate::indexing::indexer_core::IndexerCore;
use crate::indexing::tantivy_adapter::TantivyAdapter;
use crate::metrics::IndexingMetrics;
use crate::vector_store::VectorStore;
use anyhow::{Context, Result};
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tantivy::Index;
use walkdir::WalkDir;

/// Statistics from an indexing operation
#[derive(Debug, Clone, Default)]
pub struct IndexStats {
    /// Total number of files found
    pub total_files: usize,
    /// Number of files that were indexed
    pub indexed_files: usize,
    /// Number of files skipped (unchanged)
    pub unchanged_files: usize,
    /// Number of files that failed to index
    pub skipped_files: usize,
    /// Total number of chunks generated
    pub total_chunks: usize,
}

impl IndexStats {
    /// Create stats indicating no changes
    pub fn unchanged() -> Self {
        Self::default()
    }
}

/// Result of indexing a single file
#[derive(Debug)]
pub enum IndexFileResult {
    /// File was indexed successfully with N chunks
    Indexed { chunks_count: usize },
    /// File was unchanged, no reindexing needed
    Unchanged,
    /// File was skipped (error or no chunks)
    Skipped,
}

/// Unified indexer that coordinates Tantivy and vector store operations
pub struct UnifiedIndexer {
    /// Core indexing logic
    core: IndexerCore,
    /// Tantivy adapter for BM25 indexing
    tantivy: TantivyAdapter,
    /// Vector store for semantic indexing (LanceDB)
    vector_store: VectorStore,
    /// Performance metrics
    metrics: IndexingMetrics,
}

impl UnifiedIndexer {
    /// Create a new unified indexer with embedded LanceDB backend
    ///
    /// # Arguments
    /// * `cache_path` - Path to metadata cache directory
    /// * `tantivy_path` - Path to Tantivy index directory
    /// * `collection_name` - Collection/table name for vector store
    /// * `vector_size` - Vector dimensions (384 for all-MiniLM-L6-v2)
    /// * `codebase_loc` - Estimated lines of code (for Tantivy optimization)
    pub async fn for_embedded(
        cache_path: &Path,
        tantivy_path: &Path,
        collection_name: &str,
        vector_size: usize,
        codebase_loc: Option<usize>,
    ) -> Result<Self> {
        tracing::info!("Initializing UnifiedIndexer with embedded LanceDB...");

        // Initialize core
        let core = IndexerCore::new(cache_path, None)?;

        // Initialize Tantivy adapter
        let tantivy_config = crate::config::TantivyConfig::for_codebase_size(tantivy_path, codebase_loc);
        let tantivy = TantivyAdapter::new(tantivy_config)?;

        // Initialize embedded vector store
        let vector_path = cache_path.parent().unwrap_or(cache_path).join("vectors").join(collection_name);
        let vector_store = VectorStore::new_embedded(vector_path, vector_size)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to initialize VectorStore: {}", e))?;

        tracing::info!("UnifiedIndexer initialized successfully with embedded backend");

        Ok(Self {
            core,
            tantivy,
            vector_store,
            metrics: IndexingMetrics::new(),
        })
    }

    /// Index a single file to both Tantivy and vector store
    pub async fn index_file(&mut self, file_path: &Path) -> Result<IndexFileResult> {
        let file_start = Instant::now();

        // Check if file should be processed
        if !self.core.should_process_file(file_path)? {
            return Ok(IndexFileResult::Skipped);
        }

        // Read file content
        let content = std::fs::read_to_string(file_path)
            .context(format!("Failed to read file: {}", file_path.display()))?;

        // Check if file changed
        if !self.core.has_file_changed(file_path, &content)? {
            tracing::debug!("File unchanged: {}", file_path.display());
            return Ok(IndexFileResult::Unchanged);
        }

        tracing::debug!("Indexing changed file: {}", file_path.display());

        // Process file (parse and chunk)
        let processed = self.core.process_file_sync(file_path)?;

        if processed.chunks.is_empty() {
            tracing::warn!("No chunks generated for {}", file_path.display());
            return Ok(IndexFileResult::Skipped);
        }

        tracing::debug!(
            "Generated {} chunks from {}",
            processed.chunks.len(),
            file_path.display()
        );

        // Generate embeddings
        let embeddings = self.core.generate_embeddings_batched(&processed.chunks)?;

        tracing::debug!("Generated {} embeddings", embeddings.len());

        // Index to both stores
        let chunks_count = processed.chunks.len();
        self.tantivy.index_chunks(&processed.chunks)?;

        // Index to vector store
        let chunk_data: Vec<(ChunkId, Vec<f32>, CodeChunk)> = processed
            .chunks
            .into_iter()
            .zip(embeddings.into_iter())
            .map(|(chunk, embedding)| (chunk.id, embedding, chunk))
            .collect();
        self.vector_store.upsert_chunks(chunk_data).await
            .map_err(|e| anyhow::anyhow!("Failed to index chunks to vector store: {}", e))?;

        // Update metadata cache
        self.core.update_file_metadata(file_path, &content)?;

        // Track metrics
        let file_duration = file_start.elapsed();
        self.metrics.file_latencies.push(file_duration);

        self.core.refresh_memory_monitor();
        self.metrics.peak_memory_bytes = self
            .metrics
            .peak_memory_bytes
            .max(self.core.memory_used_bytes());

        tracing::info!(
            "✓ Indexed {} chunks from {} in {:?}",
            chunks_count,
            file_path.display(),
            file_duration
        );

        Ok(IndexFileResult::Indexed { chunks_count })
    }

    /// Index an entire directory
    pub async fn index_directory(&mut self, dir_path: &Path) -> Result<IndexStats> {
        let total_start = Instant::now();
        tracing::info!("Indexing directory: {}", dir_path.display());

        let mut stats = IndexStats::default();
        self.metrics = IndexingMetrics::new();

        // Find all Rust files
        let rust_files = self.collect_rust_files(dir_path, &mut stats)?;

        if rust_files.is_empty() {
            return Ok(stats);
        }

        tracing::info!("Found {} Rust files in {}", rust_files.len(), dir_path.display());

        // Index each file
        for file in rust_files {
            match self.index_file(&file).await {
                Ok(IndexFileResult::Indexed { chunks_count }) => {
                    stats.indexed_files += 1;
                    stats.total_chunks += chunks_count;
                }
                Ok(IndexFileResult::Unchanged) => {
                    stats.unchanged_files += 1;
                }
                Ok(IndexFileResult::Skipped) => {
                    stats.skipped_files += 1;
                }
                Err(e) => {
                    tracing::error!("Failed to index {}: {}", file.display(), e);
                    stats.skipped_files += 1;
                }
            }
        }

        // Commit Tantivy changes
        self.tantivy.commit().context("Failed to commit Tantivy index")?;

        // Finalize metrics
        self.finalize_metrics(&stats, total_start.elapsed());

        tracing::info!(
            "✓ Indexing complete: {} files indexed, {} chunks, {} unchanged, {} skipped",
            stats.indexed_files,
            stats.total_chunks,
            stats.unchanged_files,
            stats.skipped_files
        );

        Ok(stats)
    }

    /// Index an entire directory with automatic backup management
    pub async fn index_directory_with_backup(
        &mut self,
        dir_path: &Path,
        backup_manager: Option<&crate::monitoring::backup::BackupManager>,
    ) -> Result<IndexStats> {
        let stats = self.index_directory(dir_path).await?;

        if let Some(manager) = backup_manager {
            if stats.indexed_files > 0 && stats.indexed_files % 100 == 0 {
                tracing::info!(
                    "Creating Merkle snapshot backup after {} indexed files",
                    stats.indexed_files
                );

                match crate::indexing::merkle::FileSystemMerkle::from_directory(dir_path) {
                    Ok(merkle) => {
                        if let Err(e) = manager.create_backup(&merkle) {
                            tracing::warn!("Failed to create backup: {}", e);
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed to build Merkle tree for backup: {}", e);
                    }
                }
            }
        }

        Ok(stats)
    }

    /// Index an entire directory using parallel processing
    pub async fn index_directory_parallel(&mut self, dir_path: &Path) -> Result<IndexStats> {
        let total_start = Instant::now();
        tracing::info!("Indexing directory (parallel mode): {}", dir_path.display());

        let mut stats = IndexStats::default();
        self.metrics = IndexingMetrics::new();

        // Find all Rust files
        let rust_files = self.collect_rust_files(dir_path, &mut stats)?;

        if rust_files.is_empty() {
            return Ok(stats);
        }

        tracing::info!("Found {} Rust files, processing in parallel", rust_files.len());

        // Calculate safe batch size
        let batch_size = self.core.calculate_safe_batch_size();
        tracing::info!("Using batch size: {}", batch_size);

        // Process in batches
        for (batch_idx, file_batch) in rust_files.chunks(batch_size).enumerate() {
            tracing::info!(
                "Processing batch {}/{} ({} files)",
                batch_idx + 1,
                (rust_files.len() + batch_size - 1) / batch_size,
                file_batch.len()
            );

            // Check memory before batch
            let memory_usage = self.core.memory_usage_percent();
            if memory_usage > 85.0 {
                tracing::warn!(
                    "High memory usage ({:.1}%), pausing to allow GC",
                    memory_usage
                );
                tokio::time::sleep(Duration::from_secs(5)).await;
            }

            // PHASE 1: Parallel parse and chunk (CPU-bound)
            let parse_start = Instant::now();
            let error_collector = ErrorCollector::new();
            let error_collector_clone = error_collector.clone();

            let processed: Vec<_> = file_batch
                .par_iter()
                .filter_map(|file_path| {
                    match self.core.process_file_sync(file_path) {
                        Ok(processed) => {
                            tracing::debug!("Parsed: {}", file_path.display());
                            Some(processed)
                        }
                        Err(e) => {
                            error_collector_clone.record(ErrorDetail {
                                file_path: file_path.clone(),
                                category: categorize_error(&e),
                                message: e.to_string(),
                            });
                            None
                        }
                    }
                })
                .collect();

            let parse_duration = parse_start.elapsed();
            self.metrics.parse_duration += parse_duration;

            tracing::info!(
                "Parsed {} files in {:.2}s ({:.1} files/sec)",
                processed.len(),
                parse_duration.as_secs_f64(),
                processed.len() as f64 / parse_duration.as_secs_f64()
            );

            // Log errors and update stats
            self.process_batch_errors(&error_collector, &mut stats);

            // PHASE 2: Batch embedding for all files
            if !processed.is_empty() {
                self.process_and_index_batch(&processed, &mut stats).await?;
            }

            // Commit after each batch
            self.tantivy.commit()?;
        }

        // Finalize metrics
        self.finalize_metrics(&stats, total_start.elapsed());

        tracing::info!(
            "✓ Parallel indexing complete: {} files indexed, {} chunks, {} skipped",
            stats.indexed_files,
            stats.total_chunks,
            stats.skipped_files
        );

        Ok(stats)
    }

    /// Delete all chunks for a specific file from both Tantivy and vector store
    pub async fn delete_file_chunks(&mut self, file_path: &Path) -> Result<()> {
        self.tantivy.delete_file_chunks(file_path)?;
        let file_path_str = file_path.to_string_lossy().to_string();
        self.vector_store.delete_by_file_path(&file_path_str).await
            .map_err(|e| anyhow::anyhow!("Failed to delete chunks from vector store: {}", e))?;
        tracing::debug!("Deleted chunks for file: {}", file_path.display());
        Ok(())
    }

    /// Commit Tantivy changes
    pub fn commit(&mut self) -> Result<()> {
        self.tantivy.commit()
    }

    /// Clear all indexed data (metadata cache, Tantivy, and vector store)
    pub async fn clear_all_data(&mut self) -> Result<()> {
        tracing::info!("Clearing all indexed data (metadata cache, Tantivy, vector store)...");

        self.core.clear_metadata_cache()?;
        tracing::info!("✓ Cleared metadata cache");

        self.tantivy.delete_all()?;
        self.tantivy.commit()?;
        tracing::info!("✓ Cleared Tantivy index");

        self.vector_store.clear_collection().await
            .map_err(|e| anyhow::anyhow!("Failed to clear vector store: {}", e))?;
        tracing::info!("✓ Cleared vector store");

        tracing::info!("✓ All indexed data cleared successfully");
        Ok(())
    }

    /// Get access to the Tantivy index for searching
    pub fn tantivy_index(&self) -> &Index {
        self.tantivy.index()
    }

    /// Get cloned vector store for searching
    pub fn vector_store_cloned(&self) -> VectorStore {
        self.vector_store.clone()
    }

    /// Get cloned embedding generator for searching
    pub fn embedding_generator_cloned(&self) -> EmbeddingGenerator {
        self.core.embedding_generator().clone()
    }

    /// Get access to the Tantivy schema
    pub fn tantivy_schema(&self) -> &crate::schema::ChunkSchema {
        self.tantivy.schema()
    }

    /// Get access to the current metrics
    pub fn metrics(&self) -> &IndexingMetrics {
        &self.metrics
    }

    /// Create a Bm25Search instance from the Tantivy index
    pub fn create_bm25_search(&self) -> Result<crate::search::bm25::Bm25Search> {
        self.tantivy.create_bm25_search()
    }

    // Private helper methods

    fn collect_rust_files(&self, dir_path: &Path, stats: &mut IndexStats) -> Result<Vec<PathBuf>> {
        let mut rust_files = Vec::new();
        let mut walk_errors = 0;

        for entry in WalkDir::new(dir_path) {
            match entry {
                Ok(e)
                    if e.file_type().is_file()
                        && e.path().extension() == Some(std::ffi::OsStr::new("rs")) =>
                {
                    rust_files.push(e.path().to_path_buf());
                }
                Ok(_) => {}
                Err(err) => {
                    let path = err.path().unwrap_or_else(|| Path::new("<unknown>"));
                    tracing::warn!("Failed to access {}: {}", path.display(), err);
                    walk_errors += 1;
                }
            }
        }

        if walk_errors > 0 {
            tracing::warn!(
                "Encountered {} errors during directory walk, continuing with accessible files",
                walk_errors
            );
        }

        stats.total_files = rust_files.len();
        Ok(rust_files)
    }

    fn process_batch_errors(&self, error_collector: &ErrorCollector, stats: &mut IndexStats) {
        for error in error_collector.get_errors() {
            match error.category {
                crate::indexing::errors::ErrorCategory::Permanent => {
                    tracing::debug!("Skipped {}: {}", error.file_path.display(), error.message);
                    stats.skipped_files += 1;
                }
                crate::indexing::errors::ErrorCategory::Transient => {
                    tracing::warn!("Failed {}: {}", error.file_path.display(), error.message);
                    stats.skipped_files += 1;
                }
            }
        }
    }

    async fn process_and_index_batch(
        &mut self,
        processed: &[crate::indexing::indexer_core::ProcessedFile],
        stats: &mut IndexStats,
    ) -> Result<()> {
        let embed_start = Instant::now();

        // Collect all chunks from all files for batch processing
        let all_chunks: Vec<CodeChunk> = processed
            .iter()
            .flat_map(|p| p.chunks.iter())
            .cloned()
            .collect();

        let total_chunks = all_chunks.len();
        tracing::info!(
            "Batch embedding {} chunks from {} files...",
            total_chunks,
            processed.len()
        );

        // Generate embeddings in GPU-optimized batches
        let all_embeddings = self.core.generate_embeddings_batched(&all_chunks)?;

        let embed_duration = embed_start.elapsed();
        self.metrics.embed_duration += embed_duration;

        tracing::info!(
            "Generated {} embeddings in {:.2}s ({:.1} chunks/sec)",
            all_embeddings.len(),
            embed_duration.as_secs_f64(),
            all_embeddings.len() as f64 / embed_duration.as_secs_f64()
        );

        // PHASE 3: Batched indexing (Tantivy + LanceDB)
        // Instead of N file-by-file calls, make single batched calls to each store
        let index_start = Instant::now();

        // Batch all chunks for Tantivy (single call instead of N calls)
        tracing::debug!("Indexing {} chunks to Tantivy...", all_chunks.len());
        self.tantivy.index_chunks(&all_chunks)?;

        // Prepare all chunk data for vector store (single batch)
        let all_chunk_data: Vec<(ChunkId, Vec<f32>, CodeChunk)> = all_chunks
            .into_iter()
            .zip(all_embeddings.into_iter())
            .map(|(chunk, embedding)| (chunk.id, embedding, chunk))
            .collect();

        // Single batched upsert to vector store (instead of N calls)
        tracing::debug!("Upserting {} chunks to vector store...", all_chunk_data.len());
        self.vector_store
            .upsert_chunks(all_chunk_data)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to batch index chunks to vector store: {}", e))?;

        let index_duration = index_start.elapsed();
        self.metrics.index_duration += index_duration;

        // Update metadata for all files after successful indexing
        for processed_file in processed {
            self.core
                .update_file_metadata(&processed_file.path, &processed_file.content)?;

            stats.indexed_files += 1;
            stats.total_chunks += processed_file.chunks.len();

            // Track per-file latency (approximated as batch time / num files)
            let approx_file_duration = index_duration / processed.len() as u32;
            self.metrics.file_latencies.push(approx_file_duration);
        }

        tracing::info!(
            "Indexed {} chunks ({} files) in {:.2}s ({:.1} chunks/sec)",
            total_chunks,
            processed.len(),
            index_duration.as_secs_f64(),
            total_chunks as f64 / index_duration.as_secs_f64()
        );

        Ok(())
    }

    fn finalize_metrics(&mut self, stats: &IndexStats, total_duration: Duration) {
        self.metrics.total_duration = total_duration;
        self.metrics.total_files = stats.total_files;
        self.metrics.indexed_files = stats.indexed_files;
        self.metrics.skipped_files = stats.skipped_files;
        self.metrics.unchanged_files = stats.unchanged_files;
        self.metrics.total_chunks = stats.total_chunks;

        if stats.total_files > 0 {
            self.metrics.cache_hit_rate = stats.unchanged_files as f64 / stats.total_files as f64;
        }

        self.metrics.print_summary();
    }
}

impl Drop for UnifiedIndexer {
    fn drop(&mut self) {
        // TantivyAdapter handles cleanup in its Drop implementation
        tracing::debug!("UnifiedIndexer dropped");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    #[ignore] // Requires embedding model
    async fn test_unified_indexer_creation() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join("cache");
        let tantivy_path = temp_dir.path().join("tantivy");

        let indexer = UnifiedIndexer::for_embedded(
            &cache_path,
            &tantivy_path,
            "test_collection",
            384,
            None,
        )
        .await;

        assert!(
            indexer.is_ok(),
            "Failed to create UnifiedIndexer: {:?}",
            indexer.err()
        );
    }

    #[tokio::test]
    #[ignore] // Requires embedding model
    async fn test_index_file() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join("cache");
        let tantivy_path = temp_dir.path().join("tantivy");

        let mut indexer = UnifiedIndexer::for_embedded(
            &cache_path,
            &tantivy_path,
            "test_index_file",
            384,
            None,
        )
        .await
        .unwrap();

        let test_file = temp_dir.path().join("test.rs");
        std::fs::write(
            &test_file,
            r#"
/// A test function
pub fn test_function() {
    println!("hello");
}
            "#,
        )
        .unwrap();

        let result = indexer.index_file(&test_file).await;
        assert!(result.is_ok(), "Failed to index file: {:?}", result.err());

        match result.unwrap() {
            IndexFileResult::Indexed { chunks_count } => {
                assert!(chunks_count > 0, "Should generate at least one chunk");
            }
            _ => panic!("Expected file to be indexed"),
        }

        let vector_store = indexer.vector_store_cloned();
        let count = vector_store.count().await.unwrap();
        assert!(count > 0, "Vector store should have at least one vector");
    }
}
