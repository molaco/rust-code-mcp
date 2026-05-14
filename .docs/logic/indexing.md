# indexing — Detailed Logic

## Module: mod

Module declarations and re-exports only:
- Declares `consistency`, `embedding_batcher` (pub(crate)), `error`, `errors`, `file_processor` (pub(crate)), `incremental`, `indexer_core`, `merkle`, `retry`, `tantivy_adapter`, `unified` submodules.
- Re-exports: `ConsistencyChecker`, `ConsistencyReport`, `IndexingError`, `ErrorCategory`, `ErrorCollector`, `ErrorDetail`, `get_snapshot_path`, `IncrementalIndexer`, `IndexerCore`, `ProcessedFile`, `retry_sync_with_backoff`, `retry_with_backoff`, `TantivyAdapter`, `IndexFileResult`, `IndexStats`, `UnifiedIndexer`.

## Module: consistency

### `ConsistencyReport::print_summary(&self)`
**Call graph:** `Iterator::take` -> `Iterator::collect` -> `tracing::info!`
**Steps:**
1. Take up to 10 elements from `missing_from_vectors` into a `Vec<&ChunkId>` preview slice.
2. Take up to 10 elements from `missing_from_tantivy` into a second preview slice.
3. Emit a single structured `tracing::info!` event ("Index consistency report") with `tantivy_count`, `vector_count`, `is_consistent`, both `missing_from_*_count` totals, and both preview slices; stdout is reserved for JSON-RPC, so no `println!` is used.

### `ConsistencyChecker::new(tantivy_index: Index, vector_store: VectorStore, schema: ChunkSchema) -> Self`
**Call graph:** none
**Steps:**
1. Construct a `ConsistencyChecker` populating its three fields with the supplied owned values.

### `ConsistencyChecker::check(&self) -> Result<ConsistencyReport>` (async)
**Call graph:** `tracing::info!` -> `Self::get_tantivy_chunk_ids` -> `HashSet::len` -> `VectorStore::count` -> `anyhow::anyhow!`
**Steps:**
1. Log the start of the consistency check.
2. Call `get_tantivy_chunk_ids` to enumerate every chunk ID currently stored in Tantivy; log the resulting count.
3. `await vector_store.count()` to fetch the count of stored chunks, mapping any error to `anyhow!("Failed to count vector store chunks: {}", e)`; log the count.
4. Compare `tantivy_ids.len() == vector_count` to derive the `is_consistent` flag.
5. Build a `ConsistencyReport { tantivy_count, vector_count, missing_from_vectors: Vec::new(), missing_from_tantivy: Vec::new(), is_consistent }` — the missing lists are TODO placeholders awaiting full chunk-ID listing on the vector store.
6. Log "OK" if consistent or "FAILED" otherwise and return the report.

### `ConsistencyChecker::get_tantivy_chunk_ids(&self) -> Result<HashSet<ChunkId>>` (private)
**Call graph:** `Index::reader_builder` -> `IndexReaderBuilder::reload_policy` -> `IndexReaderBuilder::try_into` -> `IndexReader::searcher` -> `Searcher::segment_readers` -> `SegmentReader::get_store_reader` -> `SegmentReader::max_doc` -> `StoreReader::get::<TantivyDocument>` -> `Document::get_first` -> `Value::as_str` -> `ChunkId::from_string` -> `HashSet::insert`
**Steps:**
1. Build a Tantivy `IndexReader` with `ReloadPolicy::Manual`, mapping failure with `Context`.
2. Acquire a searcher and initialise an empty `HashSet<ChunkId>`.
3. Iterate every `segment_reader` in the searcher, opening its store reader for doc retrieval (`get_store_reader(0)`).
4. For each `doc_id` in `0..segment_reader.max_doc()`, fetch the document via `store_reader.get::<TantivyDocument>(doc_id)`.
5. Pull the `chunk_id` field via `doc.get_first(self.schema.chunk_id)`, coerce it via `Value::as_str`, parse it through `ChunkId::from_string`, and insert into the set on success (silently skips malformed entries).
6. Return the populated set.

### `ConsistencyChecker::repair(&self, _report: &ConsistencyReport) -> Result<()>` (async)
**Call graph:** `anyhow::bail!`
**Steps:**
1. Always return `anyhow::bail!("Repair not yet implemented. Use force reindex instead.")` — placeholder for future logic that would re-embed/re-index missing chunks.

## Module: embedding_batcher

### `EmbeddingBatcher::new(embedding_generator: EmbeddingGenerator, gpu_batch_size: usize) -> Self` (pub(crate))
**Call graph:** `MemoryMonitor::new` -> `Arc::new` -> `Mutex::new`
**Steps:**
1. Instantiate a fresh `MemoryMonitor`.
2. Wrap it in `Arc<Mutex<...>>` and store alongside the supplied generator and `gpu_batch_size`.

### `EmbeddingBatcher::generate_embeddings_batched(&self, chunks: &[CodeChunk]) -> Result<Vec<Embedding>, IndexingError>` (pub(crate))
**Call graph:** `CodeChunk::format_for_embedding` -> `Iterator::collect` -> `slice::chunks` -> `EmbeddingGenerator::embed_batch` -> `Vec::extend`
**Steps:**
1. Map every chunk into a string via `format_for_embedding`, collected into `chunk_texts`.
2. Iterate `chunk_texts.chunks(self.gpu_batch_size)` to bound per-GPU-call workload.
3. For each window, call `embedding_generator.embed_batch(window.to_vec())?` and extend the running `all_embeddings` vector with the results.
4. Return the collected `Vec<Embedding>`.

### `EmbeddingBatcher::calculate_safe_batch_size(&self) -> usize` (pub(crate))
**Call graph:** `Mutex::lock` -> `MemoryMonitor::available_bytes` -> `num_cpus::get` -> `tracing::debug!`
**Steps:**
1. Lock the memory monitor and divide `available_bytes()` by `1_000_000` to obtain MB.
2. Estimate concurrent file capacity assuming ~15 MB per file (`available_mb / 15`), clamped to a minimum of 1.
3. Cap by `num_cpus::get()` (avoid CPU thrashing) and an absolute ceiling of 100.
4. Log the chosen batch size with available MB and CPU cap.
5. Return the resulting batch size.

