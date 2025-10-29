//! Unified indexing pipeline that populates both Tantivy (BM25) and Qdrant (Vector)
//!
//! This module coordinates indexing operations by delegating to specialized adapters:
//! - TantivyAdapter: BM25 indexing operations
//! - QdrantAdapter: Vector indexing operations
//! - IndexerCore: Core file processing and embedding generation

use crate::chunker::CodeChunk;
use crate::config::IndexerConfig;
use crate::embeddings::EmbeddingGenerator;
use crate::indexing::errors::{categorize_error, ErrorCollector, ErrorDetail};
use crate::indexing::indexer_core::IndexerCore;
use crate::indexing::qdrant_adapter::QdrantAdapter;
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

/// Unified indexer that coordinates Tantivy and Qdrant operations
pub struct UnifiedIndexer {
    /// Core indexing logic
    core: IndexerCore,
    /// Tantivy adapter for BM25 indexing
    tantivy: TantivyAdapter,
    /// Qdrant adapter for vector indexing
    qdrant: QdrantAdapter,
    /// Performance metrics
    metrics: IndexingMetrics,
}

impl UnifiedIndexer {
    /// Create a new unified indexer
    ///
    /// # Arguments
    /// * `cache_path` - Path to metadata cache directory
    /// * `tantivy_path` - Path to Tantivy index directory
    /// * `qdrant_url` - Qdrant server URL (e.g., "http://localhost:6333")
    /// * `collection_name` - Qdrant collection name
    /// * `vector_size` - Vector dimensions (384 for all-MiniLM-L6-v2)
    pub async fn new(
        cache_path: &Path,
        tantivy_path: &Path,
        qdrant_url: &str,
        collection_name: &str,
        vector_size: usize,
    ) -> Result<Self> {
        Self::new_with_optimization(
            cache_path,
            tantivy_path,
            qdrant_url,
            collection_name,
            vector_size,
            None,
        )
        .await
    }

    /// Create a new unified indexer from a consolidated configuration
    ///
    /// This is the preferred constructor that uses dependency injection
    /// to reduce coupling and simplify configuration management.
    ///
    /// # Arguments
    /// * `config` - Consolidated indexer configuration
    pub async fn from_config(config: IndexerConfig) -> Result<Self> {
        tracing::info!("Initializing UnifiedIndexer from config...");

        // Initialize core with injected config
        let core = IndexerCore::new(&config.core.cache_path, Some(config.core.clone()))?;

        // Initialize Tantivy adapter with injected config
        let tantivy = TantivyAdapter::new(config.tantivy)?;

        // Initialize Qdrant adapter with injected config
        let base_config = crate::vector_store::VectorStoreConfig {
            url: config.qdrant.url.clone(),
            collection_name: config.qdrant.collection_name.clone(),
            vector_size: config.qdrant.vector_size,
        };

        let optimized_config = crate::vector_store::QdrantOptimizedConfig {
            base_config: base_config.clone(),
            hnsw_m: config.qdrant.hnsw_m,
            hnsw_ef_construct: config.qdrant.hnsw_ef_construct,
            hnsw_ef: config.qdrant.hnsw_ef,
            indexing_threads: config.qdrant.indexing_threads,
            full_scan_threshold: config.qdrant.full_scan_threshold,
            memmap_threshold: config.qdrant.memmap_threshold,
        };

        let vector_store = VectorStore::new_with_optimization(base_config, Some(optimized_config))
            .await
            .map_err(|e| anyhow::anyhow!("Failed to connect to VectorStore: {}", e))?;

        let qdrant = QdrantAdapter::new(vector_store);

        tracing::info!("UnifiedIndexer initialized successfully from config");

        Ok(Self {
            core,
            tantivy,
            qdrant,
            metrics: IndexingMetrics::new(),
        })
    }

    /// Create a new unified indexer with optimized configuration
    ///
    /// # Arguments
    /// * `cache_path` - Path to metadata cache directory
    /// * `tantivy_path` - Path to Tantivy index directory
    /// * `qdrant_url` - Qdrant server URL (e.g., "http://localhost:6333")
    /// * `collection_name` - Qdrant collection name
    /// * `vector_size` - Vector dimensions (384 for all-MiniLM-L6-v2)
    /// * `codebase_loc` - Estimated lines of code (for optimization)
    pub async fn new_with_optimization(
        cache_path: &Path,
        tantivy_path: &Path,
        qdrant_url: &str,
        collection_name: &str,
        vector_size: usize,
        codebase_loc: Option<usize>,
    ) -> Result<Self> {
        tracing::info!("Initializing UnifiedIndexer...");

        // Initialize core
        let core = IndexerCore::new(cache_path, None)?;

        // Initialize Tantivy adapter
        let tantivy_config = crate::config::TantivyConfig::for_codebase_size(tantivy_path, codebase_loc);
        let tantivy = TantivyAdapter::new(tantivy_config)?;

        // Initialize Qdrant adapter
        let base_config = crate::vector_store::VectorStoreConfig {
            url: qdrant_url.to_string(),
            collection_name: collection_name.to_string(),
            vector_size,
        };

        let vector_store = if let Some(loc) = codebase_loc {
            let optimized_config = crate::vector_store::QdrantOptimizedConfig::for_codebase_size(
                loc,
                base_config.clone(),
            );
            VectorStore::new_with_optimization(base_config, Some(optimized_config))
                .await
                .map_err(|e| anyhow::anyhow!("Failed to connect to VectorStore: {}", e))?
        } else {
            VectorStore::new(base_config)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to connect to VectorStore: {}", e))?
        };

        let qdrant = QdrantAdapter::new(vector_store);

        tracing::info!("UnifiedIndexer initialized successfully");

        Ok(Self {
            core,
            tantivy,
            qdrant,
            metrics: IndexingMetrics::new(),
        })
    }

