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

/// Version of [`ChunkSchema`]. Part of the BM25 index directory's path
/// ([`bm25_index_dir`]), so an index of one version is never opened with the
/// schema of another.
///
/// The path, not a field inside the index, carries it because two daemons of
/// different builds run side by side while the old one still has clients: a
/// new build writing its schema under the old build's reader is the tantivy
/// schema panic, and a new build reading the old layout finds fields it does
/// not know. Separate directories make the two builds simply not meet.
///
/// History:
/// - 1 — the original layout (directly in `index/<dir_hash>/`): every chunk
///   field stored, and the whole chunk JSON indexed as ONE `STRING` term. That
///   term made the dictionary ~85 % of a 17 GB index, rewritten by every merge.
/// - 2 — an inverted index and nothing else: only the queried fields are
///   indexed, only `chunk_id` is stored; the chunk itself is read from the
///   vector store, which holds it anyway.
pub const BM25_SCHEMA_VERSION: u32 = 2;

/// Directory of the BM25 index of [`BM25_SCHEMA_VERSION`] over the vector
/// collection `collection_name`, inside the project's index directory
/// (`<data>/index/<dir_hash>`).
///
/// One per collection, because the index holds that collection's chunk ids
/// and nothing else: the chunk of a hit is read from the collection. A
/// project has a collection per embedding profile and chunking identity, and
/// a BM25 index shared between two of them answers with ids the other does
/// not have — so it was, with one directory per project, between the CPU and
/// the GPU profile and between a daemon and its successor with a new
/// chunking identity, both writing into it.
///
/// Inside the project's directory, not next to it: disk tooling treats
/// `index/<dir_hash>` as the project's whole keyword index and deletes it as
/// one, and a sibling with a suffix would be an unknown name to it. tantivy
/// never touches a subdirectory of its own directory — its garbage
/// collection removes only the files it lists in `.managed.json` — so a
/// version-1 index living in the parent directory and these do not
/// interfere.
pub fn bm25_index_dir(
    project_index_dir: &std::path::Path,
    collection_name: &str,
) -> std::path::PathBuf {
    project_index_dir.join(format!("bm25-v{BM25_SCHEMA_VERSION}-{collection_name}"))
}

/// Schema for chunk-level BM25 search — an inverted index of chunk ids.
///
/// Answers "which chunks match these words" and nothing else. Everything
/// about a chunk beyond its id comes from the vector store
/// (`VectorStore::chunks_by_ids`), which stores the full chunk JSON for every
/// chunk already; keeping a second copy here was most of this index's size,
/// and every segment merge copied it again.
///
/// What each field is for, and why its options are what they are:
/// - `chunk_id` — `STRING | STORED`: the one value a hit returns. Indexed as a
///   term too, which costs one dictionary entry per chunk and lets the
///   consistency check look a chunk up by id.
/// - `file_path` — `STRING`, not stored: the incremental indexer deletes a
///   changed file's chunks by this term; nothing reads it back.
/// - `content`, `symbol_name`, `docstring` — tokenized, not stored: the
///   fields the query parser searches. Positions are kept: the default
///   tokenizer splits `parse_file` into `parse`, `file`, and the query
///   parser turns a query word that yields several tokens into a phrase
///   query, which needs positions — without them a search for any
///   snake_case identifier is a query error.
#[derive(Clone)]
pub struct ChunkSchema {
    pub schema: Schema,

    /// Unique chunk ID (UUID) — the hit's identity, used to fuse with vector
    /// results and to fetch the chunk from the vector store.
    pub chunk_id: Field,

    /// Code content (indexed only)
    pub content: Field,

    /// Symbol name (indexed only, e.g., "parse_file")
    pub symbol_name: Field,

    /// File path (indexed as one term, for deleting a file's chunks)
    pub file_path: Field,

    /// Docstring/documentation (indexed only)
    pub docstring: Field,
}

