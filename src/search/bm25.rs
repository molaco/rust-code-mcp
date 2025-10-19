//! BM25 (lexical) search for code chunks using Tantivy
//!
//! Provides chunk-level BM25 search to complement vector search in hybrid scenarios.

use crate::chunker::{ChunkId, CodeChunk};
use crate::schema::ChunkSchema;
use std::path::Path;
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::Value;
use tantivy::{Index, IndexReader, TantivyDocument};

/// BM25 search for code chunks
///
/// This wraps a Tantivy index configured with ChunkSchema, providing
/// chunk-level lexical search that can be merged with vector search.
pub struct Bm25Search {
    index: Index,
    schema: ChunkSchema,
    reader: IndexReader,
}

impl Bm25Search {
    /// Create a new BM25 search instance from an existing index
    ///
    /// Opens an index at the given path. The index should have been created
    /// with ChunkSchema.
    pub fn new(index_path: &Path) -> Result<Self, Box<dyn std::error::Error + Send>> {
        let schema = ChunkSchema::new();

        // Open existing index
        let index = if index_path.join("meta.json").exists() {
            Index::open_in_dir(index_path).map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?
        } else {
            // Create new index if doesn't exist
            std::fs::create_dir_all(index_path).map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;
            Index::create_in_dir(index_path, schema.schema()).map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?
        };

        let reader = index.reader().map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;

        Ok(Self {
            index,
            schema,
            reader,
        })
    }

    /// Create a BM25 search instance from an existing Tantivy Index
    ///
    /// This is useful when you already have an Index instance (e.g., from UnifiedIndexer)
    pub fn from_index(index: Index) -> Result<Self, Box<dyn std::error::Error + Send>> {
        let schema = ChunkSchema::new();
        let reader = index.reader().map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;

        Ok(Self {
            index,
            schema,
            reader,
        })
    }

    /// Search for chunks matching a query
    ///
    /// Returns chunks with their BM25 scores, sorted by relevance (highest first).
    /// The query is parsed against content, symbol_name, and docstring fields.
    pub fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<(ChunkId, f32, CodeChunk)>, Box<dyn std::error::Error + Send>> {
        let searcher = self.reader.searcher();

        // Parse query across multiple fields
        let query_parser = QueryParser::for_index(
            &self.index,
            vec![
                self.schema.content,      // Main code content
                self.schema.symbol_name,  // Symbol names
                self.schema.docstring,    // Documentation
            ],
        );

        let query = query_parser.parse_query(query).map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;

        // Search with limit
        let top_docs = searcher.search(&query, &TopDocs::with_limit(limit)).map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;

        // Convert Tantivy results to our format
        let mut results = Vec::new();
        for (score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher.doc(doc_address).map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;

            // Extract chunk_id
            let chunk_id_str = doc
                .get_first(self.schema.chunk_id)
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    Box::new(std::io::Error::new(std::io::ErrorKind::NotFound, "Missing chunk_id field"))
                        as Box<dyn std::error::Error + Send>
                })?;
            let chunk_id = ChunkId::from_string(chunk_id_str).map_err(|e| {
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{:?}", e)))
                    as Box<dyn std::error::Error + Send>
            })?;

            // Deserialize CodeChunk from JSON
            let chunk_json = doc
                .get_first(self.schema.chunk_json)
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    Box::new(std::io::Error::new(std::io::ErrorKind::NotFound, "Missing chunk_json field"))
                        as Box<dyn std::error::Error + Send>
                })?;
            let chunk: CodeChunk = serde_json::from_str(chunk_json).map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;

            results.push((chunk_id, score, chunk));
        }

        Ok(results)
    }

    /// Get the index for writing operations
    ///
    /// This allows external code to add documents to the index.
    pub fn index(&self) -> &Index {
        &self.index
    }

    /// Get the schema
    pub fn schema(&self) -> &ChunkSchema {
        &self.schema
    }

    /// Reload the index reader to see newly committed documents
    ///
    /// Call this after committing new documents to make them searchable.
    pub fn reload(&mut self) -> Result<(), Box<dyn std::error::Error + Send>> {
        self.reader.reload().map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?;
        Ok(())
    }
}

