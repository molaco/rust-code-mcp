# indexing — Architecture

## Overview

The `indexing` module is the workspace's ingestion pipeline: it walks a Rust codebase, parses and chunks each `.rs` file, generates embeddings, and writes the results into both a Tantivy text index and a LanceDB-backed vector store. It owns the change-detection (Merkle-snapshot), throttling (memory- and GPU-aware batching), error categorization, retry, and consistency-checking machinery that surrounds those writes. The high-level `UnifiedIndexer` (full-pass coordinator) and `IncrementalIndexer` (snapshot-driven diff applier) are the entry points the rest of the system consumes.

## Mermaid diagram

```mermaid
graph TD
    subgraph EXT["External boundaries"]
        FS[("Filesystem<br/>.rs files")]
        TANTIVY[("Tantivy<br/>BM25 index")]
        LANCEDB[("VectorStore<br/>embedded LanceDB")]
        SNAPSHOT[("Merkle snapshot<br/>data dir")]
        EMBEDDER[["EmbeddingGenerator<br/>(GPU/CPU model)"]]
        CACHE[("MetadataCache<br/>stat + hash")]
    end

    subgraph TOP["Top-level coordinators"]
        INC["incremental<br/><i>IncrementalIndexer</i><br/>index_with_change_detection"]
        UNI["unified<br/><i>UnifiedIndexer</i><br/>IndexStats / IndexFileResult"]
    end

    subgraph CHANGE["Change detection"]
        MK["merkle<br/><i>FileSystemMerkle</i><br/>ChangeSet, Sha256Hasher"]
    end

    subgraph CORE["Per-file pipeline"]
        IC["indexer_core<br/><i>IndexerCore</i>"]
        FP["file_processor<br/><i>FileProcessor</i><br/>SecretsScanner,<br/>SensitiveFileFilter"]
        EB["embedding_batcher<br/><i>EmbeddingBatcher</i><br/>MemoryMonitor"]
    end

    subgraph WRITE["Dual-write"]
        TA["tantivy_adapter<br/><i>TantivyAdapter</i><br/>ChunkSchema, IndexWriter"]
    end

    subgraph SUPPORT["Cross-cutting"]
        ERR["error / errors<br/><i>IndexingError</i><br/>ErrorCollector, ErrorCategory"]
        RT["retry<br/>retry_with_backoff<br/>retry_sync_with_backoff"]
        CC["consistency<br/><i>ConsistencyChecker</i><br/>ConsistencyReport"]
    end

    FS -->|WalkDir .rs| MK
    SNAPSHOT <-->|load/save bincode| MK
    MK -->|ChangeSet<br/>added/modified/deleted| INC
    INC -->|delete_file_chunks<br/>+ index_file| UNI

    FS -->|collect_rust_files| UNI
    UNI -->|process_file_sync| IC
    IC --> FP
    FP <-->|stat + content hash| CACHE
    IC -->|CodeChunks| EB
    EB <-->|embed_batch<br/>gpu_batch_size window| EMBEDDER
    EB -.->|usage% / 85%<br/>backoff| UNI

    UNI -->|batched index_chunks| TA
    TA -->|writer.add_document<br/>commit per batch| TANTIVY
    UNI -->|upsert_chunks triples<br/>ChunkId,Vec f32,Chunk| LANCEDB

    UNI -.->|per-file errors| ERR
    ERR -.->|categorize_error| UNI
    UNI -.->|optional backoff wrapper| RT
    TANTIVY --> CC
    LANCEDB --> CC
    CC -->|ConsistencyReport| TOP
```

## Module responsibilities

| Module | Role | Key types |
|---|---|---|
| `mod` | Declares submodules and re-exports the public surface. | (re-exports only) |
| `consistency` | Cross-checks Tantivy and vector-store contents to detect divergence; structured-log reporter. | `ConsistencyChecker`, `ConsistencyReport` |
| `embedding_batcher` | Batches chunk embedding under memory- and GPU-aware limits via `MemoryMonitor`. | `EmbeddingBatcher`, `MemoryMonitor` |
| `error` | Unified per-file error enum with transparent `From` conversions. | `IndexingError` |
| `errors` | Thread-safe collection and keyword-based categorization of per-file failures. | `ErrorCollector`, `ErrorDetail`, `ErrorCategory`, `categorize_error` |
| `file_processor` | Filters sensitive paths, caps size, screens for secrets, and gates work via stat + content-hash cache. | `FileProcessor`, `MetadataCache`, `SecretsScanner`, `SensitiveFileFilter` |
| `incremental` | Drives Merkle-snapshot–based incremental reindexing. | `IncrementalIndexer`, `get_snapshot_path` |
| `indexer_core` | Bundles file processing, parsing, chunking, and embedding into a per-file pipeline. | `IndexerCore`, `IndexerCoreConfig`, `ProcessedFile` |
| `merkle` | Builds, persists, and diffs SHA-256 Merkle trees over `.rs` files. | `FileSystemMerkle`, `ChangeSet`, `MerkleSnapshot`, `Sha256Hasher`, `FileNode` |
| `retry` | Generic exponential-backoff retry helpers (sync + async). | `retry_with_backoff`, `retry_sync_with_backoff` |
| `tantivy_adapter` | Wraps a Tantivy `Index` and `IndexWriter` for chunk-oriented ops; releases lock on `Drop`. | `TantivyAdapter`, `TantivyConfig`, `ChunkSchema`, `Bm25Search` |
| `unified` | Top-level indexer coordinating parsing, embedding, Tantivy, and the vector store across files and directories. | `UnifiedIndexer`, `IndexStats`, `IndexFileResult`, `IndexingMetrics` |

