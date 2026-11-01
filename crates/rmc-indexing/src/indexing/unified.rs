//! Unified indexing pipeline that populates both Tantivy (BM25) and vector store
//!
//! This module coordinates indexing operations by delegating to specialized adapters:
//! - TantivyAdapter: BM25 indexing operations
//! - VectorStore: Vector indexing operations (LanceDB embedded backend)
//! - IndexerCore: Core file processing and embedding generation

use crate::indexing::error_collection::{ErrorCategory, categorize_error};
use crate::indexing::indexer_core::IndexerCore;
use crate::indexing::tantivy_adapter::TantivyAdapter;
use crate::indexing::unified_parallel::{
    collect_rust_files, parallel_parse_batch, process_batch_errors,
};
use crate::metrics::IndexingMetrics;
use anyhow::{Context, Result};
use rmc_config::config::IndexerConfig;
use rmc_engine::chunker::CodeChunk;
use rmc_engine::embeddings::{Embedding, EmbeddingBackend, EmbeddingGenerator};
use rmc_engine::search::Bm25Search;
use rmc_engine::vector_store::VectorStore;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tantivy::Index;

/// Statistics from an indexing operation
#[derive(Debug, Clone, Default)]
pub struct IndexStats {
    /// Total number of files found
    pub total_files: usize,
    /// Number of files that were indexed
    pub indexed_files: usize,
    /// Number of unchanged files skipped through the metadata cache.
    pub unchanged_files: usize,
    /// Number of files intentionally skipped or removed.
    pub skipped_files: usize,
    /// Number of transient failures that must be retried.
    pub failed_files: usize,
    /// Paths that the Merkle snapshot must leave at their previous state.
    #[doc(hidden)]
    pub retry_files: Vec<PathBuf>,
    /// Total number of chunks generated
    pub total_chunks: usize,
    /// The run wrote to the vector store (appended, rewrote or dropped
    /// files), so it left versions and fragments a [`VectorMaintenance`]
    /// pass should reclaim.
    ///
    /// Indexing never runs that pass itself: the caller holds the
    /// workspace's exclusive lock while indexing, and maintenance of a large
    /// store takes minutes (425 s on a 249k-row table, 2026-10-31) — run
    /// under the same lock it kept every `search` of the workspace waiting.
    /// The caller releases the workspace lock first and then runs
    /// maintenance under the workspace's maintenance lock.
    pub needs_maintenance: bool,
}

/// Reclaims the disk space an indexing run's writes left behind: compaction,
/// index extension, old-version prune (`VectorStore::maintain`).
///
/// Every path that writes to the vector store — whole directory, parallel
/// whole directory, the per-file incremental update, dropping files outside
/// the tree — reports it through [`IndexStats::needs_maintenance`] (or, for
/// the drop, a non-empty result), and its caller runs this handle. The
/// handle holds only the store, not the indexer: the metadata cache and the
/// Tantivy writer are closed by the time it runs, so a reader that opens
/// them meanwhile finds them free.
///
/// Concurrent readers are safe: LanceDB readers see a committed version
/// (MVCC), and the prune keeps every version younger than
/// `RMC_VECTOR_PRUNE_KEEP_SECS` for a reader that opened one before the pass
/// began. Concurrent WRITERS are not, which is the caller's job: run this
/// under the workspace's maintenance lock, which every index writer takes.
#[derive(Clone)]
pub struct VectorMaintenance {
    vector_store: VectorStore,
}

impl std::fmt::Debug for VectorMaintenance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("VectorMaintenance")
    }
}

impl VectorMaintenance {
    /// Run the pass.
    ///
    /// Deliberately infallible: the index is correct either way, and a
    /// maintenance failure is a disk-space problem rather than an indexing
    /// one. Failing the run over it would mean a full disk also destroys
    /// the record of the work that did complete.
    ///
    /// `workspace` only names the pass in its `mem:` line: the line used to
    /// say just "vector store maintenance", and a log of several projects'
    /// syncs left `--tool-latency` to guess whose pass it was.
    pub async fn run(&self, workspace: &Path) {
        if !vector_maintenance_enabled() {
            tracing::debug!(
                "Vector store maintenance disabled by {}",
                VECTOR_MAINTENANCE_ENV
            );
            return;
        }
        let started = std::time::Instant::now();
        let mem = rmc_engine::mem_trace::MemSpan::start(format!(
            "vector store maintenance of {}",
            workspace.display()
        ));
        let result = self.vector_store.maintain(vector_prune_keep()).await;
        mem.finish();
        match result {
            Ok(stats) => tracing::info!(
                "Vector store maintenance: {} fragment(s) merged into {}, {} old version(s) pruned, {:.1} MiB reclaimed in {:.2}s",
                stats.fragments_removed,
                stats.fragments_added,
                stats.old_versions_removed,
                stats.bytes_removed as f64 / (1024.0 * 1024.0),
                started.elapsed().as_secs_f64(),
            ),
            Err(e) => tracing::warn!(
                "Vector store maintenance failed (index is still correct, disk use will keep growing): {}",
                e
            ),
        }
    }
}

impl IndexStats {
    /// Create stats indicating no changes
    pub fn unchanged() -> Self {
        Self::default()
    }

    pub(crate) fn record_error(&mut self, file_path: &Path, error: &anyhow::Error) {
        match categorize_error(error.as_ref()) {
            ErrorCategory::Permanent => {
                tracing::debug!("Skipped {}: {}", file_path.display(), error);
                self.skipped_files += 1;
            }
            ErrorCategory::Transient => {
                tracing::warn!("Failed {} (will retry): {}", file_path.display(), error);
                self.failed_files += 1;
                self.retry_files.push(file_path.to_path_buf());
            }
        }
    }
}

/// Result of indexing a single file
#[derive(Debug)]
pub enum IndexFileResult {
    /// File was indexed successfully with N chunks
    Indexed { chunks_count: usize },
    /// File was unchanged, no reindexing needed
    Unchanged,
    /// File was skipped (error or no chunks)
    Skipped,
}

