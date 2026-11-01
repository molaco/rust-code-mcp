//! Incremental indexing using Merkle tree change detection
//!
//! This module provides the core incremental indexing functionality that:
//! 1. Loads previous Merkle snapshot (if exists)
//! 2. Builds new Merkle tree from current filesystem
//! 3. Detects changes (added/modified/deleted files)
//! 4. Reindexes only changed files
//! 5. Saves new snapshot for next run
//!
//! This achieves 100-1000x speedup vs full reindexing for unchanged codebases.

use crate::indexing::identity::{
    active_chunking_identity_for_backend, identity_hash, indexing_identity,
};
use crate::indexing::merkle::{ChangeSet, FileSystemMerkle};
use crate::indexing::tantivy_adapter::TantivyAdapter;
use crate::indexing::traversal::collect_project_rust_files;
use crate::indexing::unified::{
    IndexStats, Preparation, PreparedFile, UnifiedIndexer, VectorMaintenance,
    ensure_index_disk_headroom, rebuild_bm25_from_existing_store,
};
use anyhow::Result;
use rmc_engine::embeddings::EmbeddingBackend;
use std::path::{Path, PathBuf};
use tracing;

/// Prepared rows a sync holds before writing them out.
///
/// A memory bound, not a tuning knob: an ordinary sync (tens of files, a few
/// thousand rows) is far below it and writes once. A branch switch that
/// changes thousands of files would otherwise hold every vector and chunk of
/// the tree at once; at roughly 4 KB a row (vector plus chunk JSON) this
/// caps the pending batch near 80 MB, and costs one more delete+append pair
/// per 20 000 rows.
const WRITE_BATCH_ROWS: usize = 20_000;

/// How a run keeps the workspace's readers off the stores while it writes.
///
/// # The defect this closes
///
/// A background sync used to hold its workspace's operation lock for write
/// for the whole run: the Merkle walk, the chunking and the embedding of every
/// changed file, and only then the writes. A `search` of the workspace waited
/// all of it. Measured on 2026-10-31 (16 changed files of a large workspace):
/// the run took 23 s — Merkle 1.6 s, preparing and embedding 21 s — and its
/// writes 0.4 s of that (store 0.1 s, BM25 commit 0.3 s). A reader needs to be
/// kept off only while the stores change, so that it never sees a file's old
/// chunks deleted and its new ones not yet appended, or vectors ahead of BM25.
#[derive(Clone, Copy)]
pub enum ReaderExclusion<'a> {
    /// The caller keeps readers out for the whole run (it holds the operation
    /// lock for write), or there are none (CLI, tests).
    HeldByCaller,
    /// The run takes this lock for write itself, only around what readers
    /// could see half done: from its first store write to the BM25 commit,
    /// and for the whole of a full pass. Preparation, embedding included, runs
    /// beside the readers.
    AroundPublish(&'a tokio::sync::RwLock<()>),
}

impl<'a> ReaderExclusion<'a> {
    /// Keep the readers out from now until the returned guard drops. `None`
    /// when the caller already does.
    pub async fn exclude(self) -> Option<tokio::sync::RwLockWriteGuard<'a, ()>> {
        match self {
            Self::HeldByCaller => None,
            Self::AroundPublish(lock) => Some(lock.write().await),
        }
    }
}

/// The readers' exclusion of a run that writes in steps: taken at the first
/// write, held to the end, so a reader sees all of the run or none of it.
struct Publishing<'a> {
    exclusion: ReaderExclusion<'a>,
    entered: bool,
    guard: Option<tokio::sync::RwLockWriteGuard<'a, ()>>,
}

impl<'a> Publishing<'a> {
    fn new(exclusion: ReaderExclusion<'a>) -> Self {
        Self {
            exclusion,
            entered: false,
            guard: None,
        }
    }

    /// Called before every write; waits for the readers inside only the first
    /// time.
    async fn before_write(&mut self) {
        if !self.entered {
            self.guard = self.exclusion.exclude().await;
            self.entered = true;
        }
    }
}

/// The writes a sync has prepared and not yet made.
#[derive(Default)]
struct PendingWrite {
    files: Vec<PreparedFile>,
    rows: usize,
    /// Files gone from the tree: their chunks and their cache entry go.
    gone: Vec<PathBuf>,
    /// Modified files the indexer no longer processes: their chunks go.
    cleared: Vec<PathBuf>,
}

