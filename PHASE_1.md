# Phase 1: Parallel Mode Implementation Plan

**Project**: rust-code-mcp indexing pipeline optimization
**Phase**: 1 - Parallel Mode (File-Level Parallelism)
**Estimated Effort**: 2-3 days
**Expected Improvement**: 4-6x (626s → 100-150s)
**Risk Level**: Medium
**Prerequisites**: Phase 0 completed

---

## Overview

Phase 1 implements **file-level parallelism** - processing 8-12 files concurrently instead of sequentially. This is the **default indexing mode** that will be used for most codebases.

### Architecture: Parallel Mode

**Current (Sequential)**:
```
File 1 → Read → Parse → Chunk → Embed → Index
File 2 → Read → Parse → Chunk → Embed → Index
File 3 → Read → Parse → Chunk → Embed → Index
... (sequential)
```

**Phase 1 (Parallel)**:
```
File 1 ┐
File 2 ├─→ Read → Parse → Chunk → Embed (in parallel) ─→ Batch Index
File 3 │
...    ┘
(8-12 files processed concurrently)
```

### Goals

1. ✅ Implement `index_directory_parallel()` with file-level parallelism
2. ✅ Replace synchronous file I/O with async (`tokio::fs`)
3. ✅ Parallelize Merkle tree file hashing
4. ✅ Make Parallel the default mode
5. ✅ Achieve 4-6x speedup on medium/large codebases

### Key Design Decisions

- **Concurrency**: 8-12 files (matches CPU cores)
- **Pattern**: Message passing via channels (avoids complex locking)
- **Batching**: Collect chunks from parallel workers, batch index at end
- **Fallback**: Keep sequential mode for debugging

---

## Task 1.1: Implement Parallel Mode (File-Level Parallelism)

**Priority**: P1
**Effort**: 1-2 days
**Impact**: 5-8x speedup (biggest single improvement)
**Risk**: Medium (requires careful state management)

### Problem

Current sequential processing wastes CPU resources:
- While reading file N, parser sits idle
- While parsing file N, embedding model sits idle
- While generating embeddings, I/O subsystem sits idle

Modern systems have 8-16 cores that could process multiple files simultaneously.

### Solution

Use tokio tasks with semaphore-based concurrency limiting:
1. Spawn up to 12 concurrent worker tasks
2. Each worker processes one file completely
3. Workers send results via channel
4. Main task collects results and batch indexes

### Location

`src/indexing/unified.rs` - new method `index_directory_parallel()`

### Implementation

**Step 1**: Add dependencies to `Cargo.toml`

```toml
[dependencies]
num_cpus = "1.16"  # For detecting CPU cores
tokio = { version = "1.35", features = ["full"] }  # Already present, ensure "full"
```

**Step 2**: Update `IndexFileResult` to carry chunks

```rust
// In src/indexing/unified.rs
#[derive(Debug)]
pub enum IndexFileResult {
    Indexed {
        chunks_count: usize,
        chunks: Vec<(CodeChunk, Embedding)>,  // Add this field
    },
    Unchanged,
    Skipped,
}
```

**Step 3**: Make components Clone-able

Required for sharing across tasks:

```rust
// Check and add Clone where needed:
// - RustParser: Already Clone
// - Chunker: Already Clone
// - EmbeddingGenerator: Already Clone
// - MetadataCache: Needs Arc<> wrapper or Clone derive
// - SecretsScanner: Add #[derive(Clone)]
// - SensitiveFileFilter: Already Clone
```

Update `MetadataCache` in `src/metadata_cache.rs`:

```rust
// Wrap sled::Db in Arc for sharing
#[derive(Clone)]
pub struct MetadataCache {
    db: Arc<sled::Db>,
}
```

**Step 4**: Implement `index_directory_parallel()`

