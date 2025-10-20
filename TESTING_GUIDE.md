# Comprehensive MCP Tools Testing Guide

**Date:** 2025-10-20
**Purpose:** Test all 9 MCP tools in rust-code-mcp with a real codebase

---

## Prerequisites

1. **Qdrant Running:**
   ```bash
   docker run -d -p 6334:6334 qdrant/qdrant
   # OR
   docker start qdrant  # if already created
   ```

2. **Build Release Binary:**
   ```bash
   cd /home/molaco/Documents/rust-code-mcp
   cargo build --release
   ```

3. **Binary Location:**
   ```
   /home/molaco/Documents/rust-code-mcp/target/release/file-search-mcp
   ```

---

## Testing Prompt for Claude Code

Copy this prompt to test in Claude Code CLI (`claude`):

```
I want to test all 9 MCP tools from rust-code-mcp on a real Rust codebase.

Use the rust-code-mcp codebase itself as the test target:
Directory: /home/molaco/Documents/rust-code-mcp

Please test ALL 9 tools in this order:

1. health_check - Check system health
   - Test with no parameters (system-wide)
   - Test with directory parameter

2. search - Hybrid search test
   - Search for "async" keyword
   - Search for "embed" keyword
   - Search for "merkle" keyword

3. find_definition - Find symbol definitions
   - Find "UnifiedIndexer"
   - Find "HealthMonitor"
   - Find "ResilientHybridSearch"

4. find_references - Find symbol references
   - Find references to "VectorStore"
   - Find references to "Bm25Search"

5. read_file_content - Read specific files
   - Read src/lib.rs
   - Read PHASE4_COMPLETE.md

6. get_dependencies - Analyze imports
   - Check src/indexing/unified.rs
   - Check src/monitoring/health.rs

7. get_call_graph - Show function call relationships
   - Get call graph for src/search/resilient.rs
   - Get call graph for specific symbol "search" in resilient.rs

8. analyze_complexity - Code metrics
   - Analyze src/indexing/unified.rs
   - Analyze src/monitoring/health.rs

9. get_similar_code - Semantic search
   - Find code similar to "health check with async"
   - Find code similar to "backup management"

For each tool, show:
- The exact command you're using
- The result summary (not full output, just key findings)
- Whether it worked as expected

At the end, provide a summary table showing which tools passed/failed.
```

---

## Expected Results for Each Tool

### 1. health_check

**Command:**
```
health_check(directory: "/home/molaco/Documents/rust-code-mcp")
```

**Expected Output:**
```json
{
  "overall": "healthy" or "degraded",
  "bm25": {
    "status": "healthy",
    "message": "BM25 search operational",
    "latency_ms": 10-50
  },
  "vector": {
    "status": "healthy",
    "message": "Vector store operational (X vectors)",
    "latency_ms": 20-100
  },
  "merkle": {
    "status": "healthy" or "degraded",
    "message": "Merkle snapshot exists (X bytes)" or "not found"
  }
}
```

**Success Criteria:**
- ✅ Returns valid JSON
- ✅ Shows status for all 3 components (BM25, Vector, Merkle)
- ✅ Includes latency measurements for BM25 and Vector
- ✅ Overall status is Healthy or Degraded (not Unhealthy)

---

### 2. search (Hybrid Search)

**Command:**
```
search(directory: "/home/molaco/Documents/rust-code-mcp", keyword: "async")
```

**Expected Output:**
```
Found 5-15 results for 'async':

1. Score: 0.8543 | File: src/indexing/unified.rs | Symbol: index_file (Function)
   Lines: 217-315
   Doc: Index a single file to both Tantivy and Qdrant
   Preview:
   pub async fn index_file(&mut self, file_path: &Path) -> Result<IndexFileResult> {
       // 1. Check if file should be excluded (sensitive files)
       if !self.file_filter.should_index(file_path) {

2. Score: 0.7821 | File: src/monitoring/health.rs | Symbol: check_health (Function)
   ...

--- Indexing stats: X files indexed (Y chunks), Z unchanged, W skipped ---
```

**Success Criteria:**
- ✅ Returns 5-15 results (hybrid search working)
- ✅ Results have scores between 0.0-1.0
- ✅ Shows file path, symbol name, line numbers
- ✅ Shows preview of matched code
- ✅ Indexing stats at the end (proves incremental indexing)
- ✅ Second search for "async" should be much faster (unchanged files)

