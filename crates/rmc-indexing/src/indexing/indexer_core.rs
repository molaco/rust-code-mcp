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
use rmc_engine::embeddings::{
    Embedding, EmbeddingBackend, EmbeddingGenerator, EmbeddingOwner, EmbeddingTokenCounter,
};
use crate::indexing::embedding_batcher::EmbeddingBatcher;
use crate::indexing::file_processor::FileProcessor;
use crate::indexing::IndexingError;
use crate::indexing::error::{OUTCOME_FILE_UNCHANGED, OUTCOME_NO_CHUNKS, OUTCOME_SECURITY_FILTERED};
use crate::metadata_cache::MetadataCache;
use rmc_engine::parser::RustParser;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
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
    /// Token counter used for chunk splitting. Initialized lazily so the
    /// Merkle no-change path can still return before embedding resources.
    chunk_token_counter: OnceLock<Option<EmbeddingTokenCounter>>,
    #[cfg(test)]
    chunk_token_override: Option<Arc<dyn Fn(&CodeChunk) -> Option<usize> + Send + Sync>>,
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
        // Salt namespaces the shared per-directory sled cache by
        // embedder + chunking; see `identity::metadata_cache_salt`.
        let file_processor = FileProcessor::with_cache_key_salt(
            cache_path,
            config.max_file_size,
            crate::indexing::identity::metadata_cache_salt(&backend, &config.chunking_cache_salt()),
        )?;
        let chunker = Chunker::new();

        Ok(Self {
            file_processor,
            chunker,
            chunk_split_config,
            embedding_backend: backend,
            gpu_batch_size: config.gpu_batch_size,
            max_tokens_per_batch: config.max_tokens_per_batch,
            chunk_token_counter: OnceLock::new(),
            #[cfg(test)]
            chunk_token_override: None,
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
            EmbeddingGenerator::with_backend(self.embedding_backend.clone(), EmbeddingOwner::Index)?;
        let batcher = Arc::new(EmbeddingBatcher::new(
            embedding_generator,
            self.gpu_batch_size,
            self.max_tokens_per_batch,
        ));
        *guard = Some(Arc::clone(&batcher));
        Ok(batcher)
    }

    fn count_chunk_raw_tokens(&self, chunk: &CodeChunk) -> Option<usize> {
        #[cfg(test)]
        if let Some(counter) = self.chunk_token_override.as_ref() {
            return counter(chunk);
        }

        let counter = self.chunk_token_counter.get_or_init(|| {
            match EmbeddingTokenCounter::from_backend(&self.embedding_backend) {
                Ok(counter) => {
                    tracing::info!(
                        max_len = counter.max_len(),
                        "Chunk-splitting token counter initialized"
                    );
                    Some(counter)
                }
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        "Chunk-splitting token counter unavailable; falling back to text-length estimates"
                    );
                    None
                }
            }
        });
        let counter = counter.as_ref()?;

        match counter.count(&chunk.format_for_embedding()) {
            Ok(len) => Some(len.raw_tokens),
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "Chunk token count unavailable; falling back to estimated split size"
                );
                None
            }
        }
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

    /// File paths the metadata cache considers already indexed, for this
    /// embedder+chunking identity.
    pub(crate) fn cached_paths(&self) -> Result<HashSet<String>, IndexingError> {
        self.file_processor.cached_paths()
    }

    /// Drop one file's cache entry so the next run re-embeds it.
    pub(crate) fn forget_file(&self, file_path: &Path) -> Result<(), IndexingError> {
        self.file_processor.forget_file(file_path)
    }

    // --- Orchestration (uses FileProcessor + Chunker) ---

    /// Process a single file: parse, chunk, and prepare for indexing
    ///
    /// This is a synchronous operation suitable for parallel processing with Rayon.
    pub(crate) fn process_file_sync(&self, file_path: &Path) -> Result<ProcessedFile, IndexingError> {
        let parse_start = Instant::now();

        // Security checks (delegated to file_processor)
        if !self.file_processor.should_process_file(file_path)? {
            return Err(IndexingError::Parser(OUTCOME_SECURITY_FILTERED.into()));
        }

        // Fast stat-based change detection (avoids reading file content)
        if !self.file_processor.has_stat_changed(file_path)? {
            return Err(IndexingError::Parser(OUTCOME_FILE_UNCHANGED.into()));
        }

        // Read file (only if stat suggests change)
        let content = std::fs::read_to_string(file_path)?;

        // Secrets check (delegated to file_processor)
        self.file_processor.check_secrets(file_path, &content)?;

        // Content hash check (confirms stat-based detection)
        if !self.file_processor.has_file_changed(file_path, &content)? {
            return Err(IndexingError::Parser(OUTCOME_FILE_UNCHANGED.into()));
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
            |chunk| self.count_chunk_raw_tokens(chunk),
        );

        if chunks.is_empty() {
            tracing::warn!("No chunks generated for {}", file_path.display());
            return Err(IndexingError::Parser(OUTCOME_NO_CHUNKS.into()));
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
        .with_context_in_first_part_only(config.chunk_context_first_part_only)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::TempDir;

    /// Chunk-count probe, not a test: what a whole tree (`RMC_CHUNK_PROBE_TREE`)
    /// costs under each split policy, chunked the way the sync chunks it —
    /// the same file walk, parser, chunker and oversize split, token counts
    /// from the real tokenizer of `RMC_CHUNK_PROBE_PROFILE` (`local-gpu-bge`
    /// unless set) and its split budget. No store, no embedding.
    ///
    /// Per policy: chunks, the tokens the model is fed (capped at its max
    /// length — the GPU's work), and stored chunk JSON bytes; plus the chunk
    /// counts of the files named in `RMC_CHUNK_PROBE_FILES` (comma-separated
    /// suffixes). Runs on `RMC_CHUNK_PROBE_THREADS` threads (8 unless set).
    #[test]
    #[ignore]
    fn probe_chunk_counts_of_a_real_tree() {
        use crate::indexing::traversal::collect_project_rust_files;
        use rayon::prelude::*;
        use rmc_engine::embeddings::{EmbeddingProfile, EmbeddingTokenCounter};

        let tree = PathBuf::from(std::env::var("RMC_CHUNK_PROBE_TREE").unwrap());
        let profile =
            std::env::var("RMC_CHUNK_PROBE_PROFILE").unwrap_or_else(|_| "local-gpu-bge".into());
        let threads: usize = std::env::var("RMC_CHUNK_PROBE_THREADS")
            .map(|v| v.parse().unwrap())
            .unwrap_or(8);
        let named: Vec<String> = std::env::var("RMC_CHUNK_PROBE_FILES")
            .map(|v| v.split(',').map(str::to_string).collect())
            .unwrap_or_default();

        let spec = EmbeddingProfile::parse(&profile).unwrap();
        let budget = ChunkSplitConfig::new(
            spec.default_chunk_target_tokens(),
            spec.default_chunk_hard_max_tokens(),
        );
        let backend = EmbeddingBackend::from_profile(spec);
        let counter = EmbeddingTokenCounter::from_backend(&backend).unwrap();
        let max_len = counter.max_len();
        let (files, walk_errors) = collect_project_rust_files(&tree);
        eprintln!(
            "{}: {} .rs files ({walk_errors} walk errors); {profile}, split {}/{} tokens, model max {max_len}",
            tree.display(),
            files.len(),
            budget.target_tokens,
            budget.hard_max_tokens
        );

        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        for first_part_only in [false, true] {
            let config = budget.with_context_in_first_part_only(first_part_only);
            let started = Instant::now();
            // Per file: (path, chunks, capped tokens, chunk JSON bytes, parts of split symbols).
            let per_file: Vec<(PathBuf, usize, usize, usize, usize)> = pool.install(|| {
                files
                    .par_iter()
                    .filter_map(|path| {
                        let source = std::fs::read_to_string(path).ok()?;
                        let parsed = RustParser::new()
                            .unwrap()
                            .parse_source_complete(&source)
                            .ok()?;
                        let chunker = Chunker::new();
                        let chunks = chunker.chunk_file(path, &source, &parsed).ok()?;
                        let chunks = chunker.split_oversized_chunks(chunks, config, |chunk| {
                            counter
                                .count(&chunk.format_for_embedding())
                                .ok()
                                .map(|len| len.raw_tokens)
                        });
                        let tokens = chunks
                            .iter()
                            .map(|c| {
                                counter
                                    .count(&c.format_for_embedding())
                                    .unwrap()
                                    .raw_tokens
                                    .min(max_len)
                            })
                            .sum();
                        let json = chunks
                            .iter()
                            .map(|c| serde_json::to_string(c).unwrap().len())
                            .sum();
                        let parts = chunks
                            .iter()
                            .filter(|c| c.context.split_part.is_some())
                            .count();
                        Some((path.clone(), chunks.len(), tokens, json, parts))
                    })
                    .collect()
            });
            let chunks: usize = per_file.iter().map(|f| f.1).sum();
            let tokens: usize = per_file.iter().map(|f| f.2).sum();
            let json: usize = per_file.iter().map(|f| f.3).sum();
            let parts: usize = per_file.iter().map(|f| f.4).sum();
            eprintln!(
                "context_in_first_part_only={first_part_only}: {} files, {chunks} chunks ({parts} of them parts \
                 of split symbols), {:.1} M model tokens, chunk JSON {:.0} MiB; {:.0}s",
                per_file.len(),
                tokens as f64 / 1e6,
                json as f64 / (1024.0 * 1024.0),
                started.elapsed().as_secs_f64()
            );
            for suffix in &named {
                if let Some(f) = per_file.iter().find(|f| f.0.ends_with(suffix)) {
                    eprintln!(
                        "  {}: {} chunks, {} parts, {} tokens",
                        suffix, f.1, f.4, f.2
                    );
                }
            }
        }
    }

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

    #[test]
    fn process_file_sync_uses_token_counts_without_embedding_batcher() {
        let temp_dir = TempDir::new().unwrap();
        let src_dir = temp_dir.path().join("src");
        std::fs::create_dir(&src_dir).unwrap();
        let test_file = src_dir.join("lib.rs");
        std::fs::write(
            &test_file,
            r#"
pub fn long_function() {
    let alpha = 1;
    let beta = alpha + 1;
    let gamma = beta + 1;
    let delta = gamma + 1;
    let epsilon = delta + 1;
    println!("{epsilon}");
}
"#,
        )
        .unwrap();

        let backend = EmbeddingBackend::from_profile_name("openrouter-qwen3-8b").unwrap();
        let mut fallback_core = IndexerCore::with_backend(
            &temp_dir.path().join("fallback-cache"),
            None,
            backend.clone(),
        )
        .unwrap();
        fallback_core.chunk_token_override = Some(Arc::new(|_| None));

        let fallback = fallback_core.process_file_sync(&test_file).unwrap();

        assert!(fallback_core.embedding_batcher.lock().unwrap().is_none());
        assert!(fallback_core.chunk_token_counter.get().is_none());
        assert!(
            fallback
                .chunks
                .iter()
                .all(|chunk| chunk.context.split_part.is_none())
        );

        let mut counted_core = IndexerCore::with_backend(
            &temp_dir.path().join("counted-cache"),
            None,
            backend,
        )
        .unwrap();
        counted_core.chunk_token_override = Some(Arc::new(|chunk| {
            Some(chunk.content.lines().count() * 500)
        }));

        let counted = counted_core.process_file_sync(&test_file).unwrap();

        assert!(counted_core.embedding_batcher.lock().unwrap().is_none());
        assert!(counted_core.chunk_token_counter.get().is_none());
        assert!(
            counted
                .chunks
                .iter()
                .any(|chunk| chunk.context.split_part.is_some())
        );
    }
}