/// A file read, chunked and embedded, and not written anywhere yet — see
/// [`UnifiedIndexer::prepare_file`] and [`UnifiedIndexer::write_files`].
pub(crate) struct PreparedFile {
    pub(crate) path: PathBuf,
    /// The text the chunks were cut from: the metadata cache records its
    /// hash once the chunks are in the store.
    pub(crate) content: String,
    pub(crate) chunks: Vec<CodeChunk>,
    /// One per chunk, in chunk order.
    pub(crate) embeddings: Vec<Embedding>,
}

/// What [`UnifiedIndexer::prepare_file`] made of a file.
pub(crate) enum Preparation {
    Ready(PreparedFile),
    /// The metadata cache says the stores already hold this content.
    Unchanged,
    /// Not a file this indexer processes.
    Skipped,
}

/// Unified indexer that coordinates Tantivy and vector store operations
pub struct UnifiedIndexer {
    /// Core indexing logic
    core: IndexerCore,
    /// Tantivy adapter for BM25 indexing
    tantivy: TantivyAdapter,
    /// Vector store for semantic indexing (LanceDB)
    vector_store: VectorStore,
    /// Performance metrics
    metrics: IndexingMetrics,
    /// Active embedding backend configuration. Stored for downstream
    /// callers (e.g. cache-version checks) that need to reconcile
    /// against the model that built this index.
    backend: EmbeddingBackend,
    /// Existing path on the filesystem that stores the persistent indexes.
    storage_root: PathBuf,
}

const MIN_INDEX_FREE_ENV: &str = "RMC_MIN_INDEX_FREE_MB";
const DEFAULT_MIN_INDEX_FREE_MB: u64 = 2048;

fn configured_min_index_free_mb() -> u64 {
    std::env::var(MIN_INDEX_FREE_ENV)
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(DEFAULT_MIN_INDEX_FREE_MB)
}

fn has_disk_headroom(available_bytes: u64, minimum_mb: u64) -> bool {
    minimum_mb == 0 || available_bytes >= minimum_mb.saturating_mul(1024 * 1024)
}

/// Set to `0` to skip the post-run vector-store maintenance pass entirely.
const VECTOR_MAINTENANCE_ENV: &str = "RMC_VECTOR_MAINTENANCE";

/// Age below which dataset versions are left alone by the prune pass.
///
/// Not a tuning number: it is the margin for a reader that opened the
/// table before maintenance started and is still walking it. Ten minutes
/// is far longer than any single query and far shorter than the interval
/// at which versions pile up.
const VECTOR_PRUNE_KEEP_ENV: &str = "RMC_VECTOR_PRUNE_KEEP_SECS";
const DEFAULT_VECTOR_PRUNE_KEEP_SECS: u64 = 600;

fn vector_maintenance_enabled() -> bool {
    !matches!(
        std::env::var(VECTOR_MAINTENANCE_ENV)
            .ok()
            .map(|v| v.trim().to_ascii_lowercase())
            .as_deref(),
        Some("0") | Some("false") | Some("no") | Some("off")
    )
}

fn vector_prune_keep() -> std::time::Duration {
    let secs = std::env::var(VECTOR_PRUNE_KEEP_ENV)
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(DEFAULT_VECTOR_PRUNE_KEEP_SECS);
    std::time::Duration::from_secs(secs)
}

fn indexer_config(
    cache_path: &Path,
    tantivy_path: &Path,
    codebase_loc: Option<usize>,
) -> IndexerConfig {
    codebase_loc
        .map(|loc| IndexerConfig::for_codebase_size(loc, cache_path, tantivy_path))
        .unwrap_or_else(|| IndexerConfig::default(cache_path, tantivy_path))
}

/// Where the vector store of `collection_name` lives, next to the project's
/// metadata cache.
fn vector_store_path(cache_path: &Path, collection_name: &str) -> PathBuf {
    cache_path
        .parent()
        .unwrap_or(cache_path)
        .join("vectors")
        .join(collection_name)
}

/// [`rebuild_bm25_if_empty`] without a [`UnifiedIndexer`]: the BM25 writer
/// and the EXISTING vector store only, no embedding generator.
///
/// For a sync tick that found no file changes. Bringing up the whole indexer
/// there would build an embedding session just to rebuild a keyword index
/// that needs none, on a path promised to stay cheap when nothing changed.
/// No vector store yet means nothing to rebuild from: `None`, as for an
/// index that is not empty.
pub async fn rebuild_bm25_from_existing_store(
    cache_path: &Path,
    tantivy_path: &Path,
    collection_name: &str,
    vector_size: usize,
    embedder_identity: &str,
    codebase_loc: Option<usize>,
) -> Result<Option<usize>> {
    let vector_store = match VectorStore::open_existing_embedded(
        vector_store_path(cache_path, collection_name),
        vector_size,
        embedder_identity,
    )
    .await
    {
        Ok(store) => store,
        Err(rmc_engine::vector_store::VectorStoreError::NotFound(_)) => return Ok(None),
        Err(e) => return Err(anyhow::anyhow!("Failed to open the vector store: {}", e)),
    };
    let mut tantivy =
        TantivyAdapter::new(indexer_config(cache_path, tantivy_path, codebase_loc).tantivy)?;
    rebuild_bm25_if_empty(&mut tantivy, &vector_store).await
}

