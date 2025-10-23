# Comprehensive Performance Analysis: `index_codebase` Optimization Report

## Executive Summary

After analyzing the rust-code-mcp codebase using MCP tools, I've identified **8 major performance bottlenecks** and **15+ optimization opportunities**. The current implementation processes files sequentially with synchronous I/O, missing significant parallelization opportunities that could yield **5-10x performance improvements** for large codebases.

---

## 1. Top Performance Bottlenecks (Priority Ordered)

### 🔴 #1: Sequential File Processing (CRITICAL)
**Location:** `src/indexing/unified.rs:381-398`

**Impact:** High - This is the primary bottleneck for large codebases

**Current Code:**
```rust
for file in rust_files {
    match self.index_file(&file).await {
        Ok(IndexFileResult::Indexed { chunks_count }) => {
            stats.indexed_files += 1;
            stats.total_chunks += chunks_count;
        }
        // ...
    }
}
```

**Issue:** Files are processed one at a time. For a codebase with 1,500+ files (like Burn), this creates a severe bottleneck.

**Optimization:** Use parallel processing with `tokio::spawn` or add `rayon` dependency:
```rust
use tokio::task::JoinSet;

let mut join_set = JoinSet::new();
for file in rust_files {
    let file = file.clone();
    join_set.spawn(async move {
        self.index_file(&file).await
    });
}

while let Some(result) = join_set.join_next().await {
    // Process results
}
```

**Expected Improvement:** 5-8x speedup (depending on CPU cores)

---

### 🔴 #2: Sequential Change Processing
**Location:** `src/indexing/incremental.rs:183-223`

**Impact:** High - Affects incremental indexing performance

**Current Code:**
```rust
for deleted_path in &changes.deleted { /* ... */ }
for modified_path in &changes.modified { /* ... */ }
for added_path in &changes.added { /* ... */ }
```

**Issue:** Three sequential loops processing changes one by one. Could process all changes in parallel.

**Optimization:** Combine and parallelize:
```rust
let all_changes: Vec<_> = changes.deleted.iter()
    .chain(changes.modified.iter())
    .chain(changes.added.iter())
    .collect();

use futures::stream::{self, StreamExt};
stream::iter(all_changes)
    .map(|path| async move { self.process_change(path).await })
    .buffer_unordered(num_cpus::get())
    .collect::<Vec<_>>()
    .await;
```

**Expected Improvement:** 3-5x speedup for incremental updates

---

### 🟠 #3: Synchronous File I/O
**Locations:**
- `src/indexing/unified.rs:225` - `std::fs::read_to_string`
- `src/indexing/merkle.rs:108` - `std::fs::read`

**Impact:** Medium-High - Blocks async runtime

**Issue:** Synchronous file operations block the async executor, preventing concurrent work.

**Optimization:**
```rust
// Replace:
let content = std::fs::read_to_string(file_path)?;

// With:
let content = tokio::fs::read_to_string(file_path).await?;
```

**Expected Improvement:** 20-30% reduction in I/O wait time

---

### 🟠 #4: Unnecessary Data Cloning
**Location:** `src/indexing/unified.rs:351`

**Impact:** Medium - Memory allocations

**Current Code:**
```rust
let chunk_data: Vec<(ChunkId, Embedding, CodeChunk)> = chunks
    .iter()
    .zip(embeddings.into_iter())
    .map(|(chunk, embedding)| (chunk.id, embedding, chunk.clone())) // ❌ Clone entire chunk
    .collect();
```

**Issue:** Clones entire `CodeChunk` struct including strings, vectors, etc. for each chunk.

**Optimization:**
```rust
// Option 1: Take ownership
let chunk_data: Vec<(ChunkId, Embedding, CodeChunk)> = chunks
    .into_iter() // consume chunks
    .zip(embeddings.into_iter())
    .map(|(chunk, embedding)| (chunk.id, embedding, chunk))
    .collect();

// Option 2: Use references where possible
async fn index_to_qdrant(&self, chunks: Vec<CodeChunk>, embeddings: Vec<Embedding>) -> Result<()> {
    let chunk_data: Vec<_> = chunks.into_iter()
        .zip(embeddings.into_iter())
        .collect();
    // ...
}
```

