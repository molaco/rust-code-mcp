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

use crate::indexing::merkle::{ChangeSet, FileSystemMerkle};
use crate::indexing::unified::{IndexFileResult, IndexStats, UnifiedIndexer};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tracing;

/// Get snapshot storage path for a codebase
///
/// Uses consistent path with data_dir(): ~/.local/share/search/merkle/{hash}.snapshot
pub fn get_snapshot_path(codebase_path: &Path) -> PathBuf {
    use directories::ProjectDirs;

    // Use same ProjectDirs config as data_dir() for consistency
    let merkle_dir = if let Some(proj_dirs) = ProjectDirs::from("dev", "rust-code-mcp", "search") {
        proj_dirs.data_dir().join("merkle")
    } else {
        // Fallback to current directory if we can't find system directories
        PathBuf::from(".merkle")
    };
    std::fs::create_dir_all(&merkle_dir).ok();

    // Hash codebase path to create unique snapshot file
    let path_hash = {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(codebase_path.to_string_lossy().as_bytes());
        format!("{:x}", hasher.finalize())
    };

    merkle_dir.join(format!("{}.snapshot", &path_hash[..16]))
}

/// Incremental indexer that uses Merkle tree for change detection
pub struct IncrementalIndexer {
    indexer: UnifiedIndexer,
}

impl IncrementalIndexer {
    /// Create a new incremental indexer with embedded LanceDB backend
    pub async fn new(
        cache_path: &Path,
        tantivy_path: &Path,
        collection_name: &str,
        vector_size: usize,
        codebase_loc: Option<usize>,
    ) -> Result<Self> {
        let indexer = UnifiedIndexer::for_embedded(
            cache_path,
            tantivy_path,
            collection_name,
            vector_size,
            codebase_loc,
        )
        .await?;

        Ok(Self { indexer })
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
        tracing::info!(
            "Starting incremental indexing for {}",
            codebase_path.display()
        );

        let snapshot_path = get_snapshot_path(codebase_path);
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

        // Step 3: Determine indexing strategy
        let stats = if let Some(old) = old_merkle {
            // Incremental: compare trees and index only changes
            self.incremental_update(codebase_path, &old, &new_merkle)
                .await?
        } else {
            // First time: full index with parallel processing
            tracing::info!("Performing full index (first time) with parallel processing");
            self.indexer.index_directory_parallel(codebase_path).await?
        };

        // Step 4: Save new snapshot for next time
        new_merkle.save_snapshot(&snapshot_path)?;
        tracing::info!(
            "Saved new Merkle snapshot to {}",
            snapshot_path.display()
        );

        Ok(stats)
    }

    /// Perform incremental update based on Merkle tree comparison
    async fn incremental_update(
        &mut self,
        codebase_path: &Path,
        old_merkle: &FileSystemMerkle,
        new_merkle: &FileSystemMerkle,
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
        self.process_changes(codebase_path, changes).await
    }

    /// Process detected changes: add, modify, delete
    async fn process_changes(
        &mut self,
        _codebase_path: &Path,
        changes: ChangeSet,
    ) -> Result<IndexStats> {
        let mut stats = IndexStats::default();

        // Handle deletions
        for deleted_path in &changes.deleted {
            tracing::info!("Deleting chunks for removed file: {}", deleted_path.display());
            self.indexer.delete_file_chunks(deleted_path).await?;
            stats.skipped_files += 1;
        }

        // Handle modifications (delete old + reindex)
        for modified_path in &changes.modified {
            tracing::info!("Reindexing modified file: {}", modified_path.display());
            // Delete old chunks first
            self.indexer.delete_file_chunks(modified_path).await?;

            match self.indexer.index_file(modified_path).await {
                Ok(IndexFileResult::Indexed { chunks_count }) => {
                    stats.indexed_files += 1;
                    stats.total_chunks += chunks_count;
                }
                Ok(_) => stats.skipped_files += 1,
                Err(e) => {
                    tracing::error!("Failed to index {}: {}", modified_path.display(), e);
                    stats.skipped_files += 1;
                }
            }
        }

        // Handle additions
        for added_path in &changes.added {
            tracing::info!("Indexing new file: {}", added_path.display());

            match self.indexer.index_file(added_path).await {
                Ok(IndexFileResult::Indexed { chunks_count }) => {
                    stats.indexed_files += 1;
                    stats.total_chunks += chunks_count;
                }
                Ok(_) => stats.skipped_files += 1,
                Err(e) => {
                    tracing::error!("Failed to index {}: {}", added_path.display(), e);
                    stats.skipped_files += 1;
                }
            }
        }

        // Commit changes to Tantivy
        self.indexer.commit()?;

        tracing::info!(
            "✓ Incremental update complete: {} files indexed, {} chunks",
            stats.indexed_files,
            stats.total_chunks
        );

        Ok(stats)
    }

    /// Get access to underlying indexer for other operations
    pub fn indexer(&self) -> &UnifiedIndexer {
        &self.indexer
    }

    /// Get mutable access to underlying indexer
    pub fn indexer_mut(&mut self) -> &mut UnifiedIndexer {
        &mut self.indexer
    }

    /// Clear all indexed data (metadata cache, Tantivy, and vector store)
    ///
    /// This is used for force reindexing to ensure a completely clean slate.
    /// Note: This does NOT delete the Merkle snapshot - that should be handled separately
    /// by the caller (e.g., in index_tool.rs) before calling index_with_change_detection.
    pub async fn clear_all_data(&mut self) -> Result<()> {
        self.indexer.clear_all_data().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

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
            384,
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
            384,
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
            384,
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