/// Fill an empty BM25 index from the vector store, if the store has chunks.
/// Returns how many chunks were indexed, `None` when there was nothing to do.
///
/// Each BM25 schema version lives in its own directory (see
/// `rmc_engine::schema::BM25_SCHEMA_VERSION`), so the first run of a new
/// version finds an empty index next to a full vector store. Without this
/// the index would stay empty: change detection skips every file the Merkle
/// snapshot already knows, and nothing would ever reach BM25 again. The store
/// holds every chunk in full, so this is a rewrite of the keyword index only —
/// no file is re-read and nothing is re-embedded.
///
/// Committed once, at the end. A run that dies halfway leaves an index with
/// no committed documents, which is exactly the condition that brings the
/// next run back here; committing per batch would leave a partial index that
/// no longer looks empty.
pub(crate) async fn rebuild_bm25_if_empty(
    tantivy: &mut TantivyAdapter,
    vector_store: &VectorStore,
) -> Result<Option<usize>> {
    if tantivy.committed_doc_count()? > 0 {
        return Ok(None);
    }
    let stored = vector_store
        .count()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to count vector store rows: {}", e))?;
    if stored == 0 {
        return Ok(None);
    }

    tracing::info!(
        stored_chunks = stored,
        "BM25 index is empty while the vector store holds chunks; rebuilding it from the store"
    );
    let started = Instant::now();
    let indexed = vector_store
        .scan_chunks(&mut |chunks| tantivy.index_chunks(&chunks))
        .await
        .context("Failed to rebuild the BM25 index from the vector store")?;
    tantivy.commit()?;
    tracing::info!(
        indexed_chunks = indexed,
        stored_chunks = stored,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "BM25 index rebuilt from the vector store"
    );
    Ok(Some(indexed))
}

fn nearest_existing_path(path: &Path) -> &Path {
    let mut candidate = path;
    while !candidate.exists() {
        let Some(parent) = candidate.parent() else {
            break;
        };
        candidate = parent;
    }
    candidate
}

pub(crate) fn ensure_index_disk_headroom(path: &Path) -> Result<()> {
    let minimum_mb = configured_min_index_free_mb();
    if minimum_mb == 0 {
        return Ok(());
    }
    let probe = nearest_existing_path(path);
    let available = fs2::available_space(probe)
        .with_context(|| format!("Failed to read free space at {}", probe.display()))?;
    if !has_disk_headroom(available, minimum_mb) {
        anyhow::bail!(
            "index write refused: {} MiB free at {}, below {}={} MiB; free disk space or set the threshold to 0 to disable",
            available / (1024 * 1024),
            probe.display(),
            MIN_INDEX_FREE_ENV,
            minimum_mb,
        );
    }
    Ok(())
}

impl UnifiedIndexer {
    /// Create a new unified indexer with embedded LanceDB backend
    ///
    /// # Arguments
    /// * `cache_path` - Path to metadata cache directory
    /// * `tantivy_path` - Path to Tantivy index directory
    /// * `collection_name` - Collection/table name for vector store
    /// * `vector_size` - Vector dimensions (depends on active embedder
    ///   backend; default is 1024 for Qwen3-Embedding-0.6B)
    /// * `embedder_identity` - Stable identity for the active embedder
    ///   (`EmbeddingBackend::identity()`); written to the vector store's
    ///   `metadata.json` and reconciled on reopen.
    /// * `codebase_loc` - Estimated lines of code (for Tantivy optimization)
    pub async fn for_embedded(
        cache_path: &Path,
        tantivy_path: &Path,
        collection_name: &str,
        vector_size: usize,
        embedder_identity: &str,
        codebase_loc: Option<usize>,
    ) -> Result<Self> {
        Self::for_embedded_with_backend(
            cache_path,
            tantivy_path,
            collection_name,
            vector_size,
            embedder_identity,
            codebase_loc,
            EmbeddingBackend::default(),
        )
        .await
    }

    /// Create a new unified indexer with an explicit embedding backend.
    ///
    /// `vector_size` and `embedder_identity` should normally be derived
    /// from `backend.dim()` and `backend.identity()`; they are passed
    /// explicitly so existing call sites that pre-compute them stay
    /// compatible without re-deriving.
    pub async fn for_embedded_with_backend(
        cache_path: &Path,
        tantivy_path: &Path,
        collection_name: &str,
        vector_size: usize,
        embedder_identity: &str,
        codebase_loc: Option<usize>,
        backend: EmbeddingBackend,
    ) -> Result<Self> {
        tracing::info!(
            "Initializing UnifiedIndexer with embedded LanceDB (embedder={})...",
            backend.identity()
        );

        let indexer_config = indexer_config(cache_path, tantivy_path, codebase_loc);

        // Initialize core with the explicit backend so the embedding
        // generator inside agrees with what we tell the vector store.
        let core =
            IndexerCore::with_backend(cache_path, Some(indexer_config.core), backend.clone())?;

        // Initialize Tantivy adapter
        let tantivy = TantivyAdapter::new(indexer_config.tantivy)?;

        // Initialize embedded vector store
        let vector_path = vector_store_path(cache_path, collection_name);
        let vector_store =
            VectorStore::new_embedded(vector_path.clone(), vector_size, embedder_identity)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to initialize VectorStore: {}", e))?;

        tracing::info!("UnifiedIndexer initialized successfully with embedded backend");

        let mut indexer = Self {
            core,
            tantivy,
            vector_store,
            metrics: IndexingMetrics::new(),
            backend,
            storage_root: vector_path,
        };
        rebuild_bm25_if_empty(&mut indexer.tantivy, &indexer.vector_store).await?;
        Ok(indexer)
    }

    /// Borrow the active embedding backend configuration.
    pub fn backend(&self) -> &EmbeddingBackend {
        &self.backend
    }

    /// Index a single file to both Tantivy and vector store
    pub async fn index_file(&mut self, file_path: &Path) -> Result<IndexFileResult> {
        self.index_file_impl(file_path, true).await
    }

    /// Reindex a path that the Merkle tree proved was modified.
    ///
    /// This deliberately bypasses the metadata-cache fast path. If a previous
    /// multi-file run wrote this file successfully but failed before committing
    /// its Merkle snapshot, the cache already describes the new bytes. The next
    /// run still has to rebuild after deleting the path's old chunks.
    pub(crate) async fn reindex_file(&mut self, file_path: &Path) -> Result<IndexFileResult> {
        self.index_file_impl(file_path, false).await
    }

