# indexing — Detailed Logic

## Module: mod

Module declarations and re-exports only:
- Declares `consistency`, `embedding_batcher` (pub(crate)), `error`, `errors`, `file_processor` (pub(crate)), `incremental`, `indexer_core`, `merkle`, `retry`, `tantivy_adapter`, `unified` submodules.
- Re-exports: `ConsistencyChecker`, `ConsistencyReport`, `IndexingError`, `ErrorCategory`, `ErrorCollector`, `ErrorDetail`, `get_snapshot_path`, `IncrementalIndexer`, `IndexerCore`, `ProcessedFile`, `retry_sync_with_backoff`, `retry_with_backoff`, `TantivyAdapter`, `IndexFileResult`, `IndexStats`, `UnifiedIndexer`.

## Module: consistency

### `ConsistencyReport::print_summary(&self)`
**Call graph:** `tracing::info!`
**Steps:**
1. Take up to 10 elements from `missing_from_vectors` for a preview slice.
2. Take up to 10 elements from `missing_from_tantivy` for a preview slice.
3. Emit a structured `tracing::info!` event with counts, the boolean `is_consistent`, and both previews; stdout is reserved for JSON-RPC.

### `ConsistencyChecker::new(tantivy_index: Index, vector_store: VectorStore, schema: ChunkSchema) -> Self`
**Call graph:** none
**Steps:**
1. Construct a `ConsistencyChecker` populating its three fields with the supplied owned values.

### `ConsistencyChecker::check(&self) -> Result<ConsistencyReport>`
**Call graph:** `tracing::info!` -> `Self::get_tantivy_chunk_ids` -> `VectorStore::count` -> `anyhow::anyhow!`
**Steps:**
1. Log the start of the consistency check.
2. Call `get_tantivy_chunk_ids` to enumerate all chunk IDs stored in Tantivy.
3. Fetch the count of stored chunks from `vector_store.count().await`, mapping errors to `anyhow`.
4. Compare counts to derive `is_consistent` (counts equal).
5. Build a `ConsistencyReport` with both counts, empty `missing_from_*` lists (TODO), and the consistency flag.
6. Log "OK" or "FAILED" and return the report.

### `ConsistencyChecker::get_tantivy_chunk_ids(&self) -> Result<HashSet<ChunkId>>`
**Call graph:** `Index::reader_builder` -> `IndexReader::searcher` -> `Searcher::segment_readers` -> `SegmentReader::get_store_reader` -> `StoreReader::get` -> `Document::get_first` -> `ChunkId::from_string`
**Steps:**
1. Build a Tantivy `IndexReader` with manual reload policy.
2. Acquire a searcher and create an empty `HashSet<ChunkId>`.
3. Iterate every segment reader, opening its store reader for doc retrieval.
4. For every doc id in `0..segment_reader.max_doc()`, fetch the document from the store.
5. Read the `chunk_id` field, parse it via `ChunkId::from_string`, and insert into the set.
6. Return the populated set.

### `ConsistencyChecker::repair(&self, _report: &ConsistencyReport) -> Result<()>`
**Call graph:** `anyhow::bail!`
**Steps:**
1. Always return `anyhow::bail!("Repair not yet implemented...")` — placeholder for future logic.

## Module: embedding_batcher

### `EmbeddingBatcher::new(embedding_generator: EmbeddingGenerator, gpu_batch_size: usize) -> Self` (pub(crate))
**Call graph:** `MemoryMonitor::new`
**Steps:**
1. Instantiate a fresh `MemoryMonitor`.
2. Wrap it in `Arc<Mutex<...>>` and store alongside the generator and the GPU batch size.

### `EmbeddingBatcher::generate_embeddings_batched(&self, chunks: &[CodeChunk]) -> Result<Vec<Embedding>, IndexingError>` (pub(crate))
**Call graph:** `CodeChunk::format_for_embedding` -> `EmbeddingGenerator::embed_batch`
**Steps:**
1. Format each chunk into a string using `format_for_embedding`.
2. Slice the texts into windows of `gpu_batch_size`.
3. For each window, call `embedding_generator.embed_batch(window.to_vec())` and extend the running result vector.
4. Return all collected embeddings.

### `EmbeddingBatcher::calculate_safe_batch_size(&self) -> usize` (pub(crate))
**Call graph:** `MemoryMonitor::available_bytes` -> `num_cpus::get` -> `tracing::debug!`
**Steps:**
1. Lock the memory monitor and read available bytes, dividing by 1_000_000 to get MB.
2. Estimate concurrent file count assuming ~15 MB per file (minimum 1).
3. Cap the result by CPU count and a hard ceiling of 100.
4. Log the chosen batch size for diagnostics.
5. Return the resulting batch size.

### `EmbeddingBatcher::memory_usage_percent(&self) -> f64` (pub(crate))
**Call graph:** `MemoryMonitor::usage_percent`
**Steps:**
1. Lock the monitor and forward the usage percentage value.

