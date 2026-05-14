# root — Detailed Logic

## Module: lib.rs

The crate root `src/lib.rs` is a thin module-declaration manifest. It contains no executable logic of its own; it only declares the public modules that compose the crate and sets a crate-wide lint configuration.

- Doc comment: `//! Rust Code MCP - Scalable code search for large Rust codebases` with the sub-line `//! Library modules for the MCP server`.
- Lint attribute: `#![warn(unreachable_pub, dead_code)]`.
- Public modules declared (in order): `chunker`, `config`, `embeddings`, `indexing`, `mcp`, `metadata_cache`, `metrics`, `monitoring`, `parser`, `schema`, `search`, `security`, `tools`, `vector_store`, `semantic`, `graph`.
- A commented-out reservation exists for `pub mod watcher;` (planned future module).

There are no `pub fn`, `pub struct`, or `impl` blocks in this file, so there is no per-function logic to document.

## Module: main.rs

The binary entrypoint for the MCP server. Wires up tracing, constructs the background sync manager, spawns the periodic sync task, and starts the rmcp service over stdio transport.

### `main() -> Result<(), Box<dyn std::error::Error>>`
**Call graph:** `main` -> `EnvFilter::try_from_default_env`, `EnvFilter::new`, `tracing_subscriber::fmt`, `fmt::with_env_filter`, `fmt::with_writer`, `std::io::stderr`, `fmt::with_ansi`, `fmt::init`, `tracing::info!`, `SyncManager::with_defaults`, `Arc::new`, `Arc::clone`, `tokio::spawn`, `SyncManager::run`, `SearchTool::with_sync_manager`, `ServiceExt::serve`, `rmcp::transport::stdio`, `Result::inspect_err`, `tracing::error!`, `service.waiting`

**Annotations:** Decorated with `#[tokio::main]`, so the async function is wrapped in a Tokio multi-thread runtime by the macro at compile time.

**Steps:**
1. Build a `tracing` `EnvFilter`. Try to parse the filter from the `RUST_LOG` environment variable via `EnvFilter::try_from_default_env`; on error fall back to a hard-coded default `"warn,file_search_mcp=info"` (WARN globally, INFO for the crate). The in-source rationale comment notes that rust-analyzer floods debug logs and that keeping the global level at WARN avoids a 7s -> 7+ minute regression in `build_hypergraph` caused purely by log formatting overhead.
2. Initialize the global tracing subscriber: `tracing_subscriber::fmt()` with the chosen `EnvFilter`, writer set to `std::io::stderr` (stdout is reserved for the MCP stdio protocol), and ANSI escapes disabled (`with_ansi(false)`); call `.init()` to install it as the global default.
3. Emit `tracing::info!("Starting MCP Server...")`.
4. Construct the background sync manager: `SyncManager::with_defaults(300)` (5-minute interval) wrapped in an `Arc` so it can be shared with the spawned task and the `SearchTool`. Log `"Created background sync manager (5-minute interval)"`.
5. Clone the `Arc` (`sync_manager_clone`) and `tokio::spawn` an async block that calls `sync_manager_clone.run().await`, which is the long-running periodic sync loop. Log `"Started background sync task"`.
6. Create the MCP service: `SearchTool::with_sync_manager(Arc::clone(&sync_manager))` produces a `SearchTool` instance bound to the shared sync manager, then `.serve(stdio())` from `rmcp::ServiceExt` starts serving the rmcp protocol over stdin/stdout. The future is awaited.
7. If `serve(...).await` returns an error, `inspect_err` logs it via `tracing::error!("serving error: {:?}", e)` before propagating the error with `?`.
8. On success, call `service.waiting().await` to block the task until the rmcp service finishes (typically when stdin closes or the client disconnects). Propagate any error via `?`.
9. Return `Ok(())`.

## Module: config.rs

The top-level configuration module. It re-exports two submodule namespaces (`errors`, `indexer`) and defines the global `Config` struct populated either from defaults or from environment variables. This is the server's process-wide knob panel (port, data directory, file-size limit, threading, retry policy, debug flag).