    async fn index_file_impl(
        &mut self,
        file_path: &Path,
        consult_metadata_cache: bool,
    ) -> Result<IndexFileResult> {
        let file = match self.prepare_file(file_path, consult_metadata_cache).await? {
            Preparation::Ready(file) => file,
            Preparation::Unchanged => return Ok(IndexFileResult::Unchanged),
            Preparation::Skipped => return Ok(IndexFileResult::Skipped),
        };
        let chunks_count = file.chunks.len();
        self.write_files(vec![file], &[]).await?;
        Ok(if chunks_count == 0 {
            IndexFileResult::Skipped
        } else {
            IndexFileResult::Indexed { chunks_count }
        })
    }

    /// Read, chunk and embed one file, writing nothing.
    ///
    /// The expensive half of indexing a file, split from the write so that a
    /// sync can prepare every changed file first and then write them all in
    /// one go ([`Self::write_files`]). A file with no chunks is still
    /// `Ready`: writing it removes the chunks it used to have.
    pub(crate) async fn prepare_file(
        &mut self,
        file_path: &Path,
        consult_metadata_cache: bool,
    ) -> Result<Preparation> {
        ensure_index_disk_headroom(&self.storage_root)?;
        let file_start = Instant::now();

        // Check if file should be processed
        if !self.core.should_process_file(file_path)? {
            return Ok(Preparation::Skipped);
        }

        // Fast stat-based change detection (avoids reading file content)
        if consult_metadata_cache {
            if !self.core.has_stat_changed(file_path)? {
                tracing::debug!("File unchanged (stat): {}", file_path.display());
                return Ok(Preparation::Unchanged);
            }
        }

        // Read file content (only if stat suggests change)
        let content = std::fs::read_to_string(file_path)
            .context(format!("Failed to read file: {}", file_path.display()))?;

        // Content hash check (confirms stat-based detection)
        if consult_metadata_cache && !self.core.has_file_changed(file_path, &content)? {
            tracing::debug!("File unchanged (hash): {}", file_path.display());
            return Ok(Preparation::Unchanged);
        }

        // Process file (parse and chunk)
        let processed = self.core.process_file_sync(file_path)?;
        if processed.chunks.is_empty() {
            tracing::warn!("No chunks generated for {}", file_path.display());
        }
        let embeddings = if processed.chunks.is_empty() {
            Vec::new()
        } else {
            self.core
                .generate_embeddings_batched(&processed.chunks)
                .await?
        };

        let file_duration = file_start.elapsed();
        self.metrics.file_latencies.push(file_duration);
        tracing::debug!(
            "Prepared {} chunks from {} in {:?}",
            processed.chunks.len(),
            file_path.display(),
            file_duration
        );
        Ok(Preparation::Ready(PreparedFile {
            path: processed.path,
            content: processed.content,
            chunks: processed.chunks,
            embeddings,
        }))
    }

    /// Replace the chunks of `files` and drop those of `removed`, in both
    /// stores: ONE delete and ONE append in the vector store, whatever the
    /// number of files.
    ///
    /// Every write publishes a vector-store version, and the append a
    /// fragment, so writing file by file made a 36-file sync publish 72
    /// versions and 36 fragments (measured 2026-10-29) — and pay a scan of
    /// the table for each delete. Here it is two versions and one fragment.
    ///
    /// The chunks of every written file are deleted first, whether or not
    /// the caller thinks the stores hold any: chunk ids are random, so a
    /// write without the delete leaves the old chunks beside the new ones
    /// for good, and "the cache did not know the file" (a wipe, a renamed
    /// cache) does not mean the stores do not.
    ///
    /// The metadata cache records a file only after its chunks are in the
    /// store; a failure anywhere leaves every file here unrecorded, so the
    /// caller retries all of them. BM25 is written in between and committed
    /// by the caller.
    pub(crate) async fn write_files(
        &mut self,
        files: Vec<PreparedFile>,
        removed: &[PathBuf],
    ) -> Result<()> {
        if files.is_empty() && removed.is_empty() {
            return Ok(());
        }
        let started = Instant::now();
        let paths: Vec<&Path> = removed
            .iter()
            .map(PathBuf::as_path)
            .chain(files.iter().map(|file| file.path.as_path()))
            .collect();
        self.delete_files_chunks(&paths).await?;

        let mut rows = Vec::with_capacity(files.iter().map(|file| file.chunks.len()).sum());
        let mut written = Vec::with_capacity(files.len());
        for file in files {
            assert_eq!(
                file.chunks.len(),
                file.embeddings.len(),
                "one embedding per chunk of {}",
                file.path.display()
            );
            self.tantivy.index_chunks(&file.chunks)?;
            let chunk_count = file.chunks.len();
            rows.extend(
                file.chunks
                    .into_iter()
                    .zip(file.embeddings)
                    .map(|(chunk, embedding)| (chunk.id, embedding, chunk)),
            );
            written.push((file.path, file.content, chunk_count));
        }
        let row_count = rows.len();
        self.vector_store
            .insert_chunks(rows)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to index chunks to vector store: {}", e))?;

        // A file with no chunks is not recorded: the next run looks at it
        // again, as it always has.
        for (path, content, chunk_count) in &written {
            if *chunk_count > 0 {
                self.core.update_file_metadata(path, content)?;
            }
        }

        // Persistence is complete at this point. A best-effort metrics probe
        // must not turn that success into an indexing failure.
        if let Err(error) = self.core.refresh_memory_monitor() {
            tracing::warn!("Could not refresh indexing memory metrics: {error}");
        } else if let Ok(memory_used) = self.core.memory_used_bytes() {
            self.metrics.peak_memory_bytes = self.metrics.peak_memory_bytes.max(memory_used);
        }

        tracing::info!(
            "✓ Wrote {} file(s), {} chunks, removed {} file(s) in {:?}",
            written.len(),
            row_count,
            removed.len(),
            started.elapsed()
        );
        Ok(())
    }

