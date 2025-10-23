# Implementation Plan: Fix Health Check Merkle Path (Issue #2)

**Date:** 2025-10-22
**Issue:** #2 - Health Check False Negative for Merkle Snapshot
**Strategy:** Strategy 1 - Reuse `get_snapshot_path()` Function
**Severity:** Low
**Complexity:** Low (5-10 lines changed)

---

## Problem Summary

The health check reports "Merkle snapshot not found (first index pending)" even when the snapshot file exists and is functional.

**Root Cause:** Health tool calculates Merkle snapshot path incorrectly:
- **Actual path:** `~/.local/share/rust-code-mcp/merkle/{hash[..16]}.snapshot`
- **Health check looks for:** `~/.local/share/search/cache/merkle.snapshot`

Three critical mismatches:
1. Different base directory: `rust-code-mcp/` vs `search/`
2. Different subdirectory: `merkle/` vs `cache/`
3. Different filename pattern: `{hash}.snapshot` vs `merkle.snapshot`

---

## Strategy 1: Reuse Existing Function

**Approach:** Import and use the existing `get_snapshot_path()` function from `src/indexing/incremental.rs` instead of reimplementing the path calculation.

**Advantages:**
- ✅ Guaranteed consistency with actual snapshot location
- ✅ Single source of truth (DRY principle)
- ✅ Minimal code changes (5-10 lines)
- ✅ No risk of logic drift
- ✅ Automatically inherits any future path changes

---

## Implementation Steps

### Step 1: Import the Function

**File:** `src/tools/health_tool.rs`
**Location:** Top of file (around line 12)

**Change:**
```rust
// Add to existing imports
use crate::indexing::incremental::get_snapshot_path;
```

### Step 2: Replace Path Calculation Logic

**File:** `src/tools/health_tool.rs`
**Location:** Lines 38-56 (path determination block)

**Before:**
```rust
// Determine paths
let (bm25_path, merkle_path, collection_name) = if let Some(ref dir) = directory {
    let project_name = std::path::Path::new(dir)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("default")
        .replace(|c: char| !c.is_alphanumeric(), "_");

    (
        data_dir().join(format!("index_{}", project_name)),
        data_dir().join(format!("cache_{}/merkle.snapshot", project_name)),  // ❌ WRONG!
        format!("code_chunks_{}", project_name),
    )
} else {
    (
        data_dir().join("index"),
        data_dir().join("cache/merkle.snapshot"),  // ❌ WRONG!
        "code_chunks_default".to_string(),
    )
};
```

**After:**
```rust
// Determine paths
let (bm25_path, merkle_path, collection_name) = if let Some(ref dir) = directory {
    let dir_path = std::path::Path::new(dir);

    // Calculate directory hash for collection name (same as index_tool)
    let dir_hash = {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(dir_path.to_string_lossy().as_bytes());
        format!("{:x}", hasher.finalize())
    };

    let cache_hash = &dir_hash[..8];

    (
        data_dir().join("index").join(cache_hash),
        get_snapshot_path(dir_path),  // ✅ Use actual function!
        format!("code_chunks_{}", cache_hash),
    )
} else {
    // System-wide check: can't determine specific snapshot path
    // Merkle snapshots are directory-specific, so this will report as missing
    (
        data_dir().join("index"),
        std::path::PathBuf::from("/nonexistent/merkle.snapshot"),  // Sentinel value
        "code_chunks_default".to_string(),
    )
};
```

### Step 3: Update Health Check Documentation

**File:** `src/tools/health_tool.rs`
**Location:** Around line 100-103

**Change:**
```rust
response.push_str("\n\n=== Health Check Guide ===\n");
response.push_str("- Healthy: All components operational\n");
response.push_str("- Degraded: One search engine down OR Merkle snapshot missing\n");
response.push_str("- Unhealthy: Both BM25 and Vector search are down\n");
response.push_str("\nNote: Merkle snapshots are directory-specific. Use 'directory' parameter for accurate check.\n");  // NEW
```

### Step 4: Add Additional Fix - Collection Name Consistency

**Issue:** Health check also uses wrong collection naming (directory name instead of hash)

**Change in same block (Step 2):**
```rust
// OLD collection name (inconsistent with index_tool)
format!("code_chunks_{}", project_name)  // Uses directory name

// NEW collection name (consistent with index_tool)
format!("code_chunks_{}", cache_hash)  // Uses hash (first 8 chars)
```

---

## Code Changes Summary

### File: `src/tools/health_tool.rs`

**Imports (add):**
```rust
use crate::indexing::incremental::get_snapshot_path;
use sha2::{Digest, Sha256};  // For hash calculation
```

**Lines 38-56 (replace entire block):**
- Remove manual path construction
- Add hash calculation for collection name
- Use `get_snapshot_path()` for Merkle path
- Handle system-wide case with sentinel value

**Lines ~103 (add documentation):**
- Add note about directory-specific snapshots

**Total Changes:**
- ~2 lines added (imports)
- ~20 lines replaced (path calculation logic)
- ~1 line added (documentation)
- **Net: ~15-20 lines changed**

---

## Testing Plan

### 1. Verify Snapshot Path Calculation

**Test:**
```bash
# Check where snapshot actually exists
ls -la ~/.local/share/rust-code-mcp/merkle/

# Example output:
# eb5e3f0336e172f5.snapshot  (Burn codebase)
```

