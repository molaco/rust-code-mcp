//! Background sync manager for automatic incremental reindexing
//!
//! Matches claude-context's SyncManager behavior:
//! - Runs continuously in background
//! - Syncs every 5 minutes (configurable)
//! - Uses the indexing incremental service for fast change detection
//! - Tracks multiple directories independently

use rmc_indexing::indexing::{index_project_incrementally, IncrementalIndexRequest};
use anyhow::Result;
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{watch, RwLock};
use tracing;

use super::workspace_locks::WorkspaceLockRegistry;

fn normalize_directory(dir: &Path) -> PathBuf {
    std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf())
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
            "Starting background sync with {}s interval",
            self.interval.as_secs()
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

            if let Err(e) = self.sync_directory(dir).await {
                tracing::error!("Failed to sync {}: {}", dir.display(), e);
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
        use crate::mcp::project_paths::ProjectPaths;

        let _workspace_lock = self.workspace_locks.lock_exclusive(dir).await;

        let indexes = ProjectPaths::indexed_profiles(dir)
            .map_err(|msg| anyhow::anyhow!(msg))?;
        if indexes.is_empty() {
            tracing::debug!(
                "No existing embedding indexes found for {}; sync skipped",
                dir.display()
            );
            return Ok(());
        }

        for indexed in indexes {
            let backend = indexed.backend;
            let paths = indexed.paths;
            let stored_identity = indexed.stored_identity;

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
            })
            .await?;
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

        Ok(())
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
}
