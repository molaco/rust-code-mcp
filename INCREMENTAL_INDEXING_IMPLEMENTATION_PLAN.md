# Incremental Indexing Implementation Plan
## Achieving claude-context Parity

**Created:** October 21, 2025
**Status:** READY FOR IMPLEMENTATION
**Estimated Time:** 3-4 days focused work
**Priority:** CRITICAL

---

## Executive Summary

This plan rewires your existing Merkle tree implementation to work correctly for incremental indexing, matching claude-context's architecture. All necessary code exists - we just need to connect it properly.

### Key Insight

**You have all the pieces, they're just assembled backwards.**

- ✅ Merkle tree implementation: `src/indexing/merkle.rs` (440 lines, tested)
- ✅ Snapshot persistence: Working save/load
- ✅ Change detection: `detect_changes()` method exists
- ❌ **Wrong:** Used for backups AFTER indexing
- ✅ **Right:** Use for change detection BEFORE indexing

---

## Phase 1: Core Incremental Indexing (Day 1-2)

### Step 1.1: Create IncrementalIndexer Module

**File:** `src/indexing/incremental.rs` (NEW)

```rust
//! Incremental indexing using Merkle tree change detection
//!
//! This module provides the core incremental indexing functionality that:
//! 1. Loads previous Merkle snapshot (if exists)
//! 2. Builds new Merkle tree from current filesystem
//! 3. Detects changes (added/modified/deleted files)
//! 4. Reindexes only changed files
//! 5. Saves new snapshot for next run

use crate::indexing::merkle::{FileSystemMerkle, ChangeSet};
use crate::indexing::unified::{UnifiedIndexer, IndexStats};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tracing;

/// Snapshot storage location
fn get_snapshot_path(codebase_path: &Path) -> PathBuf {
    // Use same pattern as claude-context: ~/.local/share/rust-code-mcp/merkle/{hash}.snapshot
    let home = dirs::home_dir().expect("Could not find home directory");
    let merkle_dir = home.join(".local/share/rust-code-mcp/merkle");
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
    /// Create a new incremental indexer
    pub async fn new(
        cache_path: &Path,
        tantivy_path: &Path,
        qdrant_url: &str,
        collection_name: &str,
        vector_size: usize,
        codebase_loc: Option<usize>,
    ) -> Result<Self> {
        let indexer = UnifiedIndexer::new_with_optimization(
            cache_path,
            tantivy_path,
            qdrant_url,
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
        tracing::info!("Starting incremental indexing for {}", codebase_path.display());

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
            self.incremental_update(codebase_path, &old, &new_merkle).await?
        } else {
            // First time: full index
            tracing::info!("Performing full index (first time)");
            self.indexer.index_directory(codebase_path).await?
        };

        // Step 4: Save new snapshot for next time
        new_merkle.save_snapshot(&snapshot_path)?;
        tracing::info!("Saved new Merkle snapshot to {}", snapshot_path.display());

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
            return Ok(IndexStats::unchanged());
        }

        tracing::info!("Merkle roots differ - detecting specific changes...");

        // Detect specific file changes
        let changes = new_merkle.detect_changes(old_merkle);

        if changes.is_empty() {
            tracing::info!("No file-level changes detected");
            return Ok(IndexStats::unchanged());
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
        codebase_path: &Path,
        changes: ChangeSet,
    ) -> Result<IndexStats> {
        let mut stats = IndexStats::default();

        // Handle deletions
        for deleted_path in &changes.deleted {
            tracing::info!("Deleting chunks for removed file: {}", deleted_path.display());
            // TODO: Implement delete_file_chunks in UnifiedIndexer
            // self.indexer.delete_file_chunks(deleted_path).await?;
            stats.skipped_files += 1; // Count as skipped for now
        }

        // Handle modifications (delete old + reindex)
        for modified_path in &changes.modified {
            tracing::info!("Reindexing modified file: {}", modified_path.display());
            // TODO: Delete old chunks first
            // self.indexer.delete_file_chunks(modified_path).await?;

            match self.indexer.index_file(modified_path).await {
                Ok(crate::indexing::unified::IndexFileResult::Indexed { chunks_count }) => {
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
                Ok(crate::indexing::unified::IndexFileResult::Indexed { chunks_count }) => {
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
        // Note: UnifiedIndexer's Drop already does rollback, but we should commit on success
        // TODO: Add explicit commit method to UnifiedIndexer

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
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    #[ignore] // Requires Qdrant
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
            "http://localhost:6334",
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
    #[ignore] // Requires Qdrant
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
            "http://localhost:6334",
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
        assert_eq!(stats.unchanged_files, 0);
    }

    #[tokio::test]
    #[ignore] // Requires Qdrant
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
            "http://localhost:6334",
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
```

