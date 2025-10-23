# Strategic Optimization Analysis: rust-code-mcp Indexing Pipeline

## Executive Summary

The current indexing implementation is **functionally correct** but has significant performance headroom. For the Burn codebase (1,569 files, 19,075 chunks), indexing took **546 seconds (~9 minutes)**. With strategic optimizations, this could be reduced to **90-150 seconds (1.5-2.5 minutes)** - a **3.6-6x speedup**.

## Critical Performance Bottlenecks

### 1. **Sequential File Processing** (HIGHEST IMPACT)
**Location:** `src/indexing/unified.rs:381-398`

**Current Behavior:**
```rust
for file in rust_files {
    match self.index_file(&file).await {
        // Process one file at a time
    }
}
```

**Problem:**
- 1,569 files processed sequentially
- Each file: read (IO) → parse (CPU) → chunk (CPU) → embed (GPU/CPU) → index (IO)
- Different stages use different resources, wasting parallelization opportunities
- Modern CPUs have 8-16+ cores sitting idle

**Impact:** **40-60% of total time**

**Optimization Opportunity:**
- Parallel file processing with `tokio::spawn` or `rayon`
- Target: 8-12 concurrent file indexing tasks
- Expected speedup: **3-5x** on multi-core systems

---

### 2. **Bulk Indexing Mode Not Used** (HIGH IMPACT)
**Location:** `src/indexing/bulk.rs` (exists but unused!)

**Current Behavior:**
- HNSW index built incrementally during insertion
- Each batch of vectors triggers graph updates
- For 19,075 chunks, this means thousands of HNSW update operations

**Problem:**
The code has a `BulkIndexer` implementation that can disable HNSW during bulk operations and rebuild once at the end, providing **3-5x speedup**, but it's never actually used in the indexing pipeline!

**Impact:** **20-30% of total time** (for force reindex scenarios)

**Optimization Opportunity:**
- Use `BulkIndexer::start_bulk_mode()` before force reindex
- Disable HNSW temporarily
- Insert all vectors
- Call `BulkIndexer::end_bulk_mode()` to rebuild HNSW once
- Expected speedup: **3-5x for Qdrant insertion phase**

---

### 3. **Synchronous File I/O** (MEDIUM IMPACT)
**Location:** `src/indexing/unified.rs:225`

**Current Behavior:**
```rust
let content = std::fs::read_to_string(file_path)?;
```

**Problem:**
- Blocking I/O on every file read
- Thread blocked while waiting for disk
- Tokio runtime underutilized

**Impact:** **5-10% of total time**

**Optimization Opportunity:**
```rust
let content = tokio::fs::read_to_string(file_path).await?;
```
- Non-blocking async I/O
- Better resource utilization
- Works well with parallel processing

---

### 4. **Small Embedding Batch Sizes** (MEDIUM IMPACT)
**Location:** `src/indexing/unified.rs:269-277`

**Current Behavior:**
- Embeddings batched **per file** (typically 5-20 chunks per file)
- Small batches = poor GPU utilization for embedding model
- `EmbeddingPipeline` has configurable batch size (default: 32) but not used in main pipeline

**Problem:**
```rust
// Current: batch per file
let embeddings = self.embedding_generator.embed_batch(chunk_texts)?;
```

For a file with 10 chunks, we're only batching 10 texts. The embedding model (fastembed with ONNX) performs **much better** with larger batches (64-128).

**Impact:** **10-15% of total time**

**Optimization Opportunity:**
- Accumulate chunks across multiple files
- Batch embeddings in groups of 64-128
- Expected speedup: **1.5-2x for embedding phase**

---

## Secondary Optimizations

### 5. **Memory Allocation Patterns**
**Location:** `src/indexing/unified.rs:348-352`

**Current:**
```rust
let chunk_data: Vec<(ChunkId, Embedding, CodeChunk)> = chunks
    .iter()
    .zip(embeddings.into_iter())
    .map(|(chunk, embedding)| (chunk.id, embedding, chunk.clone()))  // ← Clone!
    .collect();
```

**Problem:** Clones entire `CodeChunk` (includes String content, PathBuf, Vec<String> for imports/calls)

**Impact:** **2-3% of total time**

**Optimization:**
```rust
// Use Arc to avoid clones
type SharedChunk = Arc<CodeChunk>;
```

---

### 6. **Qdrant Batch Size Tuning**
**Location:** `src/vector_store/mod.rs:218`

**Current:** Fixed 100 vectors per batch
```rust
for batch in points.chunks(100) {
    self.client.upsert_points(...).await?;
}
```

**Optimization:**
- Tune based on codebase size
- Small (<10k chunks): 50
- Medium (10k-50k): 100 (current)
- Large (>50k): 200-500

**Impact:** **3-5% of total time**

---

### 7. **Tantivy Writer Configuration**
**Location:** `src/indexing/unified.rs:160-164`

**Current Memory Budget:**
- Small (<100k LOC): 50MB × 2 threads = 100MB
- Medium (100k-1M LOC): 100MB × 4 threads = 400MB  ← Burn is here
- Large (>1M LOC): 200MB × 8 threads = 1600MB

**Optimization:**
- Increase memory budget for medium codebases to 200MB × 4 = 800MB
- More aggressive memory use = fewer segment merges
- Expected speedup: **1.2-1.3x for Tantivy indexing**

---

### 8. **Change Detection Parallelization**
**Location:** `src/indexing/incremental.rs:175-233`

**Current:** Sequential processing of added/modified/deleted files

