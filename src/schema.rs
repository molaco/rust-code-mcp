//! Tantivy schema for file indexing with metadata tracking
//!
//! Based on bloop's indexes/schema.rs design, adapted for single-repository
//! use case with focus on persistent indexing and incremental updates.

use tantivy::schema::{
    Field, IndexRecordOption, Schema, SchemaBuilder, TextFieldIndexing, TextOptions, STORED,
    STRING,
};

/// Schema for indexing files with metadata for change detection
#[derive(Clone)]
pub struct FileSchema {
    pub schema: Schema,

    /// SHA-256 hash of file content (for change detection)
    pub unique_hash: Field,

    /// Path to the file, relative to indexed directory
    pub relative_path: Field,

    /// File content (indexed for search + stored for retrieval)
    pub content: Field,

    /// Unix timestamp of last modification
    pub last_modified: Field,

    /// File size in bytes
    pub file_size: Field,
}

impl FileSchema {
    /// Create a new file schema
    pub fn new() -> Self {
        let mut builder = SchemaBuilder::new();

        // Text field options with default tokenizer and position tracking
        let text_options = TextOptions::default().set_stored().set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer("default")
                .set_index_option(IndexRecordOption::WithFreqsAndPositions),
        );

        // Unique hash for change detection (stored but not indexed for search)
        let unique_hash = builder.add_text_field("unique_hash", STRING | STORED);

        // File path (indexed and stored)
        let relative_path = builder.add_text_field("relative_path", text_options.clone());

        // File content (indexed and stored)
        let content = builder.add_text_field("content", text_options);

        // Metadata fields (stored for filtering/sorting, not indexed for text search)
        let last_modified = builder.add_u64_field("last_modified", STORED);
        let file_size = builder.add_u64_field("file_size", STORED);

        Self {
            schema: builder.build(),
            unique_hash,
            relative_path,
            content,
            last_modified,
            file_size,
        }
    }

    /// Get the schema
    pub fn schema(&self) -> Schema {
        self.schema.clone()
    }
}

impl Default for FileSchema {
    fn default() -> Self {
        Self::new()
    }
}

/// Schema for indexing code chunks with rich context
///
/// This schema is designed for chunk-level BM25 search to complement
/// vector search in hybrid search scenarios. Each chunk represents a
/// semantic unit of code (function, struct, impl block, etc.)
#[derive(Clone)]
pub struct ChunkSchema {
    pub schema: Schema,

    /// Unique chunk ID (UUID) for deduplication in RRF
    pub chunk_id: Field,

    /// Code content (indexed and stored)
    pub content: Field,

    /// Symbol name (indexed and stored, e.g., "parse_file")
    pub symbol_name: Field,

    /// Symbol kind (stored, e.g., "function", "struct")
    pub symbol_kind: Field,

    /// File path (stored for display/filtering)
    pub file_path: Field,

    /// Module path (stored, e.g., "crate::parser::mod")
    pub module_path: Field,

    /// Docstring/documentation (indexed with higher boost, stored)
    pub docstring: Field,

    /// Full CodeChunk as JSON (stored only, for retrieval)
    pub chunk_json: Field,
}

impl ChunkSchema {
    /// Create a new chunk schema
    pub fn new() -> Self {
        let mut builder = SchemaBuilder::new();

        // Text field options for code content
        let code_options = TextOptions::default().set_stored().set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer("default")
                .set_index_option(IndexRecordOption::WithFreqsAndPositions),
        );

        // Text field options for documentation (same as code for now)
        let doc_options = TextOptions::default().set_stored().set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer("default")
                .set_index_option(IndexRecordOption::WithFreqsAndPositions),
        );

        // Chunk ID (UUID as string, stored and indexed for exact matching)
        let chunk_id = builder.add_text_field("chunk_id", STRING | STORED);

        // Code content (indexed and stored)
        let content = builder.add_text_field("content", code_options.clone());

        // Symbol name (indexed and stored)
        let symbol_name = builder.add_text_field("symbol_name", code_options);

        // Symbol kind (stored only, used for filtering)
        let symbol_kind = builder.add_text_field("symbol_kind", STRING | STORED);

        // File path (stored only)
        let file_path = builder.add_text_field("file_path", STRING | STORED);

        // Module path (stored only)
        let module_path = builder.add_text_field("module_path", STRING | STORED);

        // Docstring (indexed and stored)
        let docstring = builder.add_text_field("docstring", doc_options);

        // Full chunk as JSON (stored only)
        let chunk_json = builder.add_text_field("chunk_json", STRING | STORED);

        Self {
            schema: builder.build(),
            chunk_id,
            content,
            symbol_name,
            symbol_kind,
            file_path,
            module_path,
            docstring,
            chunk_json,
        }
    }

    /// Get the schema
    pub fn schema(&self) -> Schema {
        self.schema.clone()
    }
}

impl Default for ChunkSchema {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_creation() {
        let schema = FileSchema::new();

        // Verify all fields exist in schema
        assert!(schema.schema.get_field("unique_hash").is_ok());
        assert!(schema.schema.get_field("relative_path").is_ok());
        assert!(schema.schema.get_field("content").is_ok());
        assert!(schema.schema.get_field("last_modified").is_ok());
        assert!(schema.schema.get_field("file_size").is_ok());
    }

    #[test]
    fn test_schema_clone() {
        let schema1 = FileSchema::new();
        let schema2 = schema1.clone();

        // Both should have same field IDs
        assert_eq!(schema1.unique_hash, schema2.unique_hash);
        assert_eq!(schema1.relative_path, schema2.relative_path);
        assert_eq!(schema1.content, schema2.content);
        assert_eq!(schema1.last_modified, schema2.last_modified);
        assert_eq!(schema1.file_size, schema2.file_size);
    }

    #[test]
    fn test_chunk_schema_creation() {
        let schema = ChunkSchema::new();

        // Verify all fields exist in schema
        assert!(schema.schema.get_field("chunk_id").is_ok());
        assert!(schema.schema.get_field("content").is_ok());
        assert!(schema.schema.get_field("symbol_name").is_ok());
        assert!(schema.schema.get_field("symbol_kind").is_ok());
        assert!(schema.schema.get_field("file_path").is_ok());
        assert!(schema.schema.get_field("module_path").is_ok());
        assert!(schema.schema.get_field("docstring").is_ok());
        assert!(schema.schema.get_field("chunk_json").is_ok());
    }

    #[test]
    fn test_chunk_schema_clone() {
        let schema1 = ChunkSchema::new();
        let schema2 = schema1.clone();

        // Both should have same field IDs
        assert_eq!(schema1.chunk_id, schema2.chunk_id);
        assert_eq!(schema1.content, schema2.content);
        assert_eq!(schema1.symbol_name, schema2.symbol_name);
        assert_eq!(schema1.symbol_kind, schema2.symbol_kind);
        assert_eq!(schema1.file_path, schema2.file_path);
        assert_eq!(schema1.module_path, schema2.module_path);
        assert_eq!(schema1.docstring, schema2.docstring);
        assert_eq!(schema1.chunk_json, schema2.chunk_json);
    }
}