### Step 1.2: Update indexing/mod.rs

**File:** `src/indexing/mod.rs`

```rust
pub mod bulk;
pub mod merkle;
pub mod unified;
pub mod incremental;  // ADD THIS

pub use bulk::*;
pub use merkle::*;
pub use unified::*;
pub use incremental::*;  // ADD THIS
```

### Step 1.3: Add delete_file_chunks to UnifiedIndexer

**File:** `src/indexing/unified.rs`

Add this method:

```rust
impl UnifiedIndexer {
    /// Delete all chunks for a specific file from both Tantivy and Qdrant
    ///
    /// This is needed for incremental indexing when files are modified or deleted
    pub async fn delete_file_chunks(&mut self, file_path: &Path) -> Result<()> {
        let file_path_str = file_path.to_string_lossy().to_string();

        // Delete from Tantivy
        let file_path_term = self.tantivy_schema.file_path.into();
        let query = tantivy::query::TermQuery::new(
            tantivy::Term::from_field_text(file_path_term, &file_path_str),
            tantivy::schema::IndexRecordOption::Basic,
        );

        self.tantivy_writer.delete_query(Box::new(query))?;

        // Delete from Qdrant
        // TODO: Need collection name - might need to pass it as parameter
        // For now, this is a placeholder
        tracing::warn!("Qdrant deletion not yet implemented for {}", file_path_str);

        Ok(())
    }

    /// Commit Tantivy changes
    ///
    /// Useful for forcing a commit after incremental updates
    pub fn commit(&mut self) -> Result<()> {
        self.tantivy_writer.commit().context("Failed to commit Tantivy index")?;
        Ok(())
    }
}
```

---

## Phase 2: Background Sync (Day 2-3)

### Step 2.1: Create SyncManager Module

**File:** `src/mcp/sync.rs` (NEW)

```rust
//! Background sync manager for automatic incremental reindexing
//!
//! Matches claude-context's SyncManager behavior:
//! - Runs continuously in background
//! - Syncs every 5 minutes
//! - Uses IncrementalIndexer for fast change detection

use crate::indexing::incremental::IncrementalIndexer;
use anyhow::Result;
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

    /// Add a directory to track
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
    /// This runs forever, checking for changes every `interval` duration
    pub async fn run(self: Arc<Self>) {
        tracing::info!("Starting background sync with {}s interval", self.interval.as_secs());

        // Initial sync after 5 seconds
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
}
```

### Step 2.2: Integrate SyncManager into MCP Server

**File:** `src/mcp/server.rs`

Modify to start background sync:

```rust
use crate::mcp::sync::SyncManager;
use std::sync::Arc;

pub struct McpServer {
    // ... existing fields ...
    sync_manager: Arc<SyncManager>,
}

impl McpServer {
    pub async fn new(/* params */) -> Result<Self> {
        // ... existing initialization ...

        // Create sync manager
        let sync_manager = Arc::new(SyncManager::new(
            qdrant_url.to_string(),
            cache_path.to_path_buf(),
            tantivy_path.to_path_buf(),
            300, // 5 minutes
        ));

        Ok(Self {
            // ... existing fields ...
            sync_manager,
        })
    }

    pub async fn run(self) -> Result<()> {
        // Start background sync task
        let sync_manager_clone = Arc::clone(&self.sync_manager);
        tokio::spawn(async move {
            sync_manager_clone.run().await;
        });

        // Start MCP protocol handling
        // ... existing server logic ...

        Ok(())
    }

    // Method to add directory to sync
    pub async fn track_for_sync(&self, dir: PathBuf) {
        self.sync_manager.track_directory(dir).await;
    }
}
```

### Step 2.3: Update mcp/mod.rs

**File:** `src/mcp/mod.rs`

```rust
pub mod server;
pub mod sync;  // ADD THIS

pub use server::*;
pub use sync::*;  // ADD THIS
```

---

## Phase 3: MCP Tool Integration (Day 3)

### Step 3.1: Add index_codebase Tool

**File:** `src/tools/index_tool.rs` (NEW)

