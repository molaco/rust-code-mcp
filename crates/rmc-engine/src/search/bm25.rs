//! BM25 (lexical) search for code chunks using Tantivy
//!
//! Provides chunk-level BM25 search to complement vector search in hybrid scenarios.

use crate::chunker::ChunkId;
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
    /// Returns the ids of the matching chunks with their BM25 scores, sorted
    /// by relevance (highest first). The query is parsed against content,
    /// symbol_name, and docstring fields.
    ///
    /// Ids only: the index stores nothing else (see [`ChunkSchema`]). The
    /// chunks are read from the vector store, and only for the hits that make
    /// it into the answer.
    pub fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<(ChunkId, f32)>, Box<dyn std::error::Error + Send>> {
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

            results.push((chunk_id, score));
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
                parent_symbol_name: None,
                split_part: None,
                split_total: None,
                line_start: 1,
                line_end: 5,
            },
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

        index_writer.add_document(schema.document(&chunk1)).unwrap();
        index_writer.add_document(schema.document(&chunk2)).unwrap();
        index_writer.commit().unwrap();

        // Search for "parse"
        let mut bm25_search_mut = bm25_search.clone();
        bm25_search_mut.reload().unwrap();

        let results = bm25_search_mut.search("parse", 10).unwrap();

        // Should find chunk1 (parse_file)
        assert!(!results.is_empty());
        assert_eq!(results[0].0, chunk1_id);

        // A snake_case identifier tokenizes into several tokens, which the
        // query parser turns into a phrase query: this is what fails with a
        // "no positions" error if the schema ever drops positions.
        let results = bm25_search_mut.search("read_content", 10).unwrap();
        assert_eq!(results.first().map(|hit| hit.0), Some(chunk2_id));
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

        index_writer.add_document(schema.document(&chunk)).unwrap();
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
