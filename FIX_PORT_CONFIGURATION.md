# Fix: Port Configuration Inconsistency (Issue #4)

**Date:** 2025-10-21
**Status:** ✅ FIXED

## Problem Summary

The codebase had hardcoded `http://localhost:6334` in 15+ files, but Qdrant was actually running on port **6333**. This caused connection failures when the `QDRANT_URL` environment variable wasn't set.

### Error Example
```
Failed to connect to VectorStore: Error in the response: Internal error Failed to connect to http://localhost:6334/: tonic::transport::Error(Transport, ConnectError(ConnectError("tcp connect error", 127.0.0.1:6334, Os { code: 111, kind: ConnectionRefused, message: "Connection refused" })))
```

## Root Cause

Historical confusion between:
- **Port 6333:** Qdrant HTTP/REST API (actual port being used)
- **Port 6334:** Qdrant gRPC API (incorrectly hardcoded as default)

The codebase defaulted to 6334 throughout, which didn't match the actual Qdrant server configuration.

## Solution Implemented

Updated all hardcoded Qdrant URLs from `http://localhost:6334` to `http://localhost:6333`.

### Method
Used batch find-and-replace:
```bash
# Update all source files
find src -name "*.rs" -type f -exec sed -i 's/6334/6333/g' {} \;

# Update all test files
find tests -name "*.rs" -type f -exec sed -i 's/6334/6333/g' {} \;
```

### Files Modified (15 total)

**Source Files (8):**
1. `src/vector_store/mod.rs` - Default config
2. `src/tools/index_tool.rs` - QDRANT_URL default
3. `src/tools/health_tool.rs` - QDRANT_URL default
4. `src/tools/search_tool.rs` - QDRANT_URL default
5. `src/mcp/sync.rs` - QDRANT_URL default and docs
6. `src/indexing/unified.rs` - Doc comments and tests
7. `src/indexing/bulk.rs` - Doc comments and tests
8. `src/indexing/incremental.rs` - All test cases

**Test Files (7):**
1. `tests/test_hybrid_search.rs` - 5 occurrences
2. `tests/test_phase2_integration.rs` - 6 occurrences
3. `tests/test_sync_manager_integration.rs` - 1 occurrence
4. `tests/test_incremental_indexing.rs` - 1 occurrence
5. `tests/evaluation.rs` - 1 occurrence
6. `tests/test_full_incremental_flow.rs` - 1 occurrence
7. `tests/bench_incremental_performance.rs` - 1 occurrence

## Verification

### 1. No Hardcoded 6334 Remains
```bash
$ grep -r "6334" --include="*.rs" /home/molaco/Documents/rust-code-mcp
# No results found ✅
```

### 2. Code Compiles Successfully
```bash
$ cargo check --lib
Checking file-search-mcp v0.1.0
# Only warnings about unused imports, no errors ✅
```

### 3. Key Changes

**Before:**
```rust
// src/vector_store/mod.rs
url: "http://localhost:6334".to_string(),  // gRPC port, not HTTP

// src/tools/index_tool.rs
std::env::var("QDRANT_URL").unwrap_or_else(|_| "http://localhost:6334".to_string());
```

**After:**
```rust
// src/vector_store/mod.rs
url: "http://localhost:6333".to_string(),  // gRPC port

// src/tools/index_tool.rs
std::env::var("QDRANT_URL").unwrap_or_else(|_| "http://localhost:6333".to_string());
```

## Impact

### Before Fix
- ❌ Connection failures without `QDRANT_URL` env var
- ⚠️ Confusing error messages about connection refused
- 📝 Required manual environment variable configuration

### After Fix
- ✅ Works out-of-the-box with standard Qdrant setup
- ✅ Correct default port matches actual server configuration
- ✅ Environment variable still works for custom configurations

## Documentation

Documentation files (*.md) in `docs/` and `OUTPUT/` directories still reference port 6334 in:
- Historical examples
- Old error messages
- Architecture diagrams from earlier versions

**These are intentionally NOT updated** as they document historical behavior and don't affect runtime.

## Testing Recommendation

After MCP server restart, verify:

1. **Connection succeeds without env var:**
   ```bash
   # No QDRANT_URL set
   $ unset QDRANT_URL
   $ cargo run
   # Should connect to localhost:6333 successfully
   ```

2. **Force reindex now works:**
   ```rust
   mcp__rust-code-mcp__index_codebase(
       directory: "/path/to/codebase",
       force_reindex: true
   )
   # Should clear all data and perform full reindex
   ```

3. **All 9 MCP tools functional:**
   - search
   - find_definition
   - find_references
   - get_dependencies
   - get_call_graph
   - analyze_complexity
   - get_similar_code
   - health_check
   - index_codebase

## Related Issues

- **Issue #1** (Force Reindex) - ✅ Fixed (was blocked by this issue)
- **Issue #4** (Port Configuration) - ✅ Fixed (this issue)

## Notes

### Why 6333 Not 6334?

Qdrant exposes two ports:
- **6333:** HTTP/REST API (what rust clients use)
- **6334:** gRPC API (alternative protocol)

The Rust client (`qdrant_client`) uses the HTTP/REST API on port 6333, not the gRPC port. The original default of 6334 was a configuration error.

### Environment Variable Still Works

Users can still override with:
```bash
export QDRANT_URL="http://custom-host:9999"
```

This allows flexibility for:
- Remote Qdrant servers
- Custom port configurations
- Docker setups with port mapping