```rust
//! MCP tool for manual codebase indexing

use crate::indexing::incremental::IncrementalIndexer;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize)]
pub struct IndexCodebaseParams {
    pub directory: String,
    pub force_reindex: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IndexCodebaseResult {
    pub indexed_files: usize,
    pub total_chunks: usize,
    pub status: String,
}

pub async fn index_codebase(params: IndexCodebaseParams) -> Result<IndexCodebaseResult> {
    let dir = PathBuf::from(&params.directory);
    let force = params.force_reindex.unwrap_or(false);

    // TODO: Get these from server config
    let cache_path = dirs::home_dir()
        .unwrap()
        .join(".local/share/rust-code-mcp/cache");
    let tantivy_path = dirs::home_dir()
        .unwrap()
        .join(".local/share/rust-code-mcp/index");
    let qdrant_url = "http://localhost:6334";

    // Create collection name from directory hash
    let collection_name = {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(dir.to_string_lossy().as_bytes());
        let hash = format!("{:x}", hasher.finalize());
        format!("code_chunks_{}", &hash[..8])
    };

    let mut indexer = IncrementalIndexer::new(
        &cache_path,
        &tantivy_path,
        qdrant_url,
        &collection_name,
        384,
        None,
    )
    .await?;

    let stats = if force {
        // Force full reindex by deleting snapshot first
        let snapshot_path = crate::indexing::incremental::get_snapshot_path(&dir);
        if snapshot_path.exists() {
            std::fs::remove_file(&snapshot_path)?;
        }
        indexer.index_with_change_detection(&dir).await?
    } else {
        indexer.index_with_change_detection(&dir).await?
    };

    Ok(IndexCodebaseResult {
        indexed_files: stats.indexed_files,
        total_chunks: stats.total_chunks,
        status: if stats.indexed_files > 0 {
            "indexed".to_string()
        } else {
            "no_changes".to_string()
        },
    })
}
```

### Step 3.2: Register Tool in MCP Server

Update tool registration to include `index_codebase`:

```rust
// In server.rs tool setup
fn setup_tools() {
    // ... existing tools ...

    // Add index_codebase tool
    server.add_tool(
        "index_codebase",
        "Manually index a codebase directory",
        json!({
            "type": "object",
            "properties": {
                "directory": {
                    "type": "string",
                    "description": "Absolute path to codebase directory"
                },
                "force_reindex": {
                    "type": "boolean",
                    "description": "Force full reindex even if already indexed",
                    "default": false
                }
            },
            "required": ["directory"]
        }),
    );
}
```

---

## Phase 4: Testing & Validation (Day 4)

### Test Plan

**Unit Tests:**
```bash
# Test Merkle tree change detection
cargo test --lib merkle

# Test incremental indexer
cargo test --lib incremental

# Test sync manager
cargo test --lib sync
```

**Integration Tests:**

Create `tests/test_incremental_integration.rs`:

```rust
#[tokio::test]
#[ignore]
async fn test_full_incremental_flow() {
    // 1. Index codebase first time
    // 2. Verify snapshot created
    // 3. Modify a file
    // 4. Reindex and verify only 1 file indexed
    // 5. No changes
    // 6. Reindex and verify 0 files indexed (< 10ms)
}
```

**Performance Benchmarks:**

```rust
#[tokio::test]
#[ignore]
async fn bench_unchanged_detection() {
    // Index 1000 files
    // Measure reindex time with no changes
    // Should be < 10ms
}
```

---

## Implementation Checklist

### Day 1: Core Incremental Indexing
- [ ] Create `src/indexing/incremental.rs`
- [ ] Implement `IncrementalIndexer::index_with_change_detection()`
- [ ] Implement `IncrementalIndexer::process_changes()`
- [ ] Add `delete_file_chunks()` to `UnifiedIndexer`
- [ ] Update `src/indexing/mod.rs` exports
- [ ] Write unit tests for incremental indexer

### Day 2: Background Sync
- [ ] Create `src/mcp/sync.rs`
- [ ] Implement `SyncManager::run()`
- [ ] Implement `SyncManager::sync_directory()`
- [ ] Integrate `SyncManager` into `McpServer`
- [ ] Test background sync manually

### Day 3: MCP Tools
- [ ] Create `src/tools/index_tool.rs`
- [ ] Implement `index_codebase` tool handler
- [ ] Register tool in MCP server
- [ ] Add `force_reindex` parameter support
- [ ] Test via MCP client