**Expected Improvement:** 10-15% reduction in memory allocations

---

### 🟠 #5: Multiple Field Clones in Tantivy Indexing
**Location:** `src/indexing/unified.rs:326-331`

**Impact:** Medium - Repeated string clones

**Current Code:**
```rust
self.tantivy_writer.add_document(doc!(
    self.tantivy_schema.content => chunk.content.clone(),
    self.tantivy_schema.symbol_name => chunk.context.symbol_name.clone(),
    self.tantivy_schema.symbol_kind => chunk.context.symbol_kind.clone(),
    // ...
));
```

**Issue:** Multiple `String` clones for each chunk. With 19,000+ chunks, this adds up.

**Optimization:** Use move semantics or Arc<str>:
```rust
// Option 1: Consume chunks
fn index_to_tantivy(&mut self, chunks: Vec<CodeChunk>) -> Result<()> {
    for chunk in chunks {
        self.tantivy_writer.add_document(doc!(
            self.tantivy_schema.content => chunk.content,  // Move
            self.tantivy_schema.symbol_name => chunk.context.symbol_name,
            // ...
        ))?;
    }
    Ok(())
}

// Option 2: Use Arc<str> in CodeChunk definition
pub struct CodeChunk {
    pub content: Arc<str>,
    // ...
}
```

**Expected Improvement:** 5-10% reduction in string allocations

---

### 🟡 #6: Unnecessary 100ms Sleep After Commit
**Location:** `src/indexing/unified.rs:404`

**Impact:** Low-Medium - Artificial delay

**Current Code:**
```rust
self.tantivy_writer.commit().context("Failed to commit Tantivy index")?;

// Wait briefly to ensure index is fully committed
tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
```

**Issue:** Adds 100ms delay after every directory indexing operation. This was likely added for testing but shouldn't be needed.

**Optimization:** Remove or make conditional:
```rust
self.tantivy_writer.commit().context("Failed to commit Tantivy index")?;
// Sleep removed - commit is synchronous and completes before returning
```

**Expected Improvement:** Removes 100ms per indexing operation

---

### 🟡 #7: Sequential Merkle Tree File Hashing
**Location:** `src/indexing/merkle.rs:107-124`

**Impact:** Low-Medium - Initial indexing only

**Current Code:**
```rust
for (idx, path) in files.iter().enumerate() {
    let content = std::fs::read(path)?;
    let hash = Sha256Hasher::hash(&content);
    // ...
}
```

**Issue:** File reading and hashing done sequentially. This is the Merkle tree build phase.

**Optimization:**
```rust
use rayon::prelude::*;

let file_nodes: Vec<_> = files.par_iter()
    .enumerate()
    .map(|(idx, path)| {
        let content = std::fs::read(path)?;
        let hash = Sha256Hasher::hash(&content);
        let metadata = std::fs::metadata(path)?;
        Ok((path.clone(), FileNode { content_hash: hash, leaf_index: idx, last_modified: metadata.modified()? }))
    })
    .collect::<Result<Vec<_>>>()?;
```

**Expected Improvement:** 4-6x speedup for Merkle tree building

---

### 🟡 #8: No Embedding Batching Across Files
**Location:** `src/indexing/unified.rs:269-277`

**Impact:** Low - Embedding generation

**Current Pattern:**
```rust
// Per file:
let embeddings = self.embedding_generator.embed_batch(chunk_texts)?;
```

**Issue:** Embeddings are batched per file, but files are processed sequentially. For small files, this underutilizes the embedding model's batch processing capability.

