//! Background sync manager for automatic incremental reindexing
//!
//! Matches claude-context's SyncManager behavior:
//! - Runs continuously in background
//! - Syncs every 5 minutes (configurable)
//! - Uses the indexing incremental service for fast change detection
//! - Tracks multiple directories independently

use rmc_indexing::indexing::{
    index_project_incrementally, IncrementalIndexRequest, ReaderExclusion, VectorMaintenance,
};
use crate::mcp::workspace_locks::run_outside_operation_lock;
use anyhow::Result;
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{watch, RwLock};
use tracing;

use rmc_engine::embeddings::EmbeddingBackend;

use super::defaults::{
    automatic_embedding_backend, automatic_embedding_profile_name, is_background_embedding_backend,
};
use super::workspace_locks::WorkspaceLockRegistry;

fn normalize_directory(dir: &Path) -> PathBuf {
    std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf())
}

/// Whether a discovered index belongs to the profile background sync maintains.
///
/// Compared by profile name rather than by identity string: a legacy identity
/// decodes into the same built-in profile, and matching on the raw string
/// would drop such an index out of background sync for no real reason.
fn is_automatic_profile_backend(
    backend: &EmbeddingBackend,
    automatic: &EmbeddingBackend,
) -> bool {
    backend.profile.name() == automatic.profile.name()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SyncManagerStatus {
    pub tracked_count: usize,
    pub tracked_directories: Vec<String>,
}

/// Manages background synchronization of indexed codebases
pub struct SyncManager {
    /// List of directories being tracked
    tracked_dirs: Arc<RwLock<HashSet<PathBuf>>>,
    /// Sync interval (default: 5 minutes)
    interval: Duration,
    /// Per-workspace operation locks shared with tool endpoints
    workspace_locks: WorkspaceLockRegistry,
}

impl SyncManager {
    /// Create a new sync manager
    ///
    /// # Arguments
    /// * `interval_secs` - Sync interval in seconds (default: 300 = 5 minutes)
    pub fn new(interval_secs: u64) -> Self {
        Self {
            tracked_dirs: Arc::new(RwLock::new(HashSet::new())),
            interval: Duration::from_secs(interval_secs),
            workspace_locks: WorkspaceLockRegistry::new(),
        }
    }

    /// Create a new sync manager with default paths
    ///
    /// Uses XDG-compliant directories or falls back to current directory
    pub fn with_defaults(interval_secs: u64) -> Self {
        Self {
            tracked_dirs: Arc::new(RwLock::new(HashSet::new())),
            interval: Duration::from_secs(interval_secs),
            workspace_locks: WorkspaceLockRegistry::new(),
        }
    }

    /// Get the workspace lock registry shared with this sync manager.
    pub fn workspace_locks(&self) -> WorkspaceLockRegistry {
        self.workspace_locks.clone()
    }

    /// Add a directory to track
    ///
    /// The directory will be automatically synced on the next sync cycle
    pub async fn track_directory(&self, dir: PathBuf) {
        let dir = normalize_directory(&dir);
        let mut dirs = self.tracked_dirs.write().await;
        if dirs.insert(dir.clone()) {
            tracing::info!("Now tracking directory for sync: {}", dir.display());
        }
    }

    /// Add a directory to background sync only when the profile is safe for
    /// automatic work.
    pub async fn track_directory_for_backend(
        &self,
        dir: PathBuf,
        backend: &EmbeddingBackend,
    ) -> bool {
        if !is_background_embedding_backend(backend) {
            tracing::info!(
                "Not tracking {} for background sync because profile {} runs a local Qwen3/CUDA model; explicit foreground commands remain supported",
                dir.display(),
                backend.profile.name(),
            );
            return false;
        }

        self.track_directory(dir).await;
        true
    }

    /// Remove a directory from tracking
    pub async fn untrack_directory(&self, dir: &Path) -> bool {
        let dir = normalize_directory(dir);
        let mut dirs = self.tracked_dirs.write().await;
        if dirs.remove(&dir) {
            tracing::info!("Stopped tracking directory: {}", dir.display());
            true
        } else {
            false
        }
    }

    /// Remove all directories from tracking
    pub async fn untrack_all_directories(&self) -> usize {
        let mut dirs = self.tracked_dirs.write().await;
        let count = dirs.len();
        dirs.clear();
        if count > 0 {
            tracing::info!("Stopped tracking all directories: {}", count);
        }
        count
    }

    /// Get list of tracked directories
    pub async fn get_tracked_directories(&self) -> Vec<PathBuf> {
        let dirs = self.tracked_dirs.read().await;
        let mut dirs = dirs.iter().cloned().collect::<Vec<_>>();
        dirs.sort();
        dirs
    }

    pub async fn tracked_count(&self) -> usize {
        self.tracked_dirs.read().await.len()
    }

    pub async fn status(&self) -> SyncManagerStatus {
        let tracked_directories = self
            .get_tracked_directories()
            .await
            .into_iter()
            .map(|dir| dir.display().to_string())
            .collect::<Vec<_>>();
        SyncManagerStatus {
            tracked_count: tracked_directories.len(),
            tracked_directories,
        }
    }

    /// Run background sync loop
    ///
    /// This runs forever, checking for changes every `interval` duration.
    /// Designed to be spawned as a background task.
    ///
    /// # Example
    /// ```rust,ignore
    /// use rmc_server::mcp::SyncManager;
    /// use std::sync::Arc;
    /// let sync_manager = Arc::new(SyncManager::with_defaults(300));
    /// tokio::spawn(async move {
    ///     sync_manager.run().await;
    /// });
    /// ```
    pub async fn run(self: Arc<Self>) {
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);
        self.run_until_shutdown(shutdown_rx).await;
    }

    /// Run background sync loop until shutdown is requested.
    ///
    /// Cancellation is checked before the initial delayed sync and between
    /// periodic sync cycles. An in-flight workspace sync is allowed to finish.
    pub async fn run_until_shutdown(self: Arc<Self>, mut shutdown: watch::Receiver<bool>) {
        tracing::info!(
            "Starting background sync with {}s interval; automatic/default profile {}; other profiles' indexes on disk are left to explicit commands, and local Qwen3/CUDA profiles are skipped entirely",
            self.interval.as_secs(),
            automatic_embedding_profile_name(),
        );

        if *shutdown.borrow() {
            tracing::info!("Background sync shutdown requested before start");
            return;
        }

        // Initial sync after 5 seconds (give system time to start)
        let initial_delay = tokio::time::sleep(Duration::from_secs(5));
        tokio::pin!(initial_delay);
        tokio::select! {
            _ = &mut initial_delay => self.handle_sync_all().await,
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    tracing::info!("Background sync shutdown requested");
                    return;
                }
            }
        }

        // Periodic sync
        let mut interval = tokio::time::interval(self.interval);
        loop {
            tokio::select! {
                _ = interval.tick() => self.handle_sync_all().await,
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        tracing::info!("Background sync shutdown requested");
                        break;
                    }
                }
            }
        }
    }

    /// Sync all tracked directories
    async fn handle_sync_all(&self) {
        let dirs = self.get_tracked_directories().await;

        if dirs.is_empty() {
            tracing::debug!("No directories to sync");
            return;
        }

        tracing::info!("Syncing {} tracked directories", dirs.len());

        for (i, dir) in dirs.iter().enumerate() {
            tracing::debug!("[{}/{}] Syncing: {}", i + 1, dirs.len(), dir.display());

            // A panic is this directory's failed sync, like an `Err`: left to
            // unwind, it ended the background task for the daemon's lifetime
            // with nothing in the log to say so (see `panic_boundary`).
            match crate::panic_boundary::catch_panic(self.sync_directory(dir)).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => tracing::error!("Failed to sync {}: {}", dir.display(), e),
                Err(message) => tracing::error!(
                    target: "panic",
                    "sync of {} panicked: {message}; the next cycle tries again",
                    dir.display()
                ),
            }
        }

        tracing::info!("Sync cycle complete");
    }

    /// Sync a single directory
    ///
    /// Uses the indexing incremental service for fast change detection:
    /// - < 10ms if no changes
    /// - Only reindexes changed files if changes detected
    async fn sync_directory(&self, dir: &Path) -> Result<()> {
        // A store writer: the walk and the embedding of changed files run
        // beside the workspace's readers, which are kept out only while the
        // run writes (16 files, 2026-10-31: 0.4 s of a 23 s run).
        let writer = self.workspace_locks.lock_store_writer(dir).await;
        let maintenance = self
            .sync_directory_indexes(dir, writer.operation_lock())
            .await?;
        if maintenance.is_empty() {
            return Ok(());
        }
        // The stores' maintenance runs outside the operation lock too: a
        // `search` of this workspace must not wait for it (it waited 413.9 s
        // for a 425.2 s pass, 2026-10-31). The next writer still does. Inline
        // rather than spawned: the sync visits its directories one by one,
        // and that keeps at most one pass of this loop running at a time.
        run_outside_operation_lock(writer.into_maintenance(), async {
            for pass in &maintenance {
                pass.run(dir).await;
            }
        })
        .await;
        Ok(())
    }

    /// The indexing half of [`Self::sync_directory`], under its store-writer
    /// lock; `operation` is the workspace's operation lock, which each run
    /// takes for write around its writes. Returns the maintenance passes the
    /// writes call for, one per index that wrote.
    async fn sync_directory_indexes(
        &self,
        dir: &Path,
        operation: &tokio::sync::RwLock<()>,
    ) -> Result<Vec<VectorMaintenance>> {
        use crate::mcp::project_paths::ProjectPaths;
        let mut maintenance = Vec::new();

        // Background sync maintains the automatic profile's index and nothing
        // else. A directory can carry several indexes at once — typically a
        // current GPU one plus a stale CPU one left over from an earlier
        // default — and chasing all of them means the slow profile burns the
        // machine on vectors no search ever reads: same model, same dim, but
        // ~80x apart in throughput (260 vs 3 chunks/s measured on ONNX
        // migraphx vs CPU). Other profiles stay reachable through an explicit
        // index_codebase call with `embedding_profile`.
        let automatic_backend = automatic_embedding_backend();

        let indexes = ProjectPaths::indexed_profiles(dir)
            .map_err(|msg| anyhow::anyhow!(msg))?;
        if indexes.is_empty() {
            tracing::debug!(
                "No existing embedding indexes found for {}; sync skipped",
                dir.display()
            );
            return Ok(maintenance);
        }

        for indexed in indexes {
            let backend = indexed.backend;
            let paths = indexed.paths;
            let stored_identity = indexed.stored_identity;

            if !is_automatic_profile_backend(&backend, &automatic_backend) {
                tracing::info!(
                    "Skipping background sync for {} profile {} because background sync maintains only the automatic profile {}; use an explicit indexing command with embedding_profile=\"{}\" to update it",
                    dir.display(),
                    backend.profile.name(),
                    automatic_backend.profile.name(),
                    backend.profile.name(),
                );
                continue;
            }

            if !is_background_embedding_backend(&backend) {
                tracing::info!(
                    "Skipping background sync for {} profile {} because automatic/background sync runs ONNX (CPU or local GPU) and remote embedding profiles only; use an explicit indexing command with embedding_profile=\"{}\" for local Qwen3/CUDA updates",
                    dir.display(),
                    backend.profile.name(),
                    backend.profile.name(),
                );
                continue;
            }

            let mem = rmc_engine::mem_trace::MemSpan::start(format!(
                "background sync of {}",
                dir.display()
            ));
            let outcome = index_project_incrementally(IncrementalIndexRequest {
                codebase_path: dir,
                cache_path: &paths.cache_path,
                tantivy_path: &paths.tantivy_path,
                collection_name: &paths.collection_name,
                backend,
                embedder_identity: &stored_identity,
                snapshot_path: None,
                codebase_loc: None,
                force_reindex: false,
                // Background sync only chases file changes; repairing coverage
                // is an explicit, user-triggered decision.
                repair_coverage: false,
                reader_exclusion: ReaderExclusion::AroundPublish(operation),
            })
            .await?;
            mem.finish();
            maintenance.extend(outcome.maintenance);
            let stats = outcome.stats;

            if stats.indexed_files > 0 {
                tracing::info!(
                    "✓ Synced {} profile {}: {} files indexed, {} chunks",
                    dir.display(),
                    paths.collection_name,
                    stats.indexed_files,
                    stats.total_chunks
                );
            } else {
                tracing::debug!(
                    "No changes detected for {} profile {}",
                    dir.display(),
                    paths.collection_name
                );
            }
        }

        Ok(maintenance)
    }

    /// Trigger an immediate sync for all tracked directories
    ///
    /// Useful for manual sync requests or testing
    pub async fn sync_now(&self) {
        tracing::info!("Manual sync triggered");
        self.handle_sync_all().await;
    }

    /// Trigger an immediate sync for a specific directory
    pub async fn sync_directory_now(&self, dir: &Path) -> Result<()> {
        tracing::info!("Manual sync triggered for: {}", dir.display());
        self.sync_directory(dir).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_sync_manager_creation() {
        let sync_manager = SyncManager::with_defaults(300);
        assert_eq!(sync_manager.interval, Duration::from_secs(300));
    }

    #[tokio::test]
    async fn test_track_directory() {
        let sync_manager = SyncManager::with_defaults(300);
        let test_dir = PathBuf::from("/tmp/test");

        // Track directory
        sync_manager.track_directory(test_dir.clone()).await;

        let tracked = sync_manager.get_tracked_directories().await;
        assert!(tracked.contains(&test_dir));
        assert_eq!(tracked.len(), 1);
    }

    #[tokio::test]
    async fn test_untrack_directory() {
        let sync_manager = SyncManager::with_defaults(300);
        let test_dir = PathBuf::from("/tmp/test");

        // Track and then untrack
        sync_manager.track_directory(test_dir.clone()).await;
        sync_manager.untrack_directory(&test_dir).await;

        let tracked = sync_manager.get_tracked_directories().await;
        assert!(tracked.is_empty());
    }

    #[tokio::test]
    async fn test_track_directory_canonicalizes_existing_path() {
        let sync_manager = SyncManager::with_defaults(300);
        let temp_dir = TempDir::new().unwrap();
        let project_dir = temp_dir.path().join("project");
        std::fs::create_dir(&project_dir).unwrap();
        let dotted_dir = project_dir.join(".");

        sync_manager.track_directory(dotted_dir).await;

        let tracked = sync_manager.get_tracked_directories().await;
        assert_eq!(tracked, vec![std::fs::canonicalize(&project_dir).unwrap()]);
    }

    #[tokio::test]
    async fn test_untrack_directory_canonicalizes_existing_path() {
        let sync_manager = SyncManager::with_defaults(300);
        let temp_dir = TempDir::new().unwrap();
        let project_dir = temp_dir.path().join("project");
        std::fs::create_dir(&project_dir).unwrap();

        sync_manager.track_directory(project_dir.clone()).await;
        sync_manager.untrack_directory(&project_dir.join(".")).await;

        let tracked = sync_manager.get_tracked_directories().await;
        assert!(tracked.is_empty());
    }

    #[tokio::test]
    async fn test_untrack_all_directories() {
        let sync_manager = SyncManager::with_defaults(300);
        sync_manager.track_directory(PathBuf::from("/tmp/test1")).await;
        sync_manager.track_directory(PathBuf::from("/tmp/test2")).await;

        let count = sync_manager.untrack_all_directories().await;

        assert_eq!(count, 2);
        let tracked = sync_manager.get_tracked_directories().await;
        assert!(tracked.is_empty());
    }

    #[tokio::test]
    async fn runtime_sync_status_reports_tracked_directories() {
        let sync_manager = SyncManager::with_defaults(300);
        let temp_dir = TempDir::new().unwrap();
        let dir1 = temp_dir.path().join("test1");
        let dir2 = temp_dir.path().join("test2");
        sync_manager.track_directory(dir2.clone()).await;
        sync_manager.track_directory(dir1.clone()).await;

        let status = sync_manager.status().await;

        assert_eq!(status.tracked_count, 2);
        assert_eq!(
            status.tracked_directories,
            vec![dir1.display().to_string(), dir2.display().to_string()]
        );
        assert_eq!(sync_manager.tracked_count().await, 2);
    }

    #[tokio::test]
    async fn test_track_multiple_directories() {
        let sync_manager = SyncManager::with_defaults(300);
        let dir1 = PathBuf::from("/tmp/test1");
        let dir2 = PathBuf::from("/tmp/test2");
        let dir3 = PathBuf::from("/tmp/test3");

        sync_manager.track_directory(dir1.clone()).await;
        sync_manager.track_directory(dir2.clone()).await;
        sync_manager.track_directory(dir3.clone()).await;

        let tracked = sync_manager.get_tracked_directories().await;
        assert_eq!(tracked.len(), 3);
        assert!(tracked.contains(&dir1));
        assert!(tracked.contains(&dir2));
        assert!(tracked.contains(&dir3));
    }

    #[tokio::test]
    async fn test_track_same_directory_twice() {
        let sync_manager = SyncManager::with_defaults(300);
        let test_dir = PathBuf::from("/tmp/test");

        sync_manager.track_directory(test_dir.clone()).await;
        sync_manager.track_directory(test_dir.clone()).await;

        let tracked = sync_manager.get_tracked_directories().await;
        assert_eq!(tracked.len(), 1); // Should not duplicate
    }

    #[tokio::test]
    #[ignore] // Requires embedding model
    async fn test_sync_directory_now() {
        let temp_dir = TempDir::new().unwrap();
        let test_codebase = temp_dir.path().join("codebase");
        std::fs::create_dir(&test_codebase).unwrap();
        std::fs::write(
            test_codebase.join("test.rs"),
            "fn main() { println!(\"hello\"); }",
        )
        .unwrap();

        let sync_manager = SyncManager::with_defaults(300);
        sync_manager.track_directory(test_codebase.clone()).await;

        // Should not panic
        let result = sync_manager.sync_directory_now(&test_codebase).await;
        assert!(result.is_ok());
    }

    /// The line runs between MODELS, not between CPU and GPU: a local Qwen3
    /// on CUDA is kept out because an unattended session takes VRAM from
    /// whoever is at the machine, while the ONNX GPU profile is the same
    /// small model as the CPU one on a different execution provider.
    #[test]
    fn background_sync_rejects_local_qwen3_cuda_profiles() {
        let qwen3_cuda_backend =
            rmc_engine::embeddings::EmbeddingBackend::from_profile_name("local-gpu-small")
                .unwrap();
        let onnx_gpu_backend =
            rmc_engine::embeddings::EmbeddingBackend::from_profile_name("local-gpu-bge").unwrap();
        let cpu_backend =
            rmc_engine::embeddings::EmbeddingBackend::from_profile_name("local-cpu-small")
                .unwrap();
        let remote_backend =
            rmc_engine::embeddings::EmbeddingBackend::from_profile_name("openrouter-qwen3-8b")
                .unwrap();

        assert!(!is_background_embedding_backend(&qwen3_cuda_backend));
        assert!(is_background_embedding_backend(&onnx_gpu_backend));
        assert!(is_background_embedding_backend(&cpu_backend));
        assert!(is_background_embedding_backend(&remote_backend));
    }

    /// A directory keeps every index ever built for it, so discovery returns
    /// the stale ones too. Background sync must maintain only the automatic
    /// profile: a leftover CPU index next to a live GPU one is ~80x slower per
    /// chunk and feeds a collection no search reads.
    #[test]
    fn background_sync_maintains_only_the_automatic_profile() {
        let gpu_backend =
            rmc_engine::embeddings::EmbeddingBackend::from_profile_name("local-gpu-bge").unwrap();
        let cpu_backend =
            rmc_engine::embeddings::EmbeddingBackend::from_profile_name("local-cpu-small")
                .unwrap();

        assert!(is_automatic_profile_backend(&gpu_backend, &gpu_backend));
        assert!(!is_automatic_profile_backend(&cpu_backend, &gpu_backend));
        // Symmetric: with a CPU default it is the GPU index that is left alone.
        assert!(is_automatic_profile_backend(&cpu_backend, &cpu_backend));
        assert!(!is_automatic_profile_backend(&gpu_backend, &cpu_backend));
    }

    /// The stale-profile filter must not be confused with the Qwen3/CUDA one:
    /// both CPU and ONNX-GPU pass `is_background_embedding_backend`, so only
    /// the automatic-profile check separates them.
    #[test]
    fn stale_cpu_index_passes_the_qwen3_filter_and_is_stopped_by_the_profile_check() {
        let gpu_backend =
            rmc_engine::embeddings::EmbeddingBackend::from_profile_name("local-gpu-bge").unwrap();
        let cpu_backend =
            rmc_engine::embeddings::EmbeddingBackend::from_profile_name("local-cpu-small")
                .unwrap();

        assert!(is_background_embedding_backend(&cpu_backend));
        assert!(!is_automatic_profile_backend(&cpu_backend, &gpu_backend));
    }

    #[tokio::test]
    async fn background_sync_tracking_skips_local_qwen3_cuda_profiles() {
        let sync_manager = SyncManager::with_defaults(300);
        let temp_dir = tempfile::tempdir().unwrap();
        let qwen3_cuda_backend =
            rmc_engine::embeddings::EmbeddingBackend::from_profile_name("local-gpu-small")
                .unwrap();
        let onnx_gpu_backend =
            rmc_engine::embeddings::EmbeddingBackend::from_profile_name("local-gpu-bge").unwrap();

        assert!(
            !sync_manager
                .track_directory_for_backend(temp_dir.path().to_path_buf(), &qwen3_cuda_backend)
                .await
        );
        assert!(sync_manager.get_tracked_directories().await.is_empty());

        assert!(
            sync_manager
                .track_directory_for_backend(temp_dir.path().to_path_buf(), &onnx_gpu_backend)
                .await
        );
        assert_eq!(sync_manager.get_tracked_directories().await.len(), 1);
    }
}