impl PendingWrite {
    /// Write everything pending in one [`UnifiedIndexer::write_files`] and
    /// account for it. On failure every file of the write goes to the retry
    /// list: nothing of it was recorded, so the next tick redoes all of it.
    async fn flush(&mut self, indexer: &mut UnifiedIndexer, stats: &mut IndexStats) {
        let pending = std::mem::take(self);
        let written: Vec<(PathBuf, usize)> = pending
            .files
            .iter()
            .map(|file| (file.path.clone(), file.chunks.len()))
            .collect();
        let removed: Vec<PathBuf> = pending
            .gone
            .iter()
            .chain(&pending.cleared)
            .cloned()
            .collect();

        if let Err(error) = indexer.write_files(pending.files, &removed).await {
            tracing::warn!(
                "Failed to write {} file(s) and remove {} (will retry): {}",
                written.len(),
                removed.len(),
                error
            );
            for path in written.into_iter().map(|(path, _)| path).chain(removed) {
                stats.failed_files += 1;
                stats.retry_files.push(path);
            }
            return;
        }

        stats.skipped_files += pending.cleared.len();
        for (_, chunks) in written {
            if chunks == 0 {
                stats.skipped_files += 1;
            } else {
                stats.indexed_files += 1;
                stats.total_chunks += chunks;
            }
        }
        for path in pending.gone {
            // The metadata cache has to forget it too, or the entry outlives
            // the file forever: `monitoring::health` compares cache entries
            // against vectors in the store, so a ghost entry for a file that
            // no longer exists reads as "indexed but missing vectors" and
            // holds coverage at `degraded` permanently. Measured on the live
            // index of a large workspace (2026-08-25): 38 of 39 `stale_skips`
            // were renamed-away files, not lost vectors.
            if let Err(error) = indexer.forget_file(&path) {
                tracing::warn!(
                    "Failed to forget metadata for {} (will retry): {}",
                    path.display(),
                    error
                );
                stats.failed_files += 1;
                stats.retry_files.push(path);
                continue;
            }
            stats.skipped_files += 1;
        }
    }
}

/// Get snapshot storage path for a codebase with the default backend.
///
/// Uses consistent path with data_dir(): ~/.local/share/search/merkle/{hash}.snapshot
pub fn get_snapshot_path(codebase_path: &Path) -> PathBuf {
    get_snapshot_path_for_backend(codebase_path, &EmbeddingBackend::default())
}

/// Get snapshot storage path for a codebase/backend/chunking identity.
pub(crate) fn get_snapshot_path_for_backend(
    codebase_path: &Path,
    backend: &EmbeddingBackend,
) -> PathBuf {
    let chunking_identity = active_chunking_identity_for_backend(backend);
    let identity = indexing_identity(codebase_path, backend, &chunking_identity);
    get_snapshot_path_for_identity(&identity)
}

/// Get snapshot storage path for a precomputed indexing identity.
pub(super) fn get_snapshot_path_for_identity(indexing_identity: &str) -> PathBuf {
    use directories::ProjectDirs;

    // Use same ProjectDirs config as data_dir() for consistency
    let merkle_dir = if let Some(proj_dirs) = ProjectDirs::from("dev", "rust-code-mcp", "search") {
        proj_dirs.data_dir().join("merkle")
    } else {
        // Fallback to current directory if we can't find system directories
        PathBuf::from(".merkle")
    };
    std::fs::create_dir_all(&merkle_dir).ok();

    // Hash the full indexing identity to create a unique snapshot file.
    let path_hash = identity_hash(indexing_identity);

    merkle_dir.join(format!("{}.snapshot", &path_hash[..16]))
}

/// Incremental indexer that uses Merkle tree for change detection
pub struct IncrementalIndexer {
    config: IncrementalIndexerConfig,
    indexer: Option<UnifiedIndexer>,
}

#[derive(Clone)]
struct IncrementalIndexerConfig {
    cache_path: PathBuf,
    tantivy_path: PathBuf,
    collection_name: String,
    vector_size: usize,
    embedder_identity: String,
    codebase_loc: Option<usize>,
    backend: EmbeddingBackend,
}

