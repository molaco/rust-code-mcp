# Phase 0: Quick Wins Implementation Plan

**Project**: rust-code-mcp indexing pipeline optimization
**Phase**: 0 - Quick Wins
**Estimated Effort**: 2-4 hours
**Expected Improvement**: 1.2-1.3x (626s → 500-530s)
**Risk Level**: Very Low

---

## Overview

Phase 0 focuses on **low-risk, high-impact changes** that provide immediate performance improvements without major architectural changes. These optimizations lay the groundwork for more advanced parallelization in later phases.

### Goals

1. ✅ Remove artificial delays (100ms sleep)
2. ✅ Enable BulkIndexer for 3-5x Qdrant speedup on force reindex
3. ✅ Eliminate unnecessary memory allocations (clone removal)
4. ✅ Set up mode selection infrastructure for Phase 1+

### Prerequisites

- Qdrant running at http://localhost:6334
- Rust toolchain installed
- All tests passing before starting

---

## Task 0.1: Remove 100ms Artificial Sleep ⚡

**Priority**: P0 (Critical - Do First!)
**Effort**: 5 minutes
**Impact**: Instant 100ms savings per indexing operation
**Risk**: None

### Problem

An unnecessary 100ms sleep was added after Tantivy commits, likely for testing. Since `commit()` is synchronous and completes before returning, this delay serves no purpose.

### Location

`src/indexing/unified.rs:404`

### Current Code

```rust
// Commit Tantivy changes
self.tantivy_writer.commit().context("Failed to commit Tantivy index")?;

// Wait briefly to ensure index is fully committed
tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
```

### Implementation

**Step 1**: Remove the sleep line

```rust
// Commit Tantivy changes
self.tantivy_writer.commit().context("Failed to commit Tantivy index")?;

// Note: No sleep needed - commit() is synchronous and completes before returning
```

**Step 2**: Verify the change

```bash
git diff src/indexing/unified.rs
```

### Testing

```bash
# Run integration tests
cargo test --test test_phase4_integration -- --nocapture

# Verify indexing still works
cargo run -- index /path/to/small-codebase
```

### Verification

- ✅ All tests pass
- ✅ Indexing completes successfully
- ✅ Timing shows ~100ms reduction

### Commit Message

```
perf: Remove unnecessary 100ms sleep after Tantivy commit

The sleep was added for testing but is not needed since
Tantivy's commit() is synchronous and blocks until complete.

This saves 100ms per indexing operation.
```

---

## Task 0.2: Integrate BulkIndexer for Force Reindex 🔥

**Priority**: P0
**Effort**: 2-3 hours
**Impact**: 3-5x speedup for Qdrant insertion during force reindex
**Risk**: Low (only affects force reindex path)

### Problem

`src/indexing/bulk.rs` contains a complete `BulkIndexer` implementation that:
- Disables HNSW graph construction during bulk operations
- Defers indexing optimization
- Rebuilds HNSW once at the end

**But it's never used in production code!**

### Background

When inserting thousands of vectors, building the HNSW graph incrementally is slow. Disabling HNSW, inserting all vectors, then rebuilding the graph is 3-5x faster.

### Location

`src/tools/index_tool.rs:98-138`

### Implementation

**Step 1**: Add BulkIndexer initialization before clearing data

Insert after line 110 (after creating `indexer`):

```rust
.await
.map_err(|e| {
    McpError::invalid_params(format!("Failed to initialize indexer: {}", e), None)
})?;

// Enable bulk mode if force reindexing
let mut bulk_indexer = if force {
    use crate::indexing::bulk::{BulkIndexer, HnswConfig};

    tracing::info!("Force reindex: enabling bulk indexing mode for 3-5x speedup");

    // Create Qdrant client
    let qdrant_client = qdrant_client::Qdrant::from_url(&qdrant_url)
        .build()
        .map_err(|e| McpError::invalid_params(format!("Failed to connect to Qdrant: {}", e), None))?;

    let mut bulk_indexer = BulkIndexer::new(qdrant_client, collection_name.clone());

    // Start bulk mode with standard HNSW config
    let hnsw_config = HnswConfig::new(16, 100);
    bulk_indexer.start_bulk_mode(hnsw_config).await
        .map_err(|e| McpError::invalid_params(format!("Failed to start bulk mode: {}", e), None))?;

    Some(bulk_indexer)
} else {
    None
};

// Clear all indexed data if force reindex
if force {
    tracing::info!("Force reindex: clearing all indexed data (metadata cache, Tantivy, Qdrant)");
    indexer.clear_all_data().await.map_err(|e| {
        McpError::invalid_params(format!("Failed to clear indexed data: {}", e), None)
    })?;
}
```

