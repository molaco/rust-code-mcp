# root — Abstract Logic

## Module: lib.rs
**Purpose:** Crate-root manifest that declares public modules and sets a crate-wide lint policy; contains no executable logic.

1. **Apply crate-wide lints** -> `#![warn(unreachable_pub, dead_code)]`
2. **Expose all top-level subsystems as public modules** -> `pub mod chunker`, `pub mod config`, `pub mod embeddings`, `pub mod indexing`, `pub mod mcp`, `pub mod metadata_cache`, `pub mod metrics`, `pub mod monitoring`, `pub mod parser`, `pub mod schema`, `pub mod search`, `pub mod security`, `pub mod tools`, `pub mod vector_store`, `pub mod semantic`, `pub mod graph`

## Module: main.rs
**Purpose:** Binary entrypoint that boots tracing, the background sync manager, and the rmcp stdio service.

1. **Configure tracing with stderr writer and env-driven filter** -> `EnvFilter::try_from_default_env()`, `EnvFilter::new()`, `tracing_subscriber::fmt()`, `fmt::with_env_filter()`, `fmt::with_writer()`, `fmt::with_ansi()`, `fmt::init()`
2. **Construct and share the periodic sync manager** -> `SyncManager::with_defaults()`, `Arc::new()`, `Arc::clone()`
3. **Spawn the background sync loop on the Tokio runtime** -> `tokio::spawn()`, `SyncManager::run()`
4. **Build the MCP service and serve it over stdio until disconnect** -> `SearchTool::with_sync_manager()`, `ServiceExt::serve()`, `rmcp::transport::stdio()`, `service.waiting()`
5. **Log and propagate any serving error** -> `Result::inspect_err()`, `tracing::error!`, `tracing::info!`

## Module: schema.rs
**Purpose:** Defines the two Tantivy schemas (`FileSchema` for whole-file documents, `ChunkSchema` for symbol-aware chunks) used by the indexing layer.

1. **Build the file-level schema with hash, path, content, mtime, and size fields** -> `FileSchema::new()`, `FileSchema::default()`
2. **Build the chunk-level schema with symbol context and stored chunk JSON** -> `ChunkSchema::new()`, `ChunkSchema::default()`
3. **Expose cheap clones of the inner Tantivy schema for index/searcher construction** -> `FileSchema::schema()`, `ChunkSchema::schema()`

## Module: metadata_cache.rs
**Purpose:** Sled-backed persistent file-metadata cache that drives incremental indexing via mtime/size pre-checks and SHA-256 content hashing.

1. **Capture cheap filesystem stats without reading file contents** -> `FileStat::from_path()`
2. **Compute and assemble per-file metadata records (hash, mtime, size, indexed_at)** -> `FileMetadata::from_content()`, `FileMetadata::hash_content()`
3. **Open or create the embedded sled database, ensuring parent directories exist** -> `MetadataCache::new()`
4. **Read, write, and delete cached metadata entries by file path** -> `MetadataCache::get()`, `MetadataCache::set()`, `MetadataCache::remove()`
5. **Decide whether a file needs reindexing via fast stat check then content hash** -> `MetadataCache::has_stat_changed()`, `MetadataCache::has_changed()`
6. **Enumerate and manage the cache as a whole** -> `MetadataCache::list_files()`, `MetadataCache::clear()`, `MetadataCache::len()`, `MetadataCache::is_empty()`

## Module: bin/test_tools_direct.rs
**Purpose:** Standalone smoke-test binary that drives `RustParser` and stdlib IO against a hard-coded sibling project, bypassing the MCP layer.

1. **Read a source file and print its first lines** -> `fs::read_to_string()`, `String::lines()`, `Iterator::take()`
2. **Construct the tree-sitter Rust parser and walk the source tree** -> `RustParser::new()`, `fs::read_dir()`, `Path::is_dir()`, `Path::join()`, `Path::exists()`
3. **Locate a target symbol by parsing candidate modules** -> `RustParser::parse_file()`, `SymbolKind::as_str()`
4. **Inspect a module's imports via the full parse result** -> `RustParser::parse_file_complete()`
5. **Compute simple complexity metrics (LOC, function count, call-graph edges)** -> `RustParser::parse_file_complete()`, `Iterator::filter()`, `Iterator::count()`, `CallGraph::edge_count()`, pattern match on `SymbolKind::Function { .. }`