### `EmbeddingBatcher::refresh_memory_monitor(&self)` (pub(crate))
**Call graph:** `MemoryMonitor::refresh`
**Steps:**
1. Lock the monitor and call `refresh()` to update its sampled state.

### `EmbeddingBatcher::memory_used_bytes(&self) -> u64` (pub(crate))
**Call graph:** `MemoryMonitor::used_bytes`
**Steps:**
1. Lock the monitor and return the currently used bytes value.

### `EmbeddingBatcher::embedding_generator(&self) -> &EmbeddingGenerator` (pub(crate))
**Call graph:** none
**Steps:**
1. Return a reference to the wrapped embedding generator.

## Module: error

### `enum IndexingError`
**Call graph:** none (definition only)
**Steps:**
1. Defines variants: `Io(#[from] std::io::Error)`, `Embedding(#[from] EmbeddingError)`, `VectorStore(#[from] VectorStoreError)`, `Parser(String)`, `Cache(String)` — `thiserror`-derived `Display`/`Error` impls auto-generated.

## Module: errors

### `ErrorCollector::new() -> Self`
**Call graph:** none
**Steps:**
1. Wrap an empty `Vec<ErrorDetail>` in `Arc<Mutex<...>>` and return the collector.

### `ErrorCollector::record(&self, error: ErrorDetail)`
**Call graph:** `Mutex::lock`
**Steps:**
1. Lock the inner mutex and push the supplied `ErrorDetail` into the vector.

### `ErrorCollector::get_errors(&self) -> Vec<ErrorDetail>`
**Call graph:** `Mutex::lock` -> `Vec::clone`
**Steps:**
1. Lock the mutex and return a clone of the underlying error vector.

### `ErrorCollector::error_count(&self) -> usize`
**Call graph:** `Mutex::lock`
**Steps:**
1. Lock the mutex and return the vector length.

### `ErrorCollector::errors_by_category(&self, category: ErrorCategory) -> Vec<ErrorDetail>`
**Call graph:** `Mutex::lock` -> `Iterator::filter` -> `Iterator::cloned`
**Steps:**
1. Lock the mutex.
2. Filter entries whose category equals `category`.
3. Clone the filtered entries into a new vector and return it.

### `ErrorCollector::clear(&self)`
**Call graph:** `Mutex::lock` -> `Vec::clear`
**Steps:**
1. Lock the mutex and clear the underlying vector.

### `ErrorCollector::default() -> Self` (impl Default)
**Call graph:** `ErrorCollector::new`
**Steps:**
1. Delegate to `Self::new()`.

### `categorize_error(error: &dyn std::error::Error) -> ErrorCategory`
**Call graph:** `Display::to_string` -> `String::to_lowercase` -> `str::contains`
**Steps:**
1. Convert the error to a lowercase string.
2. Match keywords (`permission denied`, `not found`, `invalid utf`, `is a directory`) to flag `Permanent`.
3. Otherwise default to `Transient`.

## Module: file_processor

### `FileProcessor::new(cache_path: &Path, max_file_size: u64) -> Result<Self, IndexingError>` (pub(crate))
**Call graph:** `MetadataCache::new` -> `SecretsScanner::new` -> `SensitiveFileFilter::default`
**Steps:**
1. Open or create the metadata cache at `cache_path`, mapping errors to `IndexingError::Cache`.
2. Build a default secrets scanner.
3. Build the default sensitive-file filter.
4. Return the assembled `FileProcessor`.

### `FileProcessor::should_process_file(&self, file_path: &Path) -> Result<bool, IndexingError>` (pub(crate))
**Call graph:** `SensitiveFileFilter::should_index` -> `tracing::warn!` -> `std::fs::metadata`
**Steps:**
1. Reject (return `Ok(false)`) if the sensitive-file filter excludes the path, logging a warning.
2. Read filesystem metadata; propagate I/O errors.
3. If file size exceeds `max_file_size`, log a warning and return `Ok(false)`.
4. Otherwise return `Ok(true)`.

### `FileProcessor::has_stat_changed(&self, file_path: &Path) -> Result<bool, IndexingError>` (pub(crate))
**Call graph:** `Path::to_string_lossy` -> `FileStat::from_path` -> `MetadataCache::has_stat_changed`
**Steps:**
1. Stringify the path.
2. Build a `FileStat` from filesystem metadata, mapping errors to `Cache`.
3. Delegate to `metadata_cache.has_stat_changed` and forward the boolean.

### `FileProcessor::has_file_changed(&self, file_path: &Path, content: &str) -> Result<bool, IndexingError>` (pub(crate))
**Call graph:** `Path::to_string_lossy` -> `MetadataCache::has_changed`
**Steps:**
1. Stringify the path.
2. Call `metadata_cache.has_changed(&path_str, content)`, mapping errors to `Cache`.

