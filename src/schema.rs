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
}
