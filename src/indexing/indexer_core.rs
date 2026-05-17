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
//! ```rust,no_run
//! use rust_code_mcp::indexing::indexer_core::IndexerCore;
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

use crate::chunker::{Chunker, ChunkSplitConfig, CodeChunk};
use crate::config::IndexerCoreConfig;
use crate::embeddings::{Embedding, EmbeddingBackend, EmbeddingGenerator};
use crate::indexing::embedding_batcher::EmbeddingBatcher;
use crate::indexing::file_processor::FileProcessor;
use crate::indexing::IndexingError;
use crate::metadata_cache::MetadataCache;
use crate::parser::RustParser;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Result of processing a single file
#[derive(Debug)]
pub struct ProcessedFile {
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
pub struct IndexerCore {
    /// File filtering, security, and change detection
    file_processor: FileProcessor,
    /// Code chunker
    chunker: Chunker,
    /// Token limits for oversized chunk splitting
    chunk_split_config: ChunkSplitConfig,
    /// Batch embedding generation and memory monitoring
    embedding_batcher: EmbeddingBatcher,
}

impl IndexerCore {
    /// Create a new IndexerCore with the default embedding backend.
    pub fn new(
        cache_path: &Path,
        config: Option<IndexerCoreConfig>,
    ) -> Result<Self, IndexingError> {
        Self::with_backend(cache_path, config, EmbeddingBackend::default())
    }

    /// Create a new IndexerCore with an explicit embedding backend.
    pub fn with_backend(
        cache_path: &Path,
        config: Option<IndexerCoreConfig>,
        backend: EmbeddingBackend,
    ) -> Result<Self, IndexingError> {
        let config = config
            .unwrap_or_default()
            .with_embedding_profile(backend.profile.clone())
            .with_env_overrides();

        let chunk_split_config = ChunkSplitConfig::new(
            config.chunk_target_tokens,
            config.chunk_hard_max_tokens,
        );
        let file_processor = FileProcessor::with_cache_key_salt(
            cache_path,
            config.max_file_size,
            config.chunking_cache_salt(),
        )?;
        let chunker = Chunker::new();
        let embedding_generator = EmbeddingGenerator::with_backend(backend)?;
        let embedding_batcher = EmbeddingBatcher::new(
            embedding_generator,
            config.gpu_batch_size,
            config.max_tokens_per_batch,
        );

        Ok(Self {
            file_processor,
            chunker,
            chunk_split_config,
            embedding_batcher,
        })
    }

    // --- File processing delegation (FileProcessor) ---

    /// Check if a file should be processed (security and size checks)
    pub fn should_process_file(&self, file_path: &Path) -> Result<bool, IndexingError> {
        self.file_processor.should_process_file(file_path)
    }

    /// Fast check if file has likely changed using only stat info (mtime + size).
    /// Avoids reading file content. Use as a pre-filter before `has_file_changed`.
    pub fn has_stat_changed(&self, file_path: &Path) -> Result<bool, IndexingError> {
        self.file_processor.has_stat_changed(file_path)
    }

    /// Check if file has changed (using metadata cache, reads content hash)
    pub fn has_file_changed(&self, file_path: &Path, content: &str) -> Result<bool, IndexingError> {
        self.file_processor.has_file_changed(file_path, content)
    }

    /// Update metadata cache for a file
    pub fn update_file_metadata(&self, file_path: &Path, content: &str) -> Result<(), IndexingError> {
        self.file_processor.update_file_metadata(file_path, content)
    }

    /// Get reference to metadata cache
    pub fn metadata_cache(&self) -> &MetadataCache {
        self.file_processor.metadata_cache()
    }

    /// Clear metadata cache
    pub fn clear_metadata_cache(&self) -> Result<(), IndexingError> {
        self.file_processor.clear_metadata_cache()
    }

    // --- Orchestration (uses FileProcessor + Chunker) ---

    /// Process a single file: parse, chunk, and prepare for indexing
    ///
    /// This is a synchronous operation suitable for parallel processing with Rayon.
    pub fn process_file_sync(&self, file_path: &Path) -> Result<ProcessedFile, IndexingError> {
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
            |chunk| self.embedding_batcher.count_chunk_raw_tokens(chunk),
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
    pub async fn generate_embeddings_batched(
        &self,
        chunks: &[CodeChunk],
    ) -> Result<Vec<Embedding>, IndexingError> {
        self.embedding_batcher
            .generate_embeddings_batched(chunks)
            .await
    }

    /// Calculate safe batch size for parallel processing based on available memory
    pub fn calculate_safe_batch_size(&self) -> usize {
        self.embedding_batcher.calculate_safe_batch_size()
    }

    /// Get current memory usage percentage
    pub fn memory_usage_percent(&self) -> f64 {
        self.embedding_batcher.memory_usage_percent()
    }

    /// Refresh memory monitor
    pub fn refresh_memory_monitor(&self) {
        self.embedding_batcher.refresh_memory_monitor()
    }

    /// Get current memory used in bytes
    pub fn memory_used_bytes(&self) -> u64 {
        self.embedding_batcher.memory_used_bytes()
    }

    /// Get reference to embedding generator for cloning
    pub fn embedding_generator(&self) -> &EmbeddingGenerator {
        self.embedding_batcher.embedding_generator()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_indexer_core_creation() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join("cache");

        let core = IndexerCore::new(&cache_path, None);
        assert!(core.is_ok(), "Failed to create IndexerCore: {:?}", core.err());
    }

    #[test]
    fn test_should_process_file() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join("cache");
        let core = IndexerCore::new(&cache_path, None).unwrap();

        // Create a test file
        let test_file = temp_dir.path().join("test.rs");
        std::fs::write(&test_file, "fn test() {}").unwrap();

        let should_process = core.should_process_file(&test_file);
        assert!(should_process.is_ok());
        assert!(should_process.unwrap());
    }

    #[test]
    fn test_calculate_safe_batch_size() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join("cache");
        let core = IndexerCore::new(&cache_path, None).unwrap();

        let batch_size = core.calculate_safe_batch_size();
        assert!(batch_size > 0);
        assert!(batch_size <= 100); // Should be capped at 100
    }
}