impl ChunkSchema {
    /// Create a new chunk schema
    pub fn new() -> Self {
        let mut builder = SchemaBuilder::new();

        // Searched text: indexed with positions, not stored (see the type docs).
        let text_options = TextOptions::default().set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer("default")
                .set_index_option(IndexRecordOption::WithFreqsAndPositions),
        );

        let chunk_id = builder.add_text_field("chunk_id", STRING | STORED);
        let content = builder.add_text_field("content", text_options.clone());
        let symbol_name = builder.add_text_field("symbol_name", text_options.clone());
        let file_path = builder.add_text_field("file_path", STRING);
        let docstring = builder.add_text_field("docstring", text_options);

        Self {
            schema: builder.build(),
            chunk_id,
            content,
            symbol_name,
            file_path,
            docstring,
        }
    }

    /// The document of `chunk` in this schema — the only place a chunk becomes
    /// a BM25 document, so the indexer and the rebuild from the vector store
    /// cannot drift apart.
    pub fn document(&self, chunk: &crate::chunker::CodeChunk) -> tantivy::TantivyDocument {
        // `doc!` expands to itself recursively, so it has to be in scope by
        // its bare name.
        use tantivy::doc;
        doc!(
            self.chunk_id => chunk.id.to_string(),
            self.content => chunk.content.clone(),
            self.symbol_name => chunk.context.symbol_name.clone(),
            self.file_path => chunk.context.file_path.display().to_string(),
            self.docstring => chunk.context.docstring.clone().unwrap_or_default(),
        )
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
        assert!(schema.schema.get_field("file_path").is_ok());
        assert!(schema.schema.get_field("docstring").is_ok());
    }

    /// The point of version 2: the index stores the chunk id and nothing
    /// else, and no field is a whole-value term of the chunk's text. Either
    /// one coming back is the 17 GB index again.
    #[test]
    fn chunk_schema_stores_only_the_chunk_id() {
        let schema = ChunkSchema::new();
        let stored: Vec<&str> = schema
            .schema
            .fields()
            .filter(|(_, entry)| entry.is_stored())
            .map(|(_, entry)| entry.name())
            .collect();
        assert_eq!(stored, vec!["chunk_id"]);

        for name in ["chunk_json", "symbol_kind", "module_path"] {
            assert!(schema.schema.get_field(name).is_err(), "{name} is back");
        }
        // `content` must be tokenized: as a raw (`STRING`) field every chunk
        // would be one dictionary term of its whole text.
        let content = schema.schema.get_field_entry(schema.content);
        let tokenizer = match content.field_type() {
            tantivy::schema::FieldType::Str(options) => options
                .get_indexing_options()
                .map(|indexing| indexing.tokenizer().to_string()),
            other => panic!("content is {other:?}"),
        };
        assert_eq!(tokenizer.as_deref(), Some("default"));
    }

    #[test]
    fn bm25_index_dir_is_versioned_per_collection_inside_the_project_index_dir() {
        let project = std::path::Path::new("/data/index/abc");
        assert_eq!(
            bm25_index_dir(project, "code_chunks_abc_def"),
            project.join(format!("bm25-v{BM25_SCHEMA_VERSION}-code_chunks_abc_def"))
        );
        assert_ne!(
            bm25_index_dir(project, "code_chunks_abc_def"),
            bm25_index_dir(project, "code_chunks_abc_123")
        );
    }

    #[test]
    fn test_chunk_schema_clone() {
        let schema1 = ChunkSchema::new();
        let schema2 = schema1.clone();

        // Both should have same field IDs
        assert_eq!(schema1.chunk_id, schema2.chunk_id);
        assert_eq!(schema1.content, schema2.content);
        assert_eq!(schema1.symbol_name, schema2.symbol_name);
        assert_eq!(schema1.file_path, schema2.file_path);
        assert_eq!(schema1.docstring, schema2.docstring);
    }
}