**Step 2**: Exit bulk mode after indexing completes

Insert after line 126 (after timing):

```rust
let elapsed = start.elapsed();

// Exit bulk mode if it was enabled
if let Some(mut bulk_indexer) = bulk_indexer {
    tracing::info!("Rebuilding HNSW index after bulk insertion...");
    bulk_indexer.end_bulk_mode().await
        .map_err(|e| McpError::invalid_params(format!("Failed to exit bulk mode: {}", e), None))?;
    tracing::info!("✓ HNSW index rebuilt");
}
```

**Step 3**: Update variable declaration to make it mutable

Change the initialization line to:
```rust
let mut bulk_indexer = if force { ... }
```

### Testing

```bash
# Test force reindex with bulk mode
cargo test --test test_phase2_integration -- test_bulk_indexing_mode --ignored --nocapture

# Real-world test on Burn codebase
cargo run -- index /path/to/burn --force-reindex
```

### Verification

Check logs for:
```
Force reindex: enabling bulk indexing mode for 3-5x speedup
⚡ Entering bulk indexing mode for collection 'code_chunks_...'
✓ Bulk mode active - HNSW disabled, optimizations deferred
...
Rebuilding HNSW index after bulk insertion...
🔄 Exiting bulk mode, rebuilding HNSW index for '...'
✓ HNSW indexing restored - collection ready for queries
```

### Expected Results

- Force reindex should be **3-5x faster** for Qdrant insertion phase
- Non-force reindex should work exactly as before
- HNSW index properly rebuilt at end

### Commit Message

```
feat: Enable BulkIndexer for 3-5x speedup on force reindex

Integrate the existing BulkIndexer implementation to disable
HNSW graph construction during bulk operations and rebuild
once at the end.

This provides 3-5x speedup for Qdrant insertion during
force reindex scenarios.

Related: OPTI2 finding - BulkIndexer exists but was unused
```

---

## Task 0.3: Remove Unnecessary CodeChunk Clones

**Priority**: P0
**Effort**: 30 minutes
**Impact**: 10-15% reduction in memory allocations
**Risk**: Low (ownership change)

### Problem

The `index_to_qdrant()` function clones every `CodeChunk` unnecessarily:

```rust
.map(|(chunk, embedding)| (chunk.id, embedding, chunk.clone())) // ❌ Clone!
```

Each clone copies:
- String content (~100-500 bytes)
- PathBuf (~50 bytes)
- Vec<String> for imports/calls (~100-200 bytes)

For 19,000 chunks, this adds up to significant overhead.

### Location

`src/indexing/unified.rs:348-352`

### Implementation

**Step 1**: Change function signature to take ownership

```rust
// Before
async fn index_to_qdrant(
    &self,
    chunks: &[CodeChunk],  // Borrow
    embeddings: Vec<Embedding>,
) -> Result<()> {

// After
async fn index_to_qdrant(
    &self,
    chunks: Vec<CodeChunk>,  // Take ownership
    embeddings: Vec<Embedding>,
) -> Result<()> {
```

**Step 2**: Update implementation to consume chunks

```rust
let chunk_data: Vec<(ChunkId, Embedding, CodeChunk)> = chunks
    .into_iter()  // Consume instead of iterate
    .zip(embeddings.into_iter())
    .map(|(chunk, embedding)| (chunk.id, embedding, chunk))  // No clone!
    .collect();

self.vector_store
    .upsert_chunks(chunk_data)
    .await
    .map_err(|e| anyhow::anyhow!("Failed to index to Qdrant: {}", e))?;

Ok(())
```

**Step 3**: Update caller (line 292)

```rust
// Before
self.index_to_tantivy(&chunks)?;
self.index_to_qdrant(&chunks, embeddings).await?;

// After
self.index_to_tantivy(&chunks)?;
self.index_to_qdrant(chunks, embeddings).await?;  // Move chunks
```

**Note**: This works because `index_to_tantivy()` only needs a borrow and is called first.

### Testing

```bash
# Unit tests
cargo test indexing::unified::tests::test_index_file

# Integration tests
cargo test --test test_phase4_integration
```