### `FileProcessor::update_file_metadata(&self, file_path: &Path, content: &str) -> Result<(), IndexingError>` (pub(crate))
**Call graph:** `Path::to_string_lossy` -> `std::fs::metadata` -> `Metadata::modified` -> `SystemTime::duration_since` -> `FileMetadata::from_content` -> `MetadataCache::set`
**Steps:**
1. Stringify the path and read filesystem metadata.
2. Compute the seconds since UNIX epoch from the modified timestamp (mapping error to `Cache`).
3. Construct `FileMetadata::from_content(content, mtime_secs, len)`.
4. Persist via `metadata_cache.set`.

### `FileProcessor::check_secrets(&self, file_path: &Path, content: &str) -> Result<(), IndexingError>` (pub(crate))
**Call graph:** `SecretsScanner::should_exclude` -> `SecretsScanner::scan_summary` -> `tracing::warn!` -> `IndexingError::Parser`
**Steps:**
1. Run the secrets scanner; if no exclusion needed, return `Ok(())`.
2. Otherwise compute a human summary, log a warning, and return `IndexingError::Parser("Contains secrets")`.

### `FileProcessor::metadata_cache(&self) -> &MetadataCache` (pub(crate))
**Call graph:** none
**Steps:**
1. Return a reference to the inner `MetadataCache`.

### `FileProcessor::clear_metadata_cache(&self) -> Result<(), IndexingError>` (pub(crate))
**Call graph:** `MetadataCache::clear`
**Steps:**
1. Forward to `metadata_cache.clear()`, mapping errors to `Cache`.

## Module: incremental

### `get_snapshot_path(codebase_path: &Path) -> PathBuf`
**Call graph:** `ProjectDirs::from` -> `std::fs::create_dir_all` -> `Sha256::new` -> `Sha256::update` -> `Sha256::finalize`
**Steps:**
1. Resolve the merkle directory using `ProjectDirs("dev", "rust-code-mcp", "search")` data dir, falling back to `.merkle` if unavailable.
2. Best-effort `create_dir_all` on the merkle directory.
3. SHA-256 hash the codebase path's bytes; format as hex.
4. Return `merkle_dir/<first 16 hex chars>.snapshot`.

### `IncrementalIndexer::new(cache_path: &Path, tantivy_path: &Path, collection_name: &str, vector_size: usize, codebase_loc: Option<usize>) -> Result<Self>`
**Call graph:** `UnifiedIndexer::for_embedded`
**Steps:**
1. Build a `UnifiedIndexer` via `for_embedded(...)`.
2. Wrap it in `IncrementalIndexer` and return.

### `IncrementalIndexer::index_with_change_detection(&mut self, codebase_path: &Path) -> Result<IndexStats>`
**Call graph:** `get_snapshot_path` -> `FileSystemMerkle::load_snapshot` -> `FileSystemMerkle::from_directory` -> `Self::incremental_update` -> `UnifiedIndexer::index_directory_parallel` -> `FileSystemMerkle::save_snapshot`
**Steps:**
1. Compute the snapshot path for the codebase.
2. Load the previous Merkle snapshot if it exists; log file count and version.
3. Build a fresh Merkle tree from the current filesystem.
4. If a previous snapshot existed, run `incremental_update`; otherwise full reindex via `index_directory_parallel`.
5. Persist the new snapshot to disk.
6. Return the resulting `IndexStats`.

### `IncrementalIndexer::incremental_update(&mut self, codebase_path: &Path, old_merkle: &FileSystemMerkle, new_merkle: &FileSystemMerkle) -> Result<IndexStats>` (private)
**Call graph:** `FileSystemMerkle::has_changes` -> `IndexStats::unchanged` -> `FileSystemMerkle::file_count` -> `FileSystemMerkle::detect_changes` -> `ChangeSet::is_empty` -> `Self::process_changes`
**Steps:**
1. Fast path: if root hashes match, log and return an `unchanged` `IndexStats` populated with the new file count.
2. Otherwise call `detect_changes` to enumerate added/modified/deleted files.
3. If the resulting `ChangeSet` is empty, return `unchanged` stats.
4. Log the per-category change counts and delegate to `process_changes`.

### `IncrementalIndexer::process_changes(&mut self, _codebase_path: &Path, changes: ChangeSet) -> Result<IndexStats>` (private)
**Call graph:** `UnifiedIndexer::delete_file_chunks` -> `UnifiedIndexer::index_file` -> `UnifiedIndexer::commit`
**Steps:**
1. For each `deleted` path: call `delete_file_chunks` and increment `skipped_files`.
2. For each `modified` path: delete prior chunks, then call `index_file`; bump `indexed_files`/`total_chunks` on `Indexed`, otherwise `skipped_files`.
3. For each `added` path: call `index_file` and update stats analogously.
4. Commit Tantivy and log the summary.
5. Return populated stats.

### `IncrementalIndexer::indexer(&self) -> &UnifiedIndexer`
**Call graph:** none
**Steps:**
1. Borrow the inner `UnifiedIndexer`.

### `IncrementalIndexer::indexer_mut(&mut self) -> &mut UnifiedIndexer`
**Call graph:** none
**Steps:**
1. Mutably borrow the inner `UnifiedIndexer`.

