# Phase 3: Optional Fine-Tuning & Advanced Optimizations

**Project**: rust-code-mcp indexing pipeline optimization
**Phase**: 3 - Optional Tuning
**Estimated Effort**: 1-2 weeks (pick and choose)
**Expected Improvement**: 1.2-1.5x additional (marginal gains)
**Risk Level**: Low-Medium
**Prerequisites**: Phases 0, 1, and optionally 2 completed

---

## Overview

Phase 3 contains **optional optimizations** that provide incremental improvements. Only implement these if:
- Phases 0-2 don't meet your performance goals
- You want to squeeze out maximum performance
- You have time for fine-tuning

Most users will NOT need Phase 3.

### What's Included

1. Memory budget tuning (Tantivy, Qdrant)
2. Batch size optimization
3. Advanced parallelization tweaks
4. Monitoring and profiling
5. Long-term architectural improvements

---

## Task 3.1: Increase Tantivy Memory Budgets

**Priority**: P2
**Effort**: 15 minutes
**Impact**: 1.2-1.3x faster Tantivy indexing
**Risk**: Low (may use more memory)

### Problem

Current Tantivy memory budgets are conservative:
- Small codebases: 100MB total
- Medium codebases (Burn): 400MB total
- Large codebases: 1600MB total

Increasing budgets reduces segment merges, improving performance.

### Location

`src/indexing/unified.rs:148-158`

### Current Configuration

```rust
let (memory_budget_mb, num_threads) = if let Some(loc) = codebase_loc {
    if loc < 100_000 {
        (50, 2)   // 100MB total
    } else if loc < 1_000_000 {
        (100, 4)  // 400MB total ← Burn is here
    } else {
        (200, 8)  // 1600MB total
    }
} else {
    (50, 2)
};
```

### Optimized Configuration

```rust
let (memory_budget_mb, num_threads) = if let Some(loc) = codebase_loc {
    if loc < 100_000 {
        (50, 2)   // Small: 100MB total (unchanged)
    } else if loc < 1_000_000 {
        (200, 4)  // Medium: 800MB total (2x increase)
    } else {
        (400, 8)  // Large: 3200MB total (2x increase)
    }
} else {
    (100, 2)  // Default: 200MB (2x increase)
};
```

### Testing

```bash
# Test on Burn codebase
cargo run --release -- index /path/to/burn --force-reindex

# Monitor memory usage
top -p $(pgrep rust-code-mcp)
```

### Verification

- ✅ No OOM errors
- ✅ Slight performance improvement (5-10%)
- ✅ Memory usage within system limits

### Rollback

Revert to original values if memory issues occur.

---

## Task 3.2: Tune Qdrant Batch Sizes

**Priority**: P2
**Effort**: 30 minutes
**Impact**: 3-5% faster Qdrant insertion
**Risk**: Low

### Problem

Fixed batch size of 100 points is suboptimal:
- Too small for large codebases (more network roundtrips)
- Could be larger for medium/large collections

### Location

`src/vector_store/mod.rs:217-226`

### Current Code

```rust
pub async fn upsert_chunks(
    &self,
    chunk_data: Vec<(ChunkId, Embedding, CodeChunk)>,
) -> Result<()> {
    let points = self.prepare_points(chunk_data)?;

    // Fixed batch size
    for batch in points.chunks(100) {
        self.client
            .upsert_points(/* ... */)
            .await?;
    }

    Ok(())
}
```

### Optimized Code

```rust
pub async fn upsert_chunks(
    &self,
    chunk_data: Vec<(ChunkId, Embedding, CodeChunk)>,
) -> Result<()> {
    let points = self.prepare_points(chunk_data)?;

    // Dynamic batch size based on total points
    let batch_size = Self::calculate_batch_size(points.len());

    tracing::debug!("Upserting {} points in batches of {}", points.len(), batch_size);

    for batch in points.chunks(batch_size) {
        self.client
            .upsert_points(/* ... */)
            .await?;
    }

    Ok(())
}

fn calculate_batch_size(total_points: usize) -> usize {
    match total_points {
        0..=1000 => 50,        // Small: smaller batches
        1001..=10000 => 100,   // Medium: current default
        10001..=50000 => 200,  // Large: bigger batches
        _ => 500,              // Very large: maximum batches
    }
}
```