    /// Index a single file to both Tantivy and Qdrant
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
        self.qdrant
            .index_chunks(processed.chunks, embeddings)
            .await?;

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

    /// Delete all chunks for a specific file from both Tantivy and Qdrant
    pub async fn delete_file_chunks(&mut self, file_path: &Path) -> Result<()> {
        self.tantivy.delete_file_chunks(file_path)?;
        self.qdrant.delete_file_chunks(file_path).await?;
        tracing::debug!("Deleted chunks for file: {}", file_path.display());
        Ok(())
    }

    /// Commit Tantivy changes
    pub fn commit(&mut self) -> Result<()> {
        self.tantivy.commit()
    }

    /// Clear all indexed data (metadata cache, Tantivy, and Qdrant)
    pub async fn clear_all_data(&mut self) -> Result<()> {
        tracing::info!("Clearing all indexed data (metadata cache, Tantivy, Qdrant)...");

        self.core.clear_metadata_cache()?;
        tracing::info!("✓ Cleared metadata cache");

        self.tantivy.delete_all()?;
        self.tantivy.commit()?;
        tracing::info!("✓ Cleared Tantivy index");

        self.qdrant.clear_collection().await?;
        tracing::info!("✓ Cleared Qdrant collection");

        tracing::info!("✓ All indexed data cleared successfully");
        Ok(())
    }

    /// Get access to the Tantivy index for searching
    pub fn tantivy_index(&self) -> &Index {
        self.tantivy.index()
    }

    /// Get cloned vector store for searching
    pub fn vector_store_cloned(&self) -> VectorStore {
        self.qdrant.vector_store_cloned()
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

        // Collect all chunk texts
        let mut all_chunk_texts = Vec::new();
        for processed_file in processed.iter() {
            for chunk in &processed_file.chunks {
                all_chunk_texts.push(chunk.format_for_embedding());
            }
        }

        tracing::info!(
            "Batch embedding {} chunks from {} files...",
            all_chunk_texts.len(),
            processed.len()
        );

        // Generate embeddings in GPU-optimized batches
        let all_embeddings = self.core.generate_embeddings_batched(
            &processed
                .iter()
                .flat_map(|p| p.chunks.iter())
                .cloned()
                .collect::<Vec<CodeChunk>>(),
        )?;

        let embed_duration = embed_start.elapsed();
        self.metrics.embed_duration += embed_duration;

        tracing::info!(
            "Generated {} embeddings in {:.2}s ({:.1} chunks/sec)",
            all_embeddings.len(),
            embed_duration.as_secs_f64(),
            all_embeddings.len() as f64 / embed_duration.as_secs_f64()
        );

        // Index files
        let index_start = Instant::now();
        let mut embedding_idx = 0;

        for processed_file in processed {
            let file_start = Instant::now();
            let num_chunks = processed_file.chunks.len();

            let file_embeddings: Vec<_> =
                all_embeddings[embedding_idx..embedding_idx + num_chunks].to_vec();
            embedding_idx += num_chunks;

            self.tantivy.index_chunks(&processed_file.chunks)?;
            self.qdrant
                .index_chunks(processed_file.chunks.clone(), file_embeddings)
                .await?;

            self.core
                .update_file_metadata(&processed_file.path, &processed_file.content)?;

            stats.indexed_files += 1;
            stats.total_chunks += num_chunks;

            let file_duration = file_start.elapsed();
            self.metrics.file_latencies.push(file_duration);
        }

        let index_duration = index_start.elapsed();
        self.metrics.index_duration += index_duration;

        tracing::info!(
            "Indexed {} files in {:.2}s ({:.1} files/sec)",
            processed.len(),
            index_duration.as_secs_f64(),
            processed.len() as f64 / index_duration.as_secs_f64()
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
    #[ignore] // Requires Qdrant server running
    async fn test_unified_indexer_creation() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join("cache");
        let tantivy_path = temp_dir.path().join("tantivy");

        let indexer = UnifiedIndexer::new(
            &cache_path,
            &tantivy_path,
            "http://localhost:6333",
            "test_collection",
            384,
        )
        .await;

        assert!(
            indexer.is_ok(),
            "Failed to create UnifiedIndexer: {:?}",
            indexer.err()
        );
    }

    #[tokio::test]
    #[ignore] // Requires Qdrant server and embedding model
    async fn test_index_file() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join("cache");
        let tantivy_path = temp_dir.path().join("tantivy");

        let mut indexer = UnifiedIndexer::new(
            &cache_path,
            &tantivy_path,
            "http://localhost:6333",
            "test_index_file",
            384,
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
        assert!(count > 0, "Qdrant should have at least one vector");
    }
}