The file also declares the two submodules with `pub mod errors;` and `pub mod indexer;`.

### Re-exports
- `pub use errors::{Error, ErrorContextExt, Result};` — the crate-wide error type, the `?`-friendly `Result` alias, and the `ErrorContextExt` trait for attaching context to errors.
- `pub use indexer::{IndexerConfig, IndexerCoreConfig, TantivyConfig};` — indexer-specific config types.

### `struct Config`
Derives `Debug, Clone`. Public fields:
- `server_port: u16` — MCP server port (default `3000`).
- `data_dir: PathBuf` — root directory for indexes and cache.
- `max_file_size: u64` — maximum file size to index, in bytes (default `10_000_000`).
- `num_threads: usize` — parallel worker count, `0` means auto-detect.
- `debug: bool` — extra logging flag.
- `retry_attempts: u32` — retry budget for transient failures (default `3`).
- `retry_delay_ms: u64` — initial retry delay in milliseconds (default `100`).

### `impl Default for Config`
#### `default() -> Self`
**Call graph:** `Config::default` -> `default_data_dir`, `PathBuf::from`

**Steps:**
1. Construct a `Config` literal with `server_port: 3000`, `data_dir: default_data_dir()`, `max_file_size: 10_000_000` (10 MB), `num_threads: 0` (auto-detect), `debug: false`, `retry_attempts: 3`, `retry_delay_ms: 100`.
2. Return it.

### `Config::from_env() -> Self`
**Call graph:** `Config::from_env` -> `Config::default`, `std::env::var`, `str::parse::<u16>`, `PathBuf::from`, `str::parse::<u64>`, `str::parse::<usize>`, `str::eq_ignore_ascii_case`, `str::parse::<u32>`

**Steps:**
1. Start from `Self::default()`.
2. If `SERVER_PORT` is set and parses as `u16`, overwrite `config.server_port`. A parse failure is silently ignored, leaving the default.
3. If `DATA_DIR` is set, overwrite `config.data_dir` with `PathBuf::from(dir)`.
4. If `MAX_FILE_SIZE` is set and parses as `u64`, multiply by `1_000_000` (interpret env value as megabytes) and store in `config.max_file_size`.
5. If `NUM_THREADS` is set and parses as `usize`, overwrite `config.num_threads`.
6. If `DEBUG` is set, treat it as truthy when it equals `"true"` (case-insensitive) or `"1"`.
7. If `RETRY_ATTEMPTS` is set and parses as `u32`, overwrite `config.retry_attempts`.
8. If `RETRY_DELAY_MS` is set and parses as `u64`, overwrite `config.retry_delay_ms`.
9. Return the populated `config`. Parse failures on any var are silently ignored; the prior (default or previously-overridden) value is kept.

### `Config::tantivy_dir(&self) -> PathBuf`
**Call graph:** `Config::tantivy_dir` -> `PathBuf::join`

**Steps:**
1. Return `self.data_dir.join("tantivy")`.

### `Config::cache_dir(&self) -> PathBuf`
**Call graph:** `Config::cache_dir` -> `PathBuf::join`

**Steps:**
1. Return `self.data_dir.join("cache")`.

### `Config::print_summary(&self)`
**Call graph:** `Config::print_summary` -> `usize::to_string`, `tracing::info!`, `Path::display`

**Steps:**
1. Compute a human-readable threads value: `"auto"` if `self.num_threads == 0`, else `self.num_threads.to_string()`.
2. Emit one structured `tracing::info!` event tagged `"Configuration summary"` with fields `server_port`, `data_dir` (via `Display`), `max_file_size_mb` (= `self.max_file_size / 1_000_000`), `threads`, `debug`, `retry_attempts`, `retry_delay_ms`.

The doc comment explicitly warns that MCP stdio servers must keep stdout for JSON-RPC; that is why this method logs via `tracing` (stderr) instead of `println!`.

### `default_data_dir() -> PathBuf` (private)
**Call graph:** `default_data_dir` -> `directories::ProjectDirs::from`, `ProjectDirs::data_dir`, `Path::to_path_buf`, `PathBuf::from`

