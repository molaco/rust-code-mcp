# Fix: Force Reindex Not Working (Issue #1)

**Date:** 2025-10-21
**Status:** ✅ FIXED

## Problem Summary

When `force_reindex: true` was set, the system would:
1. ✅ Delete the Merkle snapshot successfully
2. ✅ Detect "no previous snapshot" (first-time indexing mode)
3. ❌ But still report "0 indexed files, 1,569 unchanged files"

### Root Cause

The system has **two layers of change detection**:

1. **Merkle Tree** (coarse-grained) - `~/.local/share/rust-code-mcp/merkle/*.snapshot`
2. **Metadata Cache** (fine-grained) - `~/.local/share/search/cache/*/` (sled database)

The `force_reindex` flag was only deleting the Merkle snapshot (layer 1), but **not clearing the metadata cache (layer 2)**.

When `index_directory()` called `index_file()` for each file, the metadata cache check (line 241 in `unified.rs`) would return `Unchanged` because the cache still had all the file hashes stored.

Additionally, the **Tantivy index** (55MB) and **Qdrant collection** (19,126 points) were not being cleared either.

## Solution Implemented

Added a comprehensive `clear_all_data()` method that clears **all three storage systems**:

### 1. Added `VectorStore::clear_collection()`
**File:** `src/vector_store/mod.rs`

```rust
pub async fn clear_collection(&self) -> Result<(), Box<dyn std::error::Error + Send>> {
    // Deletes all points from Qdrant collection using an empty filter
    let delete_points = qdrant_client::qdrant::DeletePoints {
        collection_name: self.collection_name.clone(),
        points: Some(qdrant_client::qdrant::PointsSelector {
            points_selector_one_of: Some(
                qdrant_client::qdrant::points_selector::PointsSelectorOneOf::Filter(
                    Filter::default() // Empty filter matches all points
                ),
            ),
        }),
        ..Default::default()
    };

    self.client.delete_points(delete_points).await?;
    Ok(())
}
```

### 2. Added `UnifiedIndexer::clear_all_data()`
**File:** `src/indexing/unified.rs`

```rust
pub async fn clear_all_data(&mut self) -> Result<()> {
    tracing::info!("Clearing all indexed data (metadata cache, Tantivy, Qdrant)...");

    // 1. Clear metadata cache (sled database)
    self.metadata_cache.clear()?;
    tracing::info!("✓ Cleared metadata cache");

    // 2. Delete all Tantivy documents
    self.tantivy_writer.delete_all_documents()?;
    self.tantivy_writer.commit()?;
    tracing::info!("✓ Cleared Tantivy index");

    // 3. Clear Qdrant collection
    self.vector_store.clear_collection().await?;
    tracing::info!("✓ Cleared Qdrant collection");

    tracing::info!("✓ All indexed data cleared successfully");
    Ok(())
}
```

### 3. Exposed through `IncrementalIndexer`
**File:** `src/indexing/incremental.rs`

```rust
pub async fn clear_all_data(&mut self) -> Result<()> {
    self.indexer.clear_all_data().await
}
```

### 4. Integrated into `index_tool.rs`
**File:** `src/tools/index_tool.rs`

```rust
// Handle force reindex by deleting snapshot
if force {
    let snapshot_path = get_snapshot_path(&dir);
    if snapshot_path.exists() {
        tracing::info!("Force reindex: deleting snapshot at {}", snapshot_path.display());
        std::fs::remove_file(&snapshot_path)?;
    }
}

// Create incremental indexer
let mut indexer = IncrementalIndexer::new(...).await?;

// Clear all indexed data if force reindex
if force {
    tracing::info!("Force reindex: clearing all indexed data (metadata cache, Tantivy, Qdrant)");
    indexer.clear_all_data().await?;
}

// Run incremental indexing (will now do full reindex)
let stats = indexer.index_with_change_detection(&dir).await?;
```

## Expected Behavior After Fix

When `force_reindex: true`:

1. ✅ Delete Merkle snapshot (~/.local/share/rust-code-mcp/merkle/*.snapshot)
2. ✅ Create indexer
3. ✅ **NEW:** Clear metadata cache (~/.local/share/search/cache/*/)
4. ✅ **NEW:** Delete all Tantivy documents
5. ✅ **NEW:** Clear Qdrant collection (delete all points)
6. ✅ Run `index_with_change_detection()`
7. ✅ All files treated as new → **Full reindex**
8. ✅ Result: "Successfully indexed 1,569 files (forced full reindex)"

## Testing

The implementation compiles successfully with only warnings about unused imports (not errors).

To test manually:
```rust
mcp__rust-code-mcp__index_codebase(
    directory: "/path/to/codebase",
    force_reindex: true
)
```

Expected output:
```
✓ Successfully indexed '/path/to/codebase'

Indexing stats:
- Indexed files: 1569 (forced full reindex)
- Total chunks: 19126
- Unchanged files: 0
- Skipped files: 47
- Time: 5m 30s

Background sync: enabled (5-minute interval)
Collection: code_chunks_eb5e3f03
```

## Design Decision: Option B (Encapsulation)

We chose **Option B** (add `clear_all_data()` to UnifiedIndexer) over:
- **Option A:** Clear cache in `index_tool.rs` - ❌ Violates encapsulation
- **Option C:** Pass force flag through - ❌ Over-engineered

### Why Option B is Best:

1. **Encapsulation** - Metadata cache is an internal detail of `UnifiedIndexer`
2. **Maintainability** - Only one place to update if cache format changes
3. **Completeness** - Clears all three storage systems (cache, Tantivy, Qdrant)
4. **Reusability** - Useful for other scenarios (debugging, recovery, testing)
5. **Clear API** - Intent is obvious from method name

## Files Modified

1. `src/vector_store/mod.rs` - Added `clear_collection()` method
2. `src/indexing/unified.rs` - Added `clear_all_data()` method
3. `src/indexing/incremental.rs` - Exposed `clear_all_data()`
4. `src/tools/index_tool.rs` - Call `clear_all_data()` when `force_reindex: true`

## Related Issues

This fix resolves **Issue #1** in `ISSUES.md`.

Note: Testing revealed **Issue #4** (Port Configuration) - the code tries port 6334 but Qdrant runs on 6333. This is a separate issue and doesn't affect the force_reindex logic itself.

## Metadata Cache Details

The metadata cache (sled database) stores:
- File path → SHA-256 hash mapping
- Last modification time
- File size
- Indexed timestamp

Located at: `~/.local/share/search/cache/{codebase_hash}/`

The `MetadataCache::clear()` method (line 112 in `metadata_cache.rs`) calls `self.db.clear()` which removes all key-value pairs from the sled database.

## Next Steps

1. ✅ Implementation complete
2. ✅ Issue #4 (port configuration) resolved - all references updated to port 6333
3. ✅ Update ISSUES.md to mark Issue #1 as resolved
4. 🧪 Ready for integration testing with MCP server restart