### `EmbeddingBatcher::memory_usage_percent(&self) -> f64` (pub(crate))
**Call graph:** `Mutex::lock` -> `MemoryMonitor::usage_percent`
**Steps:**
1. Lock the monitor and return its `usage_percent()` value.

### `EmbeddingBatcher::refresh_memory_monitor(&self)` (pub(crate))
**Call graph:** `Mutex::lock` -> `MemoryMonitor::refresh`
**Steps:**
1. Lock the monitor and call `refresh()` to resample system memory state.

### `EmbeddingBatcher::memory_used_bytes(&self) -> u64` (pub(crate))
**Call graph:** `Mutex::lock` -> `MemoryMonitor::used_bytes`
**Steps:**
1. Lock the monitor and return `used_bytes()`.

### `EmbeddingBatcher::embedding_generator(&self) -> &EmbeddingGenerator` (pub(crate))
**Call graph:** none
**Steps:**
1. Return a reference to the wrapped `EmbeddingGenerator`.

## Module: error

### `enum IndexingError`
**Call graph:** none (definition only)
**Steps:**
1. Defines variants: `Io(#[from] std::io::Error)`, `Embedding(#[from] EmbeddingError)`, `VectorStore(#[from] VectorStoreError)`, `Parser(String)`, `Cache(String)`. The `thiserror::Error` derive auto-generates the `Display`/`Error` impls plus the three `From` conversions for transparent error propagation.

## Module: errors

### `ErrorCollector::new() -> Self`
**Call graph:** `Arc::new` -> `Mutex::new`
**Steps:**
1. Wrap an empty `Vec<ErrorDetail>` in `Arc<Mutex<...>>` and return the collector. Cloning the collector clones the `Arc`, giving multiple threads a shared error buffer.

### `ErrorCollector::record(&self, error: ErrorDetail)`
**Call graph:** `Mutex::lock` -> `Vec::push`
**Steps:**
1. Lock the inner mutex and push the supplied `ErrorDetail` into the vector.

### `ErrorCollector::get_errors(&self) -> Vec<ErrorDetail>`
**Call graph:** `Mutex::lock` -> `Vec::clone`
**Steps:**
1. Lock the mutex and return a `clone()` of the underlying vector (the original buffer is retained).

### `ErrorCollector::error_count(&self) -> usize`
**Call graph:** `Mutex::lock` -> `Vec::len`
**Steps:**
1. Lock the mutex and return the current vector length.

### `ErrorCollector::errors_by_category(&self, category: ErrorCategory) -> Vec<ErrorDetail>`
**Call graph:** `Mutex::lock` -> `Iterator::filter` -> `Iterator::cloned` -> `Iterator::collect`
**Steps:**
1. Lock the mutex.
2. Filter entries whose `category` equals the argument.
3. Clone the filtered entries into a fresh `Vec<ErrorDetail>` and return it.

### `ErrorCollector::clear(&self)`
**Call graph:** `Mutex::lock` -> `Vec::clear`
**Steps:**
1. Lock the mutex and clear the underlying vector.

### `ErrorCollector::default() -> Self` (impl Default)
**Call graph:** `ErrorCollector::new`
**Steps:**
1. Delegate to `Self::new()`.

### `categorize_error(error: &dyn std::error::Error) -> ErrorCategory`
**Call graph:** `Display::to_string` -> `str::to_lowercase` -> `str::contains`
**Steps:**
1. Convert the error via `to_string().to_lowercase()`.
2. If the lowercased message contains `permission denied`, `not found`, `invalid utf`, or `is a directory`, return `ErrorCategory::Permanent`.
3. Otherwise fall through to `ErrorCategory::Transient` (the default for unknown failures like timeouts).

## Module: file_processor

### `FileProcessor::new(cache_path: &Path, max_file_size: u64) -> Result<Self, IndexingError>` (pub(crate))
**Call graph:** `MetadataCache::new` -> `SecretsScanner::new` -> `SensitiveFileFilter::default`
**Steps:**
1. Open or create the metadata cache at `cache_path`; on failure, map the error via `IndexingError::Cache(e.to_string())`.
2. Construct a default `SecretsScanner`.
3. Construct the default `SensitiveFileFilter`.
4. Return the assembled `FileProcessor` carrying both filters, the cache, and `max_file_size`.

### `FileProcessor::should_process_file(&self, file_path: &Path) -> Result<bool, IndexingError>` (pub(crate))
**Call graph:** `SensitiveFileFilter::should_index` -> `tracing::warn!` -> `std::fs::metadata` -> `Metadata::len`
**Steps:**
1. Reject (return `Ok(false)`) if the sensitive-file filter excludes the path, logging a warning identifying the file.
2. Read filesystem metadata; propagate I/O errors via the `From<std::io::Error>` impl on `IndexingError`.
3. If `metadata.len() > max_file_size`, log a warning showing both sizes in MB and return `Ok(false)`.
4. Otherwise return `Ok(true)`.

### `FileProcessor::has_stat_changed(&self, file_path: &Path) -> Result<bool, IndexingError>` (pub(crate))
**Call graph:** `Path::to_string_lossy` -> `FileStat::from_path` -> `MetadataCache::has_stat_changed`
**Steps:**
1. Stringify the path via `to_string_lossy().to_string()`.
2. Build a `FileStat` from filesystem metadata, mapping errors to `IndexingError::Cache`.
3. Delegate to `metadata_cache.has_stat_changed(&path_str, &stat)`; this performs the cheap mtime+size comparison without reading content.

### `FileProcessor::has_file_changed(&self, file_path: &Path, content: &str) -> Result<bool, IndexingError>` (pub(crate))
**Call graph:** `Path::to_string_lossy` -> `MetadataCache::has_changed`
**Steps:**
1. Stringify the path.
2. Call `metadata_cache.has_changed(&path_str, content)`, mapping any cache error to `IndexingError::Cache`; this compares content hashes for confirmation after the stat check.

### `FileProcessor::update_file_metadata(&self, file_path: &Path, content: &str) -> Result<(), IndexingError>` (pub(crate))
**Call graph:** `Path::to_string_lossy` -> `std::fs::metadata` -> `Metadata::modified` -> `SystemTime::duration_since` -> `FileMetadata::from_content` -> `MetadataCache::set`
**Steps:**
1. Stringify the path and read filesystem metadata.
2. Compute the seconds since UNIX epoch from the `modified()` timestamp; map `SystemTimeError` to `IndexingError::Cache`.
3. Construct a `FileMetadata::from_content(content, mtime_secs, metadata.len())` capturing the post-index state.
4. Persist via `metadata_cache.set(&path_str, &file_meta)`, mapping errors to `Cache`.