### `IncrementalIndexer::clear_all_data(&mut self) -> Result<()>`
**Call graph:** `UnifiedIndexer::clear_all_data`
**Steps:**
1. Forward to `indexer.clear_all_data().await`.

## Module: indexer_core

### `IndexerCore::new(cache_path: &Path, config: Option<IndexerCoreConfig>) -> Result<Self, IndexingError>`
**Call graph:** `IndexerCoreConfig::default` -> `FileProcessor::new` -> `Chunker::new` -> `EmbeddingGenerator::new` -> `EmbeddingBatcher::new`
**Steps:**
1. Use the provided config or `IndexerCoreConfig::default()`.
2. Build a `FileProcessor` with `cache_path` and `max_file_size`.
3. Construct a `Chunker`.
4. Construct an `EmbeddingGenerator` (loads model).
5. Wrap the generator inside an `EmbeddingBatcher` configured with `gpu_batch_size`.
6. Return the assembled core.

### `IndexerCore::should_process_file(&self, file_path: &Path) -> Result<bool, IndexingError>`
**Call graph:** `FileProcessor::should_process_file`
**Steps:**
1. Delegate to the inner `FileProcessor`.

### `IndexerCore::has_stat_changed(&self, file_path: &Path) -> Result<bool, IndexingError>`
**Call graph:** `FileProcessor::has_stat_changed`
**Steps:**
1. Delegate to the inner `FileProcessor`.

### `IndexerCore::has_file_changed(&self, file_path: &Path, content: &str) -> Result<bool, IndexingError>`
**Call graph:** `FileProcessor::has_file_changed`
**Steps:**
1. Delegate to the inner `FileProcessor`.

### `IndexerCore::update_file_metadata(&self, file_path: &Path, content: &str) -> Result<(), IndexingError>`
**Call graph:** `FileProcessor::update_file_metadata`
**Steps:**
1. Delegate to the inner `FileProcessor`.

### `IndexerCore::metadata_cache(&self) -> &MetadataCache`
**Call graph:** `FileProcessor::metadata_cache`
**Steps:**
1. Forward the borrowed metadata cache.

### `IndexerCore::clear_metadata_cache(&self) -> Result<(), IndexingError>`
**Call graph:** `FileProcessor::clear_metadata_cache`
**Steps:**
1. Forward to the inner `FileProcessor`.

### `IndexerCore::process_file_sync(&self, file_path: &Path) -> Result<ProcessedFile, IndexingError>`
**Call graph:** `FileProcessor::should_process_file` -> `FileProcessor::has_stat_changed` -> `std::fs::read_to_string` -> `FileProcessor::check_secrets` -> `FileProcessor::has_file_changed` -> `RustParser::new` -> `RustParser::parse_source_complete` -> `Chunker::chunk_file` -> `Instant::elapsed`
**Steps:**
1. Start a parse timer.
2. Reject with `IndexingError::Parser("File filtered: security check failed")` if `should_process_file` returns false.
3. Reject with `IndexingError::Parser("File unchanged")` if `has_stat_changed` is false.
4. Read file content from disk.
5. Run `check_secrets`; abort if secrets detected.
6. Reject with `IndexingError::Parser("File unchanged")` if content hash matches cached value.
7. Build a fresh `RustParser` (per-thread for safety) and call `parse_source_complete(&content)`.
8. Chunk the parse result via `chunker.chunk_file(file_path, &content, &parse_result)`.
9. If no chunks were produced, log warning and return `Parser("No chunks generated")`.
10. Capture parse duration and return a `ProcessedFile { path, content, chunks, parse_duration }`.

### `IndexerCore::generate_embeddings_batched(&self, chunks: &[CodeChunk]) -> Result<Vec<Embedding>, IndexingError>`
**Call graph:** `EmbeddingBatcher::generate_embeddings_batched`
**Steps:**
1. Delegate to the embedding batcher.

### `IndexerCore::calculate_safe_batch_size(&self) -> usize`
**Call graph:** `EmbeddingBatcher::calculate_safe_batch_size`
**Steps:**
1. Forward to the embedding batcher.

### `IndexerCore::memory_usage_percent(&self) -> f64`
**Call graph:** `EmbeddingBatcher::memory_usage_percent`
**Steps:**
1. Forward to the embedding batcher.

### `IndexerCore::refresh_memory_monitor(&self)`
**Call graph:** `EmbeddingBatcher::refresh_memory_monitor`
**Steps:**
1. Forward to the embedding batcher.

### `IndexerCore::memory_used_bytes(&self) -> u64`
**Call graph:** `EmbeddingBatcher::memory_used_bytes`
**Steps:**
1. Forward to the embedding batcher.

### `IndexerCore::embedding_generator(&self) -> &EmbeddingGenerator`
**Call graph:** `EmbeddingBatcher::embedding_generator`
**Steps:**
1. Forward to the embedding batcher.

## Module: merkle

