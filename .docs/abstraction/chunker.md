# chunker — Abstract Logic

## Module: chunker
**Purpose:** Splits parsed source files into per-symbol code chunks enriched with context headers and overlap windows for downstream embedding.

1. **Generate, stringify, and parse opaque chunk identifiers backed by UUIDv4** -> `ChunkId::new()`, `ChunkId::to_string()`, `ChunkId::from_string()`, `ChunkId::default()`
2. **Render a chunk into an embedding-ready string by prepending file/location/module/symbol/doc/imports/calls header lines before the body** -> `CodeChunk::format_for_embedding()`
3. **Construct a chunker with a default or clamped overlap percentage** -> `Chunker::new()`, `Chunker::with_overlap()`, `Chunker::default()`
4. **Chunk a parsed file by walking each symbol, slicing its source, attaching module/import/call-graph context, and stitching adjacent overlap windows** -> `Chunker::chunk_file()`
5. **Slice the exact source lines belonging to a symbol with safe bounds clamping** -> `Chunker::extract_symbol_code()`
6. **Derive the `crate::...` module path from a file path by walking components past `src` and stripping `.rs`/`mod` segments** -> `Chunker::extract_module_path()`
7. **Populate per-chunk previous/next overlap fields by sampling line tails and heads from neighboring chunks** -> `Chunker::add_overlap()`, `Chunker::calculate_overlap()`