### Optional: Concurrent Upserts

```rust
pub async fn upsert_chunks_concurrent(
    &self,
    chunk_data: Vec<(ChunkId, Embedding, CodeChunk)>,
) -> Result<()> {
    let points = self.prepare_points(chunk_data)?;
    let batch_size = Self::calculate_batch_size(points.len());

    // Create tasks for concurrent upserts
    let mut tasks = Vec::new();
    for batch in points.chunks(batch_size) {
        let client = self.client.clone();
        let batch = batch.to_vec();

        tasks.push(tokio::spawn(async move {
            client.upsert_points(/* batch */).await
        }));
    }

    // Wait for all upserts
    for task in tasks {
        task.await??;
    }

    Ok(())
}
```

### Testing

```bash
cargo test vector_store::tests
cargo run --release -- index /path/to/large-codebase
```

---

## Task 3.3: Optimize Embedding Batch Configuration

**Priority**: P2
**Effort**: 15 minutes
**Impact**: 10-15% faster embeddings
**Risk**: Low

### Location

`src/embeddings/mod.rs:109`

### Current

```rust
const DEFAULT_BATCH_SIZE: usize = 32;
```

### Optimized

```rust
// Increased from 32 to 128 for better model utilization
const DEFAULT_BATCH_SIZE: usize = 128;

// Optional: Make configurable
pub struct EmbeddingConfig {
    pub batch_size: usize,
    pub model_path: Option<PathBuf>,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            batch_size: 128,
            model_path: None,
        }
    }
}
```

### Testing

```bash
cargo test embeddings::tests
```

---

## Task 3.4: Add Incremental Embedding Cache (Advanced)

**Priority**: P3
**Effort**: 2-3 days
**Impact**: 10-30% on refactor-heavy scenarios
**Risk**: Medium

### Concept

Cache embeddings by chunk content hash. If chunk content hasn't changed, reuse embedding.

### Use Cases

- Function moved between files (same content, new location)
- Refactoring that doesn't change functionality
- Repeated patterns across codebase

### Implementation

**Step 1**: Create embedding cache

```rust
// New file: src/embeddings/cache.rs
use sled::Db;
use sha2::{Digest, Sha256};

pub struct EmbeddingCache {
    db: Db,
}

impl EmbeddingCache {
    pub fn new(cache_path: &Path) -> Result<Self> {
        let db = sled::open(cache_path.join("embedding_cache"))?;
        Ok(Self { db })
    }

    pub fn get(&self, chunk_content: &str) -> Result<Option<Embedding>> {
        let hash = self.hash_content(chunk_content);

        if let Some(bytes) = self.db.get(hash)? {
            let embedding: Embedding = bincode::deserialize(&bytes)?;
            return Ok(Some(embedding));
        }

        Ok(None)
    }

    pub fn set(&self, chunk_content: &str, embedding: &Embedding) -> Result<()> {
        let hash = self.hash_content(chunk_content);
        let bytes = bincode::serialize(embedding)?;
        self.db.insert(hash, bytes)?;
        Ok(())
    }

    fn hash_content(&self, content: &str) -> Vec<u8> {
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        hasher.finalize().to_vec()
    }
}
```

**Step 2**: Integrate into indexing

