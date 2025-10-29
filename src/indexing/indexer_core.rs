//! Core indexing logic for processing files and generating embeddings
//!
//! This module contains the core business logic for indexing operations,
//! including file processing, chunking, and embedding generation.

use crate::chunker::{Chunker, CodeChunk};
use crate::embeddings::{Embedding, EmbeddingGenerator};
use crate::metadata_cache::MetadataCache;
use crate::metrics::memory::MemoryMonitor;
use crate::parser::RustParser;
use crate::security::secrets::SecretsScanner;
use crate::security::SensitiveFileFilter;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
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

/// Configuration for IndexerCore
#[derive(Debug, Clone)]
pub struct IndexerCoreConfig {
    /// Maximum file size to process (in bytes)
    pub max_file_size: u64,
    /// GPU batch size for embedding generation
    pub gpu_batch_size: usize,
}

impl Default for IndexerCoreConfig {
    fn default() -> Self {
        Self {
            max_file_size: 10_000_000, // 10 MB
            gpu_batch_size: 96,        // Optimized for 8GB VRAM
        }
    }
}

/// Core indexing logic handler
pub struct IndexerCore {
    /// Rust code parser
    parser: RustParser,
    /// Code chunker
    chunker: Chunker,
    /// Embedding generator
    embedding_generator: EmbeddingGenerator,
    /// Metadata cache for change detection
    metadata_cache: MetadataCache,
    /// Secrets scanner
    secrets_scanner: SecretsScanner,
    /// File filter for sensitive files
    file_filter: SensitiveFileFilter,
    /// Memory monitor (wrapped in Arc<Mutex> for thread-safe interior mutability)
    memory_monitor: Arc<Mutex<MemoryMonitor>>,
    /// Configuration
    config: IndexerCoreConfig,
}

impl IndexerCore {
    /// Create a new IndexerCore
    pub fn new(
        cache_path: &Path,
        config: Option<IndexerCoreConfig>,
    ) -> Result<Self> {
        let parser = RustParser::new()
            .map_err(|e| anyhow::anyhow!("Failed to create RustParser: {}", e))?;

        let chunker = Chunker::new();

        let embedding_generator = EmbeddingGenerator::new()
            .map_err(|e| anyhow::anyhow!("Failed to create EmbeddingGenerator: {}", e))?;

        let metadata_cache = MetadataCache::new(cache_path)
            .context("Failed to open MetadataCache")?;

        let secrets_scanner = SecretsScanner::new();
        let file_filter = SensitiveFileFilter::default();
        let memory_monitor = MemoryMonitor::new();

        Ok(Self {
            parser,
            chunker,
            embedding_generator,
            metadata_cache,
            secrets_scanner,
            file_filter,
            memory_monitor: Arc::new(Mutex::new(memory_monitor)),
            config: config.unwrap_or_default(),
        })
    }

    /// Check if a file should be processed (security and size checks)
    pub fn should_process_file(&self, file_path: &Path) -> Result<bool> {
        // Check sensitive file filter
        if !self.file_filter.should_index(file_path) {
            tracing::warn!("Excluding sensitive file: {}", file_path.display());
            return Ok(false);
        }

        // Check file size
        let metadata = std::fs::metadata(file_path)
            .context(format!("Failed to read file metadata: {}", file_path.display()))?;

        if metadata.len() > self.config.max_file_size {
            tracing::warn!(
                "Skipping large file: {} ({:.2} MB exceeds {:.2} MB limit)",
                file_path.display(),
                metadata.len() as f64 / 1_000_000.0,
                self.config.max_file_size as f64 / 1_000_000.0
            );
            return Ok(false);
        }

        Ok(true)
    }

    /// Check if file has changed (using metadata cache)
    pub fn has_file_changed(&self, file_path: &Path, content: &str) -> Result<bool> {
        let file_path_str = file_path.to_string_lossy().to_string();
        self.metadata_cache.has_changed(&file_path_str, content)
            .map_err(|e| anyhow::anyhow!("Metadata cache error: {}", e))
    }

