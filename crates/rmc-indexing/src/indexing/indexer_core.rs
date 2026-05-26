//! Core indexing logic — thin facade over FileProcessor, Chunker, and EmbeddingBatcher
//!
//! `IndexerCore` orchestrates file processing and embedding generation by delegating
//! to focused sub-components:
//! - [`FileProcessor`](super::file_processor::FileProcessor): security filtering, change detection, metadata cache
//! - [`EmbeddingBatcher`](super::embedding_batcher::EmbeddingBatcher): GPU-optimized batch embedding, memory monitoring
//! - [`Chunker`]: tree-sitter parsing and semantic code chunking
//!
//! ## Processing Pipeline
//!
//! ```text
//! File → Security Checks → Parse (tree-sitter) → Chunk → Embeddings
//!        ├─ Sensitive file filter  ─┐
//!        ├─ Secrets scanner         ├─ FileProcessor
//!        └─ Size limits            ─┘
//! ```
//!
//! ## Examples
//!
//! ```rust,ignore
//! use rmc_indexing::indexing::UnifiedIndexer;
//! use std::path::Path;
//!
//! # fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let core = IndexerCore::new(Path::new("./cache"), None)?;
//!
//! let processed = core.process_file_sync(Path::new("src/main.rs"))?;
//! // generate_embeddings_batched is async — requires a runtime in real use.
//! let batch_size = core.calculate_safe_batch_size();
//! # Ok(())
//! # }
//! ```

use rmc_engine::chunker::{Chunker, ChunkSplitConfig, CodeChunk};
use rmc_config::config::IndexerCoreConfig;
use rmc_engine::embeddings::{Embedding, EmbeddingBackend, EmbeddingGenerator};
use crate::indexing::embedding_batcher::EmbeddingBatcher;
use crate::indexing::file_processor::FileProcessor;
use crate::indexing::IndexingError;
use crate::metadata_cache::MetadataCache;
use rmc_engine::parser::RustParser;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Result of processing a single file
#[derive(Debug)]
pub(crate) struct ProcessedFile {
    /// File path
    pub path: PathBuf,
    /// File content
    pub content: String,
    /// Generated code chunks
    pub chunks: Vec<CodeChunk>,
    /// Time taken to parse and chunk
    pub parse_duration: Duration,
}

/// Core indexing logic — facade over FileProcessor, Chunker, and EmbeddingBatcher
pub(crate) struct IndexerCore {
    /// File filtering, security, and change detection
    file_processor: FileProcessor,
    /// Code chunker
    chunker: Chunker,
    /// Token limits for oversized chunk splitting
    chunk_split_config: ChunkSplitConfig,
    /// Active embedding backend configuration
    embedding_backend: EmbeddingBackend,
    /// GPU batch size for embedding generation
    gpu_batch_size: usize,
    /// Padded token budget for embedding generation
    max_tokens_per_batch: usize,
    /// Batch embedding generation and memory monitoring
    embedding_batcher: Mutex<Option<Arc<EmbeddingBatcher>>>,
}

impl IndexerCore {
    /// Create a new IndexerCore with the default embedding backend.
    pub(crate) fn new(
        cache_path: &Path,
        config: Option<IndexerCoreConfig>,
    ) -> Result<Self, IndexingError> {
        Self::with_backend(cache_path, config, EmbeddingBackend::default())
    }

    /// Create a new IndexerCore with an explicit embedding backend.
    pub(crate) fn with_backend(
        cache_path: &Path,
        config: Option<IndexerCoreConfig>,
        backend: EmbeddingBackend,
    ) -> Result<Self, IndexingError> {
        let config = config
            .unwrap_or_default()
            .with_embedding_profile(backend.profile.clone())
            .with_env_overrides();

        let chunk_split_config = chunk_split_config_from(&config);
        let file_processor = FileProcessor::with_cache_key_salt(
            cache_path,
            config.max_file_size,
            config.chunking_cache_salt(),
        )?;
        let chunker = Chunker::new();

        Ok(Self {
            file_processor,
            chunker,
            chunk_split_config,
            embedding_backend: backend,
            gpu_batch_size: config.gpu_batch_size,
            max_tokens_per_batch: config.max_tokens_per_batch,
            embedding_batcher: Mutex::new(None),
        })
    }

    fn embedding_batcher(&self) -> Result<Arc<EmbeddingBatcher>, IndexingError> {
        let mut guard = self
            .embedding_batcher
            .lock()
            .map_err(|_| IndexingError::Cache("Embedding batcher lock poisoned".into()))?;

        if let Some(batcher) = guard.as_ref() {
            return Ok(Arc::clone(batcher));
        }

        let embedding_generator =
            EmbeddingGenerator::with_backend(self.embedding_backend.clone())?;
        let batcher = Arc::new(EmbeddingBatcher::new(
            embedding_generator,
            self.gpu_batch_size,
            self.max_tokens_per_batch,
        ));
        *guard = Some(Arc::clone(&batcher));
        Ok(batcher)
    }

    // --- File processing delegation (FileProcessor) ---

    /// Check if a file should be processed (security and size checks)
    pub(crate) fn should_process_file(&self, file_path: &Path) -> Result<bool, IndexingError> {
        self.file_processor.should_process_file(file_path)
    }

    /// Fast check if file has likely changed using only stat info (mtime + size).
    /// Avoids reading file content. Use as a pre-filter before `has_file_changed`.
    pub(crate) fn has_stat_changed(&self, file_path: &Path) -> Result<bool, IndexingError> {
        self.file_processor.has_stat_changed(file_path)
    }

    /// Check if file has changed (using metadata cache, reads content hash)
    pub(crate) fn has_file_changed(&self, file_path: &Path, content: &str) -> Result<bool, IndexingError> {
        self.file_processor.has_file_changed(file_path, content)
    }

