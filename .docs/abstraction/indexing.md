# indexing — Abstract Logic

## Module: mod
**Purpose:** Declares the submodules and re-exports the public indexing surface.

1. **Expose submodules and public types** -> `consistency`, `embedding_batcher`, `error`, `errors`, `file_processor`, `incremental`, `indexer_core`, `merkle`, `retry`, `tantivy_adapter`, `unified`

## Module: consistency
**Purpose:** Cross-checks Tantivy and the vector store to detect index divergence.

1. **Build a checker bundling the Tantivy index, vector store, and schema** -> `ConsistencyChecker::new()`
2. **Run a count-based consistency comparison and produce a report** -> `ConsistencyChecker::check()`
3. **Enumerate every stored chunk ID across Tantivy segments** -> `ConsistencyChecker::get_tantivy_chunk_ids()`
4. **Emit a structured summary log of the report** -> `ConsistencyReport::print_summary()`
5. **Placeholder for future divergence repair** -> `ConsistencyChecker::repair()`

## Module: embedding_batcher
**Purpose:** Batches chunk embedding generation under memory- and GPU-aware limits.

1. **Construct the batcher with a generator and GPU batch size** -> `EmbeddingBatcher::new()`
2. **Embed chunks in fixed-size GPU windows** -> `EmbeddingBatcher::generate_embeddings_batched()`
3. **Compute a memory- and CPU-aware safe batch size** -> `EmbeddingBatcher::calculate_safe_batch_size()`
4. **Expose memory-monitor diagnostics** -> `EmbeddingBatcher::memory_usage_percent()`, `EmbeddingBatcher::refresh_memory_monitor()`, `EmbeddingBatcher::memory_used_bytes()`
5. **Provide access to the underlying embedding generator** -> `EmbeddingBatcher::embedding_generator()`

## Module: error
**Purpose:** Defines the unified indexing error type.

1. **Enumerate I/O, embedding, vector-store, parser, and cache failure variants with transparent `From` conversions** -> `IndexingError`

## Module: errors
**Purpose:** Collects, categorizes, and queries per-file indexing errors across worker threads.

1. **Construct an empty thread-safe error collector** -> `ErrorCollector::new()`, `ErrorCollector::default()`
2. **Record and clear collected errors** -> `ErrorCollector::record()`, `ErrorCollector::clear()`
3. **Query collected errors by count, list, or category** -> `ErrorCollector::error_count()`, `ErrorCollector::get_errors()`, `ErrorCollector::errors_by_category()`
4. **Classify an error as permanent or transient by message keywords** -> `categorize_error()`

## Module: file_processor
**Purpose:** Filters, change-detects, and persists per-file metadata before parsing.

1. **Initialize cache, secrets scanner, and sensitive-file filter** -> `FileProcessor::new()`
2. **Decide whether a file should be processed by sensitivity and size** -> `FileProcessor::should_process_file()`
3. **Detect filesystem stat or content changes against the metadata cache** -> `FileProcessor::has_stat_changed()`, `FileProcessor::has_file_changed()`
4. **Persist updated file metadata after a successful pass** -> `FileProcessor::update_file_metadata()`
5. **Reject files containing detected secrets** -> `FileProcessor::check_secrets()`
6. **Expose and clear the underlying metadata cache** -> `FileProcessor::metadata_cache()`, `FileProcessor::clear_metadata_cache()`

## Module: incremental
**Purpose:** Drives Merkle-snapshot-based incremental reindexing of a codebase.

1. **Resolve the per-codebase snapshot path under the data directory** -> `get_snapshot_path()`
2. **Build the incremental wrapper around an embedded `UnifiedIndexer`** -> `IncrementalIndexer::new()`
3. **Run change-detected indexing using saved vs fresh Merkle trees** -> `IncrementalIndexer::index_with_change_detection()`
4. **Compute and apply per-file diffs (added, modified, deleted)** -> `IncrementalIndexer::incremental_update()`, `IncrementalIndexer::process_changes()`
5. **Expose the inner unified indexer and forward bulk clears** -> `IncrementalIndexer::indexer()`, `IncrementalIndexer::indexer_mut()`, `IncrementalIndexer::clear_all_data()`

## Module: indexer_core
**Purpose:** Bundles file processing, parsing, chunking, and embedding into a single per-file pipeline.