    /// Index an entire directory
    pub async fn index_directory(&mut self, dir_path: &Path) -> Result<IndexStats> {
        let total_start = Instant::now();
        tracing::info!("Indexing directory: {}", dir_path.display());

        let mut stats = IndexStats::default();
        self.metrics = IndexingMetrics::new();

        // Find all Rust files
        let (rust_files, walk_errors) = collect_rust_files(dir_path, &mut stats)?;

        if rust_files.is_empty() {
            return Ok(stats);
        }

        tracing::info!(
            "Found {} Rust files in {}",
            rust_files.len(),
            dir_path.display()
        );

        // Index each file
        for file in &rust_files {
            match self.index_file(file).await {
                Ok(IndexFileResult::Indexed { chunks_count }) => {
                    stats.indexed_files += 1;
                    stats.total_chunks += chunks_count;
                }
                Ok(IndexFileResult::Unchanged) => {
                    stats.unchanged_files += 1;
                }
                Ok(IndexFileResult::Skipped) => {
                    stats.skipped_files += 1;
                }
                Err(e) => {
                    stats.record_error(file, &e);
                }
            }
        }

        // Commit Tantivy changes
        self.tantivy
            .commit()
            .context("Failed to commit Tantivy index")?;

        let dropped = self
            .drop_files_outside_walk(&rust_files, walk_errors)
            .await?;

        // Finalize metrics
        self.finalize_metrics(&stats, total_start.elapsed());

        tracing::info!(
            "✓ Indexing complete: {} files indexed, {} chunks, {} unchanged, {} skipped, {} failed",
            stats.indexed_files,
            stats.total_chunks,
            stats.unchanged_files,
            stats.skipped_files,
            stats.failed_files
        );

        if stats.indexed_files > 0 || dropped > 0 {
            stats.needs_maintenance = true;
        }

        Ok(stats)
    }