### `Sha256Hasher::hash(data: &[u8]) -> [u8; 32]` (impl Hasher)
**Call graph:** `Sha256::new` -> `Sha256::update` -> `Sha256::finalize`
**Steps:**
1. Construct a SHA-256 hasher, update it with `data`, finalize it, and convert into a 32-byte array.

### `ChangeSet::empty() -> Self`
**Call graph:** none
**Steps:**
1. Return a `ChangeSet` with empty `added`/`modified`/`deleted` vectors.

### `ChangeSet::is_empty(&self) -> bool`
**Call graph:** `Vec::is_empty`
**Steps:**
1. Return true if all three vectors are empty.

### `ChangeSet::total_changes(&self) -> usize`
**Call graph:** `Vec::len`
**Steps:**
1. Sum the lengths of `added`, `modified`, and `deleted`.

### `FileSystemMerkle::from_directory(root: &Path) -> Result<Self>`
**Call graph:** `WalkDir::new` -> `Path::extension` -> `tracing::warn!` -> `Vec::sort` -> `std::fs::read` -> `Sha256Hasher::hash` -> `std::fs::metadata` -> `Metadata::modified` -> `MerkleTree::from_leaves` -> `MerkleTree::root` -> `hex::encode`
**Steps:**
1. Initialise empty `file_hashes`, `file_to_node`, and `walk_errors=0`.
2. Walk the directory, collecting only `.rs` files; warn-log each `WalkDir` error.
3. After traversal, log if any walk errors were encountered.
4. Sort the file paths for stable Merkle ordering.
5. For each file (with leaf index): read contents, SHA-256 hash them, push the hash, and store a `FileNode { content_hash, leaf_index, last_modified }` into `file_to_node`.
6. Build the `MerkleTree<Sha256Hasher>` from the leaf hashes.
7. Log the resulting file count and (hex-encoded) root hash.
8. Return the tree wrapped in `Self` with `snapshot_version = 1`.

### `FileSystemMerkle::root_hash(&self) -> Option<[u8; 32]>`
**Call graph:** `MerkleTree::root`
**Steps:**
1. Return the cloned Merkle root if available.

### `FileSystemMerkle::has_changes(&self, old: &Self) -> bool`
**Call graph:** `Self::root_hash`
**Steps:**
1. Compare the root hashes of `self` and `old`; non-equal means changes exist.

### `FileSystemMerkle::detect_changes(&self, old: &Self) -> ChangeSet`
**Call graph:** `Self::has_changes` -> `ChangeSet::empty` -> `HashMap::get` -> `HashMap::keys` -> `HashMap::contains_key`
**Steps:**
1. Fast-exit with `ChangeSet::empty()` if the root hashes match.
2. For every `(path, new_node)` pair: if `old.file_to_node` has the path, compare content hashes (push to `modified` if different); otherwise push to `added`.
3. For every path in `old.file_to_node` that is missing from `self.file_to_node`, push to `deleted`.
4. Log the change counts and return the resulting `ChangeSet`.

### `FileSystemMerkle::save_snapshot(&self, path: &Path) -> Result<()>`
**Call graph:** `Self::root_hash` -> `Path::parent` -> `std::fs::create_dir_all` -> `std::fs::File::create` -> `bincode::serialize_into`
**Steps:**
1. Build a `MerkleSnapshot { root_hash, file_to_node, snapshot_version, timestamp }`.
2. Ensure the parent directory exists.
3. Create the destination file and `bincode::serialize_into` the snapshot.
4. Log the save and return.

### `FileSystemMerkle::load_snapshot(path: &Path) -> Result<Option<Self>>`
**Call graph:** `Path::exists` -> `std::fs::File::open` -> `bincode::deserialize_from` -> `Vec::sort_by` -> `MerkleTree::from_leaves` -> `MerkleTree::root` -> `hex::encode`
**Steps:**
1. Return `Ok(None)` if the snapshot file does not exist.
2. Open the file and `bincode::deserialize_from` into a `MerkleSnapshot`.
3. Collect `(content_hash, &PathBuf)` from `file_to_node` and sort by path for deterministic order.
4. Extract sorted hashes and rebuild the `MerkleTree<Sha256Hasher>`.
5. Log the load and return `Ok(Some(Self { tree, file_to_node, snapshot_version }))`.

### `FileSystemMerkle::file_count(&self) -> usize`
**Call graph:** `HashMap::len`
**Steps:**
1. Return the number of entries in `file_to_node`.

### `FileSystemMerkle::version(&self) -> u64`
**Call graph:** none
**Steps:**
1. Return `snapshot_version`.

### `hex::encode(bytes: &[u8]) -> String` (private helper module)
**Call graph:** `format!` -> `Iterator::collect`
**Steps:**
1. For each byte, format as `"{:02x}"` and concatenate into a `String`.

## Module: retry