**Steps:**
1. Call `directories::ProjectDirs::from("com", "rust-code-mcp", "rust-code-mcp")` to discover the OS-appropriate data directory.
2. If `Some(dirs)`: return `dirs.data_dir().to_path_buf()` (e.g. `~/.local/share/rust-code-mcp` on Linux).
3. If `None`: fall back to `PathBuf::from("./data")` (current working directory).

## Module: schema.rs

Defines two Tantivy schemas used by the indexing layer: `FileSchema` (per-file documents with content + metadata) and `ChunkSchema` (per-chunk documents with rich symbol context for hybrid BM25/vector search).

### `struct FileSchema`
Public fields: `schema: Schema`, `unique_hash: Field`, `relative_path: Field`, `content: Field`, `last_modified: Field`, `file_size: Field`. Derives `Clone`.

### `FileSchema::new() -> Self`
**Call graph:** `FileSchema::new` -> `SchemaBuilder::new`, `TextOptions::default`, `TextOptions::set_stored`, `TextOptions::set_indexing_options`, `TextFieldIndexing::default`, `TextFieldIndexing::set_tokenizer`, `TextFieldIndexing::set_index_option`, `SchemaBuilder::add_text_field`, `SchemaBuilder::add_u64_field`, `SchemaBuilder::build`

**Steps:**
1. Allocate a fresh `SchemaBuilder` via `SchemaBuilder::new()`.
2. Build a reusable `TextOptions` value: `TextOptions::default().set_stored().set_indexing_options(TextFieldIndexing::default().set_tokenizer("default").set_index_option(IndexRecordOption::WithFreqsAndPositions))`. Stored + tokenized with frequencies and positions (needed for phrase/proximity queries).
3. Add the `unique_hash` text field with options `STRING | STORED` (untokenized exact match, stored). This is the SHA-256 used for change detection.
4. Add `relative_path` as a text field using a clone of `text_options` (indexed and stored).
5. Add `content` as a text field using `text_options` (moved this time, indexed and stored).
6. Add `last_modified` as a `u64` field with `STORED` only (no text indexing — used for filtering/sorting).
7. Add `file_size` as a `u64` field with `STORED` only.
8. Call `builder.build()` to finalize the `Schema`.
9. Return a `FileSchema` literal containing the built schema and the field handles captured during construction.

### `FileSchema::schema(&self) -> Schema`
**Call graph:** `FileSchema::schema` -> `Schema::clone`

**Steps:**
1. Clone the inner `Schema` (Tantivy `Schema` is cheap to clone via `Arc`) and return it.

### `impl Default for FileSchema`
#### `default() -> Self`
**Call graph:** `FileSchema::default` -> `FileSchema::new`

**Steps:**
1. Delegate to `Self::new()` and return the result.

### `struct ChunkSchema`
Public fields: `schema: Schema`, `chunk_id: Field`, `content: Field`, `symbol_name: Field`, `symbol_kind: Field`, `file_path: Field`, `module_path: Field`, `docstring: Field`, `chunk_json: Field`. Derives `Clone`.

### `ChunkSchema::new() -> Self`
**Call graph:** `ChunkSchema::new` -> `SchemaBuilder::new`, `TextOptions::default`, `TextOptions::set_stored`, `TextOptions::set_indexing_options`, `TextFieldIndexing::default`, `TextFieldIndexing::set_tokenizer`, `TextFieldIndexing::set_index_option`, `SchemaBuilder::add_text_field`, `SchemaBuilder::build`