### `FileProcessor::check_secrets(&self, file_path: &Path, content: &str) -> Result<(), IndexingError>` (pub(crate))
**Call graph:** `SecretsScanner::should_exclude` -> `SecretsScanner::scan_summary` -> `tracing::warn!`
**Steps:**
1. Call `secrets_scanner.should_exclude(content)`; if false, return `Ok(())`.
2. Otherwise compute a human-readable `scan_summary`, log a warning naming the file plus the summary, and return `Err(IndexingError::Parser("Contains secrets".into()))`.

### `FileProcessor::metadata_cache(&self) -> &MetadataCache` (pub(crate))
**Call graph:** none
**Steps:**
1. Return a reference to the inner `MetadataCache`.

### `FileProcessor::clear_metadata_cache(&self) -> Result<(), IndexingError>` (pub(crate))
**Call graph:** `MetadataCache::clear`
**Steps:**
1. Forward to `metadata_cache.clear()`, mapping errors to `IndexingError::Cache`.

## Module: incremental

### `get_snapshot_path(codebase_path: &Path) -> PathBuf`
**Call graph:** `ProjectDirs::from` -> `ProjectDirs::data_dir` -> `Path::join` -> `std::fs::create_dir_all` -> `Sha256::new` -> `Sha256::update` -> `Sha256::finalize` -> `format!`
**Steps:**
1. Resolve `merkle_dir` using `ProjectDirs::from("dev", "rust-code-mcp", "search")`'s data directory joined with `merkle`; fall back to `PathBuf::from(".merkle")` if `ProjectDirs` returns `None`.
2. Best-effort `create_dir_all(&merkle_dir)` (errors are silently ignored via `.ok()`).
3. SHA-256 the bytes of `codebase_path.to_string_lossy()` and hex-format the digest.
4. Return `merkle_dir.join(format!("{}.snapshot", &path_hash[..16]))` — uses only the first 16 hex chars so filenames stay short.

### `IncrementalIndexer::new(cache_path, tantivy_path, collection_name, vector_size, codebase_loc) -> Result<Self>` (async)
**Call graph:** `UnifiedIndexer::for_embedded`
**Steps:**
1. Build a `UnifiedIndexer` via `for_embedded(cache_path, tantivy_path, collection_name, vector_size, codebase_loc).await?`.
2. Wrap it in `IncrementalIndexer { indexer }` and return.

### `IncrementalIndexer::index_with_change_detection(&mut self, codebase_path: &Path) -> Result<IndexStats>` (async)
**Call graph:** `get_snapshot_path` -> `FileSystemMerkle::load_snapshot` -> `FileSystemMerkle::file_count` -> `FileSystemMerkle::version` -> `FileSystemMerkle::from_directory` -> `Self::incremental_update` -> `UnifiedIndexer::index_directory_parallel` -> `FileSystemMerkle::save_snapshot`
**Steps:**
1. Log the start of incremental indexing and compute the snapshot path via `get_snapshot_path(codebase_path)`.
2. Load any previous Merkle snapshot via `FileSystemMerkle::load_snapshot(&snapshot_path)?`; if `Some`, log file count + version, otherwise log "first time indexing".
3. Build a fresh Merkle tree from the current filesystem via `FileSystemMerkle::from_directory(codebase_path)?`.
4. Branch: if a previous snapshot existed, call `self.incremental_update(codebase_path, &old, &new_merkle).await?`; otherwise call `self.indexer.index_directory_parallel(codebase_path).await?` for the cold start path.
5. Persist the new snapshot to disk via `new_merkle.save_snapshot(&snapshot_path)?` so the next run can compare.
6. Return the resulting `IndexStats`.

### `IncrementalIndexer::incremental_update(&mut self, codebase_path: &Path, old_merkle: &FileSystemMerkle, new_merkle: &FileSystemMerkle) -> Result<IndexStats>` (private, async)
**Call graph:** `FileSystemMerkle::has_changes` -> `IndexStats::unchanged` -> `FileSystemMerkle::file_count` -> `FileSystemMerkle::detect_changes` -> `ChangeSet::is_empty` -> `Self::process_changes`
**Steps:**
1. Fast path: if `new_merkle.has_changes(old_merkle)` is false, log "no changes detected" and return `IndexStats::unchanged()` populated with `unchanged_files = total_files = new_merkle.file_count()`.
2. Otherwise log that roots differ and call `new_merkle.detect_changes(old_merkle)` to enumerate added/modified/deleted files.
3. If the resulting `ChangeSet::is_empty()` (a rare structural diff with no file-level deltas), return the same `unchanged` stats template.
4. Log the per-category counts and delegate the work to `self.process_changes(codebase_path, changes).await`.

### `IncrementalIndexer::process_changes(&mut self, _codebase_path: &Path, changes: ChangeSet) -> Result<IndexStats>` (private, async)
**Call graph:** `UnifiedIndexer::delete_file_chunks` -> `UnifiedIndexer::index_file` -> `UnifiedIndexer::commit`
**Steps:**
1. For each path in `changes.deleted`: log, call `delete_file_chunks(deleted_path).await?`, and increment `skipped_files` (deletions count as "no new content indexed").
2. For each path in `changes.modified`: log, call `delete_file_chunks(modified_path).await?` to clear stale chunks, then `index_file(modified_path).await`. On `Indexed { chunks_count }` bump `indexed_files` and `total_chunks`; on any other `Ok` variant or on `Err` (which is also logged), bump `skipped_files`.
3. For each path in `changes.added`: log and call `index_file(added_path).await`; mirror the same Ok/Err handling.
4. Commit the Tantivy writer via `self.indexer.commit()?`.
5. Log a summary and return the accumulated stats.

### `IncrementalIndexer::indexer(&self) -> &UnifiedIndexer`
**Call graph:** none
**Steps:**
1. Borrow the inner `UnifiedIndexer`.

### `IncrementalIndexer::indexer_mut(&mut self) -> &mut UnifiedIndexer`
**Call graph:** none
**Steps:**
1. Mutably borrow the inner `UnifiedIndexer`.