```rust
use tokio::sync::{mpsc, Semaphore};
use std::sync::Arc;

impl UnifiedIndexer {
    /// Parallel mode: Process multiple files concurrently
    pub async fn index_directory_parallel(&mut self, dir_path: &Path) -> Result<IndexStats> {
        tracing::info!("Using PARALLEL mode: file-level parallelism");

        let mut stats = IndexStats::default();

        // Find all Rust files
        let rust_files: Vec<PathBuf> = WalkDir::new(dir_path)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .filter(|e| e.path().extension() == Some(std::ffi::OsStr::new("rs")))
            .map(|e| e.path().to_path_buf())
            .collect();

        stats.total_files = rust_files.len();
        tracing::info!("Found {} Rust files", rust_files.len());

        // Determine concurrency (CPU cores, max 12)
        let concurrency = num_cpus::get().min(12);
        tracing::info!("Processing with {} concurrent workers", concurrency);

        // Create channel for results
        let (result_tx, mut result_rx) = mpsc::channel::<Result<IndexFileResult>>(100);

        // Create semaphore to limit concurrency
        let semaphore = Arc::new(Semaphore::new(concurrency));

        // Clone components for tasks
        let parser = self.parser.clone();
        let chunker = self.chunker.clone();
        let embedding_generator = self.embedding_generator.clone();
        let metadata_cache = self.metadata_cache.clone();
        let secrets_scanner = self.secrets_scanner.clone();
        let file_filter = self.file_filter.clone();

        // Spawn worker tasks
        for file in rust_files {
            let tx = result_tx.clone();
            let permit = semaphore.clone().acquire_owned().await?;

            // Clone components for this task
            let parser = parser.clone();
            let chunker = chunker.clone();
            let embedding_generator = embedding_generator.clone();
            let metadata_cache = metadata_cache.clone();
            let secrets_scanner = secrets_scanner.clone();
            let file_filter = file_filter.clone();

            tokio::spawn(async move {
                // Process file
                let result = Self::index_file_worker(
                    &file,
                    &parser,
                    &chunker,
                    &embedding_generator,
                    &metadata_cache,
                    &secrets_scanner,
                    &file_filter,
                ).await;

                // Send result
                let _ = tx.send(result).await;

                drop(permit); // Release semaphore
            });
        }

        // Drop sender so channel closes when workers finish
        drop(result_tx);

        // Collect results from workers
        let mut all_chunks = Vec::new();
        while let Some(result) = result_rx.recv().await {
            match result {
                Ok(IndexFileResult::Indexed { chunks_count, chunks }) => {
                    stats.indexed_files += 1;
                    stats.total_chunks += chunks_count;
                    all_chunks.extend(chunks);
                }
                Ok(IndexFileResult::Unchanged) => {
                    stats.unchanged_files += 1;
                }
                Ok(IndexFileResult::Skipped) => {
                    stats.skipped_files += 1;
                }
                Err(e) => {
                    tracing::error!("Failed to index file: {}", e);
                    stats.skipped_files += 1;
                }
            }
        }

        // Batch index all chunks
        if !all_chunks.is_empty() {
            self.batch_index_chunks(all_chunks).await?;
        }

        // Commit Tantivy changes
        self.tantivy_writer.commit().context("Failed to commit Tantivy index")?;

        tracing::info!(
            "✓ Parallel indexing complete: {} files, {} chunks, {} unchanged, {} skipped",
            stats.indexed_files,
            stats.total_chunks,
            stats.unchanged_files,
            stats.skipped_files
        );

        Ok(stats)
    }

    /// Worker function for parallel file processing
    /// Returns chunks instead of immediately indexing them
    async fn index_file_worker(
        file_path: &Path,
        parser: &RustParser,
        chunker: &Chunker,
        embedding_generator: &EmbeddingGenerator,
        metadata_cache: &MetadataCache,
        secrets_scanner: &SecretsScanner,
        file_filter: &SensitiveFileFilter,
    ) -> Result<IndexFileResult> {
        // 1. Check if file should be excluded
        if !file_filter.should_index(file_path) {
            return Ok(IndexFileResult::Skipped);
        }

        // 2. Read file (will be async in Task 1.2)
        let content = std::fs::read_to_string(file_path)
            .context(format!("Failed to read file: {}", file_path.display()))?;

        // 3. Check for secrets
        if secrets_scanner.should_exclude(&content) {
            tracing::warn!("Excluding file with secrets: {}", file_path.display());
            return Ok(IndexFileResult::Skipped);
        }

        // 4. Check metadata cache
        let file_path_str = file_path.to_string_lossy().to_string();
        if !metadata_cache.has_changed(&file_path_str, &content)
            .map_err(|e| anyhow::anyhow!("Metadata cache error: {}", e))? {
            return Ok(IndexFileResult::Unchanged);
        }

        // 5. Parse
        let parse_result = parser
            .parse_source_complete(&content)
            .map_err(|e| anyhow::anyhow!("Parse failed: {}", e))?;

        // 6. Chunk
        let chunks = chunker
            .chunk_file(file_path, &content, &parse_result)
            .map_err(|e| anyhow::anyhow!("Chunk failed: {}", e))?;

        if chunks.is_empty() {
            return Ok(IndexFileResult::Skipped);
        }

        // 7. Generate embeddings
        let chunk_texts: Vec<String> = chunks
            .iter()
            .map(|c| c.format_for_embedding())
            .collect();

        let embeddings = embedding_generator
            .embed_batch(chunk_texts)
            .map_err(|e| anyhow::anyhow!("Embedding failed: {}", e))?;

        if embeddings.len() != chunks.len() {
            anyhow::bail!("Embedding count mismatch");
        }

        // 8. Update metadata cache
        let file_meta = crate::metadata_cache::FileMetadata::from_content(
            &content,
            std::fs::metadata(file_path)?
                .modified()?
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs(),
            std::fs::metadata(file_path)?.len(),
        );
        metadata_cache.set(&file_path_str, &file_meta)
            .map_err(|e| anyhow::anyhow!("Cache update failed: {}", e))?;

        // 9. Return chunks with embeddings (don't index yet)
        let chunks_with_embeddings: Vec<(CodeChunk, Embedding)> = chunks
            .into_iter()
            .zip(embeddings.into_iter())
            .collect();

        let chunks_count = chunks_with_embeddings.len();

        Ok(IndexFileResult::Indexed {
            chunks_count,
            chunks: chunks_with_embeddings,
        })
    }

    /// Batch index chunks to both Tantivy and Qdrant
    async fn batch_index_chunks(&mut self, chunks_with_embeddings: Vec<(CodeChunk, Embedding)>) -> Result<()> {
        tracing::info!("Batch indexing {} chunks", chunks_with_embeddings.len());

        // Separate chunks and embeddings
        let (chunks, embeddings): (Vec<_>, Vec<_>) = chunks_with_embeddings.into_iter().unzip();

        // Index to Tantivy
        self.index_to_tantivy(&chunks)?;

        // Index to Qdrant (uses ownership, no clone)
        self.index_to_qdrant(chunks, embeddings).await?;

        Ok(())
    }
}
```