**Steps:**
1. Allocate a `SchemaBuilder`.
2. Build `code_options`: `TextOptions::default().set_stored().set_indexing_options(...)` with the `default` tokenizer and `WithFreqsAndPositions` indexing.
3. Build `doc_options`: identical to `code_options` (kept separate for future tuning of docstring boost/tokenizer).
4. Add `chunk_id` as text with `STRING | STORED` (UUIDs need exact match, not tokenization).
5. Add `content` as text with a clone of `code_options` (indexed and stored).
6. Add `symbol_name` as text using `code_options` (moved). Tokenized so that `parse_file` matches `parse` etc.
7. Add `symbol_kind` as text with `STRING | STORED` (used for filtering by exact kind, e.g., `"function"`).
8. Add `file_path` as text with `STRING | STORED` (display/filter only).
9. Add `module_path` as text with `STRING | STORED` (display/filter only).
10. Add `docstring` as text with `doc_options` (indexed + stored).
11. Add `chunk_json` as text with `STRING | STORED` (the entire `CodeChunk` JSON-serialized for retrieval; not searched).
12. Build the schema via `builder.build()`.
13. Return the `ChunkSchema` literal with the built schema and all eight `Field` handles.

### `ChunkSchema::schema(&self) -> Schema`
**Call graph:** `ChunkSchema::schema` -> `Schema::clone`

**Steps:**
1. Clone and return the inner `Schema`.

### `impl Default for ChunkSchema`
#### `default() -> Self`
**Call graph:** `ChunkSchema::default` -> `ChunkSchema::new`

**Steps:**
1. Forward to `Self::new()`.

## Module: metadata_cache.rs

Persistent file-metadata cache backed by a sled embedded key-value store. Used to drive incremental indexing: each indexed file is keyed by its path string, and the value is a bincode-serialized `FileMetadata` (SHA-256 hash + mtime + size + indexed-at timestamp). Includes a lightweight `FileStat` for fast pre-check that avoids reading file contents.

### `struct FileMetadata`
Derives `Debug, Clone, Serialize, Deserialize, PartialEq`. Public fields: `hash: String`, `last_modified: u64`, `size: u64`, `indexed_at: u64`.

### `struct FileStat`
Derives `Debug, Clone`. Public fields: `last_modified: u64`, `size: u64`.

### `FileStat::from_path(path: &Path) -> Result<Self, Box<dyn std::error::Error>>`
**Call graph:** `FileStat::from_path` -> `std::fs::metadata`, `Metadata::modified`, `SystemTime::duration_since`, `Duration::as_secs`, `Metadata::len`

**Steps:**
1. Call `std::fs::metadata(path)?` to fetch filesystem metadata (one stat syscall, no read of contents).
2. Read the modification time with `metadata.modified()?`.
3. Convert it to seconds since UNIX epoch: `.duration_since(std::time::UNIX_EPOCH)?.as_secs()`. Errors short-circuit via `?`.
4. Read the file size via `metadata.len()`.
5. Construct and return a `FileStat { last_modified, size }`.

### `FileMetadata::from_content(content: &str, last_modified: u64, size: u64) -> Self`
**Call graph:** `FileMetadata::from_content` -> `FileMetadata::hash_content`, `SystemTime::now`, `SystemTime::duration_since`, `Duration::as_secs`

**Steps:**
1. Compute the SHA-256 hash of `content` via the private helper `Self::hash_content(content)`.
2. Capture the current wall-clock indexing time: `SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs()`. The `unwrap` is acceptable because the system time is post-epoch in any realistic environment.
3. Return a `FileMetadata` with `hash`, the supplied `last_modified` and `size`, and the computed `indexed_at`.

### `FileMetadata::hash_content(content: &str) -> String` (private)
**Call graph:** `FileMetadata::hash_content` -> `Sha256::new`, `Digest::update`, `Digest::finalize`, `format!`

**Steps:**
1. Construct a fresh `Sha256` hasher.
2. Feed it `content.as_bytes()` via `update`.
3. Finalize the digest and format it as a lowercase hex string with `format!("{:x}", ...)`.
4. Return the hex string.

(Not `pub` but called by both `from_content` and `MetadataCache::has_changed`.)

### `struct MetadataCache`
Holds a single `db: sled::Db` handle.

### `MetadataCache::new(path: &Path) -> Result<Self, sled::Error>`
**Call graph:** `MetadataCache::new` -> `Path::parent`, `std::fs::create_dir_all`, `sled::open`