### `IncrementalIndexer::clear_all_data(&mut self) -> Result<()>` (async)
**Call graph:** `UnifiedIndexer::clear_all_data`
**Steps:**
1. Forward to `self.indexer.clear_all_data().await`. The caller is responsible for separately removing the Merkle snapshot file (this method does not touch disk snapshots).

## Module: indexer_core

### `IndexerCore::new(cache_path: &Path, config: Option<IndexerCoreConfig>) -> Result<Self, IndexingError>`
**Call graph:** `IndexerCoreConfig::default` -> `FileProcessor::new` -> `Chunker::new` -> `EmbeddingGenerator::new` -> `EmbeddingBatcher::new`
**Steps:**
1. Use the provided config or `IndexerCoreConfig::default()`.
2. Build a `FileProcessor` with `cache_path` and `config.max_file_size`.
3. Construct a `Chunker` (tree-sitter-driven).
4. Construct an `EmbeddingGenerator` (loads the embedding model; may take several seconds).
5. Wrap the generator inside an `EmbeddingBatcher` configured with `config.gpu_batch_size`.
6. Return the assembled `IndexerCore`.

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
1. Forward the borrowed metadata cache reference.

### `IndexerCore::clear_metadata_cache(&self) -> Result<(), IndexingError>`
**Call graph:** `FileProcessor::clear_metadata_cache`
**Steps:**
1. Forward to the inner `FileProcessor`.

### `IndexerCore::process_file_sync(&self, file_path: &Path) -> Result<ProcessedFile, IndexingError>`
**Call graph:** `Instant::now` -> `FileProcessor::should_process_file` -> `FileProcessor::has_stat_changed` -> `std::fs::read_to_string` -> `FileProcessor::check_secrets` -> `FileProcessor::has_file_changed` -> `RustParser::new` -> `RustParser::parse_source_complete` -> `Chunker::chunk_file` -> `Vec::is_empty` -> `Instant::elapsed`
**Steps:**
1. Start a parse timer via `Instant::now()`.
2. Reject with `IndexingError::Parser("File filtered: security check failed")` if `should_process_file` returns false.
3. Reject with `IndexingError::Parser("File unchanged")` if `has_stat_changed` returns false (fast pre-filter).
4. Read file content via `std::fs::read_to_string` (auto-propagates `io::Error`).
5. Run `check_secrets(file_path, &content)`; abort with `Parser("Contains secrets")` if any secret-pattern hits.
6. Reject with `IndexingError::Parser("File unchanged")` if `has_file_changed(file_path, &content)` is false (content hash matches cache).
7. Build a fresh `RustParser` (per-thread for safety) and call `parser.parse_source_complete(&content)`; map parser errors to `IndexingError::Parser`.
8. Chunk the parse result via `chunker.chunk_file(file_path, &content, &parse_result)`, mapping chunker errors similarly.
9. If `chunks.is_empty()`, log a warning and return `Parser("No chunks generated")`.
10. Capture `parse_start.elapsed()` and return `ProcessedFile { path, content, chunks, parse_duration }`.

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
1. Forward to the embedding batcher; used by `UnifiedIndexer::embedding_generator_cloned`.

## Module: merkle

### `Sha256Hasher::hash(data: &[u8]) -> [u8; 32]` (impl rs_merkle::Hasher)
**Call graph:** `Sha256::new` -> `Sha256::update` -> `Sha256::finalize`
**Steps:**
1. Construct a SHA-256 hasher, update it with `data`, finalize it, and convert the digest into a 32-byte array.

### `ChangeSet::empty() -> Self`
**Call graph:** none
**Steps:**
1. Return a `ChangeSet` with empty `added`, `modified`, and `deleted` vectors.

### `ChangeSet::is_empty(&self) -> bool`
**Call graph:** `Vec::is_empty`
**Steps:**
1. Return true if all three vectors are empty.

### `ChangeSet::total_changes(&self) -> usize`
**Call graph:** `Vec::len`
**Steps:**
1. Sum the lengths of `added`, `modified`, and `deleted`.

### `FileSystemMerkle::from_directory(root: &Path) -> Result<Self>`
**Call graph:** `WalkDir::new` -> `Path::extension` -> `tracing::warn!` -> `Vec::sort` -> `std::fs::read` -> `Sha256Hasher::hash` -> `std::fs::metadata` -> `Metadata::modified` -> `HashMap::insert` -> `MerkleTree::from_leaves` -> `MerkleTree::root` -> `hex::encode`
**Steps:**
1. Initialise empty `file_hashes`, `file_to_node`, and a `walk_errors` counter.
2. Walk the directory with `WalkDir::new(root)`; for each entry that is a file with extension `rs`, push its path into `files`. Other entry types are silently skipped; `WalkDir` errors increment `walk_errors` and emit a warning naming the failing path.
3. After traversal, if `walk_errors > 0`, log a warning summarising the count and continuing with accessible files.
4. Sort the `files` vector to guarantee stable Merkle leaf ordering.
5. For each `(idx, path)`: read the bytes with `std::fs::read(path)?`, hash them via `Sha256Hasher::hash`, push the hash into `file_hashes`, and insert a `FileNode { content_hash, leaf_index: idx, last_modified }` (taken from `std::fs::metadata(path)?.modified()?`) into `file_to_node`.
6. Build the `MerkleTree::<Sha256Hasher>::from_leaves(&file_hashes)`.
7. Log the resulting file count and hex-encoded root hash.
8. Return `Self { tree, file_to_node, snapshot_version: 1 }`.

### `FileSystemMerkle::root_hash(&self) -> Option<[u8; 32]>`
**Call graph:** `MerkleTree::root` -> `Option::map` -> `Clone::clone`
**Steps:**
1. Return `self.tree.root().map(|h| h.clone())` — `None` only when no leaves were supplied.

### `FileSystemMerkle::has_changes(&self, old: &Self) -> bool`
**Call graph:** `Self::root_hash`
**Steps:**
1. Compare `self.root_hash()` against `old.root_hash()`; inequality means at least one file changed (fast O(1) check).