### Verification

- ✅ All tests pass
- ✅ No compilation errors about moved values
- ✅ Memory profiling shows reduced allocations (optional)

### Commit Message

```
perf: Remove unnecessary CodeChunk clones in index_to_qdrant

Change function signature to take ownership instead of borrowing,
eliminating 19,000+ unnecessary clones for large codebases.

Expected: 10-15% reduction in memory allocations during indexing.
```

---

## Task 0.4: Set Up Mode Selection Infrastructure

**Priority**: P0 (Foundation for Phase 1+)
**Effort**: 1-2 hours
**Impact**: Enables runtime mode selection
**Risk**: Low (additive changes)

### Goal

Add the ability to select indexing mode via MCP tool parameter, preparing for Phase 1 (Parallel) and Phase 2 (Pipeline) implementations.

### Implementation

**Step 1**: Add IndexingMode enum

Create in `src/tools/index_tool.rs`:

```rust
#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum IndexingMode {
    /// Sequential processing (baseline for testing/debugging)
    Sequential,
    /// File-level parallelism - simpler, lower memory, good for most codebases
    Parallel,
    /// Streaming pipeline - maximum performance for very large codebases
    Pipeline,
}

impl Default for IndexingMode {
    fn default() -> Self {
        Self::Sequential  // Start with sequential, change to Parallel in Phase 1
    }
}
```

**Step 2**: Add mode parameter to IndexCodebaseParams

```rust
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct IndexCodebaseParams {
    #[schemars(description = "Absolute path to codebase directory")]
    pub directory: String,

    #[schemars(description = "Force full reindex even if already indexed (default: false)")]
    pub force_reindex: Option<bool>,

    #[schemars(description = "Indexing mode: 'sequential' (default), 'parallel', or 'pipeline'")]
    pub indexing_mode: Option<IndexingMode>,
}
```

**Step 3**: Rename current index_directory() in unified.rs

```rust
impl UnifiedIndexer {
    // Rename existing method
    pub async fn index_directory_sequential(&mut self, dir_path: &Path) -> Result<IndexStats> {
        // Current implementation (lines 363-415)
        // ... existing code ...
    }

    // Add new dispatcher (for now just calls sequential)
    pub async fn index_directory_with_mode(
        &mut self,
        dir_path: &Path,
        mode: IndexingMode
    ) -> Result<IndexStats> {
        match mode {
            IndexingMode::Sequential => {
                tracing::info!("Using sequential indexing mode");
                self.index_directory_sequential(dir_path).await
            }
            IndexingMode::Parallel => {
                tracing::warn!("Parallel mode not yet implemented, using sequential");
                self.index_directory_sequential(dir_path).await
            }
            IndexingMode::Pipeline => {
                tracing::warn!("Pipeline mode not yet implemented, using sequential");
                self.index_directory_sequential(dir_path).await
            }
        }
    }

    // Keep old method for backward compatibility (temporary)
    pub async fn index_directory(&mut self, dir_path: &Path) -> Result<IndexStats> {
        self.index_directory_sequential(dir_path).await
    }
}
```

**Step 4**: Update index_codebase tool to use mode parameter

In `src/tools/index_tool.rs`, update the indexing call:

```rust
// Get mode from params or use default
let mode = params.indexing_mode.unwrap_or_default();
tracing::info!("Indexing mode: {:?}", mode);

// Run incremental indexing with mode selection
let start = std::time::Instant::now();
let stats = indexer
    .index_with_change_detection_mode(&dir, mode)  // New method
    .await
    .map_err(|e| McpError::invalid_params(format!("Indexing failed: {}", e), None))?;
```

**Step 5**: Update IncrementalIndexer to support mode

In `src/indexing/incremental.rs`:

```rust
pub async fn index_with_change_detection_mode(
    &mut self,
    codebase_path: &Path,
    mode: IndexingMode,
) -> Result<IndexStats> {
    // ... existing change detection logic ...

    // Use mode when calling indexer
    self.indexer.index_directory_with_mode(codebase_path, mode).await
}
```

### Testing

```bash
# Test that mode parameter is accepted
cargo test index_tool::tests

# Verify default mode works
cargo run -- index /path/to/codebase

# Verify explicit mode works (should warn about unimplemented)
cargo run -- index /path/to/codebase --mode parallel
```

### Verification