**Step 5**: Update mode dispatcher to use parallel by default

```rust
impl Default for IndexingMode {
    fn default() -> Self {
        Self::Parallel  // Changed from Sequential
    }
}

impl UnifiedIndexer {
    pub async fn index_directory_with_mode(
        &mut self,
        dir_path: &Path,
        mode: IndexingMode
    ) -> Result<IndexStats> {
        match mode {
            IndexingMode::Sequential => {
                tracing::info!("Using SEQUENTIAL mode");
                self.index_directory_sequential(dir_path).await
            }
            IndexingMode::Parallel => {
                self.index_directory_parallel(dir_path).await
            }
            IndexingMode::Pipeline => {
                tracing::warn!("Pipeline mode not yet implemented, using parallel");
                self.index_directory_parallel(dir_path).await
            }
        }
    }
}
```

### Testing

```bash
# Unit tests
cargo test indexing::unified::tests

# Test parallel mode explicitly
cargo run --release -- index /path/to/burn --mode parallel

# Test default (should use parallel)
cargo run --release -- index /path/to/burn

# Compare with sequential
cargo run --release -- index /path/to/burn --mode sequential
```

### Verification

- ✅ Parallel mode 4-6x faster than sequential
- ✅ All files indexed correctly
- ✅ No chunks missing or duplicated
- ✅ Search quality unchanged