**Optimization:**
```rust
// Process modified files in parallel
let modified_futures: Vec<_> = changes.modified.iter()
    .map(|path| tokio::spawn(async move {
        self.indexer.index_file(path).await
    }))
    .collect();
```

**Impact:** **Only matters for incremental updates** (less critical for full reindex)

---

## Strategic Implementation Plan

### Phase 1: Low-Hanging Fruit (Quick Wins)
**Estimated effort:** 4-6 hours
**Expected speedup:** 1.5-2x

1. **Enable bulk indexing mode for force reindex**
   - Already implemented, just needs integration
   - Modify `src/tools/index_tool.rs` to detect force reindex and use `BulkIndexer`

2. **Increase Tantivy memory budgets**
   - Simple config changes in `unified.rs:148-158`

3. **Tune Qdrant batch sizes**
   - Add logic to select batch size based on estimated chunks

---

### Phase 2: Parallel File Processing (Biggest Impact)
**Estimated effort:** 1-2 days
**Expected speedup:** 3-5x (cumulative with Phase 1)

**Implementation approaches:**

**Option A: Task-based parallelism (Tokio)**
```rust
pub async fn index_directory_parallel(&mut self, dir_path: &Path, concurrency: usize) -> Result<IndexStats> {
    let rust_files: Vec<PathBuf> = /* find files */;

    let mut tasks = FuturesUnordered::new();
    let semaphore = Arc::new(Semaphore::new(concurrency)); // Limit concurrent tasks

    for file in rust_files {
        let permit = semaphore.clone().acquire_owned().await?;
        let indexer = self.clone_for_parallel(); // Need thread-safe version

        tasks.push(tokio::spawn(async move {
            let _permit = permit; // Hold permit
            indexer.index_file(&file).await
        }));
    }

    // Collect results
    while let Some(result) = tasks.next().await {
        // Aggregate stats
    }
}
```

**Option B: Thread pool (Rayon)**
```rust
use rayon::prelude::*;

let results: Vec<_> = rust_files
    .par_iter()
    .map(|file| {
        // Need to handle async/sync boundary
        tokio::runtime::Handle::current().block_on(
            self.index_file(file)
        )
    })
    .collect();
```

**Challenges:**
- `UnifiedIndexer` has mutable state (`tantivy_writer`)
- Need to either:
  - Clone/share indexer components safely
  - Use channels to send index operations to single writer
  - Use lock-free data structures

**Recommended Approach:**
- Use **tokio tasks** with **message passing**
- Parallel workers do: read → parse → chunk → embed
- Send results to single writer task for Tantivy/Qdrant indexing
- This avoids complex locking

---

### Phase 3: Advanced Optimizations (Polish)
**Estimated effort:** 2-3 days
**Expected speedup:** 1.2-1.5x (cumulative)

1. **Cross-file embedding batching**
   - Accumulate chunks from multiple files
   - Batch embeddings in groups of 64-128

2. **Async file I/O**
   - Replace `std::fs` with `tokio::fs`

3. **Memory optimization**
   - Use `Arc<CodeChunk>` to avoid clones

4. **Progressive commits**
   - Commit Tantivy every N files instead of at end

---

## Benchmark Expectations

### Current Performance (Burn codebase)
- Files: 1,569
- Chunks: 19,075
- Time: **546 seconds (9.1 minutes)**
- Throughput: 2.87 files/sec, 34.9 chunks/sec

### After Phase 1 (Bulk mode + tuning)
- Expected: **273-364 seconds (4.5-6 minutes)**
- Speedup: **1.5-2x**
- Effort: 4-6 hours

### After Phase 2 (Parallel processing)
- Expected: **91-136 seconds (1.5-2.3 minutes)**
- Speedup: **4-6x total**
- Effort: 1-2 days

### After Phase 3 (Polish)
- Expected: **73-109 seconds (1.2-1.8 minutes)**
- Speedup: **5-7.5x total**
- Effort: 2-3 days additional

---

## Risk Analysis

### Low Risk
- Phase 1 optimizations (config changes, bulk mode)
- Very low risk of breaking existing functionality

### Medium Risk
- Parallel file processing
- Need careful handling of shared state
- More complex error handling
- Requires thorough testing

### Testing Strategy
1. Preserve existing tests (all should still pass)
2. Add integration test comparing sequential vs parallel results
3. Benchmark on multiple codebase sizes (small/medium/large)
4. Test error scenarios (concurrent failures, disk full, etc.)

---

## Additional Considerations

### Incremental Indexing Performance
Current incremental performance is **excellent** (< 10ms for no changes). The optimizations above mainly target:
- **Force reindex** scenarios
- **First-time indexing**
- **Large change sets** (e.g., after git branch switch)

For typical incremental updates (1-5 modified files), the current implementation is already optimal.

### Resource Usage
**Current:** Conservative memory usage, underutilizes CPU/GPU

**After optimization:** Will use more resources:
- **CPU:** Higher utilization (good!)
- **Memory:** +200-500MB peak usage
- **GPU/ONNX:** Better batch utilization
- **Network:** More concurrent Qdrant connections

Make sure to document system requirements.

---

## Recommendation

**Start with Phase 1** - it provides 1.5-2x speedup with minimal risk and effort. If that's sufficient, stop there.

If you need more performance (e.g., for very large codebases or CI/CD pipelines), proceed to **Phase 2** for the biggest gains.

**Phase 3** is optional polish - only pursue if you need to squeeze out the last bit of performance.