- ✅ Code compiles
- ✅ Mode parameter accepted via MCP tool
- ✅ Warning logged for unimplemented modes
- ✅ Sequential mode works as before

### Commit Message

```
feat: Add indexing mode selection infrastructure

Add IndexingMode enum with Sequential/Parallel/Pipeline options.
Rename index_directory() to index_directory_sequential() and
add index_directory_with_mode() dispatcher.

Parallel and Pipeline modes not yet implemented - fallback to
sequential with warning.

Prepares for Phase 1 (Parallel) and Phase 2 (Pipeline).
```

---

## Phase 0 Testing Strategy

### Unit Tests

```bash
# Test each changed component
cargo test indexing::unified::tests
cargo test tools::index_tool::tests
cargo test indexing::bulk::tests
```

### Integration Tests

```bash
# Phase 4 tests (should still pass)
cargo test --test test_phase4_integration -- --nocapture

# Bulk indexing tests
cargo test --test test_phase2_integration -- test_bulk_indexing_mode --ignored --nocapture
```

### Manual Testing

**Test 1**: Small codebase (baseline)
```bash
time cargo run --release -- index /path/to/rust-code-mcp
```

**Test 2**: Force reindex with BulkIndexer
```bash
time cargo run --release -- index /path/to/burn --force-reindex
```

**Test 3**: Verify mode parameter
```bash
cargo run -- index /path/to/codebase --mode sequential
cargo run -- index /path/to/codebase --mode parallel  # Should warn
```

### Benchmarking

**Before Phase 0**:
```bash
# Baseline timing
time cargo run --release -- index /path/to/burn --force-reindex
# Expected: ~626 seconds
```

**After Phase 0**:
```bash
# With optimizations
time cargo run --release -- index /path/to/burn --force-reindex
# Expected: ~500-530 seconds (1.2-1.3x faster)
```

### Performance Metrics

Track:
- Total indexing time
- Qdrant insertion time (should be 3-5x faster on force reindex)
- Memory usage (should be 10-15% lower)
- All tests still passing

---

## Success Criteria

### Phase 0 Complete When:

- ✅ Task 0.1: 100ms sleep removed, all tests pass
- ✅ Task 0.2: BulkIndexer integrated, force reindex 3-5x faster on Qdrant phase
- ✅ Task 0.3: Clones removed, no compilation errors
- ✅ Task 0.4: Mode selection infrastructure in place
- ✅ All unit tests passing
- ✅ All integration tests passing
- ✅ Burn indexing: 626s → 500-530s (15-20% improvement)
- ✅ Memory usage < 500MB
- ✅ No regression in search quality

### Quality Gates

Before proceeding to Phase 1:

1. ✅ All tests green
2. ✅ Benchmark confirms 1.2-1.3x speedup
3. ✅ Code review completed
4. ✅ Documentation updated
5. ✅ Commits have clear messages
6. ✅ No memory leaks detected

---

## Timeline

### Day 1 (2-3 hours)
- **Morning**: Task 0.1 (5 min) + Task 0.3 (30 min) + testing (1 hour)
- **Afternoon**: Task 0.2 (2-3 hours) + testing

### Day 2 (1-2 hours)
- **Morning**: Task 0.4 (1-2 hours)
- **Afternoon**: Integration testing, benchmarking

### Day 3 (Optional, if issues)
- Bug fixes, additional testing

**Total**: 2-4 hours of actual implementation

---

## Dependencies

None - Phase 0 uses existing code and libraries.

---

## Rollback Plan

Each task is independent and can be reverted individually:

**Task 0.1**: Re-add sleep line if issues discovered
**Task 0.2**: Remove BulkIndexer logic, force reindex will work (just slower)
**Task 0.3**: Revert to clone-based implementation
**Task 0.4**: Remove mode enum, keep sequential only

---

## Next Steps

After Phase 0 completion:

1. Review performance improvements
2. Benchmark on multiple codebases
3. Document findings in CHANGELOG.md
4. Proceed to **PHASE_1.md** (Parallel Mode Implementation)

---

## References

- Main plan: `SPEED.md`
- Analysis: `OPTI1.md`, `OPTI2.md`, `OPTI3.md`, `LIST.md`
- Next phase: `PHASE_1.md`
- BulkIndexer: `src/indexing/bulk.rs`

---

**Document Version**: 1.0
**Last Updated**: 2025-10-22
**Status**: ✅ Ready for Implementation