### `retry_with_backoff<F, Fut, T, E>(operation: F, max_attempts: u32, initial_delay: Duration) -> Result<T, E>`
**Call graph:** `Future::await` -> `tokio::time::sleep` -> `tracing::warn!` -> `tracing::error!`
**Steps:**
1. Initialise `delay = initial_delay`.
2. Loop attempts `1..=max_attempts`, awaiting `operation()`.
3. On `Ok`, return immediately.
4. On `Err` with attempts remaining: warn-log and `sleep(delay).await`, then double `delay` (exponential backoff).
5. On `Err` at the final attempt: error-log and return the error.
6. The trailing `unreachable!()` documents the unreachable post-loop state.

### `retry_sync_with_backoff<F, T, E>(operation: F, max_attempts: u32, initial_delay_ms: u64) -> Result<T, E>`
**Call graph:** `std::thread::sleep` -> `Duration::from_millis` -> `tracing::warn!` -> `tracing::error!`
**Steps:**
1. Initialise `delay_ms = initial_delay_ms`.
2. Iterate attempts `1..=max_attempts`, calling `operation()` synchronously.
3. On `Ok`, return.
4. On `Err` with attempts remaining: warn-log, sleep blocking `delay_ms`, then double the delay.
5. On the last attempt's `Err`: error-log and return.
6. End with `unreachable!()`.

## Module: tantivy_adapter

### `TantivyAdapter::new(config: TantivyConfig) -> Result<Self>`
**Call graph:** `ChunkSchema::new` -> `Path::join` -> `Path::exists` -> `Index::open_in_dir` -> `std::fs::create_dir_all` -> `Index::create_in_dir` -> `Index::writer_with_num_threads`
**Steps:**
1. Build a `ChunkSchema`.
2. If `meta.json` exists in `config.index_path`, open the existing index; otherwise create the directory and a new index using the schema.
3. Compute total memory budget = `memory_budget_mb * num_threads * 1 MiB`.
4. Build the writer via `writer_with_num_threads(num_threads, total_memory_budget)`.
5. Log the configuration and return the assembled adapter.

### `TantivyAdapter::index_chunk(&mut self, chunk: &CodeChunk) -> Result<()>`
**Call graph:** `serde_json::to_string` -> `IndexWriter::add_document` -> `tantivy::doc!`
**Steps:**
1. Serialize the chunk to JSON.
2. Build a Tantivy document populating fields `chunk_id`, `content`, `symbol_name`, `symbol_kind`, `file_path`, `module_path` (joined by `::`), `docstring` (defaulted), and `chunk_json`.
3. Add the document to the writer; propagate errors.

### `TantivyAdapter::index_chunks(&mut self, chunks: &[CodeChunk]) -> Result<()>`
**Call graph:** `Self::index_chunk`
**Steps:**
1. Iterate `chunks` and call `index_chunk` for each, short-circuiting on error.

### `TantivyAdapter::delete_file_chunks(&mut self, file_path: &Path) -> Result<()>`
**Call graph:** `Path::to_string_lossy` -> `Term::from_field_text` -> `TermQuery::new` -> `IndexWriter::delete_query` -> `tracing::debug!`
**Steps:**
1. Stringify the file path.
2. Build a `Term` on the `file_path` field with that string.
3. Wrap in a `TermQuery` with `IndexRecordOption::Basic`.
4. Call `writer.delete_query(Box::new(query))` and log the deletion.

### `TantivyAdapter::delete_all(&mut self) -> Result<()>`
**Call graph:** `IndexWriter::delete_all_documents`
**Steps:**
1. Call `writer.delete_all_documents()`, propagating errors with `Context`.

### `TantivyAdapter::commit(&mut self) -> Result<()>`
**Call graph:** `IndexWriter::commit`
**Steps:**
1. Call `writer.commit()` with context-mapped error.

### `TantivyAdapter::rollback(&mut self) -> Result<()>`
**Call graph:** `IndexWriter::rollback`
**Steps:**
1. Call `writer.rollback()` with context-mapped error.

### `TantivyAdapter::index(&self) -> &Index`
**Call graph:** none
**Steps:**
1. Borrow the underlying Tantivy `Index`.

### `TantivyAdapter::schema(&self) -> &ChunkSchema`
**Call graph:** none
**Steps:**
1. Borrow the `ChunkSchema`.

### `TantivyAdapter::create_bm25_search(&self) -> Result<Bm25Search>`
**Call graph:** `Index::clone` -> `Bm25Search::from_index`
**Steps:**
1. Clone the Tantivy index handle.
2. Call `Bm25Search::from_index(...)`, mapping errors via `anyhow::anyhow!`.

### `TantivyAdapter::drop(&mut self)` (impl Drop)
**Call graph:** `IndexWriter::rollback` -> `tracing::warn!`
**Steps:**
1. Attempt `writer.rollback()` to release locks; warn-log on failure.

## Module: unified

### `IndexStats::unchanged() -> Self`
**Call graph:** `IndexStats::default`
**Steps:**
1. Return `Self::default()` representing zero changes.