### `FileSystemMerkle::detect_changes(&self, old: &Self) -> ChangeSet`
**Call graph:** `Self::has_changes` -> `ChangeSet::empty` -> `HashMap::get` -> `HashMap::keys` -> `HashMap::contains_key` -> `tracing::info!`
**Steps:**
1. Fast-exit with `ChangeSet::empty()` if `has_changes(old)` returns false.
2. Initialise empty `added`, `modified`, `deleted` vectors.
3. For every `(path, new_node)` in `self.file_to_node`: if `old.file_to_node` contains the path, compare `content_hash`; push to `modified` on mismatch. If absent from `old`, push to `added`.
4. For every path in `old.file_to_node.keys()` that is missing from `self.file_to_node`, push to `deleted`.
5. Log the change counts and return the resulting `ChangeSet`.

### `FileSystemMerkle::save_snapshot(&self, path: &Path) -> Result<()>`
**Call graph:** `Self::root_hash` -> `SystemTime::now` -> `Path::parent` -> `std::fs::create_dir_all` -> `std::fs::File::create` -> `bincode::serialize_into`
**Steps:**
1. Build a `MerkleSnapshot { root_hash: self.root_hash().unwrap_or([0u8; 32]), file_to_node: self.file_to_node.clone(), snapshot_version, timestamp: SystemTime::now() }`.
2. If `path.parent()` exists, ensure it via `create_dir_all`.
3. `File::create(path)?` and `bincode::serialize_into(file, &snapshot)?` to persist.
4. Log the save and return `Ok(())`.

### `FileSystemMerkle::load_snapshot(path: &Path) -> Result<Option<Self>>`
**Call graph:** `Path::exists` -> `std::fs::File::open` -> `bincode::deserialize_from` -> `HashMap::iter` -> `Iterator::collect` -> `Vec::sort_by` -> `MerkleTree::from_leaves` -> `MerkleTree::root` -> `hex::encode`
**Steps:**
1. Return `Ok(None)` if the snapshot file does not exist (logs a `debug!`).
2. Open the file and `bincode::deserialize_from` into a `MerkleSnapshot`.
3. Collect `(content_hash, &PathBuf)` pairs from `snapshot.file_to_node`, then `sort_by` the paths for deterministic ordering — this must match the build-time sort to preserve leaf identity.
4. Extract the sorted hashes and rebuild `MerkleTree::<Sha256Hasher>::from_leaves(&hashes)`.
5. Log the load (version, path, file count, hex root) and return `Ok(Some(Self { tree, file_to_node: snapshot.file_to_node, snapshot_version: snapshot.snapshot_version }))`.

### `FileSystemMerkle::file_count(&self) -> usize`
**Call graph:** `HashMap::len`
**Steps:**
1. Return the number of entries in `file_to_node`.

### `FileSystemMerkle::version(&self) -> u64`
**Call graph:** none
**Steps:**
1. Return `snapshot_version`.

### `hex::encode(bytes: &[u8]) -> String` (private helper module)
**Call graph:** `Iterator::map` -> `format!` -> `Iterator::collect`
**Steps:**
1. For each byte, format as `"{:02x}"` and concatenate into a `String`. Used only for log output.

## Module: retry

### `retry_with_backoff<F, Fut, T, E>(mut operation: F, max_attempts: u32, initial_delay: Duration) -> Result<T, E>` (async)
**Call graph:** `Future::await` -> `tokio::time::sleep` -> `tracing::warn!` -> `tracing::error!`
**Steps:**
1. Initialise `delay = initial_delay`.
2. Iterate `attempt in 1..=max_attempts`, awaiting `operation()`.
3. On `Ok(result)`, return it immediately.
4. On `Err(e)` with attempts remaining (`attempt < max_attempts`): warn-log the attempt counter and the error, `sleep(delay).await`, then double `delay` (exponential backoff).
5. On `Err(e)` at the final attempt: error-log "All N attempts failed" and return the error.
6. Trailing `unreachable!()` after the loop documents that the loop body always returns.

### `retry_sync_with_backoff<F, T, E>(mut operation: F, max_attempts: u32, initial_delay_ms: u64) -> Result<T, E>`
**Call graph:** `std::thread::sleep` -> `Duration::from_millis` -> `tracing::warn!` -> `tracing::error!`
**Steps:**
1. Initialise `delay_ms = initial_delay_ms`.
2. Iterate `attempt in 1..=max_attempts`, calling `operation()` synchronously.
3. On `Ok(result)`, return it.
4. On `Err(e)` with attempts remaining: warn-log, `std::thread::sleep(Duration::from_millis(delay_ms))`, then double `delay_ms`.
5. On the final attempt's `Err(e)`: error-log and return.
6. Trailing `unreachable!()` documents the post-loop unreachable state.

## Module: tantivy_adapter

### `TantivyAdapter::new(config: TantivyConfig) -> Result<Self>`
**Call graph:** `ChunkSchema::new` -> `Path::join` -> `Path::exists` -> `Index::open_in_dir` -> `std::fs::create_dir_all` -> `Index::create_in_dir` -> `Index::writer_with_num_threads` -> `tracing::info!`
**Steps:**
1. Build a `ChunkSchema`.
2. If `config.index_path.join("meta.json").exists()`, open the existing index via `Index::open_in_dir`. Otherwise `create_dir_all` the directory and `Index::create_in_dir(&path, schema.schema())`.
3. Compute `total_memory_budget = config.memory_budget_mb * config.num_threads * 1024 * 1024` (in bytes).
4. Build the writer via `index.writer_with_num_threads(config.num_threads, total_memory_budget)`.
5. Log the configuration (total MB and thread count) and return `Self { index, writer, schema }`.

### `TantivyAdapter::index_chunk(&mut self, chunk: &CodeChunk) -> Result<()>`
**Call graph:** `serde_json::to_string` -> `ChunkId::to_string` -> `Vec::join` -> `Option::unwrap_or_default` -> `IndexWriter::add_document` -> `tantivy::doc!`
**Steps:**
1. Serialize the chunk to JSON via `serde_json::to_string(chunk)?`.
2. Build a Tantivy document with the `doc!` macro populating fields `chunk_id` (`chunk.id.to_string()`), `content`, `symbol_name`, `symbol_kind`, `file_path` (`display().to_string()`), `module_path` (`module_path.join("::")`), `docstring` (defaulted to empty when `None`), and `chunk_json`.
3. Call `writer.add_document(...)`, contextualising any failure.