---

### 3. find_definition

**Command:**
```
find_definition(symbol_name: "UnifiedIndexer", directory: "/home/molaco/Documents/rust-code-mcp")
```

**Expected Output:**
```
Found 1 definition(s) for 'UnifiedIndexer':
- src/indexing/unified.rs:63 (Struct)
```

**Success Criteria:**
- ✅ Finds exactly 1 definition
- ✅ Correct file path
- ✅ Correct line number (~63)
- ✅ Identifies as Struct

**Test Cases:**
| Symbol | Expected File | Type |
|--------|---------------|------|
| UnifiedIndexer | src/indexing/unified.rs | Struct |
| HealthMonitor | src/monitoring/health.rs | Struct |
| ResilientHybridSearch | src/search/resilient.rs | Struct |

---

### 4. find_references

**Command:**
```
find_references(symbol_name: "VectorStore", directory: "/home/molaco/Documents/rust-code-mcp")
```

**Expected Output:**
```
Found 15-30 reference(s) to 'VectorStore' in 5-10 file(s):

Function Calls (X references):
- src/indexing/unified.rs (called by: index_to_qdrant, ...)
- src/search/hybrid.rs (called by: search, ...)

Type Usage (Y references):
- src/indexing/unified.rs (field 'vector_store' in struct UnifiedIndexer)
- src/monitoring/health.rs (parameter in new)
- src/search/resilient.rs (field 'vector_store' in struct ResilientHybridSearch)
```

**Success Criteria:**
- ✅ Finds 15-30 references
- ✅ Shows both function calls AND type usage
- ✅ Lists multiple files
- ✅ Separates call references from type references

---

### 5. read_file_content

**Command:**
```
read_file_content(file_path: "/home/molaco/Documents/rust-code-mcp/src/lib.rs")
```

**Expected Output:**
```
//! Rust Code MCP - Scalable code search for large Rust codebases
//!
//! Library modules for the MCP server

pub mod chunker;
pub mod embeddings;
pub mod indexing;
pub mod metadata_cache;
pub mod monitoring;
pub mod parser;
pub mod schema;
pub mod search;
pub mod security;
pub mod tools;
pub mod vector_store;
```

**Success Criteria:**
- ✅ Returns full file content
- ✅ Shows all module declarations
- ✅ Includes `pub mod monitoring;` (from Phase 4)

---

### 6. get_dependencies

**Command:**
```
get_dependencies(file_path: "/home/molaco/Documents/rust-code-mcp/src/indexing/unified.rs")
```

**Expected Output:**
```
Dependencies for 'src/indexing/unified.rs':

Imports (15-20):
- crate::chunker::{ChunkId, Chunker, CodeChunk}
- crate::embeddings::{Embedding, EmbeddingGenerator}
- crate::metadata_cache::MetadataCache
- crate::parser::RustParser
- crate::schema::ChunkSchema
- crate::security::secrets::SecretsScanner
- crate::security::SensitiveFileFilter
- crate::vector_store::VectorStore
- anyhow::{Context, Result}
- std::path::{Path, PathBuf}
- tantivy::{doc, Index, IndexWriter}
- tracing
- walkdir::WalkDir
```

**Success Criteria:**
- ✅ Lists 15-20 imports
- ✅ Shows both internal (crate::) and external imports
- ✅ Includes security imports (SecretsScanner, SensitiveFileFilter)
- ✅ Shows standard library imports (std::path)

---

### 7. get_call_graph

**Command:**
```
get_call_graph(file_path: "/home/molaco/Documents/rust-code-mcp/src/search/resilient.rs")
```

**Expected Output:**
```
Call graph for 'src/search/resilient.rs':

Functions: 8-12
Call relationships: 10-20

Call relationships:
new → []
with_defaults → [new]
search → [try_hybrid_search, fallback_search]
try_hybrid_search → [bm25_search, vector_search, merge_results]
bm25_search → []
vector_search → []
fallback_search → [bm25_search, vector_search]
merge_results → []
is_fallback_mode → []
```

**Success Criteria:**
- ✅ Shows 8-12 functions
- ✅ Shows 10-20 call relationships
- ✅ Correctly maps caller → callee relationships
- ✅ Shows that `search` calls `try_hybrid_search` and `fallback_search`

**Specific Symbol Test:**
```
get_call_graph(file_path: "src/search/resilient.rs", symbol_name: "search")
```