    /// Index an entire directory with automatic backup management
    pub(crate) async fn index_directory_with_backup(
        &mut self,
        dir_path: &Path,
        backup_manager: Option<&dyn crate::indexing::backup::Backup>,
    ) -> Result<IndexStats> {
        let stats = self.index_directory(dir_path).await?;

        if let Some(manager) = backup_manager {
            if stats.indexed_files > 0 && stats.indexed_files % 100 == 0 {
                tracing::info!(
                    "Creating Merkle snapshot backup after {} indexed files",
                    stats.indexed_files
                );

                match crate::indexing::merkle::FileSystemMerkle::from_directory(dir_path) {
                    Ok(merkle) => {
                        if let Err(e) = manager.create_backup(&merkle) {
                            tracing::warn!("Failed to create backup: {}", e);
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed to build Merkle tree for backup: {}", e);
                    }
                }
            }
        }

        Ok(stats)
    }

    /// Index an entire directory using parallel processing
    pub async fn index_directory_parallel(&mut self, dir_path: &Path) -> Result<IndexStats> {
        let total_start = Instant::now();
        tracing::info!("Indexing directory (parallel mode): {}", dir_path.display());

        let mut stats = IndexStats::default();
        self.metrics = IndexingMetrics::new();

        // Find all Rust files
        let (rust_files, walk_errors) = collect_rust_files(dir_path, &mut stats)?;

        if rust_files.is_empty() {
            return Ok(stats);
        }

        tracing::info!(
            "Found {} Rust files, processing in parallel",
            rust_files.len()
        );

        // Calculate safe batch size
        let batch_size = self.core.calculate_safe_batch_size()?;
        tracing::info!("Using batch size: {}", batch_size);

        // Process in batches
        for (batch_idx, file_batch) in rust_files.chunks(batch_size).enumerate() {
            ensure_index_disk_headroom(&self.storage_root)?;
            tracing::info!(
                "Processing batch {}/{} ({} files)",
                batch_idx + 1,
                (rust_files.len() + batch_size - 1) / batch_size,
                file_batch.len()
            );

            // Check memory before batch
            let memory_usage = self.core.memory_usage_percent()?;
            if memory_usage > 85.0 {
                tracing::warn!(
                    "High memory usage ({:.1}%), pausing to allow GC",
                    memory_usage
                );
                tokio::time::sleep(Duration::from_secs(5)).await;
            }

            // PHASE 1: Parallel parse and chunk (CPU-bound)
            let parse_start = Instant::now();
            let (processed, error_collector) = parallel_parse_batch(&self.core, file_batch);

            let parse_duration = parse_start.elapsed();
            self.metrics.parse_duration += parse_duration;

            tracing::info!(
                "Parsed {} files in {:.2}s ({:.1} files/sec)",
                processed.len(),
                parse_duration.as_secs_f64(),
                processed.len() as f64 / parse_duration.as_secs_f64()
            );

            // Log errors and update stats
            process_batch_errors(&error_collector, &mut stats);

            // PHASE 2: Batch embedding for all files
            if !processed.is_empty() {
                self.process_and_index_batch(processed, &mut stats).await?;
            }

            // Commit after each batch
            self.tantivy.commit()?;
        }

        let dropped = self
            .drop_files_outside_walk(&rust_files, walk_errors)
            .await?;

        // Finalize metrics
        self.finalize_metrics(&stats, total_start.elapsed());

        tracing::info!(
            "✓ Parallel indexing complete: {} files indexed, {} chunks, {} skipped, {} failed",
            stats.indexed_files,
            stats.total_chunks,
            stats.skipped_files,
            stats.failed_files
        );

        if stats.indexed_files > 0 || dropped > 0 {
            stats.needs_maintenance = true;
        }

        Ok(stats)
    }

    /// The end of a full pass: drop the files the stores hold but the walk
    /// did not find (see [`Self::drop_files_outside`]), unless the walk hit
    /// errors and so may have missed live files. Returns how many went.
    async fn drop_files_outside_walk(
        &mut self,
        walked: &[PathBuf],
        walk_errors: usize,
    ) -> Result<usize> {
        if walk_errors > 0 {
            tracing::warn!(
                "Not dropping files outside the tree: the walk hit {} error(s) and may be incomplete",
                walk_errors
            );
            return Ok(0);
        }
        Ok(self.drop_files_outside(walked).await?.len())
    }

    /// Drop from both stores, and from the metadata cache, every file the
    /// vector store holds that `tree` does not list.
    ///
    /// The stores learn that a file left the tree only from change
    /// detection, which compares the tree with the previous Merkle snapshot.
    /// A full pass has no previous snapshot, so a file deleted while the
    /// snapshot was gone (a wipe, a new chunking identity over an existing
    /// collection) kept its chunks for good, and search kept answering with
    /// code that no longer exists — 41 such files of 6510 in a live store
    /// (2026-10-30). Every full pass therefore ends here, and so does a
    /// coverage repair.
    ///
    /// `tree` must be a COMPLETE walk of the project with the indexer's own
    /// walker (`traversal::collect_project_rust_files`): a live file missing
    /// from it loses its chunks. Returns the dropped paths, sorted.
    pub async fn drop_files_outside(&mut self, tree: &[PathBuf]) -> Result<Vec<String>> {
        // The stores key files by `to_string_lossy` of the walked path, as
        // every write and delete does.
        let tree: HashSet<String> = tree
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect();
        let indexed = self
            .vector_store
            .indexed_file_paths()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to list indexed files: {}", e))?;
        let mut gone: Vec<String> = indexed
            .into_iter()
            .filter(|path| !tree.contains(path))
            .collect();
        if gone.is_empty() {
            return Ok(gone);
        }
        gone.sort();

        let paths: Vec<&Path> = gone.iter().map(Path::new).collect();
        self.delete_files_chunks(&paths).await?;
        self.tantivy.commit()?;
        // A walk that comes upon one of these files again must embed it,
        // not wave it through as cached.
        for path in &paths {
            self.core.forget_file(path)?;
        }
        tracing::info!(
            "Dropped the chunks of {} file(s) no longer in the tree",
            gone.len()
        );
        Ok(gone)
    }

    /// Delete all chunks of these files from both Tantivy and the vector
    /// store — one vector-store write for all of them.
    pub async fn delete_files_chunks(&mut self, file_paths: &[&Path]) -> Result<()> {
        for path in file_paths {
            self.tantivy.delete_file_chunks(path)?;
        }
        let paths: Vec<String> = file_paths
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect();
        self.vector_store
            .delete_by_file_paths(&paths)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to delete chunks from vector store: {}", e))?;
        tracing::debug!("Deleted chunks of {} file(s)", paths.len());
        Ok(())
    }

    /// Commit Tantivy changes
    pub fn commit(&mut self) -> Result<()> {
        self.tantivy.commit()
    }

    /// The maintenance pass over this indexer's vector store, as a handle
    /// that outlives the indexer and the caller's workspace lock — see
    /// [`IndexStats::needs_maintenance`] for when to run it and why not here.
    pub fn vector_maintenance(&self) -> VectorMaintenance {
        VectorMaintenance {
            vector_store: self.vector_store.clone(),
        }
    }

    /// Clear all indexed data (metadata cache, Tantivy, and vector store)
    pub async fn clear_all_data(&mut self) -> Result<()> {
        tracing::info!("Clearing all indexed data (metadata cache, Tantivy, vector store)...");

        self.core.clear_metadata_cache()?;
        tracing::info!("✓ Cleared metadata cache");

        self.tantivy.delete_all()?;
        self.tantivy.commit()?;
        tracing::info!("✓ Cleared Tantivy index");

        self.vector_store
            .clear_collection()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to clear vector store: {}", e))?;
        tracing::info!("✓ Cleared vector store");

        tracing::info!("✓ All indexed data cleared successfully");
        Ok(())
    }

    /// Drop one file's metadata-cache entry, so a later run treats it as new.
    ///
    /// Used when a file leaves the tree: its chunks go from the stores, and
    /// the cache entry must go with them.
    pub fn forget_file(&self, file_path: &Path) -> Result<()> {
        self.core.forget_file(file_path)?;
        Ok(())
    }

    /// Forget cache entries whose file has no vectors in the store.
    ///
    /// These are the files `monitoring::health` reports as `stale_skips`: the
    /// metadata cache says "done", the vector store holds nothing for them.
    /// Two different histories end up here, and both are repaired the same way:
    ///
    /// - the file is GONE from the tree (renamed, deleted) and its entry
    ///   outlived it — on the live index of a large workspace this was 38 of 39 cases;
    /// - the file is still there but its vectors were lost after the cache
    ///   entry was written (a run killed mid-flight, a collection rebuilt
    ///   underneath) — the remaining 1 of 39.
    ///
    /// Either way the pair is self-sustaining: the cache entry makes every
    /// later run skip the file, so the vectors are never rebuilt. Forgetting
    /// the entry ends it — a live file gets re-embedded on the next walk, a
    /// dead one simply stops being compared.
    ///
    /// Removing just those entries is what makes repair cheap: the rest of the
    /// tree keeps its cache and is waved through by the stat check, so only
    /// these files are re-embedded. Returns the paths that were forgotten.
    pub async fn forget_files_without_vectors(&mut self) -> Result<Vec<String>> {
        let cached = self.core.cached_paths()?;
        let indexed = self
            .vector_store
            .indexed_file_paths()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to list indexed files: {}", e))?;

        let mut stale: Vec<String> = cached
            .into_iter()
            .filter(|path| !indexed.contains(path))
            .collect();
        stale.sort();

        for path in &stale {
            self.core.forget_file(Path::new(path))?;
            tracing::debug!("Forgot stale cache entry: {}", path);
        }

        if stale.is_empty() {
            tracing::info!("Coverage repair: no cached files are missing vectors");
        } else {
            tracing::info!(
                "Coverage repair: forgot {} cache entries with no vectors",
                stale.len()
            );
        }

        Ok(stale)
    }

    /// Get access to the Tantivy index for searching
    pub fn tantivy_index(&self) -> &Index {
        self.tantivy.index()
    }

    /// Get cloned vector store for searching
    pub fn vector_store_cloned(&self) -> VectorStore {
        self.vector_store.clone()
    }

    /// Get cloned embedding generator for searching
    pub fn embedding_generator_cloned(&self) -> Result<EmbeddingGenerator> {
        self.core
            .embedding_generator_cloned()
            .map_err(|e| anyhow::anyhow!("Failed to initialize embedding generator: {}", e))
    }

    /// Get access to the Tantivy schema
    pub fn tantivy_schema(&self) -> &rmc_engine::schema::ChunkSchema {
        self.tantivy.schema()
    }

    /// Get access to the current metrics
    pub fn metrics(&self) -> &IndexingMetrics {
        &self.metrics
    }

    /// Create a Bm25Search instance from the Tantivy index
    pub fn create_bm25_search(&self) -> Result<Bm25Search> {
        self.tantivy.create_bm25_search()
    }

    // Private helper methods

    async fn process_and_index_batch(
        &mut self,
        processed: Vec<crate::indexing::indexer_core::ProcessedFile>,
        stats: &mut IndexStats,
    ) -> Result<()> {
        let embed_start = Instant::now();
        let file_count = processed.len();

        // Collect all chunks from all files for batch processing
        let all_chunks: Vec<CodeChunk> = processed
            .iter()
            .flat_map(|p| p.chunks.iter())
            .cloned()
            .collect();

        let total_chunks = all_chunks.len();
        tracing::info!(
            "Batch embedding {} chunks from {} files...",
            total_chunks,
            processed.len()
        );

        // Generate embeddings in GPU-optimized batches
        let all_embeddings = self.core.generate_embeddings_batched(&all_chunks).await?;

        let embed_duration = embed_start.elapsed();
        self.metrics.embed_duration += embed_duration;

        tracing::info!(
            "Generated {} embeddings in {:.2}s ({:.1} chunks/sec)",
            all_embeddings.len(),
            embed_duration.as_secs_f64(),
            all_embeddings.len() as f64 / embed_duration.as_secs_f64()
        );

        // PHASE 3: one write for the whole batch (Tantivy + LanceDB).
        let index_start = Instant::now();
        let mut embeddings = all_embeddings.into_iter();
        let files: Vec<PreparedFile> = processed
            .into_iter()
            .map(|file| PreparedFile {
                embeddings: embeddings.by_ref().take(file.chunks.len()).collect(),
                path: file.path,
                content: file.content,
                chunks: file.chunks,
            })
            .collect();
        assert!(
            embeddings.next().is_none(),
            "one embedding per chunk of the batch"
        );
        self.write_files(files, &[]).await?;

        let index_duration = index_start.elapsed();
        self.metrics.index_duration += index_duration;

        stats.indexed_files += file_count;
        stats.total_chunks += total_chunks;
        // Per-file latency, approximated as batch time / number of files.
        let approx_file_duration = index_duration / file_count as u32;
        self.metrics
            .file_latencies
            .extend(std::iter::repeat_n(approx_file_duration, file_count));

        tracing::info!(
            "Indexed {} chunks ({} files) in {:.2}s ({:.1} chunks/sec)",
            total_chunks,
            file_count,
            index_duration.as_secs_f64(),
            total_chunks as f64 / index_duration.as_secs_f64()
        );

        Ok(())
    }

    fn finalize_metrics(&mut self, stats: &IndexStats, total_duration: Duration) {
        self.metrics.total_duration = total_duration;
        self.metrics.total_files = stats.total_files;
        self.metrics.indexed_files = stats.indexed_files;
        self.metrics.skipped_files = stats.skipped_files;
        self.metrics.unchanged_files = stats.unchanged_files;
        self.metrics.total_chunks = stats.total_chunks;

        if stats.total_files > 0 {
            self.metrics.cache_hit_rate = stats.unchanged_files as f64 / stats.total_files as f64;
        }

        self.metrics.log_summary();
    }
}

impl Drop for UnifiedIndexer {
    fn drop(&mut self) {
        // TantivyAdapter handles cleanup in its Drop implementation
        tracing::debug!("UnifiedIndexer dropped");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmc_engine::chunker::ChunkId;
    use tempfile::TempDir;

    fn stored_chunk(name: &str, file: &str) -> CodeChunk {
        CodeChunk {
            id: ChunkId::new(),
            content: format!("pub fn {name}() {{ let value = {name}_body(); }}"),
            context: rmc_engine::chunker::ChunkContext {
                file_path: PathBuf::from(file),
                module_path: vec!["crate".to_string()],
                symbol_name: name.to_string(),
                symbol_kind: "function".to_string(),
                docstring: Some(format!("Docs of {name}")),
                imports: vec![],
                outgoing_calls: vec![],
                parent_symbol_name: None,
                split_part: None,
                split_total: None,
                line_start: 1,
                line_end: 3,
            },
        }
    }

    /// The rebuild oracle: an empty BM25 index next to a vector store with
    /// chunks comes back answering for exactly those chunks — every one
    /// found by its own symbol, under its own id — and a second pass, now
    /// that the index has documents, does nothing.
    #[tokio::test]
    async fn an_empty_bm25_index_is_rebuilt_from_the_vector_store() {
        let temp = TempDir::new().unwrap();
        let store = VectorStore::new_embedded(temp.path().join("vectors"), 4, "test:dim4")
            .await
            .unwrap();
        let chunks: Vec<CodeChunk> = (0..50)
            .map(|i| stored_chunk(&format!("symbol_{i}_unique"), &format!("src/f{}.rs", i % 7)))
            .collect();
        store
            .insert_chunks(
                chunks
                    .iter()
                    .map(|chunk| (chunk.id, vec![0.1, 0.2, 0.3, 0.4], chunk.clone()))
                    .collect(),
            )
            .await
            .unwrap();

        let tantivy_path = temp.path().join("bm25");
        let mut tantivy =
            TantivyAdapter::new(rmc_config::config::TantivyConfig::default(&tantivy_path)).unwrap();
        assert!(TantivyAdapter::is_empty_at(&tantivy_path).unwrap());

        let indexed = rebuild_bm25_if_empty(&mut tantivy, &store).await.unwrap();
        assert_eq!(indexed, Some(chunks.len()));
        assert_eq!(tantivy.committed_doc_count().unwrap(), chunks.len() as u64);
        assert!(!TantivyAdapter::is_empty_at(&tantivy_path).unwrap());
        assert_eq!(
            rebuild_bm25_if_empty(&mut tantivy, &store).await.unwrap(),
            None
        );

        let mut search = tantivy.create_bm25_search().unwrap();
        search.reload().unwrap();
        for chunk in &chunks {
            let hits = search.search(&chunk.context.symbol_name, 5).unwrap();
            assert_eq!(
                hits.first().map(|hit| hit.0),
                Some(chunk.id),
                "{} not found under its own id",
                chunk.context.symbol_name
            );
        }

        // And the hits resolve to their chunks through the store, which is
        // the only place the chunks now live.
        let ids: Vec<ChunkId> = chunks.iter().map(|chunk| chunk.id).collect();
        let fetched = store.chunks_by_ids(&ids).await.unwrap();
        assert_eq!(fetched.len(), chunks.len());
        for chunk in &chunks {
            assert_eq!(fetched[&chunk.id].content, chunk.content);
        }
    }

    #[test]
    fn disk_headroom_guard_is_inclusive_and_can_be_disabled() {
        let mib = 1024 * 1024;
        assert!(!has_disk_headroom(2047 * mib, 2048));
        assert!(has_disk_headroom(2048 * mib, 2048));
        assert!(has_disk_headroom(0, 0));
    }

    #[test]
    fn transient_file_errors_are_reported_for_retry() {
        let mut stats = IndexStats::default();
        let path = Path::new("/workspace/src/lib.rs");
        stats.record_error(path, &anyhow::anyhow!("No space left on device"));

        assert_eq!(stats.failed_files, 1);
        assert_eq!(stats.skipped_files, 0);
        assert_eq!(stats.retry_files, vec![path.to_path_buf()]);
    }

    #[tokio::test]
    #[ignore] // Requires embedding model
    async fn test_unified_indexer_creation() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join("cache");
        let tantivy_path = temp_dir.path().join("tantivy");
        let backend = EmbeddingBackend::default();
        let embedder_identity = backend.identity();

        let indexer = UnifiedIndexer::for_embedded(
            &cache_path,
            &tantivy_path,
            "test_collection",
            backend.dim(),
            &embedder_identity,
            None,
        )
        .await;

        assert!(
            indexer.is_ok(),
            "Failed to create UnifiedIndexer: {:?}",
            indexer.err()
        );
    }