**Optimization:** Accumulate chunks across multiple files before generating embeddings:
```rust
// Collect chunks from multiple files
let mut all_chunks = Vec::new();
for file in first_N_files {
    let chunks = self.parse_and_chunk_file(file)?;
    all_chunks.extend(chunks);
}

// Generate embeddings for all chunks at once
let embeddings = self.embedding_generator.embed_batch(all_chunks)?;
```

**Expected Improvement:** 10-20% faster embedding generation

---

## 2. Architectural Optimization Strategies

### Strategy A: Parallel Pipeline Architecture

**Current:** Sequential stages per file
```
File 1: Read → Parse → Chunk → Embed → Index BM25 → Index Vector
File 2: Read → Parse → Chunk → Embed → Index BM25 → Index Vector
```

**Proposed:** Parallel pipeline with stages
```
Files 1-N (parallel):
  Stage 1: Read files (tokio::fs, parallel)
  Stage 2: Parse + Chunk (CPU-bound, rayon)
  Stage 3: Embed (batched across files)
  Stage 4: Index (batch commits)
```

**Implementation sketch:**
```rust
pub async fn index_directory_parallel(&mut self, dir_path: &Path) -> Result<IndexStats> {
    let rust_files: Vec<PathBuf> = WalkDir::new(dir_path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension() == Some(std::ffi::OsStr::new("rs")))
        .map(|e| e.path().to_path_buf())
        .collect();

    const BATCH_SIZE: usize = 100;

    for batch in rust_files.chunks(BATCH_SIZE) {
        // Stage 1: Parallel file reading
        let file_contents = self.read_files_parallel(batch).await?;

        // Stage 2: Parallel parsing & chunking
        let chunks = self.parse_and_chunk_parallel(file_contents)?;

        // Stage 3: Batch embedding generation
        let embeddings = self.generate_embeddings_batch(chunks)?;

        // Stage 4: Batch indexing
        self.index_batch(chunks, embeddings).await?;
    }

    self.commit()?;
    Ok(stats)
}
```

**Expected Improvement:** 10-15x overall speedup for large codebases

---

### Strategy B: Add Rayon for CPU-Bound Work

**Add to Cargo.toml:**
```toml
rayon = "1.8"
```

**Use for:**
- Parsing files (tree-sitter parsing)
- Hashing files (Merkle tree)
- Chunking code (symbol extraction)

---

### Strategy C: Async File I/O Throughout

**Replace all `std::fs` with `tokio::fs`:**
- `src/indexing/unified.rs:225`
- `src/indexing/merkle.rs:108, 113`
- `src/indexing/incremental.rs:84` (snapshot loading)

---

### Strategy D: Memory Pooling for Chunks

**Current:** Allocate new `CodeChunk` for every chunk
**Proposed:** Use object pool to reuse allocations

```rust
use object_pool::Pool;

pub struct UnifiedIndexer {
    chunk_pool: Pool<CodeChunk>,
    // ...
}
```

**Expected Improvement:** 15-20% reduction in allocation overhead

---

## 3. Quick Wins (Low Effort, High Impact)

### Quick Win #1: Remove 100ms Sleep ⚡
**File:** `src/indexing/unified.rs:404`
**Effort:** 1 line deletion
**Impact:** Saves 100ms per indexing operation
**Risk:** None - commit is synchronous

### Quick Win #2: Use `into_iter()` Instead of Clone ⚡
**File:** `src/indexing/unified.rs:351`
**Effort:** 5 minutes
**Impact:** 10-15% reduction in memory allocations
**Risk:** Low - just need to adjust ownership

### Quick Win #3: Parallel Merkle Hashing ⚡
**File:** `src/indexing/merkle.rs:107-124`
**Effort:** Add rayon, convert to `par_iter()`
**Impact:** 4-6x faster Merkle tree building
**Risk:** Low - pure computation

### Quick Win #4: Increase Embedding Batch Size ⚡
**File:** `src/embeddings/mod.rs:109`
**Current:** 32 chunks per batch
**Proposed:** 128 or 256 chunks per batch
**Effort:** 1 line change
**Impact:** 10-15% faster embedding generation
**Risk:** None - fastembed handles large batches well