```rust
impl UnifiedIndexer {
    // Add embedding_cache field
    embedding_cache: EmbeddingCache,

    async fn index_file_worker(/* ... */) -> Result<IndexFileResult> {
        // ... parse and chunk ...

        // Generate embeddings with cache
        let mut embeddings = Vec::new();
        let mut chunks_to_embed = Vec::new();

        for chunk in &chunks {
            if let Some(cached) = self.embedding_cache.get(&chunk.content)? {
                embeddings.push(cached);
            } else {
                chunks_to_embed.push(chunk);
            }
        }

        // Generate embeddings for non-cached chunks
        if !chunks_to_embed.is_empty() {
            let texts: Vec<String> = chunks_to_embed.iter()
                .map(|c| c.format_for_embedding())
                .collect();

            let new_embeddings = self.embedding_generator.embed_batch(texts)?;

            // Cache new embeddings
            for (chunk, embedding) in chunks_to_embed.iter().zip(&new_embeddings) {
                self.embedding_cache.set(&chunk.content, embedding)?;
            }

            embeddings.extend(new_embeddings);
        }

        // ... continue ...
    }
}
```

### Testing

```bash
# Test cache hit rate
cargo test embeddings::cache::tests

# Real-world test: reindex same codebase
cargo run --release -- index /path/to/codebase --force-reindex
cargo run --release -- index /path/to/codebase --force-reindex
# Second run should show cache hits
```

### Expected Impact

- **First index**: No change (cache miss)
- **Reindex unchanged code**: 30-50% faster (cache hit)
- **Refactor scenarios**: 10-20% faster (partial cache hit)

---

## Task 3.5: SIMD-Accelerated Hashing

**Priority**: P3
**Effort**: 15 minutes
**Impact**: 10-20% faster Merkle tree building
**Risk**: Low

### Location

`Cargo.toml` and `src/indexing/merkle.rs`

### Implementation

**Step 1**: Enable SIMD features

```toml
[dependencies]
sha2 = { version = "0.10", features = ["asm"] }  # Hardware acceleration
```

**Step 2**: Verify it's being used

```rust
// In src/indexing/merkle.rs
use sha2::{Digest, Sha256};

// Automatically uses SIMD if available
let hash = Sha256::digest(&content);
```

### Testing

```bash
# Benchmark Merkle tree building
cargo bench --bench merkle_bench
```

---

## Task 3.6: Batch Commit Strategy

**Priority**: P2
**Effort**: 1-2 hours
**Impact**: Better progress visibility, slight performance gain
**Risk**: Low

### Problem

Current implementation commits once at the end. For very large codebases (10,000+ files), this:
- Takes a long time with no progress feedback
- Risks losing all work if interrupted

### Solution

Commit every N files for incremental progress.

### Location

`src/indexing/unified.rs` - all mode implementations

### Implementation

```rust
pub async fn index_directory_parallel(&mut self, dir_path: &Path) -> Result<IndexStats> {
    const COMMIT_INTERVAL: usize = 1000;  // Commit every 1000 files

    // ... parallel processing ...

    let mut files_since_commit = 0;

    while let Some(result) = result_rx.recv().await {
        // ... process result ...

        files_since_commit += 1;

        // Incremental commit
        if files_since_commit >= COMMIT_INTERVAL {
            tracing::info!("Committing after {} files", files_since_commit);
            self.tantivy_writer.commit()?;
            files_since_commit = 0;
        }
    }

    // Final commit
    self.tantivy_writer.commit()?;

    Ok(stats)
}
```

### Testing

```bash
# Test on very large codebase
cargo run --release -- index /path/to/huge-codebase
# Should see periodic "Committing after 1000 files" messages
```

---

## Task 3.7: Monitoring & Profiling Infrastructure

**Priority**: P2
**Effort**: 1-2 days
**Impact**: Enables data-driven optimization
**Risk**: Low

### Implementation

**Step 1**: Add performance metrics struct