impl IncrementalIndexer {
    /// Create a new incremental indexer configured for an embedded LanceDB backend.
    ///
    /// `embedder_identity` is the stable string returned by the active
    /// `EmbeddingBackend::identity()`; it is written to the vector
    /// store's `metadata.json` on first index and reconciled on reopen.
    pub async fn new(
        cache_path: &Path,
        tantivy_path: &Path,
        collection_name: &str,
        vector_size: usize,
        embedder_identity: &str,
        codebase_loc: Option<usize>,
    ) -> Result<Self> {
        Self::with_backend(
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

    /// Create a new incremental indexer with an explicit embedding
    /// backend.
    ///
    /// Stored until `UnifiedIndexer::for_embedded_with_backend` is needed so
    /// no-change Merkle checks can return without opening the vector store
    /// or constructing the embedding generator.
    pub async fn with_backend(
        cache_path: &Path,
        tantivy_path: &Path,
        collection_name: &str,
        vector_size: usize,
        embedder_identity: &str,
        codebase_loc: Option<usize>,
        backend: EmbeddingBackend,
    ) -> Result<Self> {
        Ok(Self {
            config: IncrementalIndexerConfig {
                cache_path: cache_path.to_path_buf(),
                tantivy_path: tantivy_path.to_path_buf(),
                collection_name: collection_name.to_string(),
                vector_size,
                embedder_identity: embedder_identity.to_string(),
                codebase_loc,
                backend,
            },
            indexer: None,
        })
    }

    async fn ensure_indexer(&mut self) -> Result<&mut UnifiedIndexer> {
        if self.indexer.is_none() {
            let config = self.config.clone();
            // Measured because it is the suspect: every changed background
            // sync builds a fresh indexer, and with it a fresh ONNX session.
            let mem = rmc_engine::mem_trace::MemSpan::start(format!(
                "indexer init for {}",
                config.collection_name
            ));
            let indexer = UnifiedIndexer::for_embedded_with_backend(
                &config.cache_path,
                &config.tantivy_path,
                &config.collection_name,
                config.vector_size,
                &config.embedder_identity,
                config.codebase_loc,
                config.backend,
            )
            .await?;
            mem.finish();
            self.indexer = Some(indexer);
        }

        Ok(self.indexer.as_mut().expect("indexer initialized above"))
    }

    /// Index codebase with automatic change detection
    ///
    /// This is the main entry point that matches claude-context's reindexByChange() behavior:
    /// 1. Load previous Merkle snapshot (if exists)
    /// 2. Build new Merkle tree from current filesystem
    /// 3. Detect changes using tree comparison
    /// 4. If no changes: return immediately (< 10ms)
    /// 5. If changes: reindex only changed files
    /// 6. Save new snapshot for next time
    pub async fn index_with_change_detection(
        &mut self,
        codebase_path: &Path,
    ) -> Result<IndexStats> {
        self.index_with_change_detection_excluding(codebase_path, ReaderExclusion::HeldByCaller)
            .await
    }

    /// [`Self::index_with_change_detection`], keeping the readers out as
    /// `exclusion` says: with [`ReaderExclusion::AroundPublish`] the walk and
    /// the preparation of changed files run beside them.
    pub async fn index_with_change_detection_excluding(
        &mut self,
        codebase_path: &Path,
        exclusion: ReaderExclusion<'_>,
    ) -> Result<IndexStats> {
        tracing::info!(
            "Starting incremental indexing for {}",
            codebase_path.display()
        );
        ensure_index_disk_headroom(&self.config.cache_path)?;

        let snapshot_path = get_snapshot_path_for_backend(codebase_path, &self.config.backend);
        tracing::debug!("Snapshot path: {}", snapshot_path.display());

        // Step 1: Load previous snapshot (if exists)
        let old_merkle = match FileSystemMerkle::load_snapshot(&snapshot_path)? {
            Some(merkle) => {
                tracing::info!(
                    "Loaded previous snapshot: {} files, v{}",
                    merkle.file_count(),
                    merkle.version()
                );
                Some(merkle)
            }
            None => {
                tracing::info!("No previous snapshot found - first time indexing");
                None
            }
        };

        // Step 2: Build new Merkle tree from current filesystem
        tracing::info!("Building Merkle tree from current filesystem...");
        let new_merkle = FileSystemMerkle::from_directory(codebase_path)?;
        tracing::info!("Built Merkle tree with {} files", new_merkle.file_count());

        // An empty BM25 index under an existing snapshot is a new BM25 schema
        // version (its own directory) or a rebuild that died before its
        // commit. A `UnifiedIndexer` refills it from the vector store when it
        // comes up, but a tick without file changes never creates one, and
        // the index would stay empty until some file changed — so it is
        // refilled here, without the indexer and its embedding session.
        // Readers are not kept out for it: an empty BM25 index reads as
        // "vectors only" and the refill appears whole at its commit — the same
        // refill `search` itself runs under its read lock.
        if old_merkle.is_some()
            && self.indexer.is_none()
            && TantivyAdapter::is_empty_at(&self.config.tantivy_path)?
        {
            rebuild_bm25_from_existing_store(
                &self.config.cache_path,
                &self.config.tantivy_path,
                &self.config.collection_name,
                self.config.vector_size,
                &self.config.embedder_identity,
                self.config.codebase_loc,
            )
            .await?;
        }

        // Step 3: Determine indexing strategy
        let stats = if let Some(old) = old_merkle.as_ref() {
            // Incremental: compare trees and index only changes
            self.incremental_update(codebase_path, &old, &new_merkle, exclusion)
                .await?
        } else {
            // First time: full index with parallel processing. It writes as it
            // goes, so the readers stay out for all of it.
            tracing::info!("Performing full index (first time) with parallel processing");
            let _readers_out = exclusion.exclude().await;
            self.ensure_indexer()
                .await?
                .index_directory_parallel(codebase_path)
                .await?
        };

        // Step 4: commit only the paths whose index writes completed. Failed
        // additions stay absent and failed modifications/deletions retain the
        // old hash, so the next background tick retries them automatically.
        let committed_merkle =
            new_merkle.snapshot_with_retries(old_merkle.as_ref(), &stats.retry_files);
        committed_merkle.save_snapshot(&snapshot_path)?;
        tracing::info!(
            "Saved Merkle snapshot to {} ({} retryable file failure(s) retained)",
            snapshot_path.display(),
            stats.failed_files,
        );

        // Vector-store maintenance is not run here, nor anywhere in
        // indexing: both branches above set `stats.needs_maintenance` at
        // their own ends (a first-time index through
        // `index_directory_parallel`, an update file by file), and the
        // caller runs the pass after releasing its workspace lock.

        Ok(stats)
    }

    /// Perform incremental update based on Merkle tree comparison
    async fn incremental_update(
        &mut self,
        codebase_path: &Path,
        old_merkle: &FileSystemMerkle,
        new_merkle: &FileSystemMerkle,
        exclusion: ReaderExclusion<'_>,
    ) -> Result<IndexStats> {
        // Fast path: if trees identical, no work needed!
        if !new_merkle.has_changes(old_merkle) {
            tracing::info!("✓ Merkle roots match - no changes detected (< 10ms check)");
            let mut stats = IndexStats::unchanged();
            stats.unchanged_files = new_merkle.file_count();
            stats.total_files = new_merkle.file_count();
            return Ok(stats);
        }

        tracing::info!("Merkle roots differ - detecting specific changes...");

        // Detect specific file changes
        let changes = new_merkle.detect_changes(old_merkle);

        if changes.is_empty() {
            tracing::info!("No file-level changes detected");
            let mut stats = IndexStats::unchanged();
            stats.unchanged_files = new_merkle.file_count();
            stats.total_files = new_merkle.file_count();
            return Ok(stats);
        }

        tracing::info!(
            "Detected changes: {} added, {} modified, {} deleted",
            changes.added.len(),
            changes.modified.len(),
            changes.deleted.len()
        );

        // Process changes
        let mut stats = self
            .process_changes(codebase_path, changes, exclusion)
            .await?;
        stats.total_files = new_merkle.file_count();
        Ok(stats)
    }

    /// Process detected changes: add, modify, delete
    async fn process_changes(
        &mut self,
        codebase_path: &Path,
        changes: ChangeSet,
        exclusion: ReaderExclusion<'_>,
    ) -> Result<IndexStats> {
        let mut stats = IndexStats::default();
        let storage_path = self.config.cache_path.clone();
        // Captured before `changes` is consumed below: deletions are writes
        // too, and the maintenance gate at the end needs to know about them.
        let deleted_count = changes.deleted.len();
        let indexer = self.ensure_indexer().await?;
        let mem = rmc_engine::mem_trace::MemSpan::start(format!(
            "index changes of {} ({} added, {} modified, {} deleted)",
            codebase_path.display(),
            changes.added.len(),
            changes.modified.len(),
            deleted_count
        ));

        // Prepare every changed file first, then write them together: one
        // vector-store delete and one append per sync instead of two writes
        // per file (see `UnifiedIndexer::write_files`). Removed files ride
        // along in the same delete.
        ensure_index_disk_headroom(&storage_path)?;
        let mut pending = PendingWrite {
            gone: changes.deleted.clone(),
            ..PendingWrite::default()
        };
        for deleted_path in &changes.deleted {
            tracing::info!(
                "Removing chunks of deleted file: {}",
                deleted_path.display()
            );
        }

        // Readers stay in while files are prepared (chunked and embedded —
        // nearly all of a run's time) and are kept out from the first write to
        // the BM25 commit; see `ReaderExclusion`.
        let mut publishing = Publishing::new(exclusion);

        // Modified files are prepared without the metadata cache: the Merkle
        // tree proved they changed, and a cache already describing the new
        // bytes (a run that wrote them but died before its snapshot) must not
        // turn the rewrite into a skip. Added files consult it.
        let to_prepare = changes
            .modified
            .iter()
            .map(|path| (path, false))
            .chain(changes.added.iter().map(|path| (path, true)));
        for (path, consult_metadata_cache) in to_prepare {
            tracing::info!("Indexing changed file: {}", path.display());
            match indexer.prepare_file(path, consult_metadata_cache).await {
                Ok(Preparation::Ready(file)) => {
                    pending.rows += file.chunks.len();
                    pending.files.push(file);
                }
                // A modified file this indexer no longer processes keeps no
                // chunks; an added one never had any to drop.
                Ok(Preparation::Skipped) if !consult_metadata_cache => {
                    pending.cleared.push(path.clone());
                }
                Ok(Preparation::Skipped | Preparation::Unchanged) => stats.skipped_files += 1,
                // Its old chunks stay until the retry: stale results beat none.
                Err(e) => stats.record_error(path, &e),
            }
            if pending.rows >= WRITE_BATCH_ROWS {
                publishing.before_write().await;
                pending.flush(indexer, &mut stats).await;
            }
        }
        publishing.before_write().await;
        pending.flush(indexer, &mut stats).await;

        // Commit changes to Tantivy
        indexer.commit()?;
        // Readers come back in: both stores now show the whole run.
        drop(publishing);
        mem.finish();

        tracing::info!(
            "✓ Incremental update complete: {} files indexed, {} chunks, {} retryable failures",
            stats.indexed_files,
            stats.total_chunks,
            stats.failed_files,
        );

        // Ask the caller to reclaim what those writes left behind (it runs
        // the pass after releasing the workspace lock; see
        // `IndexStats::needs_maintenance`). Deletions count as writes here:
        // they publish a version each without adding a fragment, and a tick
        // that only removed files would otherwise pile up versions no pass
        // ever collects. A tick that found nothing asks for nothing — there
        // is nothing to compact and a pass would be pure I/O.
        if stats.indexed_files > 0 || deleted_count > 0 {
            stats.needs_maintenance = true;
        }

        Ok(stats)
    }

    /// Get access to underlying indexer for other operations
    pub fn indexer(&self) -> Result<&UnifiedIndexer> {
        self.indexer
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("UnifiedIndexer has not been initialized yet"))
    }

    /// Get mutable access to underlying indexer, initializing it if needed.
    pub async fn indexer_mut(&mut self) -> Result<&mut UnifiedIndexer> {
        self.ensure_indexer().await
    }

    /// Clear all indexed data (metadata cache, Tantivy, and vector store)
    ///
    /// This is used for force reindexing to ensure a completely clean slate.
    /// Note: This does NOT delete the Merkle snapshot - that should be handled separately
    /// by the caller (e.g., in index_tool.rs) before calling index_with_change_detection.
    pub async fn clear_all_data(&mut self) -> Result<()> {
        self.ensure_indexer().await?.clear_all_data().await
    }

    /// Forget cache entries whose file has no vectors; see
    /// [`UnifiedIndexer::forget_files_without_vectors`].
    pub async fn forget_files_without_vectors(&mut self) -> Result<Vec<String>> {
        self.ensure_indexer()
            .await?
            .forget_files_without_vectors()
            .await
    }

    /// Drop the files the stores hold but the tree under `codebase_path` no
    /// longer has; see [`UnifiedIndexer::drop_files_outside`]. A walk with
    /// errors drops nothing — it may have missed live files. A non-empty
    /// result wrote to the vector store and so needs a maintenance pass,
    /// which the caller runs (see `IndexStats::needs_maintenance`).
    pub async fn drop_files_outside_tree(&mut self, codebase_path: &Path) -> Result<Vec<String>> {
        let (tree, walk_errors) = collect_project_rust_files(codebase_path);
        if walk_errors > 0 {
            tracing::warn!(
                "Not dropping files outside {}: the walk hit {} error(s) and may be incomplete",
                codebase_path.display(),
                walk_errors
            );
            return Ok(Vec::new());
        }
        let indexer = self.ensure_indexer().await?;
        indexer.drop_files_outside(&tree).await
    }

    /// The maintenance pass over this indexer's vector store, if the indexer
    /// was ever opened (a run that found nothing never opens it, and then
    /// wrote nothing to maintain).
    pub fn vector_maintenance(&self) -> Option<VectorMaintenance> {
        self.indexer.as_ref().map(UnifiedIndexer::vector_maintenance)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn snapshot_path_changes_by_backend_identity() {
        let codebase_path = Path::new("/tmp/rust-code-mcp-snapshot-test");
        let default_backend = EmbeddingBackend::default();
        let mut alternate_backend = EmbeddingBackend::default();
        alternate_backend.max_len = 2048;

        assert_ne!(
            get_snapshot_path_for_backend(codebase_path, &default_backend),
            get_snapshot_path_for_backend(codebase_path, &alternate_backend)
        );
    }

    #[test]
    fn snapshot_path_changes_by_chunking_identity() {
        let codebase_path = Path::new("/tmp/rust-code-mcp-snapshot-test");
        let backend = EmbeddingBackend::default();
        let first = indexing_identity(codebase_path, &backend, "chunk-split:v1:target768:hard1024");
        let second = indexing_identity(codebase_path, &backend, "chunk-split:v1:target512:hard768");

        assert_ne!(
            get_snapshot_path_for_identity(&first),
            get_snapshot_path_for_identity(&second)
        );
    }

    #[tokio::test]
    async fn no_changes_detection_skips_unified_indexer_initialization() {
        let temp_dir = TempDir::new().unwrap();
        let test_codebase = temp_dir.path().join("codebase");
        std::fs::create_dir(&test_codebase).unwrap();
        std::fs::write(
            test_codebase.join("test.rs"),
            "fn main() { println!(\"hello\"); }",
        )
        .unwrap();

        let backend = EmbeddingBackend::default();
        let snapshot_path = get_snapshot_path_for_backend(&test_codebase, &backend);
        let _ = std::fs::remove_file(&snapshot_path);
        FileSystemMerkle::from_directory(&test_codebase)
            .unwrap()
            .save_snapshot(&snapshot_path)
            .unwrap();

        let blocked_parent = temp_dir.path().join("not_a_directory");
        std::fs::write(&blocked_parent, "file, not directory").unwrap();
        let cache_path = blocked_parent.join("cache");
        let tantivy_path = blocked_parent.join("tantivy");

        let mut indexer = IncrementalIndexer::with_backend(
            &cache_path,
            &tantivy_path,
            "no_change_lazy_init",
            0,
            "invalid-identity-used-only-if-vector-store-initializes",
            None,
            backend,
        )
        .await
        .unwrap();

        let stats = indexer
            .index_with_change_detection(&test_codebase)
            .await
            .unwrap();

        assert_eq!(stats.total_files, 1);
        assert_eq!(stats.unchanged_files, 1);
        assert_eq!(stats.indexed_files, 0);
        assert!(indexer.indexer().is_err());

        let _ = std::fs::remove_file(snapshot_path);
    }

    /// The write shape of one background sync: three files modified, one
    /// added, one deleted. All of it lands as ONE append — one new fragment;
    /// the per-file writes this replaced made four (one per written file).
    /// Then the stores agree with the tree (no second copy of a rewritten
    /// file, nothing left of the deleted one, BM25 and vectors the same
    /// size), and maintenance left every index covering every row.
    #[tokio::test]
    #[ignore] // Requires the CPU embedding model files
    async fn a_sync_writes_its_files_as_one_append_and_keeps_the_indices_covering() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join("cache");
        let tantivy_path = temp_dir.path().join("tantivy");
        let codebase = temp_dir.path().join("codebase");
        std::fs::create_dir(&codebase).unwrap();
        let write = |name: &str, generation: usize| {
            std::fs::write(
                codebase.join(name),
                format!(
                    "pub fn {}_a() {{ {generation} }}\n\npub fn {}_b() {{}}\n",
                    &name[..2],
                    &name[..2]
                ),
            )
            .unwrap();
        };
        for name in ["f0.rs", "f1.rs", "f2.rs", "f3.rs", "f4.rs"] {
            write(name, 0);
        }

        let backend = EmbeddingBackend::from_profile_name("local-cpu-small").unwrap();
        let snapshot_path = get_snapshot_path_for_backend(&codebase, &backend);
        let mut indexer = IncrementalIndexer::with_backend(
            &cache_path,
            &tantivy_path,
            "sync_write_shape",
            backend.dim(),
            &backend.identity(),
            None,
            backend,
        )
        .await
        .unwrap();
        indexer
            .index_with_change_detection(&codebase)
            .await
            .unwrap();
        indexer.vector_maintenance().unwrap().run(&codebase).await;
        let store = indexer.indexer().unwrap().vector_store_cloned();
        let before = store.layout().await.unwrap();

        for name in ["f0.rs", "f1.rs", "f2.rs"] {
            write(name, 1);
        }
        write("f5.rs", 0);
        std::fs::remove_file(codebase.join("f4.rs")).unwrap();
        let stats = indexer
            .index_with_change_detection(&codebase)
            .await
            .unwrap();
        assert_eq!(
            (stats.indexed_files, stats.failed_files),
            (4, 0),
            "{stats:?}"
        );
        // The caller's half of the contract, as the sync runs it.
        assert!(stats.needs_maintenance, "{stats:?}");
        indexer.vector_maintenance().unwrap().run(&codebase).await;

        let after = store.layout().await.unwrap();
        assert_eq!(
            after.fragments,
            before.fragments + 1,
            "{before:#?}\n{after:#?}"
        );
        let files: std::collections::BTreeSet<String> = store
            .indexed_file_paths()
            .await
            .unwrap()
            .into_iter()
            .collect();
        let expected: std::collections::BTreeSet<String> =
            ["f0.rs", "f1.rs", "f2.rs", "f3.rs", "f5.rs"]
                .iter()
                .map(|name| codebase.join(name).to_string_lossy().to_string())
                .collect();
        assert_eq!(files, expected);
        // Same chunks per file as the first pass: five files where it had five.
        assert_eq!(after.rows, before.rows, "{before:#?}\n{after:#?}");
        let bm25 = crate::indexing::index_stats::probe_bm25_index(&tantivy_path).unwrap();
        assert_eq!(bm25.live_docs, after.rows as u64);
        assert!(!after.indices.is_empty());
        for index in &after.indices {
            assert_eq!(index.unindexed_rows, 0, "{index:#?}");
        }

        let _ = std::fs::remove_file(snapshot_path);
    }

    /// A sync prepares its changed files beside the workspace's readers and
    /// keeps them out only while it writes (`ReaderExclusion::AroundPublish`).
    ///
    /// A reader holds the lock for the whole of the run's preparation; the
    /// run must still read and embed the changed file, then queue for the
    /// write and wait there — not finish — until the reader leaves. The file
    /// is rewritten once the run is queued: the store must hold what the run
    /// read before it queued (`f0_second`), never the rewrite. Mutant
    /// (checked 2026-10-31): taking the exclusion before the preparation loop
    /// → the run queues at once and reads the file only after the reader
    /// leaves, so it never indexes `f0_second` (red on the stats assertion).
    #[tokio::test]
    #[ignore] // Requires the CPU embedding model files
    async fn a_sync_prepares_beside_the_readers_and_writes_without_them() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::{Duration, Instant};

        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join("cache");
        let tantivy_path = temp_dir.path().join("tantivy");
        let codebase = temp_dir.path().join("codebase");
        std::fs::create_dir(&codebase).unwrap();
        let write_f0 = |name: &str| {
            std::fs::write(codebase.join("f0.rs"), format!("pub fn {name}() {{}}\n")).unwrap();
        };
        write_f0("f0_first");
        std::fs::write(codebase.join("f1.rs"), "pub fn f1_only() {}\n").unwrap();

        let backend = EmbeddingBackend::from_profile_name("local-cpu-small").unwrap();
        let snapshot_path = get_snapshot_path_for_backend(&codebase, &backend);
        let _ = std::fs::remove_file(&snapshot_path);
        let mut indexer = IncrementalIndexer::with_backend(
            &cache_path,
            &tantivy_path,
            "sync_beside_readers",
            backend.dim(),
            &backend.identity(),
            None,
            backend,
        )
        .await
        .unwrap();
        indexer
            .index_with_change_detection(&codebase)
            .await
            .unwrap();

        write_f0("f0_second");
        let operation = tokio::sync::RwLock::new(());
        let reader = operation.read().await;
        let finished = AtomicBool::new(false);
        let run = async {
            let stats = indexer
                .index_with_change_detection_excluding(
                    &codebase,
                    ReaderExclusion::AroundPublish(&operation),
                )
                .await
                .unwrap();
            finished.store(true, Ordering::SeqCst);
            stats
        };
        let driver = async {
            // A writer queued on tokio's fair lock makes `try_read` fail while
            // our reader is still inside.
            let deadline = Instant::now() + Duration::from_secs(120);
            while operation.try_read().is_ok() {
                assert!(
                    !finished.load(Ordering::SeqCst),
                    "the run finished without waiting for the reader inside"
                );
                assert!(Instant::now() < deadline, "the run never queued to write");
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            write_f0("f0_third");
            tokio::time::sleep(Duration::from_millis(50)).await;
            assert!(
                !finished.load(Ordering::SeqCst),
                "the run wrote while a reader was inside"
            );
            drop(reader);
        };
        let (stats, ()) = tokio::join!(run, driver);
        assert_eq!((stats.indexed_files, stats.failed_files), (1, 0), "{stats:?}");

        let store = indexer.indexer().unwrap().vector_store_cloned();
        let f0 = codebase.join("f0.rs");
        let mut f0_contents = Vec::new();
        store
            .scan_chunks(&mut |batch| {
                f0_contents.extend(
                    batch
                        .into_iter()
                        .filter(|chunk| chunk.context.file_path == f0)
                        .map(|chunk| chunk.content),
                );
                Ok(())
            })
            .await
            .unwrap();
        let joined = f0_contents.join("\n");
        assert!(joined.contains("f0_second"), "{f0_contents:?}");
        assert!(!joined.contains("f0_third"), "{f0_contents:?}");
        assert!(!joined.contains("f0_first"), "{f0_contents:?}");

        let _ = std::fs::remove_file(snapshot_path);
    }

    /// A file deleted while the snapshot was gone has no change detection
    /// to see it go: the next pass is a full one, and only a full pass (or a
    /// coverage repair) can drop its chunks. Without `drop_files_outside`
    /// (the mutant) f2.rs stays in both stores for good — 41 such files sat
    /// in a live store. The repair also forgets the cache entry: a file that
    /// comes back with the same bytes is embedded again, not waved through.
    #[tokio::test]
    #[ignore] // Requires the CPU embedding model files
    async fn full_passes_and_repairs_drop_files_that_left_the_tree() {
        let temp_dir = TempDir::new().unwrap();
        let codebase = temp_dir.path().join("codebase");
        std::fs::create_dir(&codebase).unwrap();
        for name in ["f0.rs", "f1.rs", "f2.rs"] {
            std::fs::write(
                codebase.join(name),
                format!("pub fn {}() {{}}\n", &name[..2]),
            )
            .unwrap();
        }
        let path_of = |name: &str| codebase.join(name).to_string_lossy().to_string();
        let backend = EmbeddingBackend::from_profile_name("local-cpu-small").unwrap();
        let snapshot_path = get_snapshot_path_for_backend(&codebase, &backend);
        let mut indexer = IncrementalIndexer::with_backend(
            &temp_dir.path().join("cache"),
            &temp_dir.path().join("tantivy"),
            "drop_outside_tree",
            backend.dim(),
            &backend.identity(),
            None,
            backend,
        )
        .await
        .unwrap();
        indexer
            .index_with_change_detection(&codebase)
            .await
            .unwrap();
        let store = indexer.indexer().unwrap().vector_store_cloned();
        let files = || async {
            let mut files: Vec<String> = store
                .indexed_file_paths()
                .await
                .unwrap()
                .into_iter()
                .collect();
            files.sort();
            files
        };
        assert_eq!(
            files().await,
            [path_of("f0.rs"), path_of("f1.rs"), path_of("f2.rs")]
        );
        let rows_per_file = store.count().await.unwrap() / 3;

        // f2.rs goes while there is no snapshot; the next pass is a full one.
        std::fs::remove_file(&snapshot_path).unwrap();
        std::fs::remove_file(codebase.join("f2.rs")).unwrap();
        indexer
            .index_with_change_detection(&codebase)
            .await
            .unwrap();
        assert_eq!(files().await, [path_of("f0.rs"), path_of("f1.rs")]);
        let bm25 = || {
            crate::indexing::index_stats::probe_bm25_index(&temp_dir.path().join("tantivy"))
                .unwrap()
                .live_docs
        };
        assert_eq!(bm25(), store.count().await.unwrap() as u64);

        // The repair drops what the snapshot cannot see either: f1.rs moved
        // out of the tree and back is the shape of a file removed by hand.
        let f1 = std::fs::read(codebase.join("f1.rs")).unwrap();
        std::fs::remove_file(codebase.join("f1.rs")).unwrap();
        assert_eq!(
            indexer.drop_files_outside_tree(&codebase).await.unwrap(),
            [path_of("f1.rs")]
        );
        assert_eq!(files().await, [path_of("f0.rs")]);
        assert_eq!(bm25(), store.count().await.unwrap() as u64);

        std::fs::write(codebase.join("f1.rs"), f1).unwrap();
        std::fs::remove_file(&snapshot_path).unwrap();
        indexer
            .index_with_change_detection(&codebase)
            .await
            .unwrap();
        assert_eq!(files().await, [path_of("f0.rs"), path_of("f1.rs")]);
        assert_eq!(store.count().await.unwrap(), 2 * rows_per_file);

        let _ = std::fs::remove_file(snapshot_path);
    }

    #[tokio::test]
    #[ignore] // Requires embedding model
    async fn test_first_time_indexing() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join("cache");
        let tantivy_path = temp_dir.path().join("tantivy");
        let test_codebase = temp_dir.path().join("codebase");

        std::fs::create_dir(&test_codebase).unwrap();
        std::fs::write(
            test_codebase.join("test.rs"),
            "fn main() { println!(\"hello\"); }",
        )
        .unwrap();

        let mut indexer = IncrementalIndexer::new(
            &cache_path,
            &tantivy_path,
            "test_incremental",
            1024, // Qwen3-Embedding-0.6B
            "test-embedder:v1",
            None,
        )
        .await
        .unwrap();

        let stats = indexer
            .index_with_change_detection(&test_codebase)
            .await
            .unwrap();

        assert!(stats.indexed_files > 0);
    }