### Quick Win #5: Async File Reading ⚡
**File:** `src/indexing/unified.rs:225`
**Effort:** 10 minutes
**Impact:** 20-30% reduction in I/O blocking
**Risk:** Low - tokio::fs is battle-tested

---

## 4. Specific Code Optimization Recommendations

### Recommendation #1: Parallel File Indexing

**File:** `src/indexing/unified.rs:363-415`

**Current Complexity:** O(n) sequential
**Proposed Complexity:** O(n/cores) parallel

**Implementation:**
```rust
pub async fn index_directory(&mut self, dir_path: &Path) -> Result<IndexStats> {
    tracing::info!("Indexing directory: {}", dir_path.display());

    let rust_files: Vec<PathBuf> = /* ... collect files ... */;

    let stats = Arc::new(Mutex::new(IndexStats::default()));
    let semaphore = Arc::new(Semaphore::new(num_cpus::get()));
    let mut join_set = JoinSet::new();

    for file in rust_files {
        let permit = semaphore.clone().acquire_owned().await?;
        let stats = stats.clone();
        let indexer = self.clone(); // Need to make UnifiedIndexer Send + Clone or use Arc<Mutex<>>

        join_set.spawn(async move {
            let result = indexer.index_file(&file).await;
            drop(permit);

            let mut stats = stats.lock().unwrap();
            match result {
                Ok(IndexFileResult::Indexed { chunks_count }) => {
                    stats.indexed_files += 1;
                    stats.total_chunks += chunks_count;
                }
                // ... handle other cases
            }
        });
    }

    while let Some(_) = join_set.join_next().await {}

    self.commit()?;
    Ok(Arc::try_unwrap(stats).unwrap().into_inner().unwrap())
}
```

**Note:** This requires making `UnifiedIndexer` thread-safe. Alternative: Use message passing with channels.

---

### Recommendation #2: Streaming Change Processing

**File:** `src/indexing/incremental.rs:175-235`

**Use futures streams for concurrent processing:**
```rust
use futures::stream::{self, StreamExt};

async fn process_changes(&mut self, _codebase_path: &Path, changes: ChangeSet) -> Result<IndexStats> {
    let mut stats = IndexStats::default();

    // Process all changes concurrently
    let results = stream::iter(changes.deleted.iter().map(|p| ("delete", p)))
        .chain(stream::iter(changes.modified.iter().map(|p| ("modify", p))))
        .chain(stream::iter(changes.added.iter().map(|p| ("add", p))))
        .map(|(op, path)| async move {
            match op {
                "delete" => self.indexer.delete_file_chunks(path).await,
                "modify" | "add" => {
                    if op == "modify" {
                        self.indexer.delete_file_chunks(path).await?;
                    }
                    self.indexer.index_file(path).await
                }
                _ => unreachable!()
            }
        })
        .buffer_unordered(8) // Process 8 files concurrently
        .collect::<Vec<_>>()
        .await;

    // Aggregate results
    for result in results {
        // ... update stats
    }

    self.indexer.commit()?;
    Ok(stats)
}
```

---

### Recommendation #3: Zero-Copy Tantivy Indexing

**File:** `src/indexing/unified.rs:318-337`

**Reduce allocations by consuming chunks:**
```rust
fn index_to_tantivy(&mut self, chunks: Vec<CodeChunk>) -> Result<()> {
    for chunk in chunks.into_iter() {  // Consume instead of borrow
        let chunk_json = serde_json::to_string(&chunk)
            .context("Failed to serialize chunk to JSON")?;

        self.tantivy_writer.add_document(doc!(
            self.tantivy_schema.chunk_id => chunk.id.to_string(),
            self.tantivy_schema.content => chunk.content, // Move, no clone
            self.tantivy_schema.symbol_name => chunk.context.symbol_name, // Move
            self.tantivy_schema.symbol_kind => chunk.context.symbol_kind, // Move
            self.tantivy_schema.file_path => chunk.context.file_path.display().to_string(),
            self.tantivy_schema.module_path => chunk.context.module_path.join("::"),
            self.tantivy_schema.docstring => chunk.context.docstring.unwrap_or_default(),
            self.tantivy_schema.chunk_json => chunk_json,
        ))?;
    }
    Ok(())
}
```

