//! Unified indexing pipeline that populates both Tantivy (BM25) and Qdrant (Vector)
//!
//! This module fixes the critical issue where Qdrant was never populated during indexing,
//! making hybrid search impossible. It implements a unified pipeline that:
//! 1. Parses files with tree-sitter
//! 2. Chunks code by symbols
//! 3. Generates embeddings
//! 4. Indexes to BOTH Tantivy and Qdrant

use crate::chunker::{ChunkId, Chunker, CodeChunk};
use crate::embeddings::{Embedding, EmbeddingGenerator};
use crate::indexing::errors::{categorize_error, ErrorCategory, ErrorCollector, ErrorDetail};
use crate::metadata_cache::MetadataCache;
use crate::metrics::{IndexingMetrics, PhaseTimer, memory::MemoryMonitor};
use crate::parser::RustParser;
use crate::schema::ChunkSchema;
use crate::security::secrets::SecretsScanner;
use crate::security::SensitiveFileFilter;
use crate::vector_store::VectorStore;
use anyhow::{Context, Result};
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tantivy::{doc, Index, IndexWriter};
use tracing;
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
        Self {
            total_files: 0,
            indexed_files: 0,
            unchanged_files: 0,
            skipped_files: 0,
            total_chunks: 0,
        }
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

/// Processed file data (for parallel processing)
#[derive(Debug)]
struct ProcessedFile {
    path: PathBuf,
    content: String,
    chunks: Vec<CodeChunk>,
    parse_duration: Duration,
}

/// Unified indexer that populates both Tantivy and Qdrant
pub struct UnifiedIndexer {
    /// Tree-sitter parser for Rust code
    parser: RustParser,
    /// Semantic code chunker
    chunker: Chunker,
    /// Embedding generator (fastembed)
    embedding_generator: EmbeddingGenerator,
    /// Tantivy index
    tantivy_index: Index,
    /// Tantivy writer
    tantivy_writer: IndexWriter,
    /// Tantivy schema
    tantivy_schema: ChunkSchema,
    /// Qdrant vector store
    vector_store: VectorStore,
    /// Metadata cache for change detection
    metadata_cache: MetadataCache,
    /// Secrets scanner
    secrets_scanner: SecretsScanner,
    /// Sensitive file filter
    file_filter: SensitiveFileFilter,
    /// Performance metrics
    metrics: IndexingMetrics,
    /// Memory usage monitor
    memory_monitor: MemoryMonitor,
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
        Self::new_with_optimization(cache_path, tantivy_path, qdrant_url, collection_name, vector_size, None).await
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

        // Initialize parser
        let parser = RustParser::new()
            .map_err(|e| anyhow::anyhow!("Failed to create RustParser: {}", e))?;

        // Initialize chunker
        let chunker = Chunker::new();

        // Initialize embedding generator
        let embedding_generator = EmbeddingGenerator::new()
            .map_err(|e| anyhow::anyhow!("Failed to create EmbeddingGenerator: {}", e))?;

        // Initialize Tantivy with optimized memory budget if LOC provided
        let tantivy_schema = ChunkSchema::new();
        let tantivy_index = if tantivy_path.join("meta.json").exists() {
            Index::open_in_dir(tantivy_path).context("Failed to open Tantivy index")?
        } else {
            std::fs::create_dir_all(tantivy_path)
                .context("Failed to create Tantivy directory")?;
            Index::create_in_dir(tantivy_path, tantivy_schema.schema())
                .context("Failed to create Tantivy index")?
        };

        // Optimize Tantivy memory budget based on codebase size
        let (memory_budget_mb, num_threads) = if let Some(loc) = codebase_loc {
            if loc < 100_000 {
                (50, 2)
            } else if loc < 1_000_000 {
                (100, 4)
            } else {
                (200, 8)
            }
        } else {
            (50, 2) // Default for unknown size
        };

        let total_memory_budget = (memory_budget_mb * num_threads * 1024 * 1024) as usize;

        let tantivy_writer = tantivy_index
            .writer_with_num_threads(num_threads, total_memory_budget)
            .context("Failed to create Tantivy writer")?;