---

## Task 1.2: Async File I/O

**Priority**: P1
**Effort**: 3-4 hours
**Impact**: 20-30% additional speedup
**Risk**: Low

### Problem

`std::fs::read_to_string()` blocks the async executor, preventing concurrent work. With 8-12 parallel workers, this creates bottlenecks.

### Solution

Replace with `tokio::fs` for non-blocking I/O.

### Locations

High-priority replacements:
1. `src/indexing/unified.rs:225` - Main file reading (hot path)
2. `src/indexing/merkle.rs:108, 113` - Merkle tree construction
3. Worker function (from Task 1.1)

### Implementation

**In `index_file_worker()` (Task 1.1)**:

```rust
// Before
let content = std::fs::read_to_string(file_path)
    .context(format!("Failed to read file: {}", file_path.display()))?;

// After
let content = tokio::fs::read_to_string(file_path).await
    .context(format!("Failed to read file: {}", file_path.display()))?;
```

**In `src/indexing/merkle.rs`**:

```rust
// Update FileSystemMerkle::from_directory() to async
pub async fn from_directory(directory: &Path) -> Result<Self> {
    // ...
    let tree = Self::build_tree(&files).await?;  // Now async
    // ...
}

async fn build_tree(files: &[PathBuf]) -> Result<Vec<MerkleNode>> {
    // Use rayon for parallelism (Task 1.3), but async file reads
    for (idx, path) in files.iter().enumerate() {
        let content = tokio::fs::read(path).await?;  // Async!
        let hash = Sha256Hasher::hash(&content);
        let metadata = tokio::fs::metadata(path).await?;  // Async!
        // ...
    }
}
```

**Update all callers**:

```rust
// Before
let merkle = FileSystemMerkle::from_directory(path)?;

// After
let merkle = FileSystemMerkle::from_directory(path).await?;
```

### Testing

```bash
cargo test indexing::merkle::tests
cargo test indexing::unified::tests
```

---

## Task 1.3: Parallel Merkle Tree Hashing

**Priority**: P1
**Effort**: 2-3 hours
**Impact**: 4-6x faster Merkle tree construction
**Risk**: Low

### Problem

Merkle tree construction reads and hashes files sequentially. For 1,500 files, this is slow.

### Solution

Use `rayon` for parallel file hashing (CPU-bound work).

### Location

`src/indexing/merkle.rs:107-124`

### Implementation

**Step 1**: Add dependency

```toml
[dependencies]
rayon = "1.8"
```

**Step 2**: Parallelize file hashing

```rust
use rayon::prelude::*;

async fn build_tree(files: &[PathBuf]) -> Result<Vec<MerkleNode>> {
    tracing::info!("Building Merkle tree for {} files (parallel)", files.len());

    // Parallel file hashing using rayon
    let file_nodes: Vec<(PathBuf, FileNode)> = files
        .par_iter()  // Parallel iterator!
        .enumerate()
        .filter_map(|(idx, path)| {
            // Read and hash in parallel (using blocking IO for rayon threads)
            let content = std::fs::read(path).ok()?;
            let hash = Sha256Hasher::hash(&content);
            let metadata = std::fs::metadata(path).ok()?;
            let last_modified = metadata.modified().ok()?;

            Some((
                path.clone(),
                FileNode {
                    content_hash: hash,
                    leaf_index: idx,
                    last_modified,
                }
            ))
        })
        .collect();

    // Build file_map from results
    let mut file_map = HashMap::new();
    for (path, node) in file_nodes {
        file_map.insert(path, node);
    }

    // Build tree structure
    // ... rest of implementation
}
```

**Note**: Uses `std::fs` (not `tokio::fs`) because rayon uses thread pool, not async runtime.

### Testing

```bash
cargo test indexing::merkle::tests -- --nocapture

# Time difference
time cargo run --release -- index /path/to/burn
```

