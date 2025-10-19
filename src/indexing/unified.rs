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
use crate::metadata_cache::MetadataCache;
use crate::parser::RustParser;
use crate::schema::ChunkSchema;
use crate::security::secrets::SecretsScanner;
use crate::security::SensitiveFileFilter;
use crate::vector_store::VectorStore;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
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
}

impl UnifiedIndexer {
    /// Create a new unified indexer
    ///
    /// # Arguments
    /// * `cache_path` - Path to metadata cache directory
    /// * `tantivy_path` - Path to Tantivy index directory
    /// * `qdrant_url` - Qdrant server URL (e.g., "http://localhost:6334")
    /// * `collection_name` - Qdrant collection name
    /// * `vector_size` - Vector dimensions (384 for all-MiniLM-L6-v2)
    pub async fn new(
        cache_path: &Path,
        tantivy_path: &Path,
        qdrant_url: &str,
        collection_name: &str,
        vector_size: usize,
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

        // Initialize Tantivy
        let tantivy_schema = ChunkSchema::new();
        let tantivy_index = if tantivy_path.join("meta.json").exists() {
            Index::open_in_dir(tantivy_path).context("Failed to open Tantivy index")?
        } else {
            std::fs::create_dir_all(tantivy_path)
                .context("Failed to create Tantivy directory")?;
            Index::create_in_dir(tantivy_path, tantivy_schema.schema())
                .context("Failed to create Tantivy index")?
        };

        let tantivy_writer = tantivy_index
            .writer(50_000_000) // 50MB buffer
            .context("Failed to create Tantivy writer")?;

        // Initialize Qdrant
        let vector_store = VectorStore::new(crate::vector_store::VectorStoreConfig {
            url: qdrant_url.to_string(),
            collection_name: collection_name.to_string(),
            vector_size,
        })
        .await
        .map_err(|e| anyhow::anyhow!("Failed to connect to VectorStore: {}", e))?;

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
        })
    }

    /// Index a single file to both Tantivy and Qdrant
    ///
    /// This is the core function that fixes the bug where Qdrant was never populated.
    pub async fn index_file(&mut self, file_path: &Path) -> Result<IndexFileResult> {
        // 1. Check if file should be excluded (sensitive files)
        if !self.file_filter.should_index(file_path) {
            tracing::warn!("Excluding sensitive file: {}", file_path.display());
            return Ok(IndexFileResult::Skipped);
        }

        // 2. Read file content
        let content = std::fs::read_to_string(file_path)
            .context(format!("Failed to read file: {}", file_path.display()))?;

        // 3. Check for secrets in content
        if self.secrets_scanner.should_exclude(&content) {
            let summary = self.secrets_scanner.scan_summary(&content);
            tracing::warn!(
                "Excluding file with secrets: {}\n{}",
                file_path.display(),
                summary
            );
            return Ok(IndexFileResult::Skipped);
        }

        // 4. Check if file changed (using existing metadata cache)
        let file_path_str = file_path.to_string_lossy().to_string();
        if !self.metadata_cache.has_changed(&file_path_str, &content)
            .map_err(|e| anyhow::anyhow!("Metadata cache error: {}", e))? {
            tracing::debug!("File unchanged: {}", file_path.display());
            return Ok(IndexFileResult::Unchanged);
        }

        tracing::debug!("Indexing changed file: {}", file_path.display());

        // 5. Parse with tree-sitter
        let parse_result = self
            .parser
            .parse_source_complete(&content)
            .map_err(|e| anyhow::anyhow!("Failed to parse file {}: {}", file_path.display(), e))?;

        // 6. Chunk the code (symbol-based)
        let chunks = self
            .chunker
            .chunk_file(file_path, &content, &parse_result)
            .map_err(|e| anyhow::anyhow!("Failed to chunk file {}: {}", file_path.display(), e))?;

        if chunks.is_empty() {
            tracing::warn!("No chunks generated for {}", file_path.display());
            return Ok(IndexFileResult::Skipped);
        }

        tracing::debug!("Generated {} chunks from {}", chunks.len(), file_path.display());

        // 7. Generate embeddings (batch processing)
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

        // 8. Index to both stores
        // This is the CRITICAL FIX - we now actually populate Qdrant!
        self.index_to_tantivy(&chunks)?;
        self.index_to_qdrant(&chunks, embeddings).await?;

        // 9. Update metadata cache
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

        tracing::info!(
            "✓ Indexed {} chunks from {}",
            chunks.len(),
            file_path.display()
        );

        Ok(IndexFileResult::Indexed {
            chunks_count: chunks.len(),
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
        chunks: &[CodeChunk],
        embeddings: Vec<Embedding>,
    ) -> Result<()> {
        let chunk_data: Vec<(ChunkId, Embedding, CodeChunk)> = chunks
            .iter()
            .zip(embeddings.into_iter())
            .map(|(chunk, embedding)| (chunk.id, embedding, chunk.clone()))
            .collect();

        self.vector_store
            .upsert_chunks(chunk_data)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to index to Qdrant: {}", e))?;

        Ok(())
    }

    /// Index an entire directory
    pub async fn index_directory(&mut self, dir_path: &Path) -> Result<IndexStats> {
        tracing::info!("Indexing directory: {}", dir_path.display());

        let mut stats = IndexStats::default();

        // Find all Rust files
        let rust_files: Vec<PathBuf> = WalkDir::new(dir_path)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .filter(|e| e.path().extension() == Some(std::ffi::OsStr::new("rs")))
            .map(|e| e.path().to_path_buf())
            .collect();

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

        tracing::info!(
            "✓ Indexing complete: {} files indexed, {} chunks, {} unchanged, {} skipped",
            stats.indexed_files,
            stats.total_chunks,
            stats.unchanged_files,
            stats.skipped_files
        );

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

    /// Create a Bm25Search instance from the Tantivy index
    pub fn create_bm25_search(&self) -> Result<crate::search::bm25::Bm25Search> {
        crate::search::bm25::Bm25Search::from_index(self.tantivy_index.clone())
            .map_err(|e| anyhow::anyhow!("Failed to create Bm25Search: {}", e))
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
            "http://localhost:6334",
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
            "http://localhost:6334",
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
        let count = indexer.vector_store().count().await.unwrap();
        assert!(count > 0, "Qdrant should have at least one vector");
    }
}