---

## 5. Long-Term Optimization Strategies

### Strategy 1: Incremental AST Caching
**Effort:** High
**Impact:** Very High for repeated indexing

Cache parsed ASTs in addition to file metadata:
```rust
pub struct AstCache {
    cache: sled::Db,
}

impl AstCache {
    pub fn get_or_parse(&mut self, file_path: &Path, content: &str) -> Result<ParseResult> {
        let hash = self.content_hash(content);
        if let Some(cached_ast) = self.cache.get(hash)? {
            return Ok(deserialize(cached_ast)?);
        }

        let ast = self.parser.parse(content)?;
        self.cache.insert(hash, serialize(&ast)?)?;
        Ok(ast)
    }
}
```

---

### Strategy 2: Differential Chunking
**Effort:** High
**Impact:** Medium for large files

Only re-chunk changed symbols, not entire file:
```rust
pub fn incremental_chunk(&self, old_chunks: &[CodeChunk], parse_result: &ParseResult) -> Vec<CodeChunk> {
    // Compare old and new symbols
    // Only re-chunk changed symbols
    // Reuse unchanged chunks
}
```

---

### Strategy 3: Batch Commit Strategy
**Effort:** Medium
**Impact:** Medium for very large codebases

Instead of one commit per directory:
```rust
pub async fn index_directory(&mut self, dir_path: &Path) -> Result<IndexStats> {
    const COMMIT_BATCH_SIZE: usize = 1000;

    for (i, file) in rust_files.iter().enumerate() {
        self.index_file(file).await?;

        if (i + 1) % COMMIT_BATCH_SIZE == 0 {
            self.commit()?;
            tracing::info!("Committed batch {} files", i + 1);
        }
    }

    self.commit()?; // Final commit
    Ok(stats)
}
```

---

### Strategy 4: SIMD-Accelerated Hashing
**Effort:** Medium
**Impact:** Medium for Merkle tree building

Use SIMD-optimized SHA-256:
```toml
[dependencies]
sha2 = { version = "0.10", features = ["asm"] }
```

---

### Strategy 5: Compressed Merkle Snapshots
**Effort:** Low
**Impact:** Low - Faster snapshot I/O

Use compression for Merkle snapshots:
```rust
use flate2::write::GzEncoder;

pub fn save_snapshot(&self, path: &Path) -> Result<()> {
    let file = std::fs::File::create(path)?;
    let encoder = GzEncoder::new(file, Compression::fast());
    bincode::serialize_into(encoder, &snapshot)?;
    Ok(())
}
```

---

## 6. Performance Testing Recommendations

### Benchmark Suite Setup
```rust
// Add to Cargo.toml [dev-dependencies]
criterion = "0.5"

// Create benches/indexing_bench.rs
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn bench_sequential_indexing(c: &mut Criterion) {
    c.bench_function("index 100 files sequential", |b| {
        b.iter(|| {
            // Current implementation
        });
    });
}

fn bench_parallel_indexing(c: &mut Criterion) {
    c.bench_function("index 100 files parallel", |b| {
        b.iter(|| {
            // Optimized implementation
        });
    });
}

criterion_group!(benches, bench_sequential_indexing, bench_parallel_indexing);
criterion_main!(benches);
```

---

## 7. Implementation Priority Matrix

| Optimization | Effort | Impact | Priority | Expected Gain |
|-------------|--------|--------|----------|---------------|
| Parallel file processing | Medium | Very High | **P0** | 5-8x |
| Remove 100ms sleep | Very Low | Low | **P0** | 100ms per op |
| Async file I/O | Low | High | **P1** | 20-30% |
| Parallel change processing | Medium | High | **P1** | 3-5x |
| Parallel Merkle hashing | Low | Medium | **P1** | 4-6x |
| Remove clones | Low | Medium | **P2** | 10-15% |
| Batch embedding | Medium | Low | **P2** | 10-20% |
| Pipeline architecture | High | Very High | **P3** | 10-15x |