## Data flow

The pipeline is layered so that each stage rejects work as early as possible to keep the embedding model — the most expensive step — fed only with files that actually need re-indexing.

1. **Discovery.** `UnifiedIndexer::collect_rust_files` (full-pass) or `FileSystemMerkle::from_directory` (incremental) walks the codebase with `WalkDir`, counting walk errors and producing a sorted `.rs` path list.
2. **Change detection.** `merkle::FileSystemMerkle` SHA-256-hashes every leaf, builds a `MerkleTree<Sha256Hasher>`, and compares the new root against a `bincode`-serialized snapshot under `ProjectDirs("dev", "rust-code-mcp", "search")/merkle/<sha16>.snapshot`. `has_changes` is an O(1) root-hash compare; mismatches feed `detect_changes` which yields a `ChangeSet { added, modified, deleted }`. `IncrementalIndexer::process_changes` dispatches each set: deletions and modifications run `UnifiedIndexer::delete_file_chunks` first, then modifications/additions go through `index_file`.
3. **Per-file gating.** `IndexerCore::process_file_sync` runs `FileProcessor::should_process_file` (sensitive-file filter + size cap), `has_stat_changed` (mtime/len from `MetadataCache`), reads the contents, runs `check_secrets`, then `has_file_changed` (content hash). Any short-circuit returns `IndexingError::Parser("File unchanged" | "File filtered: …" | "Contains secrets")`, which `UnifiedIndexer::index_file` translates into `IndexFileResult::{Unchanged, Skipped}`.
4. **Parse and chunk.** A fresh per-thread `RustParser` parses the source via `parse_source_complete`; `Chunker::chunk_file` slices the parse tree into `CodeChunk`s wrapped as `ProcessedFile { path, content, chunks, parse_duration }`.
5. **Embedding.** Chunks flow into `EmbeddingBatcher::generate_embeddings_batched`, which formats each via `CodeChunk::format_for_embedding`, splits into windows of `gpu_batch_size`, and dispatches each window through `EmbeddingGenerator::embed_batch`. `calculate_safe_batch_size` consults `MemoryMonitor` (≈15 MB-per-file heuristic, capped by `num_cpus::get()` and a hard ceiling of 100) to decide outer batch size.
6. **Dual write.**
   - Text side: `TantivyAdapter::index_chunks` builds Tantivy documents (`chunk_id`, `content`, `symbol_name`, `symbol_kind`, `file_path`, `module_path`, `docstring`, `chunk_json`) via the `ChunkSchema` and pushes them through the `IndexWriter`.
   - Vector side: chunks are zipped with embeddings into `(ChunkId, Vec<f32>, CodeChunk)` triples and `VectorStore::upsert_chunks` writes to embedded LanceDB.
7. **Metadata and metrics.** `FileProcessor::update_file_metadata` persists `(content_hash, mtime, len)` to the metadata cache. `IndexingMetrics` accumulates parse / embed / index durations, per-file latencies, peak memory, and cache-hit rate; `finalize_metrics` logs the summary.
8. **Commit.** `TantivyAdapter::commit` runs after every parallel batch (and at the end of `index_directory`); the vector store is durable per upsert. `IncrementalIndexer::index_with_change_detection` finally writes the new Merkle snapshot. `clear_all_data` wipes the metadata cache, all Tantivy docs, and the LanceDB collection together.
9. **Consistency.** Out-of-band, `ConsistencyChecker::check` enumerates Tantivy chunk IDs by walking every `SegmentReader`'s store and reads `VectorStore::count`, producing a `ConsistencyReport` that `print_summary` emits as a structured `tracing::info!` event (stdout stays reserved for JSON-RPC frames). `repair` is a placeholder; force-reindex is the current remediation.