---

## Phase 1 Testing Strategy

### Unit Tests

```bash
# New parallel mode tests
cargo test indexing::unified::tests::test_parallel_mode

# Merkle tests
cargo test indexing::merkle::tests

# Ensure sequential still works
cargo test indexing::unified::tests::test_sequential_mode
```

### Integration Tests

```bash
# Full integration test
cargo test --test test_phase4_integration -- --ignored --nocapture

# Verify both modes work
cargo test --test test_mode_comparison -- --ignored --nocapture
```

### Benchmarking

**Compare modes**:

```bash
# Sequential (baseline)
time cargo run --release -- index /path/to/burn --mode sequential --force-reindex

# Parallel (Phase 1)
time cargo run --release -- index /path/to/burn --mode parallel --force-reindex
```

**Expected results** (Burn, 1,569 files):
- Sequential: ~500s (with Phase 0 optimizations)
- Parallel: ~100-150s (4-5x faster)

### Performance Validation

Test on multiple codebase sizes:

| Codebase | Files | Sequential | Parallel | Speedup |
|----------|-------|-----------|----------|---------|
| rust-code-mcp | ~50 | 10s | 5s | 2x |
| Medium project | ~500 | 120s | 30s | 4x |
| Burn | 1,569 | 500s | 100s | 5x |

---

## Success Criteria

### Phase 1 Complete When:

- ✅ Parallel mode implemented and working
- ✅ Async file I/O throughout hot paths
- ✅ Parallel Merkle tree hashing
- ✅ Burn indexing (parallel): ~100-150s (4-6x faster than Phase 0)
- ✅ Memory usage < 1.5GB
- ✅ CPU utilization high across all cores
- ✅ All tests passing (unit + integration)
- ✅ No regression in search quality
- ✅ Parallel is default mode
- ✅ Sequential mode still works (for fallback/debugging)

### Quality Gates

1. ✅ All unit tests pass
2. ✅ All integration tests pass
3. ✅ Benchmark shows 4-6x speedup over sequential
4. ✅ Memory usage within limits
5. ✅ No race conditions detected
6. ✅ Error handling works correctly
7. ✅ Documentation updated

---

## Known Issues & Solutions

### Issue 1: MetadataCache Not Thread-Safe

**Problem**: sled::Db needs to be wrapped in Arc for sharing.

**Solution**:
```rust
#[derive(Clone)]
pub struct MetadataCache {
    db: Arc<sled::Db>,  // Wrapped in Arc
}
```

### Issue 2: Embedding Model Thread Safety

**Problem**: fastembed may not be thread-safe.

**Solution**: Each worker clones the EmbeddingGenerator. If issues arise, use Arc<Mutex<>> or channels.

### Issue 3: High Memory Usage

**Problem**: 12 concurrent workers × embeddings can use significant memory.

**Solution**: Reduce concurrency or add memory limits:
```rust
let concurrency = if available_memory_gb < 4 {
    4  // Low memory mode
} else {
    num_cpus::get().min(12)
};
```

---

## Rollback Plan

1. **Change default back to sequential**:
   ```rust
   impl Default for IndexingMode {
       fn default() -> Self {
           Self::Sequential
       }
   }
   ```

2. **Disable parallel mode** via environment variable:
   ```rust
   let force_sequential = std::env::var("FORCE_SEQUENTIAL").is_ok();
   ```

3. **Revert individual tasks** independently if needed

---

## Next Steps

After Phase 1 completion:

1. Benchmark on various codebase sizes
2. Document performance characteristics
3. Collect user feedback on parallel mode
4. Decide if Phase 2 (Pipeline mode) is needed
5. If yes, proceed to **PHASE_2.md**

---

## References

- Main plan: `SPEED.md`
- Previous phase: `PHASE_0.md`
- Next phase: `PHASE_2.md`
- Analysis: `LIST.md` - Strategy #1 (Parallel file processing)

---

**Document Version**: 1.0
**Last Updated**: 2025-10-22
**Status**: ✅ Ready for Implementation