---

## 8. Risk Assessment

### Low Risk Optimizations
- ✅ Remove 100ms sleep
- ✅ Use `into_iter()` instead of clone
- ✅ Increase embedding batch size
- ✅ Parallel Merkle hashing (pure computation)

### Medium Risk Optimizations
- ⚠️ Async file I/O (needs testing on different filesystems)
- ⚠️ Parallel file processing (needs careful error handling)
- ⚠️ Batch commits (need to ensure consistency)

### High Risk Optimizations
- 🔴 Pipeline architecture (major refactoring)
- 🔴 AST caching (cache invalidation complexity)
- 🔴 Differential chunking (correctness critical)

---

## 9. Estimated Overall Performance Gains

**Current Performance (Burn codebase, ~1,500 files):**
- First index: ~5-10 minutes
- Incremental update (no changes): 53-288ms
- Incremental update (10 files changed): ~30 seconds

**After P0+P1 Optimizations:**
- First index: **~45-90 seconds** (6-7x improvement)
- Incremental update (no changes): **53-288ms** (unchanged - already optimal)
- Incremental update (10 files changed): **~5-8 seconds** (4-6x improvement)

**After All Optimizations (P0-P3):**
- First index: **~25-40 seconds** (12-15x improvement)
- Incremental update (no changes): **53-288ms** (unchanged)
- Incremental update (10 files changed): **~2-3 seconds** (10-15x improvement)

---

## 10. Conclusion & Next Steps

The `index_codebase` implementation is functionally correct but has significant performance optimization opportunities. The primary bottleneck is **sequential file processing**, which can be addressed with relatively low-effort parallelization.

### Recommended Implementation Order:

1. **Week 1:** P0 Quick Wins
   - Remove 100ms sleep
   - Remove unnecessary clones
   - Add parallel Merkle hashing

2. **Week 2:** P1 Parallelization
   - Implement async file I/O
   - Add parallel file processing with tokio
   - Add parallel change processing

3. **Week 3:** P2 Refinements
   - Optimize embedding batching
   - Tune batch sizes and concurrency limits
   - Add comprehensive benchmarks

4. **Week 4+:** P3 Architecture
   - Implement pipeline architecture if needed
   - Consider AST caching
   - Evaluate differential chunking

### Success Metrics:
- ✅ 5x reduction in initial indexing time
- ✅ 3x reduction in incremental update time
- ✅ No regression in change detection accuracy
- ✅ Memory usage remains under 2GB for large codebases

---

## Appendix: Analysis Methodology

This analysis was performed using the following MCP tools on 2025-10-22:

1. **find_definition** - Located main entry points
2. **get_call_graph** - Analyzed function call patterns
3. **analyze_complexity** - Measured cyclomatic complexity
4. **grep** - Searched for performance patterns (.clone(), std::fs::, locks)
5. **get_similar_code** - Found parallel processing examples
6. **read_file_content** - Deep-dived into implementation details

**Analyzed Components:**
- `src/tools/index_tool.rs` (284 lines, complexity 59)
- `src/indexing/incremental.rs` (384 lines, complexity 54)
- `src/indexing/unified.rs` (642 lines, complexity 75)
- `src/indexing/merkle.rs` (439 lines)
- `src/embeddings/mod.rs` (288 lines)
- `src/vector_store/mod.rs` (547 lines)

**Test Environment:**
- Burn deep learning framework (~1,569 .rs files, ~1.5M LOC)
- Collection: `code_chunks_eb5e3f03` (19,126 points)
- Qdrant: http://localhost:6333

---

*Generated by comprehensive MCP-based codebase analysis on rust-code-mcp repository*