### `UnifiedIndexer::for_embedded(cache_path, tantivy_path, collection_name, vector_size, codebase_loc) -> Result<Self>`
**Call graph:** `IndexerCore::new` -> `TantivyConfig::for_codebase_size` -> `TantivyAdapter::new` -> `Path::parent` -> `Path::join` -> `VectorStore::new_embedded` -> `IndexingMetrics::new`
**Steps:**
1. Log initialization.
2. Build an `IndexerCore` with default config.
3. Derive a `TantivyConfig` sized to `codebase_loc` and create a `TantivyAdapter`.
4. Compute the vector store path as `cache_path.parent().unwrap_or(cache_path).join("vectors").join(collection_name)`.
5. Initialise the embedded `VectorStore` with the given vector size, mapping errors to `anyhow`.
6. Construct fresh `IndexingMetrics` and return the populated `UnifiedIndexer`.

### `UnifiedIndexer::index_file(&mut self, file_path: &Path) -> Result<IndexFileResult>`
**Call graph:** `IndexerCore::should_process_file` -> `IndexerCore::has_stat_changed` -> `std::fs::read_to_string` -> `IndexerCore::has_file_changed` -> `IndexerCore::process_file_sync` -> `IndexerCore::generate_embeddings_batched` -> `TantivyAdapter::index_chunks` -> `VectorStore::upsert_chunks` -> `IndexerCore::update_file_metadata` -> `IndexerCore::refresh_memory_monitor` -> `IndexerCore::memory_used_bytes`
**Steps:**
1. Start a per-file timer.
2. Return `Skipped` if `should_process_file` is false.
3. Return `Unchanged` if `has_stat_changed` is false.
4. Read file content; map I/O errors with context.
5. Return `Unchanged` if `has_file_changed` is false (hash matches).
6. Call `process_file_sync` to parse and chunk; bail if no chunks.
7. Generate embeddings via `generate_embeddings_batched`.
8. Index chunks to Tantivy; zip chunks/embeddings into `(ChunkId, Vec<f32>, CodeChunk)` and upsert to the vector store.
9. Update the metadata cache for this file.
10. Push the elapsed file duration into `metrics.file_latencies`.
11. Refresh memory monitor and update `peak_memory_bytes`.
12. Log success and return `IndexFileResult::Indexed { chunks_count }`.

### `UnifiedIndexer::index_directory(&mut self, dir_path: &Path) -> Result<IndexStats>`
**Call graph:** `Self::collect_rust_files` -> `Self::index_file` -> `TantivyAdapter::commit` -> `Self::finalize_metrics`
**Steps:**
1. Reset metrics and start the total timer.
2. Collect Rust files via `collect_rust_files`; return early if empty.
3. For each file, call `index_file` and update the matching `IndexStats` counter (indexed/unchanged/skipped) based on the variant; on `Err`, log and increment `skipped_files`.
4. Commit Tantivy changes with context.
5. Call `finalize_metrics` with the elapsed total duration.
6. Log a summary and return stats.

### `UnifiedIndexer::index_directory_with_backup(&mut self, dir_path: &Path, backup_manager: Option<&BackupManager>) -> Result<IndexStats>`
**Call graph:** `Self::index_directory` -> `FileSystemMerkle::from_directory` -> `BackupManager::create_backup`
**Steps:**
1. Run `index_directory(dir_path).await` to obtain stats.
2. If a backup manager is provided and `indexed_files > 0` and divisible by 100, build a Merkle tree of the directory and ask the manager to create a backup; warn-log on errors.
3. Return the stats.

### `UnifiedIndexer::index_directory_parallel(&mut self, dir_path: &Path) -> Result<IndexStats>`
**Call graph:** `Self::collect_rust_files` -> `IndexerCore::calculate_safe_batch_size` -> `IndexerCore::memory_usage_percent` -> `tokio::time::sleep` -> `Rayon::par_iter` -> `IndexerCore::process_file_sync` -> `ErrorCollector::record` -> `categorize_error` -> `Self::process_batch_errors` -> `Self::process_and_index_batch` -> `TantivyAdapter::commit` -> `Self::finalize_metrics`
**Steps:**
1. Reset metrics and start total timer.
2. Gather Rust files; bail early if empty.
3. Compute a safe batch size from the indexer core.
4. Iterate the file list in chunks of `batch_size`:
   a. If memory usage > 85%, log a warning and `tokio::time::sleep(5s)` to allow GC.
   b. PHASE 1 — Parallel parse/chunk via Rayon `par_iter().filter_map(...)` calling `process_file_sync`; record failures into the shared `ErrorCollector` with categorized errors.
   c. Add the parse duration to `metrics.parse_duration` and log throughput.
   d. Forward collected errors through `process_batch_errors` to update stats.
   e. PHASE 2 — If any files succeeded, call `process_and_index_batch` which embeds and indexes them to both stores.
   f. Commit Tantivy after each batch.
5. Finalize metrics and return stats.