**Expected:**
```
Symbol: search

Calls (2):
  → try_hybrid_search
  → fallback_search

Called by (0-1):
  ← (external callers)
```

---

### 8. analyze_complexity

**Command:**
```
analyze_complexity(file_path: "/home/molaco/Documents/rust-code-mcp/src/indexing/unified.rs")
```

**Expected Output:**
```
Complexity analysis for 'src/indexing/unified.rs':

=== Code Metrics ===
Total lines:           512
Non-empty lines:       450
Comment lines:         80
Code lines (approx):   370

=== Symbol Counts ===
Functions:             12-15
Structs:               3-4
Traits:                0-1

=== Complexity ===
Total cyclomatic:      40-60
Avg per function:      3.5-5.0
Function calls:        20-30
```

**Success Criteria:**
- ✅ Total lines ~512
- ✅ Functions: 12-15
- ✅ Structs: 3-4 (IndexStats, IndexFileResult, UnifiedIndexer)
- ✅ Average complexity per function: 3-5 (reasonable)
- ✅ Shows function call count from call graph

---

### 9. get_similar_code

**Command:**
```
get_similar_code(query: "health check with async", directory: "/home/molaco/Documents/rust-code-mcp", limit: 5)
```

**Expected Output:**
```
Found 5 similar code snippet(s) for query 'health check with async':

1. Score: 0.8234 | File: src/monitoring/health.rs | Symbol: check_health (Function)
   Lines: 105-122
   Doc: Perform comprehensive health check
   Code preview:
   pub async fn check_health(&self) -> HealthStatus {
       let (bm25_health, vector_health, merkle_health) = tokio::join!(
           self.check_bm25(),

2. Score: 0.7543 | File: src/monitoring/health.rs | Symbol: check_vector (Function)
   Lines: 142-161
   Code preview:
   async fn check_vector(&self) -> ComponentHealth {
       let start = Instant::now();
       match vector_store.count().await {
```

**Success Criteria:**
- ✅ Returns 5 results
- ✅ Top result is from src/monitoring/health.rs
- ✅ Scores are between 0.0-1.0
- ✅ Results are semantically relevant to "health check"
- ✅ Shows async functions (proves semantic understanding)

**Second Test:**
```
get_similar_code(query: "backup management", directory: "/home/molaco/Documents/rust-code-mcp", limit: 5)
```

**Expected Top Results:**
- src/monitoring/backup.rs - BackupManager
- src/monitoring/backup.rs - create_backup
- src/monitoring/backup.rs - rotate_backups

---

## Complete Test Session Example

Here's what a complete test session looks like:

```bash
# 1. Start Qdrant
docker run -d -p 6334:6334 qdrant/qdrant

# 2. Run rust-code-mcp server
cd /home/molaco/Documents/rust-code-mcp
cargo run --release

# 3. In another terminal, use Claude CLI
claude "Test all 9 MCP tools on /home/molaco/Documents/rust-code-mcp"
```

---

## Expected Performance Benchmarks

Based on IMPL.md targets and Phase implementations:

| Operation | First Run | Second Run (Cached) | Target |
|-----------|-----------|---------------------|--------|
| **health_check** | 100-200ms | 50-100ms | <200ms ✅ |
| **search (first index)** | 30-120s | 1-5s | <2min for 100k LOC ✅ |
| **search (incremental)** | 5-15s | <1s | <5s ✅ |
| **find_definition** | 2-5s | 0.5-2s | Fast ✅ |
| **find_references** | 3-8s | 1-3s | Fast ✅ |
| **read_file_content** | <100ms | <50ms | Instant ✅ |
| **get_dependencies** | 1-3s | 0.5-1s | Fast ✅ |
| **get_call_graph** | 1-3s | 0.5-1s | Fast ✅ |
| **analyze_complexity** | 1-3s | 0.5-1s | Fast ✅ |
| **get_similar_code** | 2-10s | 1-3s | <200ms p95 ✅ |

**Notes:**
- First run includes indexing time (parsing, chunking, embedding, indexing)
- Second run uses cached metadata (Merkle tree detects no changes)
- Incremental runs only index changed files

---

## Success Criteria Summary

### All Tools Should:
- ✅ Return results without errors
- ✅ Show relevant, accurate information
- ✅ Complete within performance targets
- ✅ Handle the rust-code-mcp codebase itself