// Make Bm25Search cloneable by sharing the reader via Arc
// This is needed for parallel execution in HybridSearch
impl Clone for Bm25Search {
    fn clone(&self) -> Self {
        Self {
            index: self.index.clone(),
            schema: self.schema.clone(),
            reader: self.reader.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunker::{ChunkContext, CodeChunk};
    use std::path::PathBuf;
    use tantivy::doc;
    use tempfile::TempDir;

    fn create_test_chunk(id: ChunkId, name: &str, content: &str) -> CodeChunk {
        CodeChunk {
            id,
            content: content.to_string(),
            context: ChunkContext {
                file_path: PathBuf::from("test.rs"),
                module_path: vec!["crate".to_string()],
                symbol_name: name.to_string(),
                symbol_kind: "function".to_string(),
                docstring: Some(format!("Test function {}", name)),
                imports: vec![],
                outgoing_calls: vec![],
                line_start: 1,
                line_end: 5,
            },
            overlap_prev: None,
            overlap_next: None,
        }
    }

    #[test]
    fn test_bm25_search_creation() {
        let temp_dir = TempDir::new().unwrap();
        let index_path = temp_dir.path().join("bm25_index");

        let bm25_search = Bm25Search::new(&index_path);
        assert!(bm25_search.is_ok());
    }

    #[test]
    fn test_bm25_index_and_search() {
        let temp_dir = TempDir::new().unwrap();
        let index_path = temp_dir.path().join("bm25_index");

        let bm25_search = Bm25Search::new(&index_path).unwrap();

        // Create test chunks
        let chunk1_id = ChunkId::new();
        let chunk1 = create_test_chunk(
            chunk1_id,
            "parse_file",
            "fn parse_file(path: &Path) -> Result<String> { }"
        );

        let chunk2_id = ChunkId::new();
        let chunk2 = create_test_chunk(
            chunk2_id,
            "read_content",
            "fn read_content(file: &str) -> String { }"
        );

        // Index chunks
        let mut index_writer = bm25_search.index().writer(50_000_000).unwrap();
        let schema = bm25_search.schema();

        // Add chunk1
        let chunk1_json = serde_json::to_string(&chunk1).unwrap();
        index_writer.add_document(doc!(
            schema.chunk_id => chunk1_id.to_string(),
            schema.content => chunk1.content.clone(),
            schema.symbol_name => chunk1.context.symbol_name.clone(),
            schema.symbol_kind => chunk1.context.symbol_kind.clone(),
            schema.file_path => chunk1.context.file_path.display().to_string(),
            schema.module_path => chunk1.context.module_path.join("::"),
            schema.docstring => chunk1.context.docstring.clone().unwrap_or_default(),
            schema.chunk_json => chunk1_json,
        )).unwrap();

        // Add chunk2
        let chunk2_json = serde_json::to_string(&chunk2).unwrap();
        index_writer.add_document(doc!(
            schema.chunk_id => chunk2_id.to_string(),
            schema.content => chunk2.content.clone(),
            schema.symbol_name => chunk2.context.symbol_name.clone(),
            schema.symbol_kind => chunk2.context.symbol_kind.clone(),
            schema.file_path => chunk2.context.file_path.display().to_string(),
            schema.module_path => chunk2.context.module_path.join("::"),
            schema.docstring => chunk2.context.docstring.clone().unwrap_or_default(),
            schema.chunk_json => chunk2_json,
        )).unwrap();

        index_writer.commit().unwrap();

        // Search for "parse"
        let mut bm25_search_mut = bm25_search.clone();
        bm25_search_mut.reload().unwrap();

        let results = bm25_search_mut.search("parse", 10).unwrap();

        // Should find chunk1 (parse_file)
        assert!(!results.is_empty());
        assert_eq!(results[0].0, chunk1_id);
        assert_eq!(results[0].2.context.symbol_name, "parse_file");
    }

    #[test]
    fn test_bm25_search_multiple_fields() {
        let temp_dir = TempDir::new().unwrap();
        let index_path = temp_dir.path().join("bm25_index");

        let bm25_search = Bm25Search::new(&index_path).unwrap();

        // Create chunk with "error" in docstring
        let chunk_id = ChunkId::new();
        let mut chunk = create_test_chunk(
            chunk_id,
            "handle_result",
            "fn handle_result() { }"
        );
        chunk.context.docstring = Some("Handles error cases".to_string());

        // Index it
        let mut index_writer = bm25_search.index().writer(50_000_000).unwrap();
        let schema = bm25_search.schema();

        let chunk_json = serde_json::to_string(&chunk).unwrap();
        index_writer.add_document(doc!(
            schema.chunk_id => chunk_id.to_string(),
            schema.content => chunk.content.clone(),
            schema.symbol_name => chunk.context.symbol_name.clone(),
            schema.symbol_kind => chunk.context.symbol_kind.clone(),
            schema.file_path => chunk.context.file_path.display().to_string(),
            schema.module_path => chunk.context.module_path.join("::"),
            schema.docstring => chunk.context.docstring.clone().unwrap(),
            schema.chunk_json => chunk_json,
        )).unwrap();

        index_writer.commit().unwrap();

        // Search for "error" (in docstring, not in content or symbol name)
        let mut bm25_search_mut = bm25_search.clone();
        bm25_search_mut.reload().unwrap();

        let results = bm25_search_mut.search("error", 10).unwrap();

        // Should find the chunk via docstring match
        assert!(!results.is_empty());
        assert_eq!(results[0].0, chunk_id);
    }
}