1. **Assemble file processor, chunker, and embedding batcher from config** -> `IndexerCore::new()`
2. **Forward file gating and change-detection to the file processor** -> `IndexerCore::should_process_file()`, `IndexerCore::has_stat_changed()`, `IndexerCore::has_file_changed()`, `IndexerCore::update_file_metadata()`, `IndexerCore::metadata_cache()`, `IndexerCore::clear_metadata_cache()`
3. **Parse and chunk one file synchronously with timing** -> `IndexerCore::process_file_sync()`
4. **Forward batched embedding and memory diagnostics** -> `IndexerCore::generate_embeddings_batched()`, `IndexerCore::calculate_safe_batch_size()`, `IndexerCore::memory_usage_percent()`, `IndexerCore::refresh_memory_monitor()`, `IndexerCore::memory_used_bytes()`, `IndexerCore::embedding_generator()`

## Module: merkle
**Purpose:** Builds, persists, and diffs SHA-256 Merkle trees over the codebase's `.rs` files.

1. **Provide a SHA-256-based hasher for Merkle leaves** -> `Sha256Hasher::hash()`
2. **Track sets of changed paths between snapshots** -> `ChangeSet::empty()`, `ChangeSet::is_empty()`, `ChangeSet::total_changes()`
3. **Build a Merkle tree by walking and hashing all `.rs` files** -> `FileSystemMerkle::from_directory()`
4. **Expose tree metadata: root hash, file count, version** -> `FileSystemMerkle::root_hash()`, `FileSystemMerkle::file_count()`, `FileSystemMerkle::version()`
5. **Compare two trees to detect changed files** -> `FileSystemMerkle::has_changes()`, `FileSystemMerkle::detect_changes()`
6. **Persist and reload Merkle snapshots via bincode** -> `FileSystemMerkle::save_snapshot()`, `FileSystemMerkle::load_snapshot()`
7. **Encode hashes for human-readable logging** -> `hex::encode()`

## Module: retry
**Purpose:** Generic exponential-backoff retry helpers for sync and async operations.

1. **Retry an async operation with exponentially backing-off delays** -> `retry_with_backoff()`
2. **Retry a synchronous operation with exponentially backing-off blocking sleeps** -> `retry_sync_with_backoff()`

## Module: tantivy_adapter
**Purpose:** Wraps a Tantivy `Index` and writer for chunk-oriented indexing operations.

1. **Open or create a Tantivy index sized to the configured memory budget** -> `TantivyAdapter::new()`
2. **Insert one or many chunks as Tantivy documents** -> `TantivyAdapter::index_chunk()`, `TantivyAdapter::index_chunks()`
3. **Delete chunks by file path or wipe the index** -> `TantivyAdapter::delete_file_chunks()`, `TantivyAdapter::delete_all()`
4. **Commit, roll back, or release the writer on drop** -> `TantivyAdapter::commit()`, `TantivyAdapter::rollback()`, `TantivyAdapter::drop()`
5. **Expose the underlying index, schema, and BM25 searcher** -> `TantivyAdapter::index()`, `TantivyAdapter::schema()`, `TantivyAdapter::create_bm25_search()`

## Module: unified
**Purpose:** Top-level indexer coordinating parsing, embedding, Tantivy, and the vector store across files and directories.

1. **Provide a default-zero stats sentinel for unchanged runs** -> `IndexStats::unchanged()`
2. **Construct the embedded indexer wiring all subsystems** -> `UnifiedIndexer::for_embedded()`
3. **Index one file end-to-end through both stores** -> `UnifiedIndexer::index_file()`
4. **Index a directory sequentially or with periodic backups** -> `UnifiedIndexer::index_directory()`, `UnifiedIndexer::index_directory_with_backup()`
5. **Index a directory in parallel batches with memory throttling** -> `UnifiedIndexer::index_directory_parallel()`
6. **Walk a directory to enumerate `.rs` files** -> `UnifiedIndexer::collect_rust_files()`
7. **Drain and account for batch parse errors by category** -> `UnifiedIndexer::process_batch_errors()`
8. **Embed and dual-write a parsed batch into Tantivy and the vector store** -> `UnifiedIndexer::process_and_index_batch()`
9. **Aggregate and log run metrics at the end of an indexing pass** -> `UnifiedIndexer::finalize_metrics()`
10. **Delete one file's chunks, commit, or wipe all stored data** -> `UnifiedIndexer::delete_file_chunks()`, `UnifiedIndexer::commit()`, `UnifiedIndexer::clear_all_data()`
11. **Expose Tantivy index, schema, vector store, embedder, metrics, and BM25 searcher** -> `UnifiedIndexer::tantivy_index()`, `UnifiedIndexer::tantivy_schema()`, `UnifiedIndexer::vector_store_cloned()`, `UnifiedIndexer::embedding_generator_cloned()`, `UnifiedIndexer::metrics()`, `UnifiedIndexer::create_bm25_search()`
12. **Log indexer teardown** -> `UnifiedIndexer::drop()`
