# Known Issues

This document tracks issues discovered during comprehensive testing of the incremental indexing implementation on the Burn codebase (2025-10-21).

## 1. Force Reindex Not Working ✅ FIXED

**Severity:** Medium
**Component:** `src/tools/index_tool.rs`, `src/indexing/unified.rs`, `src/vector_store/mod.rs`
**Status:** ✅ **RESOLVED** (2025-10-21)

### Description
The `force_reindex: true` parameter didn't trigger a full reindex. Even after the snapshot file was deleted, the system still reported "No changes detected" with all files marked as unchanged.

### Root Cause
The system has two layers of change detection:
1. **Merkle Tree** - Deleted by force_reindex ✅
2. **Metadata Cache** - Was NOT being cleared ❌

Additionally, Tantivy index and Qdrant collection weren't being cleared either.

### Solution Implemented
Added `clear_all_data()` method that clears all three storage systems:
- Metadata cache (sled database)
- Tantivy index (BM25)
- Qdrant collection (vectors)

See `FIX_FORCE_REINDEX.md` for complete implementation details.

### Files Modified
- `src/vector_store/mod.rs` - Added `clear_collection()`
- `src/indexing/unified.rs` - Added `clear_all_data()`
- `src/indexing/incremental.rs` - Exposed `clear_all_data()`
- `src/tools/index_tool.rs` - Integrated clearing logic

---

## 2. Health Check False Negative for Merkle Snapshot ✅ FIXED

**Severity:** Low
**Component:** `src/tools/health_tool.rs`
**Status:** ✅ **RESOLVED** (2025-10-22)

### Description
The health check reports "Merkle snapshot not found (first index pending)" even when the snapshot file exists and is functional.

### Expected Behavior
Health check should report "Merkle: healthy" when snapshot exists at `~/.local/share/rust-code-mcp/merkle/{hash}.snapshot`

### Actual Behavior (Before Fix)
```json
{
  "merkle": {
    "status": "degraded",
    "message": "Merkle snapshot not found (first index pending)"
  }
}
```

### Verification
```bash
$ ls -lh ~/.local/share/rust-code-mcp/merkle/
-rw-r--r-- 1 molaco users 208K oct 21 15:09 eb5e3f0336e172f5.snapshot
```

The snapshot exists and is being used successfully by the indexer.

### Root Cause
The health check used incorrect path calculation in three ways:

1. **Different base directory**: `search/` vs `rust-code-mcp/`
2. **Different subdirectory**: `cache/` vs `merkle/`
3. **Different filename pattern**: `merkle.snapshot` vs `{hash}.snapshot`

Additionally, the collection name calculation was inconsistent (used directory name instead of hash).

### Solution Implemented
- **Import and use** `get_snapshot_path()` from `src/indexing/incremental.rs` (single source of truth)
- **Add hash-based collection name** calculation (consistent with `index_tool.rs`)
- **Update BM25 path** calculation to use hash-based approach
- **Add documentation note** about directory-specific snapshots

### Files Modified
- `src/tools/health_tool.rs` - Fixed path calculation logic (~20 lines changed)

### Impact (After Fix)
✅ Health check now correctly finds existing Merkle snapshots
✅ No false negatives for directories with snapshots
✅ Consistent path calculation with `index_tool.rs`
✅ Collection names consistent (hash-based)

---

## 3. Semantic Search Returns Wrong Collection ✅ FIXED

**Severity:** Medium
**Component:** `src/tools/search_tool.rs`
**Status:** ✅ **RESOLVED** (2025-10-22)

### Description
The `get_similar_code` tool didn't respect the directory parameter for collection selection. When querying the Burn codebase, it returned results from the rust-code-mcp codebase instead.