### `TantivyAdapter::index_chunks(&mut self, chunks: &[CodeChunk]) -> Result<()>`
**Call graph:** `Self::index_chunk`
**Steps:**
1. Iterate `chunks` and call `index_chunk` for each, short-circuiting on error.

### `TantivyAdapter::delete_file_chunks(&mut self, file_path: &Path) -> Result<()>`
**Call graph:** `Path::to_string_lossy` -> `Term::from_field_text` -> `TermQuery::new` -> `IndexWriter::delete_query` -> `tracing::debug!`
**Steps:**
1. Stringify the file path.
2. Build a `Term::from_field_text(self.schema.file_path, &file_path_str)`.
3. Wrap it in a `TermQuery::new(term, IndexRecordOption::Basic)`.
4. Call `writer.delete_query(Box::new(query))?` and log the deletion at debug level.

### `TantivyAdapter::delete_all(&mut self) -> Result<()>`
**Call graph:** `IndexWriter::delete_all_documents`
**Steps:**
1. Call `writer.delete_all_documents()`, contextualising the error.

### `TantivyAdapter::commit(&mut self) -> Result<()>`
**Call graph:** `IndexWriter::commit`
**Steps:**
1. Call `writer.commit()` with a contextual error message.

### `TantivyAdapter::rollback(&mut self) -> Result<()>`
**Call graph:** `IndexWriter::rollback`
**Steps:**
1. Call `writer.rollback()` with a contextual error message.

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
1. Clone the Tantivy `Index` handle.
2. Call `Bm25Search::from_index(self.index.clone())`, mapping any error to `anyhow::anyhow!("Failed to create Bm25Search: {}", e)`.

### `TantivyAdapter::drop(&mut self)` (impl Drop)
**Call graph:** `IndexWriter::rollback` -> `tracing::warn!`
**Steps:**
1. Attempt `writer.rollback()` to release the lockfile and any uncommitted segments; warn-log on failure (Drop must not panic).

## Module: unified

### `IndexStats::unchanged() -> Self`
**Call graph:** `IndexStats::default`
**Steps:**
1. Return `Self::default()` — represents zero changes across all counters; used by `IncrementalIndexer` when no work is needed.

### `UnifiedIndexer::for_embedded(cache_path, tantivy_path, collection_name, vector_size, codebase_loc) -> Result<Self>` (async)
**Call graph:** `IndexerCore::new` -> `TantivyConfig::for_codebase_size` -> `TantivyAdapter::new` -> `Path::parent` -> `Path::join` -> `VectorStore::new_embedded` -> `IndexingMetrics::new`
**Steps:**
1. Log "Initializing UnifiedIndexer with embedded LanceDB...".
2. Build an `IndexerCore` with the default `IndexerCoreConfig`.
3. Derive a `TantivyConfig` sized to `codebase_loc` via `TantivyConfig::for_codebase_size(tantivy_path, codebase_loc)` and create a `TantivyAdapter` from it.
4. Compute the vector store path as `cache_path.parent().unwrap_or(cache_path).join("vectors").join(collection_name)`.
5. Initialise the embedded `VectorStore::new_embedded(vector_path, vector_size).await`, mapping errors via `anyhow!`.
6. Construct fresh `IndexingMetrics::new()` and return `Self { core, tantivy, vector_store, metrics }`.

### `UnifiedIndexer::index_file(&mut self, file_path: &Path) -> Result<IndexFileResult>` (async)
**Call graph:** `Instant::now` -> `IndexerCore::should_process_file` -> `IndexerCore::has_stat_changed` -> `std::fs::read_to_string` -> `IndexerCore::has_file_changed` -> `IndexerCore::process_file_sync` -> `Vec::is_empty` -> `IndexerCore::generate_embeddings_batched` -> `TantivyAdapter::index_chunks` -> `Iterator::zip` -> `VectorStore::upsert_chunks` -> `IndexerCore::update_file_metadata` -> `Instant::elapsed` -> `Vec::push` -> `IndexerCore::refresh_memory_monitor` -> `IndexerCore::memory_used_bytes`
**Steps:**
1. Start a per-file timer.
2. Return `IndexFileResult::Skipped` if `core.should_process_file(file_path)?` is false (sensitive or too-large file).
3. Return `IndexFileResult::Unchanged` if `core.has_stat_changed(file_path)?` is false (debug-log "File unchanged (stat)").
4. Read file content via `std::fs::read_to_string`, contextualising the error.
5. Return `IndexFileResult::Unchanged` if `core.has_file_changed(file_path, &content)?` is false (debug-log "File unchanged (hash)").
6. Call `core.process_file_sync(file_path)?` to parse and chunk; if `processed.chunks.is_empty()`, warn-log and return `Skipped`.
7. Generate embeddings via `core.generate_embeddings_batched(&processed.chunks)?`.
8. Capture `chunks_count`, then index the chunks to Tantivy via `tantivy.index_chunks(&processed.chunks)?`.
9. Zip `processed.chunks` with embeddings into `Vec<(ChunkId, Vec<f32>, CodeChunk)>` and `await vector_store.upsert_chunks(chunk_data)`, mapping errors via `anyhow!`.
10. Update the metadata cache via `core.update_file_metadata(file_path, &content)?`.
11. Push the elapsed `file_duration` into `metrics.file_latencies`.
12. Refresh the memory monitor and update `metrics.peak_memory_bytes = peak.max(core.memory_used_bytes())`.
13. Log "✓ Indexed N chunks from <path> in <duration>" and return `IndexFileResult::Indexed { chunks_count }`.

### `UnifiedIndexer::index_directory(&mut self, dir_path: &Path) -> Result<IndexStats>` (async)
**Call graph:** `Instant::now` -> `IndexingMetrics::new` -> `Self::collect_rust_files` -> `Self::index_file` -> `TantivyAdapter::commit` -> `Self::finalize_metrics`
**Steps:**
1. Start the total timer and reset `self.metrics = IndexingMetrics::new()`.
2. Call `collect_rust_files(dir_path, &mut stats)` to enumerate `.rs` files; return early with empty `stats` if none.
3. Log the count, then sequentially call `self.index_file(&file).await` for each path:
   - `Ok(Indexed { chunks_count })`: `stats.indexed_files += 1; stats.total_chunks += chunks_count`.
   - `Ok(Unchanged)`: `stats.unchanged_files += 1`.
   - `Ok(Skipped)`: `stats.skipped_files += 1`.
   - `Err(e)`: log at `error!` and increment `skipped_files`.