```rust
// New file: src/monitoring/metrics.rs
use std::time::{Duration, Instant};
use std::sync::atomic::{AtomicUsize, Ordering};

pub struct IndexingMetrics {
    // Timing
    pub file_read_time: AtomicUsize,      // microseconds
    pub parse_time: AtomicUsize,
    pub chunk_time: AtomicUsize,
    pub embedding_time: AtomicUsize,
    pub tantivy_index_time: AtomicUsize,
    pub qdrant_index_time: AtomicUsize,
    pub merkle_time: AtomicUsize,

    // Counts
    pub files_processed: AtomicUsize,
    pub chunks_generated: AtomicUsize,
    pub cache_hits: AtomicUsize,
    pub cache_misses: AtomicUsize,
}

impl IndexingMetrics {
    pub fn new() -> Self {
        Self {
            file_read_time: AtomicUsize::new(0),
            parse_time: AtomicUsize::new(0),
            chunk_time: AtomicUsize::new(0),
            embedding_time: AtomicUsize::new(0),
            tantivy_index_time: AtomicUsize::new(0),
            qdrant_index_time: AtomicUsize::new(0),
            merkle_time: AtomicUsize::new(0),
            files_processed: AtomicUsize::new(0),
            chunks_generated: AtomicUsize::new(0),
            cache_hits: AtomicUsize::new(0),
            cache_misses: AtomicUsize::new(0),
        }
    }

    pub fn log_summary(&self) {
        let total_time = self.file_read_time.load(Ordering::Relaxed)
            + self.parse_time.load(Ordering::Relaxed)
            + self.chunk_time.load(Ordering::Relaxed)
            + self.embedding_time.load(Ordering::Relaxed)
            + self.tantivy_index_time.load(Ordering::Relaxed)
            + self.qdrant_index_time.load(Ordering::Relaxed);

        tracing::info!("=== Indexing Performance Breakdown ===");
        tracing::info!("Files processed:  {}", self.files_processed.load(Ordering::Relaxed));
        tracing::info!("Chunks generated: {}", self.chunks_generated.load(Ordering::Relaxed));
        tracing::info!("");
        tracing::info!("Timing (% of total):");
        tracing::info!("  File I/O:     {:?} ({:.1}%)",
            Duration::from_micros(self.file_read_time.load(Ordering::Relaxed) as u64),
            self.file_read_time.load(Ordering::Relaxed) as f64 / total_time as f64 * 100.0
        );
        tracing::info!("  Parsing:      {:?} ({:.1}%)",
            Duration::from_micros(self.parse_time.load(Ordering::Relaxed) as u64),
            self.parse_time.load(Ordering::Relaxed) as f64 / total_time as f64 * 100.0
        );
        tracing::info!("  Chunking:     {:?} ({:.1}%)",
            Duration::from_micros(self.chunk_time.load(Ordering::Relaxed) as u64),
            self.chunk_time.load(Ordering::Relaxed) as f64 / total_time as f64 * 100.0
        );
        tracing::info!("  Embeddings:   {:?} ({:.1}%)",
            Duration::from_micros(self.embedding_time.load(Ordering::Relaxed) as u64),
            self.embedding_time.load(Ordering::Relaxed) as f64 / total_time as f64 * 100.0
        );
        tracing::info!("  Tantivy:      {:?} ({:.1}%)",
            Duration::from_micros(self.tantivy_index_time.load(Ordering::Relaxed) as u64),
            self.tantivy_index_time.load(Ordering::Relaxed) as f64 / total_time as f64 * 100.0
        );
        tracing::info!("  Qdrant:       {:?} ({:.1}%)",
            Duration::from_micros(self.qdrant_index_time.load(Ordering::Relaxed) as u64),
            self.qdrant_index_time.load(Ordering::Relaxed) as f64 / total_time as f64 * 100.0
        );
        tracing::info!("");

        let cache_total = self.cache_hits.load(Ordering::Relaxed) + self.cache_misses.load(Ordering::Relaxed);
        if cache_total > 0 {
            tracing::info!("Cache hit rate: {:.1}%",
                self.cache_hits.load(Ordering::Relaxed) as f64 / cache_total as f64 * 100.0
            );
        }
    }

    pub fn record_timing<F, R>(&self, metric: &AtomicUsize, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        let start = Instant::now();
        let result = f();
        let elapsed = start.elapsed().as_micros() as usize;
        metric.fetch_add(elapsed, Ordering::Relaxed);
        result
    }
}
```

**Step 2**: Integrate into UnifiedIndexer