        tracing::info!(
            "Tantivy configured: {}MB total budget, {} threads",
            memory_budget_mb * num_threads,
            num_threads
        );

        // Initialize Qdrant with optimized configuration if LOC provided
        let base_config = crate::vector_store::VectorStoreConfig {
            url: qdrant_url.to_string(),
            collection_name: collection_name.to_string(),
            vector_size,
        };

        let vector_store = if let Some(loc) = codebase_loc {
            let optimized_config = crate::vector_store::QdrantOptimizedConfig::for_codebase_size(loc, base_config.clone());
            VectorStore::new_with_optimization(base_config, Some(optimized_config))
                .await
                .map_err(|e| anyhow::anyhow!("Failed to connect to VectorStore: {}", e))?
        } else {
            VectorStore::new(base_config)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to connect to VectorStore: {}", e))?
        };

        // Initialize metadata cache
        let metadata_cache =
            MetadataCache::new(cache_path).context("Failed to open MetadataCache")?;

        // Initialize security components
        let secrets_scanner = SecretsScanner::new();
        let file_filter = SensitiveFileFilter::default();

        tracing::info!("UnifiedIndexer initialized successfully");

        Ok(Self {
            parser,
            chunker,
            embedding_generator,
            tantivy_index,
            tantivy_writer,
            tantivy_schema,
            vector_store,
            metadata_cache,
            secrets_scanner,
            file_filter,
            metrics: IndexingMetrics::new(),
            memory_monitor: MemoryMonitor::new(),
        })
    }

    /// Index a single file to both Tantivy and Qdrant
    ///
    /// This is the core function that fixes the bug where Qdrant was never populated.
    pub async fn index_file(&mut self, file_path: &Path) -> Result<IndexFileResult> {
        let file_start = Instant::now();

        // 1. Check if file should be excluded (sensitive files)
        if !self.file_filter.should_index(file_path) {
            tracing::warn!("Excluding sensitive file: {}", file_path.display());
            return Ok(IndexFileResult::Skipped);
        }

        // 2. Check file size to avoid memory exhaustion
        let metadata = std::fs::metadata(file_path)
            .context(format!("Failed to read file metadata: {}", file_path.display()))?;

        let file_size = metadata.len();
        const MAX_FILE_SIZE: u64 = 10_000_000; // 10 MB

        if file_size > MAX_FILE_SIZE {
            tracing::warn!(
                "Skipping large file: {} ({:.2} MB exceeds {:.2} MB limit)",
                file_path.display(),
                file_size as f64 / 1_000_000.0,
                MAX_FILE_SIZE as f64 / 1_000_000.0
            );
            return Ok(IndexFileResult::Skipped);
        }

        // 3. Read file content
        let content = std::fs::read_to_string(file_path)
            .context(format!("Failed to read file: {}", file_path.display()))?;

        // 4. Check for secrets in content
        if self.secrets_scanner.should_exclude(&content) {
            let summary = self.secrets_scanner.scan_summary(&content);
            tracing::warn!(
                "Excluding file with secrets: {}\n{}",
                file_path.display(),
                summary
            );
            return Ok(IndexFileResult::Skipped);
        }

        // 5. Check if file changed (using existing metadata cache)
        let file_path_str = file_path.to_string_lossy().to_string();
        if !self.metadata_cache.has_changed(&file_path_str, &content)
            .map_err(|e| anyhow::anyhow!("Metadata cache error: {}", e))? {
            tracing::debug!("File unchanged: {}", file_path.display());
            return Ok(IndexFileResult::Unchanged);
        }

        tracing::debug!("Indexing changed file: {}", file_path.display());

        // 6. Parse with tree-sitter
        let parse_result = self
            .parser
            .parse_source_complete(&content)
            .map_err(|e| anyhow::anyhow!("Failed to parse file {}: {}", file_path.display(), e))?;

        // 7. Chunk the code (symbol-based)
        let chunks = self
            .chunker
            .chunk_file(file_path, &content, &parse_result)
            .map_err(|e| anyhow::anyhow!("Failed to chunk file {}: {}", file_path.display(), e))?;

        if chunks.is_empty() {
            tracing::warn!("No chunks generated for {}", file_path.display());
            return Ok(IndexFileResult::Skipped);
        }

        tracing::debug!("Generated {} chunks from {}", chunks.len(), file_path.display());

        // 8. Generate embeddings (batch processing)
        let chunk_texts: Vec<String> = chunks
            .iter()
            .map(|c| c.format_for_embedding())
            .collect();

        let embeddings = self
            .embedding_generator
            .embed_batch(chunk_texts)
            .map_err(|e| anyhow::anyhow!("Failed to generate embeddings: {}", e))?;

        if embeddings.len() != chunks.len() {
            anyhow::bail!(
                "Embedding count mismatch: {} chunks, {} embeddings",
                chunks.len(),
                embeddings.len()
            );
        }

        tracing::debug!("Generated {} embeddings", embeddings.len());

        // 9. Index to both stores
        // This is the CRITICAL FIX - we now actually populate Qdrant!
        let chunks_count = chunks.len();
        self.index_to_tantivy(&chunks)?;
        self.index_to_qdrant(chunks, embeddings).await?;

        // 10. Update metadata cache
        let file_meta = crate::metadata_cache::FileMetadata::from_content(
            &content,
            std::fs::metadata(file_path)?
                .modified()?
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs(),
            std::fs::metadata(file_path)?.len(),
        );
        self.metadata_cache.set(&file_path_str, &file_meta)
            .map_err(|e| anyhow::anyhow!("Failed to update metadata cache: {}", e))?;

        // Track file latency and update peak memory
        let file_duration = file_start.elapsed();
        self.metrics.file_latencies.push(file_duration);

        self.memory_monitor.refresh();
        self.metrics.peak_memory_bytes = self.metrics.peak_memory_bytes
            .max(self.memory_monitor.used_bytes());

        tracing::info!(
            "✓ Indexed {} chunks from {} in {:?}",
            chunks_count,
            file_path.display(),
            file_duration
        );

        Ok(IndexFileResult::Indexed {
            chunks_count,
        })
    }

    /// Index chunks to Tantivy (BM25)
    fn index_to_tantivy(&mut self, chunks: &[CodeChunk]) -> Result<()> {
        for chunk in chunks {
            let chunk_json =
                serde_json::to_string(chunk).context("Failed to serialize chunk to JSON")?;

            self.tantivy_writer
                .add_document(doc!(
                    self.tantivy_schema.chunk_id => chunk.id.to_string(),
                    self.tantivy_schema.content => chunk.content.clone(),
                    self.tantivy_schema.symbol_name => chunk.context.symbol_name.clone(),
                    self.tantivy_schema.symbol_kind => chunk.context.symbol_kind.clone(),
                    self.tantivy_schema.file_path => chunk.context.file_path.display().to_string(),
                    self.tantivy_schema.module_path => chunk.context.module_path.join("::"),
                    self.tantivy_schema.docstring => chunk.context.docstring.clone().unwrap_or_default(),
                    self.tantivy_schema.chunk_json => chunk_json,
                ))
                .context("Failed to add document to Tantivy")?;
        }

        Ok(())
    }

    /// Index chunks to Qdrant (Vector)
    ///
    /// THIS IS THE MISSING PIECE - Qdrant was never being populated!
    async fn index_to_qdrant(
        &self,
        chunks: Vec<CodeChunk>,
        embeddings: Vec<Embedding>,
    ) -> Result<()> {
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

    /// Index an entire directory
    pub async fn index_directory(&mut self, dir_path: &Path) -> Result<IndexStats> {
        let total_start = Instant::now();
        tracing::info!("Indexing directory: {}", dir_path.display());

        let mut stats = IndexStats::default();

        // Reset metrics for this indexing run
        self.metrics = IndexingMetrics::new();

        // Find all Rust files with proper error handling
        let mut rust_files = Vec::new();
        let mut walk_errors = 0;

        for entry in WalkDir::new(dir_path) {
            match entry {
                Ok(e) if e.file_type().is_file()
                       && e.path().extension() == Some(std::ffi::OsStr::new("rs")) => {
                    rust_files.push(e.path().to_path_buf());
                }
                Ok(_) => {}, // Directory or non-.rs file, skip silently
                Err(err) => {
                    let path = err.path().unwrap_or_else(|| Path::new("<unknown>"));
                    tracing::warn!(
                        "Failed to access {}: {}",
                        path.display(),
                        err
                    );
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
        self.tantivy_writer.commit().context("Failed to commit Tantivy index")?;

        // Finalize metrics
        self.metrics.total_duration = total_start.elapsed();
        self.metrics.total_files = stats.total_files;
        self.metrics.indexed_files = stats.indexed_files;
        self.metrics.skipped_files = stats.skipped_files;
        self.metrics.unchanged_files = stats.unchanged_files;
        self.metrics.total_chunks = stats.total_chunks;

        // Calculate cache hit rate
        if stats.total_files > 0 {
            self.metrics.cache_hit_rate = stats.unchanged_files as f64 / stats.total_files as f64;
        }

        // Print metrics summary
        self.metrics.print_summary();

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
    ///
    /// This method performs the same indexing as `index_directory`, but also
    /// creates Merkle tree snapshots and backups according to the provided policy.
    ///
    /// # Arguments
    /// * `dir_path` - Directory to index
    /// * `backup_manager` - Optional backup manager for Merkle snapshot backups
    ///
    /// # Backup Policy
    /// - Backups are created automatically after every 100 indexed files
    /// - Uses Merkle tree snapshots for fast incremental tracking
    /// - Backup manager handles retention policy (default: 7 days)
    pub async fn index_directory_with_backup(
        &mut self,
        dir_path: &Path,
        backup_manager: Option<&crate::monitoring::backup::BackupManager>,
    ) -> Result<IndexStats> {
        // Perform standard indexing
        let stats = self.index_directory(dir_path).await?;

        // Create backup if manager provided and files were indexed
        if let Some(manager) = backup_manager {
            if stats.indexed_files > 0 && stats.indexed_files % 100 == 0 {
                tracing::info!("Creating Merkle snapshot backup after {} indexed files", stats.indexed_files);

                // Build current Merkle tree
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

    /// Get access to the Tantivy index for searching
    pub fn tantivy_index(&self) -> &Index {
        &self.tantivy_index
    }

    /// Get cloned vector store for searching
    pub fn vector_store_cloned(&self) -> VectorStore {
        self.vector_store.clone()
    }

    /// Get cloned embedding generator for searching
    pub fn embedding_generator_cloned(&self) -> EmbeddingGenerator {
        self.embedding_generator.clone()
    }

    /// Get access to the Tantivy schema
    pub fn tantivy_schema(&self) -> &ChunkSchema {
        &self.tantivy_schema
    }

    /// Get access to the current metrics
    pub fn metrics(&self) -> &IndexingMetrics {
        &self.metrics
    }

    /// Create a Bm25Search instance from the Tantivy index
    pub fn create_bm25_search(&self) -> Result<crate::search::bm25::Bm25Search> {
        crate::search::bm25::Bm25Search::from_index(self.tantivy_index.clone())
            .map_err(|e| anyhow::anyhow!("Failed to create Bm25Search: {}", e))
    }

    /// Delete all chunks for a specific file from both Tantivy and Qdrant
    ///
    /// This is needed for incremental indexing when files are modified or deleted
    pub async fn delete_file_chunks(&mut self, file_path: &Path) -> Result<()> {
        let file_path_str = file_path.to_string_lossy().to_string();

        // Delete from Tantivy
        let term = tantivy::Term::from_field_text(
            self.tantivy_schema.file_path,
            &file_path_str,
        );
        let query = tantivy::query::TermQuery::new(
            term,
            tantivy::schema::IndexRecordOption::Basic,
        );

        self.tantivy_writer.delete_query(Box::new(query))?;

        // Delete from Qdrant
        self.vector_store
            .delete_by_file_path(&file_path_str)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to delete from Qdrant: {}", e))?;

        tracing::debug!("Deleted chunks for file: {}", file_path_str);

        Ok(())
    }

    /// Commit Tantivy changes
    ///
    /// Useful for forcing a commit after incremental updates
    pub fn commit(&mut self) -> Result<()> {
        self.tantivy_writer
            .commit()
            .context("Failed to commit Tantivy index")?;
        Ok(())
    }

    /// Process file synchronously for parallel execution
    ///
    /// This is a pure sync function that can be called from Rayon threads.
    /// No async operations here - those happen in the subsequent indexing phase.
    fn process_file_sync(&self, file_path: &Path) -> Result<ProcessedFile> {
        let parse_start = Instant::now();

        // Security checks
        if !self.file_filter.should_index(file_path) {
            anyhow::bail!("File filtered: sensitive file");
        }

        // File size check
        let metadata = std::fs::metadata(file_path)?;
        const MAX_FILE_SIZE: u64 = 10_000_000;
        if metadata.len() > MAX_FILE_SIZE {
            anyhow::bail!("File too large: {} MB", metadata.len() as f64 / 1_000_000.0);
        }

        // Read file
        let content = std::fs::read_to_string(file_path)?;

        // Secrets check
        if self.secrets_scanner.should_exclude(&content) {
            anyhow::bail!("Contains secrets");
        }

        // Check cache (Merkle/metadata)
        let file_path_str = file_path.to_string_lossy().to_string();
        if !self.metadata_cache.has_changed(&file_path_str, &content)
            .map_err(|e| anyhow::anyhow!("Metadata cache error: {}", e))? {
            anyhow::bail!("File unchanged");
        }

        // Parse (CPU-intensive)
        // Create fresh parser for this thread
        let mut parser = RustParser::new()
            .map_err(|e| anyhow::anyhow!("Failed to create parser: {}", e))?;
        let parse_result = parser.parse_source_complete(&content)
            .map_err(|e| anyhow::anyhow!("Failed to parse file: {}", e))?;

        // Chunk (CPU-intensive)
        let chunks = self.chunker.chunk_file(file_path, &content, &parse_result)
            .map_err(|e| anyhow::anyhow!("Failed to chunk file: {}", e))?;

        if chunks.is_empty() {
            anyhow::bail!("No chunks generated");
        }

        let parse_duration = parse_start.elapsed();

        Ok(ProcessedFile {
            path: file_path.to_path_buf(),
            content,
            chunks,
            parse_duration,
        })
    }

    /// Calculate safe batch size based on available memory
    fn calculate_safe_batch_size(&self) -> usize {
        let available_mb = self.memory_monitor.available_bytes() / 1_000_000;

        // Assume 15 MB per file in memory (content + AST + chunks)
        let safe_concurrent = (available_mb / 15).max(1) as usize;

        // Cap at CPU cores to avoid thrashing
        let max_concurrent = num_cpus::get();

        let batch_size = safe_concurrent.min(max_concurrent).min(100);

        tracing::debug!(
            "Memory-based batch size: {} (available: {} MB, max concurrent: {})",
            batch_size,
            available_mb,
            max_concurrent
        );

        batch_size
    }

    /// Index an entire directory using parallel processing
    ///
    /// This provides 2-3x speedup over sequential indexing by parallelizing
    /// the CPU-intensive parsing and chunking phases.
    pub async fn index_directory_parallel(&mut self, dir_path: &Path) -> Result<IndexStats> {
        let total_start = Instant::now();
        tracing::info!("Indexing directory (parallel mode): {}", dir_path.display());

        let mut stats = IndexStats::default();
        self.metrics = IndexingMetrics::new();

        // Find all Rust files
        let mut rust_files = Vec::new();
        let mut walk_errors = 0;

        for entry in WalkDir::new(dir_path) {
            match entry {
                Ok(e) if e.file_type().is_file()
                       && e.path().extension() == Some(std::ffi::OsStr::new("rs")) => {
                    rust_files.push(e.path().to_path_buf());
                }
                Ok(_) => {},
                Err(err) => {
                    let path = err.path().unwrap_or_else(|| Path::new("<unknown>"));
                    tracing::warn!("Failed to access {}: {}", path.display(), err);
                    walk_errors += 1;
                }
            }
        }

        if walk_errors > 0 {
            tracing::warn!("Encountered {} errors during directory walk", walk_errors);
        }

        stats.total_files = rust_files.len();
        if rust_files.is_empty() {
            return Ok(stats);
        }

        tracing::info!("Found {} Rust files, processing in parallel", rust_files.len());

        // Calculate safe batch size
        let batch_size = self.calculate_safe_batch_size();
        tracing::info!("Using batch size: {}", batch_size);

        // Process in batches to avoid memory exhaustion
        for (batch_idx, file_batch) in rust_files.chunks(batch_size).enumerate() {
            tracing::info!(
                "Processing batch {}/{} ({} files)",
                batch_idx + 1,
                (rust_files.len() + batch_size - 1) / batch_size,
                file_batch.len()
            );

            // Check memory before batch
            self.memory_monitor.refresh();
            let memory_usage = self.memory_monitor.usage_percent();

            if memory_usage > 85.0 {
                tracing::warn!("High memory usage ({:.1}%), pausing to allow GC", memory_usage);
                tokio::time::sleep(Duration::from_secs(5)).await;
            }

            // PHASE 1: Parallel parse and chunk (CPU-bound)
            let parse_start = Instant::now();
            let error_collector = ErrorCollector::new();
            let error_collector_clone = error_collector.clone();

            let processed: Vec<_> = file_batch
                .par_iter()
                .filter_map(|file_path| {
                    match self.process_file_sync(file_path) {
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

            // Log errors
            for error in error_collector.get_errors() {
                match error.category {
                    ErrorCategory::Permanent => {
                        tracing::debug!("Skipped {}: {}", error.file_path.display(), error.message);
                        stats.skipped_files += 1;
                    }
                    ErrorCategory::Transient => {
                        tracing::warn!("Failed {}: {}", error.file_path.display(), error.message);
                        stats.skipped_files += 1;
                    }
                }
            }

            // PHASE 2: Batch embedding for all files (I/O-bound, but maximally batched)
            let embed_start = Instant::now();

            if !processed.is_empty() {
                // Step 1: Collect all chunk texts from ALL processed files
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

                // Step 2: GPU batch embedding with memory-safe batch size
                // Conservative batch size to avoid OOM on 8GB VRAM (5.5GB available)
                // Testing shows 64-128 chunks is optimal balance
                let mut all_embeddings = Vec::new();
                const GPU_BATCH_SIZE: usize = 96; // Sweet spot for 8GB VRAM

                for (batch_idx, chunk_batch) in all_chunk_texts.chunks(GPU_BATCH_SIZE).enumerate() {
                    let batch_embeddings = self.embedding_generator.embed_batch(chunk_batch.to_vec())
                        .map_err(|e| anyhow::anyhow!("Failed to generate embeddings (batch {}): {}", batch_idx, e))?;
                    all_embeddings.extend(batch_embeddings);
                }

                let embed_duration = embed_start.elapsed();
                self.metrics.embed_duration += embed_duration;

                tracing::info!(
                    "Generated {} embeddings in {:.2}s ({:.1} chunks/sec)",
                    all_embeddings.len(),
                    embed_duration.as_secs_f64(),
                    all_embeddings.len() as f64 / embed_duration.as_secs_f64()
                );

                // Step 3: Distribute embeddings back to files and index
                let index_start = Instant::now();
                let mut embedding_idx = 0;

                for processed_file in &processed {
                    let file_start = Instant::now();
                    let num_chunks = processed_file.chunks.len();

                    // Extract embeddings for this file
                    let file_embeddings: Vec<_> = all_embeddings[embedding_idx..embedding_idx + num_chunks].to_vec();
                    embedding_idx += num_chunks;

                    // Index to Tantivy
                    self.index_to_tantivy(&processed_file.chunks)?;

                    // Index to Qdrant
                    self.index_to_qdrant(processed_file.chunks.clone(), file_embeddings).await?;

                    // Update cache
                    let file_meta = crate::metadata_cache::FileMetadata::from_content(
                        &processed_file.content,
                        std::fs::metadata(&processed_file.path)?
                            .modified()?
                            .duration_since(std::time::UNIX_EPOCH)?
                            .as_secs(),
                        std::fs::metadata(&processed_file.path)?.len(),
                    );
                    self.metadata_cache.set(
                        &processed_file.path.to_string_lossy(),
                        &file_meta
                    ).map_err(|e| anyhow::anyhow!("Failed to update metadata cache: {}", e))?;

                    // Update stats
                    stats.indexed_files += 1;
                    stats.total_chunks += processed_file.chunks.len();

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
            }

            // Commit after each batch to free memory
            self.tantivy_writer.commit()?;
        }

        // Finalize metrics
        self.metrics.total_duration = total_start.elapsed();
        self.metrics.total_files = stats.total_files;
        self.metrics.indexed_files = stats.indexed_files;
        self.metrics.skipped_files = stats.skipped_files;
        self.metrics.unchanged_files = stats.unchanged_files;
        self.metrics.total_chunks = stats.total_chunks;

        if stats.total_files > 0 {
            self.metrics.cache_hit_rate = stats.unchanged_files as f64 / stats.total_files as f64;
        }

        // Print metrics summary
        self.metrics.print_summary();

        tracing::info!(
            "✓ Parallel indexing complete: {} files indexed, {} chunks, {} skipped",
            stats.indexed_files,
            stats.total_chunks,
            stats.skipped_files
        );

        Ok(stats)
    }

    /// Clear all indexed data (metadata cache, Tantivy, and Qdrant)
    ///
    /// This is used for force reindexing to ensure a completely clean slate.
    /// After calling this, all files will be treated as new during indexing.
    pub async fn clear_all_data(&mut self) -> Result<()> {
        tracing::info!("Clearing all indexed data (metadata cache, Tantivy, Qdrant)...");

        // 1. Clear metadata cache
        self.metadata_cache
            .clear()
            .map_err(|e| anyhow::anyhow!("Failed to clear metadata cache: {}", e))?;
        tracing::info!("✓ Cleared metadata cache");

        // 2. Delete and recreate Tantivy index
        // We need to delete all documents from the index
        self.tantivy_writer.delete_all_documents()
            .context("Failed to delete all Tantivy documents")?;
        self.tantivy_writer.commit()
            .context("Failed to commit Tantivy deletion")?;
        tracing::info!("✓ Cleared Tantivy index");

        // 3. Clear Qdrant collection
        self.vector_store
            .clear_collection()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to clear Qdrant collection: {}", e))?;
        tracing::info!("✓ Cleared Qdrant collection");

        tracing::info!("✓ All indexed data cleared successfully");

        Ok(())
    }
}

impl Drop for UnifiedIndexer {
    fn drop(&mut self) {
        // Attempt to rollback any uncommitted changes to release the lock
        // This prevents "Failed to commit Tantivy index" errors when multiple
        // indexers are created in quick succession
        if let Err(e) = self.tantivy_writer.rollback() {
            tracing::warn!("Failed to rollback Tantivy writer during drop: {}", e);
        }
        tracing::debug!("UnifiedIndexer dropped, writer lock released");
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

        assert!(indexer.is_ok(), "Failed to create UnifiedIndexer: {:?}", indexer.err());
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

        // Create a test Rust file
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

        // Index the file
        let result = indexer.index_file(&test_file).await;
        assert!(result.is_ok(), "Failed to index file: {:?}", result.err());

        match result.unwrap() {
            IndexFileResult::Indexed { chunks_count } => {
                assert!(chunks_count > 0, "Should generate at least one chunk");
            }
            _ => panic!("Expected file to be indexed"),
        }

        // Verify Tantivy has data
        // Verify Qdrant has data
        let vector_store = indexer.vector_store_cloned();
        let count = vector_store.count().await.unwrap();
        assert!(count > 0, "Qdrant should have at least one vector");
    }
}
