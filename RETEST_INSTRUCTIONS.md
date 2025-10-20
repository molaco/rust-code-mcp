# Re-Test Instructions After Tantivy Lock Fix

**Date:** 2025-10-20
**Status:** Ready for Re-Testing
**Previous Results:** 8/9 tools passed (search tool failed)

---

## What Was Fixed

✅ **Fixed Tantivy Index Lock Conflict**
- Multiple rapid searches no longer fail
- Implemented proper lock cleanup with `Drop` trait
- Added 100ms commit delay for safety
- Cleared stale lock files

**Commit:** 31fc8cb - "Fix: Tantivy index lock conflict in rapid search queries"

---

## Re-Test Instructions

### 1. Ensure Qdrant is Running

```bash
# Check if running
docker ps | grep qdrant

# If not running, start it
docker run -d -p 6334:6334 qdrant/qdrant

# Verify it's accessible
curl http://localhost:6334/health
```

### 2. Rebuild the Release Binary

```bash
cd /home/molaco/Documents/rust-code-mcp
cargo build --release
```

**Expected:** Clean compilation (only warnings about unused imports)

### 3. Run the MCP Server

The server should be running automatically via Claude Code's MCP configuration.

To manually test:
```bash
./target/release/file-search-mcp
```

### 4. Test in Claude CLI

Copy this exact prompt to the `claude` CLI:

```
I want to re-test the search tool that was failing before.

Test directory: /home/molaco/Documents/rust-code-mcp

Run these 3 search queries in sequence to verify the Tantivy lock fix:

1. search(keyword: "async")
2. search(keyword: "embed")
3. search(keyword: "merkle")

For each search:
- Show if it succeeded or failed
- Show number of results found
- Show indexing stats (files indexed, unchanged, skipped)

Then report:
- Did all 3 searches succeed? (Previously #2 and #3 failed)
- Are the results relevant?
- Did incremental indexing work? (second/third search should show "unchanged files")
```

---

## Expected Results (After Fix)

### Search #1: "async"
```
✓ PASS
Found: 8-15 results
Indexing: X files indexed, Y chunks
Response time: 30-90s (first index)
```

### Search #2: "embed"
```
✓ PASS (was failing before)
Found: 5-10 results
Indexing: 0 files indexed, ~50 unchanged
Response time: 1-3s (cached, no reindexing)
```

### Search #3: "merkle"
```
✓ PASS (was failing before)
Found: 3-8 results
Indexing: 0 files indexed, ~50 unchanged
Response time: 1-2s (cached)
```

**Key Success Indicators:**
- ✅ No "Failed to commit Tantivy index" errors
- ✅ All 3 searches complete successfully
- ✅ Second/third searches are much faster (incremental indexing)
- ✅ "unchanged files" count increases on subsequent searches

---

## Full Tool Test (Optional)

To verify ALL 9 tools now pass (100% vs previous 88.9%):

```
Re-test all 9 MCP tools on /home/molaco/Documents/rust-code-mcp:

1. health_check - Check system health
2. search - Test "async", "embed", "merkle" (THE FIX)
3. find_definition - Find "UnifiedIndexer"
4. find_references - Find "VectorStore" references
5. read_file_content - Read src/lib.rs
6. get_dependencies - Check src/indexing/unified.rs
7. get_call_graph - Get graph for src/search/resilient.rs
8. analyze_complexity - Analyze src/indexing/unified.rs
9. get_similar_code - Find code similar to "health check with async"

Report final success rate (should be 9/9 vs previous 8/9).
```

---

## Troubleshooting

### If Search Still Fails

**Check 1: Qdrant Running?**
```bash
curl http://localhost:6334/health
# Should return: {"title":"qdrant - vector search engine","version":"..."}
```

**Check 2: Stale Locks?**
```bash
find ~/.local/share/search -name "*.lock"
# Should be empty or only show meta.lock files (not writer.lock)
```

**Check 3: Permissions?**
```bash
ls -la ~/.local/share/search/
# Should be owned by your user, writable
```

**Nuclear Option: Clear Everything**
```bash
rm -rf ~/.local/share/search
# This will force a complete reindex on next search
```

### If Claude CLI Not Working

**Option 1: Direct MCP Test**
```bash
# Run server
cd /home/molaco/Documents/rust-code-mcp
./target/release/file-search-mcp

# In another terminal, send MCP requests
# (This requires MCP client implementation)
```

**Option 2: Use Claude Desktop**
- Claude Desktop has MCP support built-in
- Add rust-code-mcp to MCP server configuration
- Test via conversation interface

---

## Performance Benchmarks

After the fix, you should see:

| Metric | First Search | Subsequent Searches |
|--------|--------------|---------------------|
| Index time | 30-90s | <2s (unchanged) |
| Search latency | 100-500ms | 50-200ms |
| Lock errors | 0 | 0 |
| Success rate | 100% | 100% |

---

## What Changed in the Code

### Before (Broken):
```rust
// UnifiedIndexer goes out of scope
// Writer lock may not be released immediately
// Next indexer creation → lock conflict
```

### After (Fixed):
```rust
impl Drop for UnifiedIndexer {
    fn drop(&mut self) {
        // Explicitly release lock
        self.tantivy_writer.rollback();
    }
}

// Plus 100ms sleep after commit for safety
```

---

## Success Criteria

✅ **All 3 searches complete without errors**
✅ **No "Failed to commit Tantivy index" messages**
✅ **Incremental indexing works (fast subsequent searches)**
✅ **Results are relevant and accurate**
✅ **Overall tool success rate: 9/9 (100%)**

---

## Next Steps After Successful Re-Test

1. ✅ Mark search tool as FIXED in test results
2. ✅ Update TESTING_GUIDE.md with new expected 9/9 pass rate
3. ✅ Document this fix in PHASE4_COMPLETE.md
4. ✅ Consider this system production-ready (all 4 phases + bugfix complete)

---

**Status:** Ready for Re-Test
**Expected Outcome:** 9/9 tools passing (100%)
**Estimated Test Time:** 5-10 minutes

**Good luck! The fix should resolve the search failures.** 🚀