    /// Update metadata cache for a file
    pub fn update_file_metadata(&self, file_path: &Path, content: &str) -> Result<()> {
        let file_path_str = file_path.to_string_lossy().to_string();
        let metadata = std::fs::metadata(file_path)?;
        let file_meta = crate::metadata_cache::FileMetadata::from_content(
            content,
            metadata
                .modified()?
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs(),
            metadata.len(),
        );
        self.metadata_cache.set(&file_path_str, &file_meta)
            .map_err(|e| anyhow::anyhow!("Failed to update metadata cache: {}", e))
    }

    /// Process a single file: parse, chunk, and prepare for indexing
    ///
    /// This is a synchronous operation suitable for parallel processing with Rayon.
    pub fn process_file_sync(&self, file_path: &Path) -> Result<ProcessedFile> {
        let parse_start = Instant::now();

        // Security checks
        if !self.should_process_file(file_path)? {
            anyhow::bail!("File filtered: security check failed");
        }

        // Read file
        let content = std::fs::read_to_string(file_path)
            .context(format!("Failed to read file: {}", file_path.display()))?;

        // Secrets check
        if self.secrets_scanner.should_exclude(&content) {
            let summary = self.secrets_scanner.scan_summary(&content);
            tracing::warn!(
                "Excluding file with secrets: {}\n{}",
                file_path.display(),
                summary
            );
            anyhow::bail!("Contains secrets");
        }

        // Check cache
        if !self.has_file_changed(file_path, &content)? {
            anyhow::bail!("File unchanged");
        }

        // Parse with tree-sitter (CPU-intensive)
        // Create fresh parser for thread safety
        let mut parser = RustParser::new()
            .map_err(|e| anyhow::anyhow!("Failed to create parser: {}", e))?;
        let parse_result = parser.parse_source_complete(&content)
            .map_err(|e| anyhow::anyhow!("Failed to parse file: {}", e))?;

        // Chunk (CPU-intensive)
        let chunks = self.chunker.chunk_file(file_path, &content, &parse_result)
            .map_err(|e| anyhow::anyhow!("Failed to chunk file: {}", e))?;

        if chunks.is_empty() {
            tracing::warn!("No chunks generated for {}", file_path.display());
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

    /// Generate embeddings for chunks in batches
    ///
    /// Uses GPU-optimized batch size to avoid OOM on GPU memory.
    pub fn generate_embeddings_batched(&self, chunks: &[CodeChunk]) -> Result<Vec<Embedding>> {
        let chunk_texts: Vec<String> = chunks
            .iter()
            .map(|c| c.format_for_embedding())
            .collect();

        let mut all_embeddings = Vec::new();

        for (batch_idx, chunk_batch) in chunk_texts.chunks(self.config.gpu_batch_size).enumerate() {
            let batch_embeddings = self.embedding_generator
                .embed_batch(chunk_batch.to_vec())
                .map_err(|e| anyhow::anyhow!("Failed to generate embeddings (batch {}): {}", batch_idx, e))?;
            all_embeddings.extend(batch_embeddings);
        }

        Ok(all_embeddings)
    }

    /// Calculate safe batch size for parallel processing based on available memory
    pub fn calculate_safe_batch_size(&self) -> usize {
        let available_mb = self.memory_monitor.lock().unwrap().available_bytes() / 1_000_000;

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

    /// Get current memory usage percentage
    pub fn memory_usage_percent(&self) -> f64 {
        self.memory_monitor.lock().unwrap().usage_percent()
    }

    /// Refresh memory monitor
    pub fn refresh_memory_monitor(&self) {
        self.memory_monitor.lock().unwrap().refresh();
    }

    /// Get current memory used in bytes
    pub fn memory_used_bytes(&self) -> u64 {
        self.memory_monitor.lock().unwrap().used_bytes()
    }

    /// Get reference to embedding generator for cloning
    pub fn embedding_generator(&self) -> &EmbeddingGenerator {
        &self.embedding_generator
    }

    /// Get reference to metadata cache
    pub fn metadata_cache(&self) -> &MetadataCache {
        &self.metadata_cache
    }

    /// Clear metadata cache
    pub fn clear_metadata_cache(&self) -> Result<()> {
        self.metadata_cache
            .clear()
            .map_err(|e| anyhow::anyhow!("Failed to clear metadata cache: {}", e))
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
