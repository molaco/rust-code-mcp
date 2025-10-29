//! Background sync manager for automatic incremental reindexing
//!
//! Matches claude-context's SyncManager behavior:
//! - Runs continuously in background
//! - Syncs every 5 minutes (configurable)
//! - Uses IncrementalIndexer for fast change detection
//! - Tracks multiple directories independently

use crate::indexing::incremental::IncrementalIndexer;
use anyhow::Result;
use directories::ProjectDirs;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing;

/// Manages background synchronization of indexed codebases
pub struct SyncManager {
    /// List of directories being tracked
    tracked_dirs: Arc<RwLock<HashSet<PathBuf>>>,
    /// Sync interval (default: 5 minutes)
    interval: Duration,
    /// Qdrant connection info
    qdrant_url: String,
    /// Base paths for cache and indices
    cache_base: PathBuf,
    tantivy_base: PathBuf,
}

impl SyncManager {
    /// Create a new sync manager
    ///
    /// # Arguments
    /// * `qdrant_url` - Qdrant server URL (e.g., "http://localhost:6334")
    /// * `cache_base` - Base directory for metadata caches
    /// * `tantivy_base` - Base directory for Tantivy indices
    /// * `interval_secs` - Sync interval in seconds (default: 300 = 5 minutes)
    pub fn new(
        qdrant_url: String,
        cache_base: PathBuf,
        tantivy_base: PathBuf,
        interval_secs: u64,
    ) -> Self {
        Self {
            tracked_dirs: Arc::new(RwLock::new(HashSet::new())),
            interval: Duration::from_secs(interval_secs),
            qdrant_url,
            cache_base,
            tantivy_base,
        }
    }

    /// Create a new sync manager with default paths
    ///
    /// Uses XDG-compliant directories or falls back to current directory
    pub fn with_defaults(interval_secs: u64) -> Self {
        let data_dir = ProjectDirs::from("dev", "rust-code-mcp", "search")
            .map(|dirs| dirs.data_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from(".rust-code-mcp"));

        let qdrant_url =
            std::env::var("QDRANT_URL").unwrap_or_else(|_| "http://localhost:6334".to_string());

        Self::new(
            qdrant_url,
            data_dir.join("cache"),
            data_dir.join("index"),
            interval_secs,
        )
    }

    /// Add a directory to track
    ///
    /// The directory will be automatically synced on the next sync cycle
    pub async fn track_directory(&self, dir: PathBuf) {
        let mut dirs = self.tracked_dirs.write().await;
        if dirs.insert(dir.clone()) {
            tracing::info!("Now tracking directory for sync: {}", dir.display());
        }
    }

    /// Remove a directory from tracking
    pub async fn untrack_directory(&self, dir: &PathBuf) {
        let mut dirs = self.tracked_dirs.write().await;
        if dirs.remove(dir) {
            tracing::info!("Stopped tracking directory: {}", dir.display());
        }
    }

    /// Get list of tracked directories
    pub async fn get_tracked_directories(&self) -> Vec<PathBuf> {
        let dirs = self.tracked_dirs.read().await;
        dirs.iter().cloned().collect()
    }

    /// Run background sync loop
    ///
    /// This runs forever, checking for changes every `interval` duration.
    /// Designed to be spawned as a background task.
    ///
    /// # Example
    /// ```no_run
    /// use file_search_mcp::mcp::SyncManager;
    /// use std::sync::Arc;
    /// let sync_manager = Arc::new(SyncManager::with_defaults(300));
    /// tokio::spawn(async move {
    ///     sync_manager.run().await;
    /// });
    /// ```
    pub async fn run(self: Arc<Self>) {
        tracing::info!(
            "Starting background sync with {}s interval",
            self.interval.as_secs()
        );

        // Initial sync after 5 seconds (give system time to start)
        tokio::time::sleep(Duration::from_secs(5)).await;
        self.handle_sync_all().await;

        // Periodic sync
        let mut interval = tokio::time::interval(self.interval);
        loop {
            interval.tick().await;
            self.handle_sync_all().await;
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
    /// Uses IncrementalIndexer for fast change detection:
    /// - < 10ms if no changes
    /// - Only reindexes changed files if changes detected
    async fn sync_directory(&self, dir: &PathBuf) -> Result<()> {
        // Create paths for this directory
        let dir_hash = {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(dir.to_string_lossy().as_bytes());
            format!("{:x}", hasher.finalize())
        };

        let cache_path = self.cache_base.join(&dir_hash);
        let tantivy_path = self.tantivy_base.join(&dir_hash);
        let collection_name = format!("code_chunks_{}", &dir_hash[..8]);

        // Create incremental indexer
        let mut indexer = IncrementalIndexer::new(
            &cache_path,
            &tantivy_path,
            &self.qdrant_url,
            &collection_name,
            384, // vector size for all-MiniLM-L6-v2
            None,
        )
        .await?;

        // Run incremental indexing
        let stats = indexer.index_with_change_detection(dir).await?;

        if stats.indexed_files > 0 {
            tracing::info!(
                "✓ Synced {}: {} files indexed, {} chunks",
                dir.display(),
                stats.indexed_files,
                stats.total_chunks
            );
        } else {
            tracing::debug!("No changes detected for {}", dir.display());
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
    pub async fn sync_directory_now(&self, dir: &PathBuf) -> Result<()> {
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
    #[ignore] // Requires Qdrant
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