### Day 4: Testing & Validation
- [ ] Write integration tests
- [ ] Run performance benchmarks
- [ ] Measure unchanged detection time (target: < 10ms)
- [ ] Verify incremental updates work correctly
- [ ] Test background sync with multiple directories

---

## Migration Strategy

### For Existing Users

**Backward Compatibility:**
- Old `index_directory()` still works
- New `index_with_change_detection()` is opt-in
- Snapshot files stored separately (won't conflict)

**Migration Path:**
1. Update to new version
2. First search will use old `index_directory()`
3. Snapshot created automatically
4. Subsequent searches use incremental indexing
5. Background sync activates after first index

**No Breaking Changes:**
- Existing MCP tools continue to work
- New `index_codebase` tool is additional
- No config changes required

---

## Performance Targets

### Unchanged Codebase (10,000 files)
- **Current:** ~10 seconds
- **Target:** < 10ms
- **Improvement:** 1000x

### 5 Files Changed (10,000 total)
- **Current:** ~10 seconds
- **Target:** ~2.5 seconds
- **Improvement:** 4x

### Background Sync Overhead
- **Target:** < 1% CPU when idle
- **Memory:** < 10MB for snapshots

---

## Cleanup: Remove Misleading Backup System

### Optional: Delete or Rename BackupManager

**Option 1: Delete (Recommended)**
```bash
rm src/monitoring/backup.rs
# Update src/monitoring/mod.rs to remove backup module
```

**Option 2: Rename to DisasterRecovery**
```bash
mv src/monitoring/backup.rs src/monitoring/disaster_recovery.rs
# Update comments to clarify it's NOT for incremental indexing
```

**Rationale:**
- Merkle snapshots now serve both purposes:
  - Change detection (primary)
  - Disaster recovery (automatic)
- No need for separate backup system
- Avoid confusion about "backup" vs "incremental"

---

## Success Criteria

### Must Have
✅ Merkle-based change detection working
✅ < 10ms for unchanged codebases
✅ Background sync running every 5 minutes
✅ `index_codebase` MCP tool available
✅ All existing tests still passing

### Nice to Have
✅ Automatic snapshot cleanup (keep last 7)
✅ Per-directory sync intervals
✅ Manual sync trigger via MCP tool
✅ Sync status reporting

---

## Risks & Mitigation

### Risk 1: Snapshot Corruption
**Mitigation:**
- Atomic writes (write to .tmp, then rename)
- Validate on load (checksum)
- Fallback to full reindex if corrupt

### Risk 2: Background Sync Performance
**Mitigation:**
- Debounce: skip sync if previous still running
- Per-directory locks
- Configurable interval

### Risk 3: Qdrant Connection Issues
**Mitigation:**
- Retry logic with exponential backoff
- Fallback to Tantivy-only if Qdrant down
- Log errors but don't crash sync

---

## Documentation Updates

### Update README.md

Add section on incremental indexing:

```markdown
## Incremental Indexing

rust-code-mcp uses Merkle tree-based change detection for lightning-fast reindexing:

- **First index:** Full scan (one-time setup)
- **Subsequent:** < 10ms if no changes, only reindex changed files
- **Background sync:** Automatic every 5 minutes

No configuration needed - it just works!
```

### Update IMPL.md

Mark Phase 2 as complete with incremental indexing:

```markdown
## Phase 2: Performance Optimization ✅

- [x] Qdrant HNSW optimization
- [x] Tantivy memory budgets
- [x] **Incremental indexing with Merkle trees** ← NEW
- [x] Background sync manager ← NEW
```

---

## Next Steps After Implementation

Once incremental indexing works:

1. **Benchmark:** Measure actual performance gains
2. **Optimize:** Tune sync interval based on usage
3. **Monitor:** Add metrics (changes detected, time saved)
4. **Document:** Create blog post about architecture
5. **Compare:** Validate matches claude-context performance

---

## Summary

This plan rewires your existing Merkle tree to work correctly:

**Before (Wrong):**
```
Index → Build Merkle → Save Backup
```

**After (Correct):**
```
Load Snapshot → Compare Trees → Index Changes → Save Snapshot
```

**Key Changes:**
- Move Merkle from AFTER indexing to BEFORE indexing
- Add IncrementalIndexer wrapper
- Add SyncManager for background updates
- Add index_codebase MCP tool

**Estimated Effort:** 3-4 focused days

**Expected Outcome:** 1000x faster change detection, matching claude-context!

---

*Ready to implement? Start with Day 1: Core Incremental Indexing*