**Steps:**
1. If `path.parent()` is `Some`, call `std::fs::create_dir_all(parent).ok()` to ensure ancestor directories exist (sled only creates the leaf). Errors are deliberately swallowed with `.ok()` because sled will report a meaningful error if the directory really cannot be created.
2. Call `sled::open(path)?` to open or create the embedded database.
3. Return `Self { db }`.

### `MetadataCache::get(&self, file_path: &str) -> Result<Option<FileMetadata>, Box<dyn std::error::Error>>`
**Call graph:** `MetadataCache::get` -> `Db::get`, `bincode::deserialize`

**Steps:**
1. Look up `file_path` in the sled tree via `self.db.get(file_path)?`.
2. If the result is `Some(bytes)`, deserialize the bytes into a `FileMetadata` via `bincode::deserialize(&bytes)?` and wrap the result in `Some`.
3. If `None`, return `Ok(None)`.

### `MetadataCache::set(&self, file_path: &str, metadata: &FileMetadata) -> Result<(), Box<dyn std::error::Error>>`
**Call graph:** `MetadataCache::set` -> `bincode::serialize`, `Db::insert`

**Steps:**
1. Serialize `metadata` to bytes via `bincode::serialize(metadata)?`.
2. Insert `(file_path, bytes)` into the sled database with `self.db.insert(...)?`.
3. Return `Ok(())`.

### `MetadataCache::remove(&self, file_path: &str) -> Result<(), Box<dyn std::error::Error>>`
**Call graph:** `MetadataCache::remove` -> `Db::remove`

**Steps:**
1. Call `self.db.remove(file_path)?` to drop the entry.
2. Return `Ok(())`.

### `MetadataCache::has_stat_changed(&self, file_path: &str, stat: &FileStat) -> Result<bool, Box<dyn std::error::Error>>`
**Call graph:** `MetadataCache::has_stat_changed` -> `MetadataCache::get`

**Steps:**
1. Call `self.get(file_path)?`.
2. If `Some(cached)`: return `Ok(cached.last_modified != stat.last_modified || cached.size != stat.size)` — any difference in mtime or size flags the file as potentially changed.
3. If `None` (file not cached): return `Ok(true)` (needs indexing).

This is the cheap pre-filter; callers should only read file contents and call `has_changed` when this returns `true`.

### `MetadataCache::has_changed(&self, file_path: &str, content: &str) -> Result<bool, Box<dyn std::error::Error>>`
**Call graph:** `MetadataCache::has_changed` -> `FileMetadata::hash_content`, `MetadataCache::get`

**Steps:**
1. Compute `current_hash = FileMetadata::hash_content(content)`.
2. Call `self.get(file_path)?`.
3. If `Some(cached)`: return `Ok(cached.hash != current_hash)`.
4. If `None`: return `Ok(true)` (never indexed).

### `MetadataCache::list_files(&self) -> Result<Vec<String>, Box<dyn std::error::Error>>`
**Call graph:** `MetadataCache::list_files` -> `Db::iter`, `String::from_utf8`, `Vec::push`

**Steps:**
1. Allocate an empty `Vec<String>`.
2. Iterate over the sled tree with `self.db.iter()`, propagating any iteration error via `?` on each item.
3. Destructure each `(key, _value)` pair (we only need the key).
4. Convert the `IVec` key to `Vec<u8>`, then to `String` via `String::from_utf8(...)?` (will error if a non-UTF-8 path was somehow stored).
5. Push the path onto the vector.
6. Return the vector.

### `MetadataCache::clear(&self) -> Result<(), sled::Error>`
**Call graph:** `MetadataCache::clear` -> `Db::clear`

**Steps:**
1. Call `self.db.clear()` and propagate the result.

### `MetadataCache::len(&self) -> usize`
**Call graph:** `MetadataCache::len` -> `Db::len`

**Steps:**
1. Return `self.db.len()` (number of entries currently stored).

### `MetadataCache::is_empty(&self) -> bool`
**Call graph:** `MetadataCache::is_empty` -> `Db::is_empty`

**Steps:**
1. Return `self.db.is_empty()`.

## Module: bin/test_tools_direct.rs