**Expected:** Health check should find this file when checking `/home/molaco/Documents/burn`

### 2. Test Directory-Specific Health Check

**Before Fix:**
```rust
mcp__rust-code-mcp__health_check(
    directory: "/home/molaco/Documents/burn"
)
```

**Expected Before:**
```json
{
  "merkle": {
    "status": "degraded",
    "message": "Merkle snapshot not found (first index pending)"
  }
}
```

**Expected After Fix:**
```json
{
  "merkle": {
    "status": "healthy",
    "message": "Merkle snapshot exists"
  }
}
```

### 3. Test System-Wide Health Check

**Test:**
```rust
mcp__rust-code-mcp__health_check()  // No directory parameter
```

**Expected After Fix:**
```json
{
  "merkle": {
    "status": "degraded",
    "message": "Merkle snapshot not found (first index pending)"
  }
}
```

**Note:** This is correct behavior - system-wide checks can't determine which directory-specific snapshot to check.

### 4. Test Collection Name Consistency

**Test:**
```bash
# After fix, health check should connect to same collection as index_tool
# For /home/molaco/Documents/burn:
# - Collection: code_chunks_eb5e3f03 (hash-based, not "code_chunks_burn")
```

### 5. Verify Compilation

**Test:**
```bash
cargo check --lib
```

**Expected:** No errors, only existing unused import warnings

---

## Edge Cases

### Case 1: Directory Never Indexed

**Scenario:** User runs health check on directory that was never indexed

**Behavior:**
- Merkle snapshot doesn't exist
- Health check correctly reports "degraded" with message "Merkle snapshot not found"
- This is CORRECT behavior (not a false negative)

### Case 2: Directory Parameter Not Provided

**Scenario:** System-wide health check (`directory: None`)

**Behavior:**
- Cannot determine which snapshot to check (snapshots are directory-specific)
- Uses sentinel path that doesn't exist
- Reports "degraded" for Merkle component
- This is EXPECTED behavior

### Case 3: Snapshot Deleted But Collection Exists

**Scenario:** User manually deleted snapshot but Qdrant collection still has data

**Behavior:**
- Merkle: degraded (snapshot missing)
- Vector store: healthy (collection exists)
- Overall: degraded
- This is CORRECT - system will perform full reindex next time

---

## Validation Checklist

Before committing:

- [ ] Import `get_snapshot_path` function
- [ ] Import `sha2` for hash calculation
- [ ] Replace path calculation with `get_snapshot_path()`
- [ ] Add hash-based collection name calculation
- [ ] Handle system-wide case with sentinel value
- [ ] Add documentation note about directory-specific snapshots
- [ ] Code compiles without errors
- [ ] Test with directory that has snapshot (should report healthy)
- [ ] Test with directory without snapshot (should report degraded)
- [ ] Test system-wide check (should report degraded for Merkle)
- [ ] Verify collection name matches index_tool approach

---

## Rollback Plan

If issues arise:

1. **Compilation errors:** Check import paths
2. **Runtime errors:** Verify `get_snapshot_path()` is public
3. **Path still wrong:** Double-check we're using correct function signature
4. **Full rollback:** `git revert HEAD` (if committed)

---

## Documentation Updates

### Files to Update:

1. **ISSUES.md**
   - Mark Issue #2 as ✅ RESOLVED
   - Add root cause explanation
   - Document fix approach

2. **FIX_HEALTH_CHECK_PATH.md** (new file)
   - Complete fix documentation
   - Before/after path comparison
   - Testing results

3. **src/tools/health_tool.rs** (inline docs)
   - Add comment explaining why we use `get_snapshot_path()`
   - Document system-wide behavior

---

## Success Criteria

✅ Health check correctly finds existing Merkle snapshots
✅ No false negatives for directories with snapshots
✅ Consistent path calculation with `index_tool.rs`
✅ Collection names consistent (hash-based)
✅ Code compiles without errors
✅ All existing tests still pass
✅ System-wide check behavior documented

---

## Estimated Time

- **Implementation:** 15-20 minutes
- **Testing:** 10-15 minutes
- **Documentation:** 15-20 minutes
- **Total:** 40-55 minutes

---

## Dependencies

- Requires `src/indexing/incremental.rs` (already exists)
- Requires `get_snapshot_path()` to be public (already is)
- No external dependencies needed

---

## Related Issues

- **Issue #1** (Force Reindex) - ✅ Fixed (2025-10-21)
- **Issue #2** (Health Check) - 🔄 This fix
- **Issue #3** (Semantic Search) - ✅ Fixed (2025-10-22)
- **Issue #4** (Port Config) - ✅ Fixed (2025-10-21)

---

## Notes

### Why Not Fix BM25 Path Too?

The BM25 path calculation also has inconsistencies:
- Health check: `data_dir().join("index_{project_name}")`
- Index tool: `data_dir().join("index").join(dir_hash)`

However, this is a separate issue and can be addressed in a future fix. For Issue #2, we're focusing only on the Merkle snapshot path since that's what's being reported incorrectly.

### Alternative: Make Collection Name Consistent

As a bonus, we're also fixing the collection name to use hash-based naming (consistent with `index_tool` and `get_similar_code`). This ensures the health check connects to the correct Qdrant collection.

Before: `code_chunks_burn` (wrong collection)
After: `code_chunks_eb5e3f03` (correct collection)