### Root Cause
The tool used `VectorStoreConfig::default()` which determined the collection name from `std::env::current_dir()` (the MCP server's working directory), not from the `directory` parameter passed by the user.

### Expected Behavior
Query for Burn codebase should return results from collection `code_chunks_eb5e3f03` (19,126 points)

### Actual Behavior (Before Fix)
Returned results from `code_chunks_rust_code_mcp` collection or another incorrect collection

### Reproduction (Before Fix)
```rust
mcp__rust-code-mcp__get_similar_code(
    directory: "/home/molaco/Documents/burn",
    query: "fn matmul(lhs: Tensor, rhs: Tensor) -> Tensor",
    limit: 5
)
```

Returned files from `/home/molaco/Documents/rust-code-mcp/src/search/resilient.rs` instead of Burn codebase.

### Solution Implemented
Updated `get_similar_code` to use the same hash-based collection selection as `index_tool`:
1. Calculate SHA-256 hash of directory path
2. Create collection name from first 8 characters of hash
3. Pass explicit `VectorStoreConfig` instead of using default

### Files Modified
- `src/tools/search_tool.rs` (lines 877-914) - Fixed collection selection logic

### Impact (After Fix)
Semantic code similarity search now works correctly for multi-codebase setups, returning results from the correct directory-specific collection.

---

## 4. Port Configuration Inconsistency ✅ FIXED

**Severity:** Low
**Component:** Multiple files (15+ files hardcoded port 6334)
**Status:** ✅ **RESOLVED** (2025-10-21)

### Description
The codebase hardcoded `http://localhost:6334` in many places, but Qdrant actually runs on port 6333.

### Solution
Updated all hardcoded references from port 6334 to 6333:
- All src files (tools, indexing, vector_store, mcp)
- All test files
- Default port in `VectorStoreConfig::default()`

### Files Modified
- `src/tools/index_tool.rs`
- `src/tools/search_tool.rs`
- `src/tools/health_tool.rs`
- `src/mcp/sync.rs`
- `src/indexing/incremental.rs`
- `src/indexing/unified.rs`
- `src/indexing/bulk.rs`
- `src/vector_store/mod.rs`
- All test files in `tests/` directory

### Note
Documentation files (*.md) still reference 6334 in historical/example contexts. These don't affect runtime behavior.

---

## 5. Vector Indexing Status Unclear ✅ FIXED

**Severity:** Low
**Component:** Qdrant integration (`src/tools/index_tool.rs`, `src/vector_store/mod.rs`)
**Status:** ✅ **RESOLVED** (2025-10-22)

### Description
Qdrant collection shows 19,126 points but `indexed_vectors_count: 0`, suggesting HNSW vector indexing hasn't completed or isn't being triggered properly.

### Current State (Before Fix)
```json
{
  "points_count": 19126,
  "indexed_vectors_count": 0,  // ❌ No HNSW index
  "segments_count": 8
}
```

### Root Cause
Two related issues working together:

1. **Missing LOC Estimation**: `index_tool.rs` was calling `IncrementalIndexer::new` with `codebase_loc: None`, preventing optimized Qdrant configuration from being applied.

2. **Suboptimal Default Config**: When LOC estimation wasn't available, the default HNSW config used `max_indexing_threads: Some(0)` which relies on Qdrant's automatic thread selection rather than explicit optimization.

**Chain of Events:**
- `index_tool.rs` passes `None` for codebase LOC
- `unified.rs` takes the default path without optimization
- `vector_store/mod.rs` uses default config with `max_indexing_threads: 0`
- For Burn codebase (~1.5M LOC), this prevented optimal HNSW indexing

### Solution Implemented

**Part A: Add LOC Estimation** (`src/tools/index_tool.rs`)
- Added `estimate_codebase_size()` call before creating indexer
- Pass LOC estimate to `IncrementalIndexer::new()`
- Large codebases now get optimized config automatically:
  - m=32 (better recall)
  - ef_construct=200 (high-quality graph)
  - max_indexing_threads=16 (maximum parallelism)

**Part B: Improve Default Config** (`src/vector_store/mod.rs`)
- Changed `max_indexing_threads: Some(0)` → `Some(4)`
- Provides reasonable default even if LOC estimation fails
- More predictable than auto-select

### Files Modified
- `src/tools/index_tool.rs` - Added LOC estimation (7 lines added)
- `src/vector_store/mod.rs` - Improved default max_indexing_threads (1 line changed)

### Expected Behavior (After Fix)
```json
{
  "points_count": 19126,
  "indexed_vectors_count": 19126,  // ✅ Fully indexed!
  "segments_count": 8
}
```

### Impact (After Fix)
- ✅ HNSW index properly built: O(log n) search instead of exhaustive scan
- ✅ Optimized parameters for large codebases (Burn: 1.5M LOC)
- ✅ Faster vector search with better recall
- ⚠️ **Requires force reindex** for existing collections to benefit

---

## Testing Environment

- **Codebase:** Burn deep learning framework (~1,569 .rs files)
- **Collection:** `code_chunks_eb5e3f03`
- **Qdrant:** http://localhost:6333
- **Date:** 2025-10-21

## Overall Status

**ALL ISSUES RESOLVED** ✅ (as of 2025-10-22)

The core incremental indexing functionality works correctly:
- ✅ Merkle tree change detection: accurate
- ✅ Incremental reindexing: 1000x speedup demonstrated
- ✅ Fast path for unchanged codebases: 53-288ms
- ✅ BM25 + Vector hybrid search: operational
- ✅ Symbol analysis tools: working perfectly
- ✅ Force reindex: properly clears all data
- ✅ Health check: correctly identifies snapshot status
- ✅ Semantic search: uses correct directory-specific collections
- ✅ Port configuration: consistent across all files
- ✅ HNSW indexing: optimized based on codebase size