    /// Update metadata cache for a file
    pub(crate) fn update_file_metadata(&self, file_path: &Path, content: &str) -> Result<(), IndexingError> {
        self.file_processor.update_file_metadata(file_path, content)
    }

    /// Get reference to metadata cache
    pub(crate) fn metadata_cache(&self) -> &MetadataCache {
        self.file_processor.metadata_cache()
    }

    /// Clear metadata cache
    pub(crate) fn clear_metadata_cache(&self) -> Result<(), IndexingError> {
        self.file_processor.clear_metadata_cache()
    }

    // --- Orchestration (uses FileProcessor + Chunker) ---

    /// Process a single file: parse, chunk, and prepare for indexing
    ///
    /// This is a synchronous operation suitable for parallel processing with Rayon.
    pub(crate) fn process_file_sync(&self, file_path: &Path) -> Result<ProcessedFile, IndexingError> {
        let parse_start = Instant::now();

        // Security checks (delegated to file_processor)
        if !self.file_processor.should_process_file(file_path)? {
            return Err(IndexingError::Parser("File filtered: security check failed".into()));
        }

        // Fast stat-based change detection (avoids reading file content)
        if !self.file_processor.has_stat_changed(file_path)? {
            return Err(IndexingError::Parser("File unchanged".into()));
        }

        // Read file (only if stat suggests change)
        let content = std::fs::read_to_string(file_path)?;

        // Secrets check (delegated to file_processor)
        self.file_processor.check_secrets(file_path, &content)?;

        // Content hash check (confirms stat-based detection)
        if !self.file_processor.has_file_changed(file_path, &content)? {
            return Err(IndexingError::Parser("File unchanged".into()));
        }

        // Parse with tree-sitter (CPU-intensive)
        // Create fresh parser for thread safety
        let mut parser = RustParser::new()
            .map_err(|e| IndexingError::Parser(e.to_string()))?;
        let parse_result = parser.parse_source_complete(&content)
            .map_err(|e| IndexingError::Parser(e.to_string()))?;

        // Chunk (CPU-intensive)
        let chunks = self.chunker.chunk_file(file_path, &content, &parse_result)
            .map_err(|e| IndexingError::Parser(e.to_string()))?;
        let chunks = self.chunker.split_oversized_chunks(
            chunks,
            self.chunk_split_config,
            |_| None,
        );

        if chunks.is_empty() {
            tracing::warn!("No chunks generated for {}", file_path.display());
            return Err(IndexingError::Parser("No chunks generated".into()));
        }

        let parse_duration = parse_start.elapsed();

        Ok(ProcessedFile {
            path: file_path.to_path_buf(),
            content,
            chunks,
            parse_duration,
        })
    }

    // --- Embedding delegation (EmbeddingBatcher) ---

    /// Generate embeddings for chunks in batches.
    ///
    /// Uses GPU-optimized batch size to avoid OOM on GPU memory.
    pub(crate) async fn generate_embeddings_batched(
        &self,
        chunks: &[CodeChunk],
    ) -> Result<Vec<Embedding>, IndexingError> {
        self.embedding_batcher()?
            .generate_embeddings_batched(chunks)
            .await
    }

    /// Calculate safe batch size for parallel processing based on available memory
    pub(crate) fn calculate_safe_batch_size(&self) -> Result<usize, IndexingError> {
        Ok(self.embedding_batcher()?.calculate_safe_batch_size())
    }

    /// Get current memory usage percentage
    pub(crate) fn memory_usage_percent(&self) -> Result<f64, IndexingError> {
        Ok(self.embedding_batcher()?.memory_usage_percent())
    }

    /// Refresh memory monitor
    pub(crate) fn refresh_memory_monitor(&self) -> Result<(), IndexingError> {
        self.embedding_batcher()?.refresh_memory_monitor();
        Ok(())
    }

    /// Get current memory used in bytes
    pub(crate) fn memory_used_bytes(&self) -> Result<u64, IndexingError> {
        Ok(self.embedding_batcher()?.memory_used_bytes())
    }

    /// Get a cloned embedding generator
    pub(crate) fn embedding_generator_cloned(&self) -> Result<EmbeddingGenerator, IndexingError> {
        Ok(self.embedding_batcher()?.embedding_generator().clone())
    }
}

fn chunk_split_config_from(config: &IndexerCoreConfig) -> ChunkSplitConfig {
    ChunkSplitConfig::new(config.chunk_target_tokens, config.chunk_hard_max_tokens)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_chunk_split_config_uses_core_config() {
        let config = IndexerCoreConfig {
            chunk_target_tokens: 512,
            chunk_hard_max_tokens: 768,
            ..Default::default()
        };

        let split_config = chunk_split_config_from(&config);

        assert_eq!(split_config.target_tokens, 512);
        assert_eq!(split_config.hard_max_tokens, 768);
    }

    #[test]
    fn test_chunk_split_config_clamps_invalid_hard_max() {
        let config = IndexerCoreConfig {
            chunk_target_tokens: 512,
            chunk_hard_max_tokens: 128,
            ..Default::default()
        };

        let split_config = chunk_split_config_from(&config);

        assert_eq!(split_config.target_tokens, 512);
        assert_eq!(split_config.hard_max_tokens, 512);
    }

    #[test]
    fn test_should_process_file_without_embedding_generator() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join("cache");
        let core = IndexerCore::with_backend(
            &cache_path,
            None,
            EmbeddingBackend::default(),
        )
        .unwrap();
        let test_file = temp_dir.path().join("test.rs");
        std::fs::write(&test_file, "fn test() {}").unwrap();

        let should_process = core.should_process_file(&test_file);
        assert!(should_process.is_ok());
        assert!(should_process.unwrap());
    }
}