A standalone binary that exercises the `RustParser` and standard-library file IO without going through the MCP layer. Intended as a smoke test that the core libraries work end-to-end against a real on-disk Rust project located at `/home/molaco/Documents/rust-code-mcp`. Note that this is hard-coded to a sibling project path, not the current crate.

### `main() -> Result<(), Box<dyn std::error::Error>>`
**Call graph:** `main` -> `println!`, `format!`, `fs::read_to_string`, `String::lines`, `Iterator::take`, `Iterator::collect`, `RustParser::new`, `fs::read_dir`, `DirEntry::path`, `Path::is_dir`, `Path::join`, `Path::exists`, `Path::file_name`, `RustParser::parse_file`, `RustParser::parse_file_complete`, `Vec::iter`, `Iterator::take`, `CallGraph::edge_count`, `Iterator::filter`, `Iterator::count`, `SymbolKind::as_str`, pattern match against `SymbolKind::Function { .. }`, `Path::display`

**Annotations:** Decorated with `#[tokio::main]` so it runs on a Tokio runtime, even though the body is synchronous.

**Steps:**
1. Print a banner: header lines about "Direct Functionality Testing (No MCP)".
2. Hard-code `project_dir = "/home/molaco/Documents/rust-code-mcp"`.
3. **Test 1 — Read file content.**
   a. Build `file_path = format!("{}/src/lib.rs", project_dir)`.
   b. `fs::read_to_string(&file_path)` and `match` on the result.
   c. On `Ok(content)`: collect the first 5 lines via `content.lines().take(5).collect::<Vec<&str>>()` and print each one prefixed with two spaces. Print a check-mark success line.
   d. On `Err(e)`: print an error line.
4. **Test 2 — Find definition with `RustParser`.**
   a. Construct `let mut parser = RustParser::new()?` (constructs the tree-sitter Rust parser).
   b. Set `symbol_to_find = "RustParser"` and `found = false`.
   c. Iterate `fs::read_dir(&src_dir)?` over `<project>/src`.
   d. For each entry: take its path; if it is a directory, build `parser_mod = path.join("mod.rs")`. If that file exists AND the directory's `file_name` is `"parser"`, parse it via `parser.parse_file(&parser_mod)`.
   e. On parse success: iterate the returned symbols; for each whose `name == "RustParser"`, print a hit line containing the absolute path, the symbol's start line (`symbol.range.start_line`), and the kind (`symbol.kind.as_str()`), and set `found = true`.
   f. On parse error: print the error.
   g. After the loop: if not `found`, print a not-found message.
5. **Test 3 — Get dependencies with `RustParser`.**
   a. Build `parser_file = format!("{}/src/parser/mod.rs", project_dir)`.
   b. Call `parser.parse_file_complete(Path::new(&parser_file))`.
   c. On `Ok(parse_result)`: print a header. If `parse_result.imports.is_empty()`, print "No imports found"; else print the count and the first 5 imports' `path` fields, plus a `"... and N more"` line if there are more than 5.
   d. On `Err(e)`: print the error.
6. **Test 4 — Analyze complexity.**
   a. Build `search_file = format!("{}/src/search/mod.rs", project_dir)`.
   b. `fs::read_to_string(&search_file)` for a manual line count.
   c. On success, also call `parser.parse_file_complete(Path::new(&search_file))`. On parse success:
      - `lines_of_code = source.lines().count()`.
      - `non_empty_loc = source.lines().filter(|l| !l.trim().is_empty()).count()`.
      - `function_count = parse_result.symbols.iter().filter(|s| matches!(s.kind, file_search_mcp::parser::SymbolKind::Function { .. })).count()` — counts only `Function` variants of the `SymbolKind` enum.
      - Edge count via `parse_result.call_graph.edge_count()`.
      - Print the four metrics.
   d. On parse error: print it. On read error: print it.
7. Print the closing banner ("All direct functionality tests completed!" etc.).
8. Return `Ok(())`.

This binary uses `?` only for the `RustParser::new` and `fs::read_dir` calls; every other fallible call is matched explicitly so partial failures are reported but do not abort the run.