## Concurrency / integration model

- **Async runtime.** The public surface (`UnifiedIndexer::index_file`, `index_directory*`, `IncrementalIndexer::index_with_change_detection`, `ConsistencyChecker::check`) is `async`, driven by the host Tokio runtime. The vector store and embedding APIs are awaited; Tantivy writes are synchronous and run on the calling task.
- **Parallel ingest.** `index_directory_parallel` is the throughput path. It iterates `rust_files.chunks(calculate_safe_batch_size())`:
  - **Phase 1 (CPU-bound, Rayon).** `par_iter().filter_map(...)` invokes `IndexerCore::process_file_sync` across the Rayon global pool to parse and chunk in parallel. Each task constructs its own `RustParser` to keep tree-sitter handles thread-local. Failures land in a cloned `ErrorCollector` rather than aborting the batch.
  - **Phase 2 (single-task).** Successful `ProcessedFile`s flatten into one `Vec<CodeChunk>`, embed in a single `generate_embeddings_batched` call, are written to Tantivy via one batched `index_chunks`, and upserted to LanceDB as one zipped triple list. `TantivyAdapter::commit` runs after every batch so progress is durable.
- **Batching strategy.** Two distinct batch sizes coexist: (a) the **outer file batch** (`calculate_safe_batch_size`, bounded by available RAM / 15 MB, CPU count, and the hard 100 ceiling) controls Phase 1 parallelism; (b) the **inner GPU window** (`gpu_batch_size`) inside `EmbeddingBatcher` slices chunks into model-sized calls. The outer batch optimizes parser throughput and memory headroom; the inner window keeps a single embedding pass within GPU VRAM.
- **Memory backoff (85% threshold).** Between every batch, `IndexerCore::memory_usage_percent` is checked; above 85% the loop warn-logs and `tokio::time::sleep(Duration::from_secs(5)).await`s to give allocators (and the GPU runtime) time to reclaim before launching the next embed call. `MemoryMonitor` lives behind `Arc<Mutex<...>>` inside `EmbeddingBatcher`, so concurrent reads of usage stats serialize on a short critical section.
- **Shared state.** `ErrorCollector` (`Arc<Mutex<Vec<ErrorDetail>>>`) is shared across Rayon workers in Phase 1; its entries are categorized by `categorize_error` (keyword match: `permission denied` / `not found` / `invalid utf` / `is a directory` → `Permanent`, else `Transient`) and folded into `IndexStats` by `process_batch_errors`. `MetadataCache`, the Tantivy `IndexWriter`, and the `VectorStore` handle are owned by `UnifiedIndexer` and accessed only from the single coordinator task in Phase 2 — there is no writer contention.
- **External APIs.**
  - `EmbeddingGenerator` (cloneable handle) — model inference, called from the coordinator task only; clone-exposed via `embedding_generator_cloned` for search components.
  - `tantivy::Index` + `IndexWriter` — opened or created in `TantivyAdapter::new` with `writer_with_num_threads(num_threads, num_threads * memory_budget_mb * MiB)`; Tantivy spawns its own merge threads internally.
  - `VectorStore` (embedded LanceDB) — created under `cache_path.parent()/vectors/<collection_name>`; cloneable for read-side consumers via `vector_store_cloned`. Auto-commits per upsert.
  - `ProjectDirs` data directory — Merkle snapshots persist to `<data>/merkle/<sha16>.snapshot` via `bincode`; missing `ProjectDirs` falls back to `./.merkle`.
  - `MetadataCache` — keyed by stringified path; stat- and content-hash entries.
- **Retry surface.** `retry_with_backoff` (async, `tokio::time::sleep`) and `retry_sync_with_backoff` (blocking `std::thread::sleep`) are exposed as generic helpers wrapping flaky external calls with exponential doubling; the indexing core itself surfaces typed errors and lets the caller decide whether to retry.
- **Error model.** All per-file failures funnel into `IndexingError` (`Io`, `Embedding`, `VectorStore`, `Parser`, `Cache`); `?`-propagation is preserved up to `index_file`, which returns `IndexFileResult::{Indexed { chunks_count }, Unchanged, Skipped}` so directory-level loops update `IndexStats` without unwinding the whole batch.
- **Drop semantics.** `TantivyAdapter::drop` calls `writer.rollback()` to discard any uncommitted segments and release the index lockfile (warn-logs on failure — Drop must not panic). `UnifiedIndexer::drop` only emits a debug log; its `TantivyAdapter` field's own `Drop` is what actually releases the lock. The vector store and `MetadataCache` rely on their own `Drop` impls for clean shutdown.