### `UnifiedIndexer::delete_file_chunks(&mut self, file_path: &Path) -> Result<()>`
**Call graph:** `TantivyAdapter::delete_file_chunks` -> `VectorStore::delete_by_file_path`
**Steps:**
1. Delete from Tantivy via the adapter.
2. Stringify the path and call `vector_store.delete_by_file_path(...).await`, mapping errors.
3. Log the deletion.

### `UnifiedIndexer::commit(&mut self) -> Result<()>`
**Call graph:** `TantivyAdapter::commit`
**Steps:**
1. Forward to `tantivy.commit()`.

### `UnifiedIndexer::clear_all_data(&mut self) -> Result<()>`
**Call graph:** `IndexerCore::clear_metadata_cache` -> `TantivyAdapter::delete_all` -> `TantivyAdapter::commit` -> `VectorStore::clear_collection`
**Steps:**
1. Clear the metadata cache.
2. Delete all Tantivy documents and commit to make the deletion durable.
3. Clear the vector store collection, mapping errors to `anyhow`.
4. Log success at each stage.

### `UnifiedIndexer::tantivy_index(&self) -> &Index`
**Call graph:** `TantivyAdapter::index`
**Steps:**
1. Forward to the adapter.

### `UnifiedIndexer::vector_store_cloned(&self) -> VectorStore`
**Call graph:** `VectorStore::clone`
**Steps:**
1. Return a `Clone` of the vector store handle.

### `UnifiedIndexer::embedding_generator_cloned(&self) -> EmbeddingGenerator`
**Call graph:** `IndexerCore::embedding_generator` -> `EmbeddingGenerator::clone`
**Steps:**
1. Borrow and clone the embedding generator.

### `UnifiedIndexer::tantivy_schema(&self) -> &ChunkSchema`
**Call graph:** `TantivyAdapter::schema`
**Steps:**
1. Forward to the adapter.

### `UnifiedIndexer::metrics(&self) -> &IndexingMetrics`
**Call graph:** none
**Steps:**
1. Borrow the metrics struct.

### `UnifiedIndexer::create_bm25_search(&self) -> Result<Bm25Search>`
**Call graph:** `TantivyAdapter::create_bm25_search`
**Steps:**
1. Forward to the adapter.

### `UnifiedIndexer::collect_rust_files(&self, dir_path: &Path, stats: &mut IndexStats) -> Result<Vec<PathBuf>>` (private)
**Call graph:** `WalkDir::new` -> `Path::extension` -> `tracing::warn!`
**Steps:**
1. Walk the directory; for each successful entry that is a `.rs` file, push to the vector.
2. Warn-log walk errors and increment `walk_errors`.
3. After traversal, log the cumulative walk-error count if any.
4. Update `stats.total_files` and return the collected paths.

### `UnifiedIndexer::process_batch_errors(&self, error_collector: &ErrorCollector, stats: &mut IndexStats)` (private)
**Call graph:** `ErrorCollector::get_errors`
**Steps:**
1. Drain `get_errors()`.
2. For `Permanent`: log at debug and increment `skipped_files`.
3. For `Transient`: log at warn and increment `skipped_files`.

### `UnifiedIndexer::process_and_index_batch(&mut self, processed: &[ProcessedFile], stats: &mut IndexStats) -> Result<()>` (private)
**Call graph:** `Iterator::flat_map` -> `Iterator::cloned` -> `IndexerCore::generate_embeddings_batched` -> `TantivyAdapter::index_chunks` -> `Iterator::zip` -> `VectorStore::upsert_chunks` -> `IndexerCore::update_file_metadata`
**Steps:**
1. Start an embed timer.
2. Flatten chunks from all `processed` files into one `Vec<CodeChunk>`.
3. Generate embeddings for all chunks via `generate_embeddings_batched`.
4. Add the embed duration to `metrics.embed_duration` and log throughput.
5. Start an index timer; log and call `tantivy.index_chunks(&all_chunks)` (single batched call).
6. Zip the chunks with embeddings into `(ChunkId, Vec<f32>, CodeChunk)` triples.
7. Call `vector_store.upsert_chunks(all_chunk_data).await`, mapping errors.
8. Add the index duration to `metrics.index_duration`.
9. For each processed file: update metadata cache, increment `indexed_files`/`total_chunks`, and push an approximated per-file latency (`index_duration / processed.len()`) into `metrics.file_latencies`.
10. Log the total indexed throughput and return.

### `UnifiedIndexer::finalize_metrics(&mut self, stats: &IndexStats, total_duration: Duration)` (private)
**Call graph:** `IndexingMetrics::log_summary`
**Steps:**
1. Copy `total_duration` plus per-category file counts into the metrics struct.
2. Compute `cache_hit_rate = unchanged_files / total_files` if files exist.
3. Call `metrics.log_summary()` for end-of-run reporting.

### `UnifiedIndexer::drop(&mut self)` (impl Drop)
**Call graph:** `tracing::debug!`
**Steps:**
1. Emit a debug-level log; the `TantivyAdapter`'s own `Drop` handles writer rollback.