### Hybrid Search Should Show:
- ✅ Both BM25 and Vector scores contributing
- ✅ Semantic understanding (finds "async" in various contexts)
- ✅ Fast incremental updates (unchanged file detection)

### Health Monitoring Should Show:
- ✅ All 3 components (BM25, Vector, Merkle) checked
- ✅ Latency measurements
- ✅ Overall status calculation

### Phase 4 Features Should Work:
- ✅ health_check tool accessible
- ✅ Graceful degradation (test by stopping Qdrant)
- ✅ Backup creation (check ./storage/backups/)

---

## Troubleshooting Common Issues

### Issue 1: "Qdrant connection failed"
```
Error: Failed to connect to VectorStore: connection refused
```

**Solution:**
```bash
# Check if Qdrant is running
docker ps | grep qdrant

# Start Qdrant
docker run -d -p 6334:6334 qdrant/qdrant

# Check connection
curl http://localhost:6334/health
```

### Issue 2: "No results found for 'X'"
```
No results found for 'async'. Indexed 0 files.
```

**Solution:**
- First search always indexes (may take 30-120s)
- Wait for indexing to complete
- Check logs for errors during indexing

### Issue 3: "Merkle snapshot not found"
```
"merkle": {
  "status": "degraded",
  "message": "Merkle snapshot not found (first index pending)"
}
```

**Solution:**
- This is normal on first run
- Run any search to trigger indexing
- Merkle snapshot will be created automatically

### Issue 4: Slow performance
```
Search took 2 minutes (expected <5s)
```

**Solution:**
- Check if this is the first index (expected)
- Verify Qdrant is running locally (not remote)
- Check system resources (RAM, CPU)
- Look for "unchanged files" in output (should be most files on re-index)

---

## Validation Checklist

After testing all 9 tools, verify:

- [ ] **health_check**: Returns valid health status
- [ ] **search**: Finds relevant results with hybrid scores
- [ ] **find_definition**: Locates symbol definitions accurately
- [ ] **find_references**: Shows both calls and type usage
- [ ] **read_file_content**: Returns complete file contents
- [ ] **get_dependencies**: Lists all imports correctly
- [ ] **get_call_graph**: Shows accurate call relationships
- [ ] **analyze_complexity**: Provides reasonable metrics
- [ ] **get_similar_code**: Returns semantically similar code

**Production Readiness:**
- [ ] All 9 tools work without errors
- [ ] Performance meets targets
- [ ] Incremental indexing works (fast re-index)
- [ ] Health monitoring shows healthy status
- [ ] No crashes or panics during testing

---

## Example Test Results Report

```markdown
# rust-code-mcp Test Results

**Date:** 2025-10-20
**Codebase:** /home/molaco/Documents/rust-code-mcp
**Files:** ~50 Rust files, ~8000 LOC

## Test Summary

| Tool | Status | Response Time | Notes |
|------|--------|---------------|-------|
| health_check | ✅ Pass | 120ms | All components healthy |
| search | ✅ Pass | 45s (first), 2s (cached) | Found 12 results for "async" |
| find_definition | ✅ Pass | 1.2s | Found UnifiedIndexer |
| find_references | ✅ Pass | 2.5s | Found 23 refs to VectorStore |
| read_file_content | ✅ Pass | 50ms | Read src/lib.rs |
| get_dependencies | ✅ Pass | 1.1s | Found 18 imports |
| get_call_graph | ✅ Pass | 1.3s | 12 functions, 18 calls |
| analyze_complexity | ✅ Pass | 1.2s | 512 LOC, avg complexity 4.2 |
| get_similar_code | ✅ Pass | 3.5s | Found health check code |

## Overall Result: ✅ ALL TESTS PASSED

- First index: 45s (within 2min target)
- Incremental: 2s (within 5s target)
- All tools functional
- No errors or crashes
```

---

## Next Steps After Testing

If all tests pass:
1. ✅ **System is production-ready**
2. Test on larger codebases (100k+ LOC projects)
3. Deploy to production environment
4. Monitor health metrics in real-world usage
5. Gather user feedback

If any tests fail:
1. Check error logs in terminal
2. Verify Qdrant is running
3. Check disk space for index/cache
4. Review TROUBLESHOOTING section above
5. Create GitHub issue with error details

---

**Testing Status:** Ready for comprehensive validation
**Expected Duration:** 5-10 minutes for all 9 tools
**Success Rate:** Should be 9/9 tools passing

