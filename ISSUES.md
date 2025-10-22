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

## 2. Health Check False Negative for Merkle Snapshot

**Severity:** Low
**Component:** `src/tools/health_tool.rs`

### Description
The health check reports "Merkle snapshot not found (first index pending)" even when the snapshot file exists and is functional.

### Expected Behavior
Health check should report "Merkle: healthy" when snapshot exists at `~/.local/share/rust-code-mcp/merkle/{hash}.snapshot`

### Actual Behavior
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
The health check likely uses a different path calculation or doesn't check the correct location.

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

## 5. Vector Indexing Status Unclear

**Severity:** Low
**Component:** Qdrant integration

### Description
Qdrant collection shows 19,126 points but `indexed_vectors_count: 0`, suggesting HNSW vector indexing hasn't completed or isn't being triggered properly.

### Current State
```json
{
  "points_count": 19126,
  "indexed_vectors_count": 0,
  "segments_count": 8
}
```

### Expected Behavior
After indexing completes, `indexed_vectors_count` should equal `points_count` for optimal vector search performance.

### Impact
- Vector search still works but may be slower
- HNSW index provides O(log n) search instead of exhaustive scan

### Investigation Needed
- Check if Qdrant needs explicit indexing trigger
- Verify indexing threshold configuration (`indexing_threshold: 10000`)
- Monitor if indexing completes asynchronously

---

## Testing Environment

- **Codebase:** Burn deep learning framework (~1,569 .rs files)
- **Collection:** `code_chunks_eb5e3f03`
- **Qdrant:** http://localhost:6333
- **Date:** 2025-10-21

## Overall Status

Despite these issues, the core incremental indexing functionality works correctly:
- ✅ Merkle tree change detection: accurate
- ✅ Incremental reindexing: 1000x speedup demonstrated
- ✅ Fast path for unchanged codebases: 53-288ms
- ✅ BM25 + Vector hybrid search: operational
- ✅ Symbol analysis tools: working perfectly