4. Commit Tantivy via `self.tantivy.commit()` with context.
5. Call `self.finalize_metrics(&stats, total_start.elapsed())` for end-of-run reporting.
6. Log the summary line and return `stats`.

### `UnifiedIndexer::index_directory_with_backup(&mut self, dir_path: &Path, backup_manager: Option<&BackupManager>) -> Result<IndexStats>` (async)
**Call graph:** `Self::index_directory` -> `FileSystemMerkle::from_directory` -> `BackupManager::create_backup` -> `tracing::warn!`
**Steps:**
1. Run `self.index_directory(dir_path).await?` to obtain stats.
2. If a `backup_manager` is supplied and `stats.indexed_files > 0 && stats.indexed_files % 100 == 0`: log the snapshot intent, build a Merkle tree of the directory via `FileSystemMerkle::from_directory(dir_path)`. On success ask `manager.create_backup(&merkle)`; on any failure (build or create), warn-log without aborting the result.
3. Return the stats unchanged.

### `UnifiedIndexer::index_directory_parallel(&mut self, dir_path: &Path) -> Result<IndexStats>` (async)
**Call graph:** `Instant::now` -> `IndexingMetrics::new` -> `Self::collect_rust_files` -> `IndexerCore::calculate_safe_batch_size` -> `slice::chunks` -> `IndexerCore::memory_usage_percent` -> `tokio::time::sleep` -> `ErrorCollector::new` -> `ErrorCollector::clone` -> `rayon::iter::ParallelIterator::par_iter` -> `IndexerCore::process_file_sync` -> `ErrorCollector::record` -> `categorize_error` -> `Self::process_batch_errors` -> `Self::process_and_index_batch` -> `TantivyAdapter::commit` -> `Self::finalize_metrics`
**Steps:**
1. Reset metrics and start the total timer; log "Indexing directory (parallel mode)".
2. Gather Rust files via `collect_rust_files`; bail early with empty stats if none.
3. Compute `batch_size = self.core.calculate_safe_batch_size()` and log it.
4. Iterate the file list with `rust_files.chunks(batch_size).enumerate()`. For each `(batch_idx, file_batch)`:
   a. Log batch progress (`Processing batch X/Y (N files)`).
   b. Read memory usage; if `> 85.0%`, warn-log "High memory usage" and `tokio::time::sleep(Duration::from_secs(5)).await` to let allocators reclaim.
   c. PHASE 1 — Parse/chunk in parallel: start a parse timer, clone an `ErrorCollector`, then run `file_batch.par_iter().filter_map(...)` calling `core.process_file_sync(file_path)`. Successful results yield `Some(processed)`; failures call `error_collector_clone.record(ErrorDetail { file_path, category: categorize_error(&e), message: e.to_string() })` and return `None`.
   d. Add `parse_start.elapsed()` to `metrics.parse_duration` and log throughput (files/sec).
   e. Forward the collected errors through `process_batch_errors(&error_collector, &mut stats)` to update `skipped_files` per category.
   f. PHASE 2 — If `!processed.is_empty()`, call `self.process_and_index_batch(&processed, &mut stats).await?` to embed and dual-index the survivors.
   g. Commit Tantivy after every batch via `self.tantivy.commit()?`.
5. Call `finalize_metrics(&stats, total_start.elapsed())`.
6. Log the parallel summary and return `stats`.

### `UnifiedIndexer::delete_file_chunks(&mut self, file_path: &Path) -> Result<()>` (async)
**Call graph:** `TantivyAdapter::delete_file_chunks` -> `Path::to_string_lossy` -> `VectorStore::delete_by_file_path`
**Steps:**
1. Delete from Tantivy via `self.tantivy.delete_file_chunks(file_path)?`.
2. Stringify the path via `to_string_lossy().to_string()` and `await self.vector_store.delete_by_file_path(&file_path_str)`, mapping errors via `anyhow!`.
3. Log the deletion at debug level.

### `UnifiedIndexer::commit(&mut self) -> Result<()>`
**Call graph:** `TantivyAdapter::commit`
**Steps:**
1. Forward to `self.tantivy.commit()`. The vector store auto-commits per upsert.

### `UnifiedIndexer::clear_all_data(&mut self) -> Result<()>` (async)
**Call graph:** `IndexerCore::clear_metadata_cache` -> `TantivyAdapter::delete_all` -> `TantivyAdapter::commit` -> `VectorStore::clear_collection`
**Steps:**
1. Log "Clearing all indexed data...".
2. Clear the metadata cache via `self.core.clear_metadata_cache()?` and log "✓ Cleared metadata cache".
3. Delete all Tantivy documents (`self.tantivy.delete_all()?`) and commit (`self.tantivy.commit()?`) to make the deletion durable; log success.
4. `await self.vector_store.clear_collection()`, mapping errors via `anyhow!`; log success.
5. Final summary log and return `Ok(())`.

### `UnifiedIndexer::tantivy_index(&self) -> &Index`
**Call graph:** `TantivyAdapter::index`
**Steps:**
1. Forward to the adapter — used by search layers to construct readers.

### `UnifiedIndexer::vector_store_cloned(&self) -> VectorStore`
**Call graph:** `VectorStore::clone`
**Steps:**
1. Return a `Clone` of the vector store handle (the handle is `Clone` for sharing across search components).

### `UnifiedIndexer::embedding_generator_cloned(&self) -> EmbeddingGenerator`
**Call graph:** `IndexerCore::embedding_generator` -> `EmbeddingGenerator::clone`
**Steps:**
1. Borrow the inner generator via `core.embedding_generator()` and clone it.

### `UnifiedIndexer::tantivy_schema(&self) -> &ChunkSchema`
**Call graph:** `TantivyAdapter::schema`
**Steps:**
1. Forward to the adapter.

### `UnifiedIndexer::metrics(&self) -> &IndexingMetrics`
**Call graph:** none
**Steps:**
1. Borrow the `metrics` struct so callers can read post-run stats.

### `UnifiedIndexer::create_bm25_search(&self) -> Result<Bm25Search>`
**Call graph:** `TantivyAdapter::create_bm25_search`
**Steps:**
1. Forward to the adapter; returns a fresh `Bm25Search` over the same index.