```rust
impl UnifiedIndexer {
    metrics: Arc<IndexingMetrics>,

    async fn index_file_worker(/* ... */) -> Result<IndexFileResult> {
        // Wrap operations with timing
        let content = metrics.record_timing(&metrics.file_read_time, || {
            std::fs::read_to_string(file_path)
        })?;

        let parse_result = metrics.record_timing(&metrics.parse_time, || {
            parser.parse_source_complete(&content)
        })?;

        // ... etc ...
    }

    pub async fn index_directory_parallel(&mut self, dir_path: &Path) -> Result<IndexStats> {
        // ... indexing ...

        // Log metrics at end
        self.metrics.log_summary();

        Ok(stats)
    }
}
```

**Step 3**: Enable with environment variable

```bash
RUST_LOG=rust_code_mcp=debug cargo run -- index /path/to/codebase
```

---

## Phase 3 Testing Strategy

### Performance Profiling

Use `cargo flamegraph` to identify hot spots:

```bash
# Install
cargo install flamegraph

# Profile
cargo flamegraph --release -- index /path/to/large-codebase

# Opens flamegraph.svg in browser
```

### Memory Profiling

Use `valgrind` or `heaptrack`:

```bash
# Heaptrack
heaptrack cargo run --release -- index /path/to/codebase
heaptrack_gui heaptrack.*.gz
```

### Benchmarking

Create comprehensive benchmarks:

```bash
# File: benches/comprehensive_bench.rs
use criterion::{criterion_group, criterion_main, Criterion, BenchmarkId};

fn bench_indexing_configurations(c: &mut Criterion) {
    let mut group = c.benchmark_group("indexing_config");

    // Test different configurations
    for batch_size in [32, 64, 128, 256] {
        group.bench_with_input(
            BenchmarkId::new("embedding_batch", batch_size),
            &batch_size,
            |b, &size| {
                b.iter(|| {
                    // Test with this batch size
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_indexing_configurations);
criterion_main!(benches);
```

---

## Success Criteria

### Phase 3 Complete When:

- ✅ Implemented desired optimizations (pick and choose)
- ✅ Measurable performance improvement
- ✅ Memory usage acceptable
- ✅ All tests passing
- ✅ Metrics/monitoring in place
- ✅ Documentation updated

### Quality Gates

1. ✅ No performance regression
2. ✅ Memory usage within limits
3. ✅ All tests pass
4. ✅ Profiling data collected
5. ✅ Optimization benefits documented

---

## Decision Matrix: Which Tasks to Implement?

| Task | Effort | Impact | Implement If... |
|------|--------|--------|-----------------|
| 3.1 Memory budgets | 15 min | Low | Using Tantivy heavily |
| 3.2 Qdrant batching | 30 min | Low | Indexing 10k+ chunks |
| 3.3 Embedding config | 15 min | Medium | Using embeddings |
| 3.4 Embedding cache | 2-3 days | Medium | Frequent reindexing |
| 3.5 SIMD hashing | 15 min | Low | Building Merkle trees |
| 3.6 Batch commits | 1-2 hours | Low | Indexing 10k+ files |
| 3.7 Monitoring | 1-2 days | High | Optimizing further |

**Recommendation**: Start with 3.7 (monitoring) to identify which other tasks will help most.

---

## Long-Term Future Enhancements (Not Included)

### Differential Chunking
Only re-chunk changed symbols, not entire file.

**Effort**: High
**Impact**: Medium for large files

### AST Caching
Cache parsed ASTs in addition to file metadata.

**Effort**: High
**Impact**: Very high for repeated indexing

### GPU-Accelerated Embeddings
Use GPU directly for embedding generation.

**Effort**: Very High
**Impact**: 2-5x faster embeddings

### Distributed Indexing
Split indexing across multiple machines.

**Effort**: Very High
**Impact**: Linear scaling with machines

---

## References

- Main plan: `SPEED.md`
- Previous phases: `PHASE_0.md`, `PHASE_1.md`, `PHASE_2.md`
- Analysis documents: `LIST.md`

---

**Document Version**: 1.0
**Last Updated**: 2025-10-22
**Status**: ✅ Optional - Pick and Choose