    #[tokio::test]
    #[ignore] // Requires embedding model
    async fn test_no_changes_detection() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join("cache");
        let tantivy_path = temp_dir.path().join("tantivy");
        let test_codebase = temp_dir.path().join("codebase");

        std::fs::create_dir(&test_codebase).unwrap();
        std::fs::write(
            test_codebase.join("test.rs"),
            "fn main() { println!(\"hello\"); }",
        )
        .unwrap();

        let mut indexer = IncrementalIndexer::new(
            &cache_path,
            &tantivy_path,
            "test_no_changes",
            1024, // Qwen3-Embedding-0.6B
            "test-embedder:v1",
            None,
        )
        .await
        .unwrap();

        // First index
        indexer
            .index_with_change_detection(&test_codebase)
            .await
            .unwrap();

        // Second index - should detect no changes
        let stats = indexer
            .index_with_change_detection(&test_codebase)
            .await
            .unwrap();

        assert_eq!(stats.indexed_files, 0);
    }

    #[tokio::test]
    #[ignore] // Requires embedding model
    async fn test_incremental_update() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join("cache");
        let tantivy_path = temp_dir.path().join("tantivy");
        let test_codebase = temp_dir.path().join("codebase");

        std::fs::create_dir(&test_codebase).unwrap();
        std::fs::write(
            test_codebase.join("test1.rs"),
            "fn main() { println!(\"hello\"); }",
        )
        .unwrap();

        let mut indexer = IncrementalIndexer::new(
            &cache_path,
            &tantivy_path,
            "test_incremental_update",
            1024, // Qwen3-Embedding-0.6B
            "test-embedder:v1",
            None,
        )
        .await
        .unwrap();

        // First index
        indexer
            .index_with_change_detection(&test_codebase)
            .await
            .unwrap();

        // Add a new file
        std::fs::write(
            test_codebase.join("test2.rs"),
            "fn helper() { println!(\"world\"); }",
        )
        .unwrap();

        // Second index - should only index new file
        let stats = indexer
            .index_with_change_detection(&test_codebase)
            .await
            .unwrap();

        assert_eq!(stats.indexed_files, 1); // Only new file
    }
}
