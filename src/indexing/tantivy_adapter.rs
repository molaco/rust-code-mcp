//! Tantivy adapter for BM25 indexing operations
//!
//! This module encapsulates all Tantivy-specific operations, providing a clean
//! interface for indexing and searching code chunks using BM25 algorithm.

use crate::chunker::CodeChunk;
use crate::schema::ChunkSchema;
use anyhow::{Context, Result};
use std::path::Path;
use tantivy::{doc, Index, IndexWriter};

/// Configuration for TantivyAdapter
#[derive(Debug, Clone)]
pub struct TantivyConfig {
    /// Path to Tantivy index directory
    pub index_path: std::path::PathBuf,
    /// Memory budget in MB per thread
    pub memory_budget_mb: usize,
    /// Number of threads for indexing
    pub num_threads: usize,
}

impl TantivyConfig {
    /// Create configuration optimized for codebase size
    pub fn for_codebase_size(index_path: &Path, codebase_loc: Option<usize>) -> Self {
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

        Self {
            index_path: index_path.to_path_buf(),
            memory_budget_mb,
            num_threads,
        }
    }

    /// Create default configuration
    pub fn default(index_path: &Path) -> Self {
        Self {
            index_path: index_path.to_path_buf(),
            memory_budget_mb: 50,
            num_threads: 2,
        }
    }
}

/// Adapter for Tantivy BM25 indexing operations
pub struct TantivyAdapter {
    /// Tantivy index
    index: Index,
    /// Index writer
    writer: IndexWriter,
    /// Schema for code chunks
    schema: ChunkSchema,
}

impl TantivyAdapter {
    /// Create a new TantivyAdapter with configuration
    pub fn new(config: TantivyConfig) -> Result<Self> {
        let schema = ChunkSchema::new();

        // Open or create index
        let index = if config.index_path.join("meta.json").exists() {
            Index::open_in_dir(&config.index_path)
                .context("Failed to open Tantivy index")?
        } else {
            std::fs::create_dir_all(&config.index_path)
                .context("Failed to create Tantivy directory")?;
            Index::create_in_dir(&config.index_path, schema.schema())
                .context("Failed to create Tantivy index")?
        };

        // Calculate total memory budget
        let total_memory_budget =
            (config.memory_budget_mb * config.num_threads * 1024 * 1024) as usize;

        // Create writer with configuration
        let writer = index
            .writer_with_num_threads(config.num_threads, total_memory_budget)
            .context("Failed to create Tantivy writer")?;

        tracing::info!(
            "Tantivy configured: {}MB total budget, {} threads",
            config.memory_budget_mb * config.num_threads,
            config.num_threads
        );

        Ok(Self {
            index,
            writer,
            schema,
        })
    }

    /// Index a single chunk to Tantivy
    pub fn index_chunk(&mut self, chunk: &CodeChunk) -> Result<()> {
        let chunk_json = serde_json::to_string(chunk)
            .context("Failed to serialize chunk to JSON")?;

        self.writer
            .add_document(doc!(
                self.schema.chunk_id => chunk.id.to_string(),
                self.schema.content => chunk.content.clone(),
                self.schema.symbol_name => chunk.context.symbol_name.clone(),
                self.schema.symbol_kind => chunk.context.symbol_kind.clone(),
                self.schema.file_path => chunk.context.file_path.display().to_string(),
                self.schema.module_path => chunk.context.module_path.join("::"),
                self.schema.docstring => chunk.context.docstring.clone().unwrap_or_default(),
                self.schema.chunk_json => chunk_json,
            ))
            .context("Failed to add document to Tantivy")?;

        Ok(())
    }

    /// Index multiple chunks to Tantivy
    pub fn index_chunks(&mut self, chunks: &[CodeChunk]) -> Result<()> {
        for chunk in chunks {
            self.index_chunk(chunk)?;
        }
        Ok(())
    }

    /// Delete all chunks for a specific file
    pub fn delete_file_chunks(&mut self, file_path: &Path) -> Result<()> {
        let file_path_str = file_path.to_string_lossy().to_string();

        let term = tantivy::Term::from_field_text(
            self.schema.file_path,
            &file_path_str,
        );
        let query = tantivy::query::TermQuery::new(
            term,
            tantivy::schema::IndexRecordOption::Basic,
        );

        self.writer.delete_query(Box::new(query))?;

        tracing::debug!("Deleted Tantivy chunks for file: {}", file_path_str);
        Ok(())
    }

    /// Delete all documents from the index
    pub fn delete_all(&mut self) -> Result<()> {
        self.writer.delete_all_documents()
            .context("Failed to delete all Tantivy documents")?;
        Ok(())
    }

    /// Commit all pending changes
    pub fn commit(&mut self) -> Result<()> {
        self.writer
            .commit()
            .context("Failed to commit Tantivy index")?;
        Ok(())
    }

    /// Rollback uncommitted changes
    pub fn rollback(&mut self) -> Result<()> {
        self.writer
            .rollback()
            .context("Failed to rollback Tantivy writer")?;
        Ok(())
    }

    /// Get reference to the Tantivy index
    pub fn index(&self) -> &Index {
        &self.index
    }

    /// Get reference to the schema
    pub fn schema(&self) -> &ChunkSchema {
        &self.schema
    }

    /// Create a Bm25Search instance from this adapter
    pub fn create_bm25_search(&self) -> Result<crate::search::bm25::Bm25Search> {
        crate::search::bm25::Bm25Search::from_index(self.index.clone())
            .map_err(|e| anyhow::anyhow!("Failed to create Bm25Search: {}", e))
    }
}

impl Drop for TantivyAdapter {
    fn drop(&mut self) {
        // Attempt to rollback any uncommitted changes to release the lock
        if let Err(e) = self.writer.rollback() {
            tracing::warn!("Failed to rollback Tantivy writer during drop: {}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_tantivy_adapter_creation() {
        let temp_dir = TempDir::new().unwrap();
        let config = TantivyConfig::default(temp_dir.path());

        let adapter = TantivyAdapter::new(config);
        assert!(adapter.is_ok(), "Failed to create TantivyAdapter: {:?}", adapter.err());
    }

    #[test]
    fn test_config_for_codebase_size() {
        let temp_dir = TempDir::new().unwrap();

        // Small codebase
        let config = TantivyConfig::for_codebase_size(temp_dir.path(), Some(50_000));
        assert_eq!(config.memory_budget_mb, 50);
        assert_eq!(config.num_threads, 2);

        // Medium codebase
        let config = TantivyConfig::for_codebase_size(temp_dir.path(), Some(500_000));
        assert_eq!(config.memory_budget_mb, 100);
        assert_eq!(config.num_threads, 4);

        // Large codebase
        let config = TantivyConfig::for_codebase_size(temp_dir.path(), Some(2_000_000));
        assert_eq!(config.memory_budget_mb, 200);
        assert_eq!(config.num_threads, 8);
    }
}