    #[tokio::test]
    #[ignore] // Requires embedding model
    async fn test_index_file() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join("cache");
        let tantivy_path = temp_dir.path().join("tantivy");
        let backend = EmbeddingBackend::default();
        let embedder_identity = backend.identity();

        let mut indexer = UnifiedIndexer::for_embedded(
            &cache_path,
            &tantivy_path,
            "test_index_file",
            backend.dim(),
            &embedder_identity,
            None,
        )
        .await
        .unwrap();

        let test_file = temp_dir.path().join("test.rs");
        std::fs::write(
            &test_file,
            r#"
/// A test function
pub fn test_function() {
    println!("hello");
}
            "#,
        )
        .unwrap();

        let result = indexer.index_file(&test_file).await;
        assert!(result.is_ok(), "Failed to index file: {:?}", result.err());

        match result.unwrap() {
            IndexFileResult::Indexed { chunks_count } => {
                assert!(chunks_count > 0, "Should generate at least one chunk");
            }
            _ => panic!("Expected file to be indexed"),
        }

        let vector_store = indexer.vector_store_cloned();
        let count = vector_store.count().await.unwrap();
        assert!(count > 0, "Vector store should have at least one vector");
    }

    /// A file the metadata cache no longer knows (the cache was wiped, as
    /// the search path's corruption handling does) is indexed again — and
    /// must replace its chunks, not add a second copy under new random ids.
    /// The mutant (no delete before the write) doubles both counts.
    #[tokio::test]
    #[ignore] // Requires embedding model
    async fn reindexing_a_file_the_cache_forgot_replaces_its_chunks() {
        let temp_dir = TempDir::new().unwrap();
        // The CPU profile: runs on any machine that has the model files.
        let backend = EmbeddingBackend::from_profile_name("local-cpu-small").unwrap();
        let mut indexer = UnifiedIndexer::for_embedded_with_backend(
            &temp_dir.path().join("cache"),
            &temp_dir.path().join("tantivy"),
            "test_reindex_replaces",
            backend.dim(),
            &backend.identity(),
            None,
            backend.clone(),
        )
        .await
        .unwrap();
        let test_file = temp_dir.path().join("lib.rs");
        std::fs::write(&test_file, "pub fn alpha() {}\n\npub fn beta() {}\n").unwrap();

        indexer.index_file(&test_file).await.unwrap();
        indexer.commit().unwrap();
        let store = indexer.vector_store_cloned();
        let first = store.count().await.unwrap();
        assert!(first > 0);

        indexer.forget_file(&test_file).unwrap();
        indexer.index_file(&test_file).await.unwrap();
        indexer.commit().unwrap();

        assert_eq!(store.count().await.unwrap(), first);
        assert_eq!(indexer.tantivy.committed_doc_count().unwrap(), first as u64);
    }
}