### `UnifiedIndexer::collect_rust_files(&self, dir_path: &Path, stats: &mut IndexStats) -> Result<Vec<PathBuf>>` (private)
**Call graph:** `WalkDir::new` -> `Path::extension` -> `OsStr::new` -> `tracing::warn!`
**Steps:**
1. Initialise an empty vector and a `walk_errors` counter.
2. Walk the directory; for each `Ok` entry that is a `.rs` file, push `entry.path().to_path_buf()`.
3. On `Err(err)`, take `err.path().unwrap_or(Path::new("<unknown>"))`, warn-log "Failed to access ...", and increment `walk_errors`.
4. After traversal, warn-log the aggregate walk-error count if non-zero.
5. Set `stats.total_files = rust_files.len()` and return the collected paths.

### `UnifiedIndexer::process_batch_errors(&self, error_collector: &ErrorCollector, stats: &mut IndexStats)` (private)
**Call graph:** `ErrorCollector::get_errors`
**Steps:**
1. Iterate `error_collector.get_errors()`.
2. For `ErrorCategory::Permanent` entries: emit a `debug!` ("Skipped <path>: <message>") and increment `skipped_files`.
3. For `ErrorCategory::Transient` entries: emit a `warn!` ("Failed <path>: <message>") and increment `skipped_files`.

### `UnifiedIndexer::process_and_index_batch(&mut self, processed: &[ProcessedFile], stats: &mut IndexStats) -> Result<()>` (private, async)
**Call graph:** `Instant::now` -> `Iterator::flat_map` -> `Iterator::cloned` -> `Iterator::collect` -> `IndexerCore::generate_embeddings_batched` -> `Instant::elapsed` -> `tracing::info!` -> `TantivyAdapter::index_chunks` -> `Iterator::zip` -> `VectorStore::upsert_chunks` -> `IndexerCore::update_file_metadata`
**Steps:**
1. Start an embed timer.
2. Flatten chunks from all `processed` files into one `Vec<CodeChunk>` via `processed.iter().flat_map(|p| p.chunks.iter()).cloned().collect()`.
3. Log "Batch embedding N chunks from M files...".
4. Call `core.generate_embeddings_batched(&all_chunks)?` to produce one flat vector of embeddings.
5. Add `embed_start.elapsed()` to `metrics.embed_duration` and log throughput (chunks/sec).
6. Start an `index_start` timer; PHASE 3 — call `tantivy.index_chunks(&all_chunks)?` once for the entire batch (single batched Tantivy call instead of N file-by-file calls).
7. Zip `all_chunks` with `all_embeddings` into `Vec<(ChunkId, Vec<f32>, CodeChunk)>` and `await vector_store.upsert_chunks(all_chunk_data)`; map errors via `anyhow!`.
8. Add `index_start.elapsed()` to `metrics.index_duration`.
9. For each `processed_file` in `processed`: call `core.update_file_metadata(&path, &content)?`, increment `stats.indexed_files`, add `processed_file.chunks.len()` to `stats.total_chunks`, and push an approximate per-file latency (`index_duration / processed.len() as u32`) into `metrics.file_latencies` so per-file percentiles remain meaningful.
10. Log "Indexed N chunks (M files) in T (chunks/sec)" and return `Ok(())`.

### `UnifiedIndexer::finalize_metrics(&mut self, stats: &IndexStats, total_duration: Duration)` (private)
**Call graph:** `IndexingMetrics::log_summary`
**Steps:**
1. Copy `total_duration` plus `stats.total_files`, `indexed_files`, `skipped_files`, `unchanged_files`, and `total_chunks` into the metrics struct.
2. If `stats.total_files > 0`, set `metrics.cache_hit_rate = unchanged_files / total_files`.
3. Call `metrics.log_summary()` for end-of-run human-readable reporting.

### `UnifiedIndexer::drop(&mut self)` (impl Drop)
**Call graph:** `tracing::debug!`
**Steps:**
1. Emit a debug-level "UnifiedIndexer dropped" log; the `TantivyAdapter`'s own `Drop` already handles writer rollback to release the index lock.

## Pipeline Summary

The end-to-end indexing flow that ties these modules together:

1. **Snapshot** — `incremental::get_snapshot_path` resolves `~/.local/share/search/merkle/<hash>.snapshot` for the codebase.
2. **Change detection** — `merkle::FileSystemMerkle::from_directory` walks `.rs` files, hashes each via SHA-256, and builds a Merkle tree; `has_changes` / `detect_changes` compare against the prior snapshot.
3. **File processing** — `file_processor::FileProcessor` filters sensitive files, enforces size limits, screens for secrets, and uses `metadata_cache` (stat + content hash) for fine-grained change detection.
4. **Parsing/chunking** — `indexer_core::IndexerCore::process_file_sync` builds a `RustParser`, runs `parse_source_complete`, and feeds the result to `Chunker::chunk_file` to produce `CodeChunk`s.
5. **Embedding** — `embedding_batcher::EmbeddingBatcher::generate_embeddings_batched` runs the embeddings model in GPU-sized windows, using `MemoryMonitor` to bound batch concurrency.
6. **Dual-write** — `unified::UnifiedIndexer` writes chunks into Tantivy through `tantivy_adapter::TantivyAdapter::index_chunks` (BM25) and into `VectorStore::upsert_chunks` (LanceDB) as `(ChunkId, Vec<f32>, CodeChunk)` triples. `process_and_index_batch` performs the batched variant of this step during parallel mode.
7. **Commit** — `TantivyAdapter::commit` flushes BM25 segments; the vector store commits per upsert.
8. **Snapshot save** — `FileSystemMerkle::save_snapshot` writes a `bincode`-serialised `MerkleSnapshot` so the next run can short-circuit unchanged trees.
9. **Resilience** — `retry::retry_with_backoff` (async) and `retry_sync_with_backoff` (sync) wrap operations that may fail transiently with exponential-backoff retries; `errors::ErrorCollector` aggregates per-file failures across Rayon worker threads and `categorize_error` classifies them as `Permanent` vs `Transient`.
10. **Consistency** — `consistency::ConsistencyChecker::check` independently iterates Tantivy's stored chunk IDs and compares the count against `VectorStore::count`, producing a `ConsistencyReport` that `print_summary` emits as a structured `tracing::info!` event (stdout stays reserved for JSON-RPC frames).
