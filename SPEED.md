# Performance Optimization Implementation Plan

**Project**: rust-code-mcp indexing pipeline
**Date**: 2025-10-22
**Status**: Ready for Implementation
**Goal**: 5-10x speedup for large codebase indexing

---

## Executive Summary

Based on comprehensive analysis using both manual code review and MCP tools, we've identified **6 critical bottlenecks** in the indexing pipeline. This document provides a phased implementation plan to achieve **5-10x performance improvement** for indexing large codebases like Burn (1,569 files, 19,075 chunks).

### Verified Critical Findings

1. ✅ **Sequential file processing** - Processing 1,500+ files one at a time (biggest impact)
2. ✅ **BulkIndexer unused** - Complete implementation exists but never used (3-5x speedup available)
3. ✅ **100ms artificial sleep** - Unnecessary delay after every indexing operation
4. ✅ **Synchronous file I/O** - 47 instances of blocking `std::fs` calls
5. ✅ **Unnecessary data cloning** - Multiple string/struct clones per chunk
6. ✅ **No Merkle parallelization** - Sequential file hashing during tree construction

### Architectural Approach: Dual-Mode Implementation

This plan implements **both** parallel file processing (#1) and streaming pipeline (#12) as **runtime-selectable modes**, allowing users to choose the best strategy for their workload via MCP tool parameters.

| Mode | Use Case | Memory | Complexity | Performance |
|------|----------|--------|------------|-------------|
| **Sequential** | Debugging, low-memory systems | ~200MB | Low | Baseline |
| **Parallel** | General use, most codebases (default) | ~1-1.5GB | Medium | 4-6x |
| **Pipeline** | Very large codebases (5000+ files) | ~1.5-2GB | High | 6-10x |

### Expected Performance Gains

| Phase | Effort | Current | Target | Speedup |
|-------|--------|---------|--------|---------|
| Quick Wins | 2-4 hours | 626s | 500-530s | 1.2-1.3x |
| Parallel Mode | 2-3 days | 626s | 100-150s | 4-6x |
| Pipeline Mode | 1-2 weeks | 626s | 60-100s | 6-10x |

---

## Runtime Mode Selection Architecture

### MCP Tool Parameter

Users will be able to select the indexing mode via the `index_codebase` MCP tool:

```json
{
  "tool": "index_codebase",
  "arguments": {
    "directory": "/path/to/codebase",
    "force_reindex": false,
    "indexing_mode": "parallel"  // Options: "sequential", "parallel", "pipeline"
  }
}
```

### Implementation Structure

**File**: `src/tools/index_tool.rs`

```rust
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct IndexCodebaseParams {
    #[schemars(description = "Absolute path to codebase directory")]
    pub directory: String,

    #[schemars(description = "Force full reindex even if already indexed (default: false)")]
    pub force_reindex: Option<bool>,

    #[schemars(description = "Indexing mode: 'parallel' (default), 'pipeline', or 'sequential'")]
    pub indexing_mode: Option<IndexingMode>,
}

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
        Self::Parallel  // Default to parallel mode
    }
}
```

**File**: `src/indexing/unified.rs`

```rust
impl UnifiedIndexer {
    /// Index directory with specified mode (main entry point)
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
                tracing::info!("Using parallel indexing mode (file-level parallelism)");
                self.index_directory_parallel(dir_path).await
            }
            IndexingMode::Pipeline => {
                tracing::info!("Using pipeline indexing mode (streaming stages)");
                self.index_directory_pipeline(dir_path).await
            }
        }
    }

    /// Sequential mode (original implementation, kept for baseline)
    async fn index_directory_sequential(&mut self, dir_path: &Path) -> Result<IndexStats> {
        // Current implementation (lines 363-415)
    }

    /// Parallel mode (Task 1.1) - file-level parallelism
    async fn index_directory_parallel(&mut self, dir_path: &Path) -> Result<IndexStats> {
        // Implemented in Phase 1
    }

    /// Pipeline mode (Task 2.2) - streaming stages
    async fn index_directory_pipeline(&mut self, dir_path: &Path) -> Result<IndexStats> {
        // Implemented in Phase 2
    }
}
```

### Mode Selection Logic (Optional Auto-Detection)

```rust
impl IndexingMode {
    /// Recommend mode based on codebase characteristics
    pub fn recommend(file_count: usize, available_memory_gb: usize) -> Self {
        match (file_count, available_memory_gb) {
            // Low memory systems
            (_, mem) if mem < 2 => Self::Parallel,

            // Small codebases - parallel is sufficient
            (files, _) if files < 500 => Self::Parallel,

            // Medium codebases - parallel is optimal
            (files, _) if files < 5000 => Self::Parallel,

            // Very large codebases - pipeline gives best performance
            _ => Self::Pipeline,
        }
    }
}
```

---

## Phase 0: Quick Wins (2-4 hours)

**Goal**: Low-risk, high-impact changes
**Expected Improvement**: 1.2-1.3x (20-30% faster)
**Risk**: Very Low

### Task 0.1: Remove 100ms Artificial Sleep ⚡

**Priority**: P0
**Effort**: 5 minutes
**Impact**: Instant 100ms savings per indexing operation

**Location**: `src/indexing/unified.rs:404`

**Current Code**:
```rust
// Commit Tantivy changes
self.tantivy_writer.commit().context("Failed to commit Tantivy index")?;

// Wait briefly to ensure index is fully committed
tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
```

**Change**:
```rust
// Commit Tantivy changes
self.tantivy_writer.commit().context("Failed to commit Tantivy index")?;

// Note: No sleep needed - commit() is synchronous and completes before returning
```

**Testing**:
```bash
cargo test --test test_phase4_integration
```

**Verification**: Confirm indexing still works correctly, timing reduced by 100ms.

---

### Task 0.2: Integrate BulkIndexer for Force Reindex 🔥

**Priority**: P0
**Effort**: 2-3 hours
**Impact**: 3-5x speedup for Qdrant insertion during force reindex

**Problem**: `src/indexing/bulk.rs` has a complete `BulkIndexer` implementation that disables HNSW during bulk operations and rebuilds once at end, but it's **never used in production code**.

**Implementation**:

**Location 1**: `src/tools/index_tool.rs:98-118`

Add BulkIndexer integration:

```rust
// After line 110
.await
.map_err(|e| {
    McpError::invalid_params(format!("Failed to initialize indexer: {}", e), None)
})?;

// Add this section before clearing data
// Enable bulk mode if force reindexing
let bulk_indexer = if force {
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

**Location 2**: After indexing completes (around line 126)

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

**Testing**:
```bash
# Test force reindex with bulk mode
cargo test --test test_phase2_integration -- test_bulk_indexing_mode --ignored --nocapture
```

**Expected Result**: Force reindex operations should be 3-5x faster for Qdrant insertion.

---

### Task 0.3: Remove Unnecessary CodeChunk Clones

**Priority**: P0
**Effort**: 30 minutes
**Impact**: 10-15% reduction in memory allocations

**Location**: `src/indexing/unified.rs:348-352`

**Current Code**:
```rust
let chunk_data: Vec<(ChunkId, Embedding, CodeChunk)> = chunks
    .iter()
    .zip(embeddings.into_iter())
    .map(|(chunk, embedding)| (chunk.id, embedding, chunk.clone())) // ❌ Clone!
    .collect();
```

**Change**:
```rust
// Change function signature to take ownership
async fn index_to_qdrant(
    &self,
    chunks: Vec<CodeChunk>,  // Take ownership instead of &[CodeChunk]
    embeddings: Vec<Embedding>,
) -> Result<()> {
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
}
```

**Update caller at line 292**:
```rust
// Before (line 291-292)
self.index_to_tantivy(&chunks)?;
self.index_to_qdrant(&chunks, embeddings).await?;

// After - need to clone for Tantivy, move for Qdrant
self.index_to_tantivy(&chunks)?;
self.index_to_qdrant(chunks, embeddings).await?;  // Move chunks
```

**Alternative** (if Tantivy needs chunks afterward):
```rust
// Clone only what Tantivy needs
self.index_to_tantivy(&chunks)?;
let chunks_for_qdrant = chunks; // Already have them
self.index_to_qdrant(chunks_for_qdrant, embeddings).await?;
```

**Testing**:
```bash
cargo test unified::tests::test_index_file
```

---

## Phase 1: Parallel Mode Implementation (2-3 days)

**Goal**: Enable concurrent file processing with file-level parallelism
**Expected Improvement**: 4-6x overall (626s → 100-150s)
**Risk**: Medium (requires careful state management)

**Note**: This implements the **Parallel** mode, which will be the default indexing strategy.

### Task 1.1: Implement Parallel Mode (File-Level Parallelism)

**Priority**: P1
**Effort**: 1-2 days
**Impact**: 5-8x speedup (biggest single improvement)

**Mode**: `IndexingMode::Parallel`

**Location**: `src/indexing/unified.rs` - new method `index_directory_parallel()`

**Current Problem (Sequential mode)**:
```rust
for file in rust_files {
    match self.index_file(&file).await {
        // Process one file at a time
    }
}
```

**Strategy**: Use message passing to avoid `UnifiedIndexer` cloning issues, process 8-12 files concurrently

**Implementation**:

```rust
use tokio::sync::{mpsc, Semaphore};
use std::sync::Arc;

/// Index an entire directory with parallel file processing
pub async fn index_directory(&mut self, dir_path: &Path) -> Result<IndexStats> {
    tracing::info!("Indexing directory: {}", dir_path.display());

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
    tracing::info!("Found {} Rust files in {}", rust_files.len(), dir_path.display());

    // Determine concurrency level (default: CPU cores, max: 12)
    let concurrency = num_cpus::get().min(12);
    tracing::info!("Processing files with {} concurrent workers", concurrency);

    // Create channels for results
    let (result_tx, mut result_rx) = mpsc::channel::<Result<IndexFileResult>>(100);

    // Create semaphore to limit concurrency
    let semaphore = Arc::new(Semaphore::new(concurrency));

    // Spawn worker tasks
    for file in rust_files {
        let tx = result_tx.clone();
        let permit = semaphore.clone().acquire_owned().await?;

        // Clone necessary components for the task
        let parser = self.parser.clone();
        let chunker = self.chunker.clone();
        let embedding_generator = self.embedding_generator.clone();
        let metadata_cache = self.metadata_cache.clone();
        let secrets_scanner = self.secrets_scanner.clone();
        let file_filter = self.file_filter.clone();

        tokio::spawn(async move {
            // Process file
            let result = Self::index_file_static(
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

    // Drop our copy of sender so channel closes when workers finish
    drop(result_tx);

    // Collect results
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

    // Batch index all chunks to Tantivy and Qdrant
    if !all_chunks.is_empty() {
        self.batch_index_chunks(all_chunks).await?;
    }

    // Commit Tantivy changes
    self.tantivy_writer.commit().context("Failed to commit Tantivy index")?;

    tracing::info!(
        "✓ Indexing complete: {} files indexed, {} chunks, {} unchanged, {} skipped",
        stats.indexed_files,
        stats.total_chunks,
        stats.unchanged_files,
        stats.skipped_files
    );

    Ok(stats)
}

/// Static version of index_file for parallel processing
/// Returns chunks instead of immediately indexing them
async fn index_file_static(
    file_path: &Path,
    parser: &RustParser,
    chunker: &Chunker,
    embedding_generator: &EmbeddingGenerator,
    metadata_cache: &MetadataCache,
    secrets_scanner: &SecretsScanner,
    file_filter: &SensitiveFileFilter,
) -> Result<IndexFileResult> {
    // Same logic as index_file but returns chunks
    // Implementation details...
    // Returns IndexFileResult::Indexed { chunks_count, chunks: Vec<(CodeChunk, Embedding)> }
}

/// Batch index chunks to both Tantivy and Qdrant
async fn batch_index_chunks(&mut self, chunks_with_embeddings: Vec<(CodeChunk, Embedding)>) -> Result<()> {
    // Separate chunks and embeddings
    let (chunks, embeddings): (Vec<_>, Vec<_>) = chunks_with_embeddings.into_iter().unzip();

    // Index to Tantivy
    self.index_to_tantivy(&chunks)?;

    // Index to Qdrant
    self.index_to_qdrant(chunks, embeddings).await?;

    Ok(())
}
```

**Required Changes**:

1. **Add dependency** to `Cargo.toml`:
```toml
num_cpus = "1.16"
```

2. **Make components Clone**:
   - `RustParser`: Already Clone
   - `Chunker`: Already Clone
   - `EmbeddingGenerator`: Already Clone
   - `MetadataCache`: Add `#[derive(Clone)]` or use `Arc<Mutex<>>`
   - `SecretsScanner`: Add `#[derive(Clone)]`
   - `SensitiveFileFilter`: Already Clone

3. **Update `IndexFileResult`** to optionally carry chunks:
```rust
pub enum IndexFileResult {
    Indexed {
        chunks_count: usize,
        chunks: Vec<(CodeChunk, Embedding)>,  // Add this
    },
    Unchanged,
    Skipped,
}
```

**Testing**:
```bash
# Test parallel indexing
cargo test --test test_phase4_integration -- --nocapture

# Benchmark comparison
cargo test --release -- --ignored bench_sequential_vs_parallel
```

**Rollback Plan**: Keep original `index_directory()` as `index_directory_sequential()` for fallback.

---

### Task 1.2: Async File I/O

**Priority**: P1
**Effort**: 3-4 hours
**Impact**: 20-30% reduction in I/O blocking

**Locations** (47 total instances):
- `src/indexing/unified.rs:225` - Main file reading
- `src/indexing/merkle.rs:108, 113` - Merkle tree file hashing
- Various other locations

**Strategy**: Replace critical path `std::fs` with `tokio::fs`

**Priority Replacements**:

1. **unified.rs:225** (Hot path):
```rust
// Before
let content = std::fs::read_to_string(file_path)
    .context(format!("Failed to read file: {}", file_path.display()))?;

// After
let content = tokio::fs::read_to_string(file_path).await
    .context(format!("Failed to read file: {}", file_path.display()))?;
```

2. **merkle.rs:108** (Merkle tree construction):
```rust
// Before
let content = std::fs::read(path)?;

// After
let content = tokio::fs::read(path).await?;
```

3. **merkle.rs:113** (Metadata reading):
```rust
// Before
let metadata = std::fs::metadata(path)?;

// After
let metadata = tokio::fs::metadata(path).await?;
```

**Function Signature Changes**:
- `FileSystemMerkle::from_directory()` → async
- `FileSystemMerkle::build_tree()` → async

**Testing**:
```bash
cargo test indexing::merkle::tests
cargo test indexing::unified::tests
```

---

### Task 1.3: Parallel Merkle Tree Hashing

**Priority**: P1
**Effort**: 2-3 hours
**Impact**: 4-6x faster Merkle tree construction

**Location**: `src/indexing/merkle.rs:107-124`

**Current Code**:
```rust
for (idx, path) in files.iter().enumerate() {
    let content = std::fs::read(path)?;
    let hash = Sha256Hasher::hash(&content);
    let metadata = std::fs::metadata(path)?;
    // ...
}
```

**Add dependency** to `Cargo.toml`:
```toml
rayon = "1.8"
```

**New Implementation**:
```rust
use rayon::prelude::*;

// Parallel file hashing
let file_nodes: Vec<(PathBuf, FileNode)> = files
    .par_iter()  // Parallel iterator
    .enumerate()
    .filter_map(|(idx, path)| {
        // Read and hash in parallel
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

let mut file_map = HashMap::new();
for (path, node) in file_nodes {
    file_map.insert(path, node);
}
```

**Note**: This uses `rayon` for CPU parallelism during file reading/hashing. Since it's CPU-bound work, using thread pool is more efficient than tokio tasks.

**Testing**:
```bash
cargo test indexing::merkle::tests -- --nocapture
```

---

## Phase 2: Advanced Optimizations (1-2 weeks)

**Goal**: Squeeze out remaining performance
**Expected Improvement**: 6-10x overall (626s → 60-100s)
**Risk**: Medium-High (architectural changes)

### Task 2.1: Cross-File Embedding Batching

**Priority**: P2
**Effort**: 1-2 days
**Impact**: 10-20% faster embedding generation

**Problem**: Current implementation batches embeddings per-file (typically 5-20 chunks). Embedding models perform much better with larger batches (64-128).

**Location**: `src/indexing/unified.rs:268-277`

**Strategy**: Accumulate chunks across multiple files before generating embeddings

**Implementation**:
```rust
/// Process files in batches for optimal embedding generation
async fn index_directory_batched(&mut self, dir_path: &Path) -> Result<IndexStats> {
    const EMBEDDING_BATCH_SIZE: usize = 128;
    const FILE_BATCH_SIZE: usize = 20;

    let rust_files: Vec<PathBuf> = /* ... find files ... */;

    let mut all_stats = IndexStats::default();

    // Process files in batches
    for file_batch in rust_files.chunks(FILE_BATCH_SIZE) {
        let mut batch_chunks = Vec::new();
        let mut batch_texts = Vec::new();

        // Parse and chunk all files in batch
        for file in file_batch {
            let chunks = self.parse_and_chunk_file(file).await?;

            for chunk in chunks {
                batch_texts.push(chunk.format_for_embedding());
                batch_chunks.push(chunk);
            }
        }

        // Generate embeddings for entire batch
        let embeddings = self.embedding_generator
            .embed_batch(batch_texts)
            .map_err(|e| anyhow::anyhow!("Failed to generate embeddings: {}", e))?;

        // Index batch
        let chunk_data: Vec<_> = batch_chunks.into_iter()
            .zip(embeddings.into_iter())
            .collect();

        self.batch_index_chunks(chunk_data).await?;
    }

    self.commit()?;

    Ok(all_stats)
}
```

**Configuration**: Update `src/embeddings/mod.rs:109` batch size:
```rust
// Before
const DEFAULT_BATCH_SIZE: usize = 32;

// After
const DEFAULT_BATCH_SIZE: usize = 128;
```

**Testing**:
```bash
cargo test embeddings::tests -- --nocapture
```

---

### Task 2.2: Implement Pipeline Mode (Streaming Stages)

**Priority**: P2
**Effort**: 1 week
**Impact**: 2-3x additional speedup over parallel mode through better resource overlap

**Mode**: `IndexingMode::Pipeline`

**Goal**: Overlap I/O and CPU work using producer-consumer streaming pattern

**Location**: `src/indexing/unified.rs` - new method `index_directory_pipeline()` and supporting infrastructure

**Use Case**: Very large codebases (5000+ files) where maximum performance is needed and memory is available

**Architecture** (Producer-Consumer Stages):
```
┌─────────────┐     ┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│  File I/O   │────▶│ Parse+Chunk │────▶│  Embedding  │────▶│   Indexing  │
│  (2 workers)│     │  (4 workers)│     │  (2 workers)│     │  (1 worker) │
└─────────────┘     └─────────────┘     └─────────────┘     └─────────────┘
     I/O bound          CPU bound         GPU/CPU bound        I/O bound
```

**Key Difference from Parallel Mode**:
- **Parallel**: Processes entire files concurrently (file-level parallelism)
- **Pipeline**: Processes stages concurrently with work streaming between them (stage-level parallelism)

**Implementation**:
```rust
pub struct StreamingPipeline {
    // Channels for each stage
    file_tx: mpsc::Sender<PathBuf>,
    chunk_tx: mpsc::Sender<(PathBuf, Vec<CodeChunk>)>,
    embed_tx: mpsc::Sender<(Vec<CodeChunk>, Vec<Embedding>)>,
}

impl StreamingPipeline {
    pub async fn new() -> Self {
        let (file_tx, file_rx) = mpsc::channel(100);
        let (chunk_tx, chunk_rx) = mpsc::channel(500);
        let (embed_tx, embed_rx) = mpsc::channel(500);

        // Stage 1: File reading (2 workers)
        for _ in 0..2 {
            tokio::spawn(file_reader_worker(file_rx.clone(), chunk_tx.clone()));
        }

        // Stage 2: Parsing + chunking (4 workers)
        for _ in 0..4 {
            tokio::spawn(parse_chunk_worker(chunk_rx.clone(), embed_tx.clone()));
        }

        // Stage 3: Embedding (2 workers, shared model)
        for _ in 0..2 {
            tokio::spawn(embedding_worker(embed_rx.clone()));
        }

        // Stage 4: Indexing (1 worker)
        tokio::spawn(indexing_worker(/* ... */));

        Self { file_tx, chunk_tx, embed_tx }
    }
}
```

**Benefits**:
- I/O and CPU work happen simultaneously across stages
- Better resource utilization than parallel mode
- Predictable memory usage (bounded channels)
- Higher throughput for very large codebases

**Tradeoffs**:
- Higher complexity than parallel mode
- More difficult to debug
- Requires more memory (~1.5-2GB vs ~1GB)
- Best for 5000+ file codebases

**When to Use**:
- Very large codebases where parallel mode isn't fast enough
- Systems with 4GB+ RAM
- When maximum performance is critical

**When NOT to Use**:
- Small/medium codebases (parallel mode is sufficient)
- Low-memory systems
- When debugging indexing issues (use sequential or parallel)

---

## Phase 3: Memory & I/O Optimization (Optional)

### Task 3.1: Increase Tantivy Memory Budget

**Priority**: P2
**Effort**: 15 minutes
**Impact**: 1.2-1.3x faster Tantivy indexing

**Location**: `src/indexing/unified.rs:148-158`

**Current Config**:
```rust
let (memory_budget_mb, num_threads) = if let Some(loc) = codebase_loc {
    if loc < 100_000 {
        (50, 2)
    } else if loc < 1_000_000 {
        (100, 4)  // ← Burn is here
    } else {
        (200, 8)
    }
} else {
    (50, 2)
};
```

**Optimized Config**:
```rust
let (memory_budget_mb, num_threads) = if let Some(loc) = codebase_loc {
    if loc < 100_000 {
        (50, 2)
    } else if loc < 1_000_000 {
        (200, 4)  // ← Increased from 100MB to 200MB
    } else {
        (400, 8)  // ← Increased from 200MB to 400MB
    }
} else {
    (100, 2)  // ← Increased default
};
```

**Testing**: Verify no OOM errors on target systems.

---

### Task 3.2: Tune Qdrant Batch Sizes

**Priority**: P2
**Effort**: 30 minutes
**Impact**: 3-5% faster Qdrant insertion

**Location**: `src/vector_store/mod.rs:217-226`

**Current**:
```rust
for batch in points.chunks(100) {
    self.client.upsert_points(...).await?;
}
```

**Dynamic Sizing**:
```rust
// Choose batch size based on total points
let batch_size = if points.len() < 1000 {
    50
} else if points.len() < 10000 {
    100
} else if points.len() < 50000 {
    200
} else {
    500
};

for batch in points.chunks(batch_size) {
    self.client.upsert_points(...).await?;
}
```

---

## Testing Strategy

### Unit Tests
```bash
# Run all tests
cargo test

# Run specific module tests
cargo test indexing::unified::tests
cargo test indexing::merkle::tests
cargo test embeddings::tests
```

### Integration Tests
```bash
# Phase 4 integration tests
cargo test --test test_phase4_integration -- --ignored --nocapture

# Phase 2 bulk indexing tests
cargo test --test test_phase2_integration -- --ignored --nocapture
```

### Benchmarking

**File**: `benches/indexing_bench.rs`

```rust
use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use rust_code_mcp::tools::index_tool::IndexingMode;

fn bench_indexing_modes(c: &mut Criterion) {
    let mut group = c.benchmark_group("indexing_modes");

    // Test on Burn codebase (or similar large codebase)
    let test_dir = "/path/to/burn";

    // Benchmark sequential mode
    group.bench_with_input(
        BenchmarkId::new("sequential", "burn"),
        &test_dir,
        |b, dir| {
            b.iter(|| {
                // Run with IndexingMode::Sequential
            });
        },
    );

    // Benchmark parallel mode
    group.bench_with_input(
        BenchmarkId::new("parallel", "burn"),
        &test_dir,
        |b, dir| {
            b.iter(|| {
                // Run with IndexingMode::Parallel
            });
        },
    );

    // Benchmark pipeline mode
    group.bench_with_input(
        BenchmarkId::new("pipeline", "burn"),
        &test_dir,
        |b, dir| {
            b.iter(|| {
                // Run with IndexingMode::Pipeline
            });
        },
    );

    group.finish();
}

criterion_group!(benches, bench_indexing_modes);
criterion_main!(benches);
```

**Run benchmarks**:
```bash
# Benchmark all modes
cargo bench --bench indexing_bench

# Compare modes directly
cargo run --release -- index /path/to/burn --mode sequential
cargo run --release -- index /path/to/burn --mode parallel
cargo run --release -- index /path/to/burn --mode pipeline
```

**Expected Results** (Burn codebase, 1,569 files):
- **Sequential**: ~626 seconds (baseline)
- **Parallel**: ~100-150 seconds (4-6x faster)
- **Pipeline**: ~60-100 seconds (6-10x faster)

### Real-World Testing

**Test Codebases**:
1. **Small**: rust-code-mcp itself (~50 files)
2. **Medium**: Burn framework (1,569 files)
3. **Large**: Find 5,000+ file Rust project

**Metrics to Track**:
- Total indexing time
- Files per second
- Chunks per second
- Memory usage (peak and average)
- CPU utilization

**Before/After Comparison**:
```bash
# Before optimization
time cargo run --release -- index /path/to/burn

# After each phase
time cargo run --release -- index /path/to/burn --force-reindex
```

---

## Risk Mitigation

### Rollback Strategy

1. **Feature Flags**: Implement optimizations behind feature flags
```toml
[features]
default = ["parallel-indexing"]
parallel-indexing = []
bulk-mode = []
```

2. **Dual Implementations**: Keep sequential versions
```rust
pub async fn index_directory(&mut self, dir_path: &Path) -> Result<IndexStats> {
    #[cfg(feature = "parallel-indexing")]
    return self.index_directory_parallel(dir_path).await;

    #[cfg(not(feature = "parallel-indexing"))]
    return self.index_directory_sequential(dir_path).await;
}
```

3. **Environment Variable Overrides**:
```rust
let use_parallel = std::env::var("RUST_CODE_MCP_PARALLEL")
    .unwrap_or_else(|_| "true".to_string()) == "true";
```

### Known Risks

| Risk | Severity | Mitigation |
|------|----------|------------|
| Increased memory usage | Medium | Monitor with `top`, add memory limits |
| Race conditions in parallel code | High | Extensive testing, message passing |
| Breaking incremental indexing | High | Comprehensive tests, validate change detection |
| Qdrant connection exhaustion | Medium | Connection pooling, rate limiting |
| Embedding model thread safety | Medium | Use Arc<Mutex<>> or channels |

---

## Success Criteria

### Performance Targets

**Phase 0 Success**:
- ✅ Burn indexing: 626s → 500-530s (15-20% faster)
- ✅ No regression in search quality
- ✅ Memory usage < 500MB
- ✅ All tests passing

**Phase 1 Success** (Parallel Mode):
- ✅ Burn indexing (parallel mode): 626s → 100-150s (4-6x faster)
- ✅ Memory usage < 1.5GB
- ✅ Parallel processing working correctly
- ✅ All tests passing
- ✅ No degradation in incremental indexing
- ✅ Mode selection via MCP tool parameter working

**Phase 2 Success** (Pipeline Mode):
- ✅ Burn indexing (pipeline mode): 626s → 60-100s (6-10x faster)
- ✅ Memory usage < 2GB
- ✅ Streaming pipeline stable
- ✅ All tests passing
- ✅ Pipeline mode faster than parallel mode on large codebases
- ✅ All three modes (sequential/parallel/pipeline) functional

### Quality Gates

Before merging each phase:
1. ✅ All unit tests pass
2. ✅ All integration tests pass
3. ✅ Benchmark shows expected improvement
4. ✅ Code review completed
5. ✅ Documentation updated
6. ✅ No memory leaks detected
7. ✅ Search quality unchanged (spot check)

---

## Implementation Timeline

### Week 1: Quick Wins + Mode Selection Setup
- **Day 1**: Task 0.1 (100ms sleep) + Task 0.3 (clone removal)
- **Day 2**: Task 0.2 (BulkIndexer integration)
- **Day 3**: Add IndexingMode enum and MCP tool parameter
- **Day 4**: Rename current `index_directory()` to `index_directory_sequential()`
- **Day 5**: Testing Phase 0, benchmark, documentation

### Week 2: Parallel Mode Implementation
- **Day 1-2**: Implement Task 1.1 (parallel mode - `index_directory_parallel()`)
- **Day 3**: Implement Task 1.2 (async file I/O)
- **Day 4**: Implement Task 1.3 (parallel Merkle)
- **Day 5**: Integration testing, benchmark parallel vs sequential

### Week 3: Refinements
- Task 2.1 (cross-file batching) + Task 3.x (tuning)
- Benchmark and optimize parallel mode

### Week 4-5: Pipeline Mode Implementation (Optional)
- **Week 4**: Implement Task 2.2 (pipeline mode - `index_directory_pipeline()`)
- **Week 5**: Testing, benchmark all three modes, documentation

---

## Monitoring & Observability

### Metrics to Add

**Location**: `src/indexing/unified.rs`

```rust
use std::time::Instant;

pub struct IndexingMetrics {
    pub file_read_time: Duration,
    pub parse_time: Duration,
    pub chunk_time: Duration,
    pub embedding_time: Duration,
    pub tantivy_index_time: Duration,
    pub qdrant_index_time: Duration,
    pub merkle_time: Duration,
}

impl UnifiedIndexer {
    fn log_metrics(&self, metrics: &IndexingMetrics) {
        tracing::info!("Indexing Performance Breakdown:");
        tracing::info!("  File I/O:      {:?}", metrics.file_read_time);
        tracing::info!("  Parsing:       {:?}", metrics.parse_time);
        tracing::info!("  Chunking:      {:?}", metrics.chunk_time);
        tracing::info!("  Embeddings:    {:?}", metrics.embedding_time);
        tracing::info!("  Tantivy:       {:?}", metrics.tantivy_index_time);
        tracing::info!("  Qdrant:        {:?}", metrics.qdrant_index_time);
        tracing::info!("  Merkle Tree:   {:?}", metrics.merkle_time);
    }
}
```

### Tracing

Enable detailed tracing:
```bash
RUST_LOG=rust_code_mcp=debug cargo run -- index /path/to/codebase
```

---

## Dependencies to Add

```toml
[dependencies]
num_cpus = "1.16"  # For detecting CPU cores
rayon = "1.8"      # For parallel iteration

[dev-dependencies]
criterion = "0.5"  # For benchmarking
```

---

## Documentation Updates

Files to update:
1. `README.md` - Add performance benchmarks section
2. `CHANGELOG.md` - Document all optimizations
3. `ARCHITECTURE.md` - Explain parallel indexing design
4. Code comments - Document concurrency decisions

---

## Appendix A: Mode Comparison Matrix

### Implementation Features by Phase

| Feature | Phase 0 (Quick Wins) | Phase 1 (Parallel) | Phase 2 (Pipeline) |
|---------|---------------------|-------------------|-------------------|
| File processing | Sequential | Parallel (8-12 files) | Streaming stages |
| File I/O | Sync | Async | Async |
| Merkle hashing | Sequential | Parallel (4-6x) | Parallel (4-6x) |
| Bulk mode | **Enabled** | Enabled | Enabled |
| Embedding batch | 32/file | 32/file | 128 cross-file |
| Memory usage | 200MB | 1-1.5GB | 1.5-2GB |
| Code complexity | Low | Medium | High |

### Runtime Mode Characteristics

| Characteristic | Sequential | Parallel | Pipeline |
|---------------|-----------|----------|----------|
| **Architecture** | Original loop | File-level parallelism | Stage-level streaming |
| **Concurrency** | None | 8-12 files | Multiple stages |
| **Memory Usage** | ~200MB | ~1-1.5GB | ~1.5-2GB |
| **CPU Utilization** | Low (single-core) | High (multi-core) | Very High (pipelined) |
| **Best For** | Debugging | General use | Very large codebases |
| **Performance** | Baseline (626s) | 4-6x faster (100-150s) | 6-10x faster (60-100s) |
| **Complexity** | Low | Medium | High |
| **When to Use** | Testing, low memory | Default mode | 5000+ files, max speed |

---

## Appendix B: Code Locations Reference

Quick reference for all modifications:

| Task | File | Lines | Change |
|------|------|-------|--------|
| 0.1 | `src/indexing/unified.rs` | 404 | Remove sleep |
| 0.2 | `src/tools/index_tool.rs` | 98-126 | Add BulkIndexer |
| 0.3 | `src/indexing/unified.rs` | 348-352 | Remove clone |
| 1.1 | `src/indexing/unified.rs` | 363-415 | Parallel processing |
| 1.2 | `src/indexing/unified.rs` | 225 | Async file I/O |
| 1.2 | `src/indexing/merkle.rs` | 108, 113 | Async file I/O |
| 1.3 | `src/indexing/merkle.rs` | 107-124 | Parallel hashing |
| 2.1 | `src/indexing/unified.rs` | 268-277 | Cross-file batching |
| 2.2 | `src/indexing/pipeline.rs` | New file | Streaming pipeline |
| 3.1 | `src/indexing/unified.rs` | 148-158 | Memory tuning |
| 3.2 | `src/vector_store/mod.rs` | 217-226 | Batch size tuning |

---

## Appendix C: Dual-Mode Strategy Advantages

### Why Implement Both Parallel and Pipeline Modes?

**1. User Flexibility**
- Users can choose the best mode for their specific workload
- No "one size fits all" - different codebases have different needs
- Easy to switch modes via MCP tool parameter

**2. Benchmarking & Comparison**
- Can A/B test performance on specific codebases
- Understand which architectural pattern works best
- Collect real-world data to refine future optimizations

**3. Graceful Degradation**
- If pipeline mode has issues, fall back to parallel
- If parallel has issues, fall back to sequential
- Multiple fallback options increase reliability

**4. Educational Value**
- Demonstrates two distinct parallelization strategies
- Shows trade-offs between simplicity and performance
- Valuable for understanding Rust concurrency patterns

**5. Future-Proofing**
- Can add more modes later (e.g., GPU-accelerated embedding)
- Architecture supports experimentation
- Don't lock into single approach

### Architecture Decision: Complementary, Not Competing

From LIST.md analysis, parallel (#1) and pipeline (#12) were flagged as **conflicting**. We resolve this by:

✅ **Making them runtime-selectable modes** (not both active simultaneously)
✅ **Keeping sequential as baseline** for comparison
✅ **Defaulting to parallel** for safety and broad compatibility
✅ **Offering pipeline for power users** who need maximum performance

This gives us the **best of all worlds** without architectural conflicts.

### Usage Examples

```bash
# Default (parallel mode)
mcp index_codebase --directory /path/to/codebase

# Explicit parallel mode
mcp index_codebase --directory /path/to/codebase --mode parallel

# Pipeline mode for large codebase
mcp index_codebase --directory /path/to/burn --mode pipeline

# Sequential for debugging
mcp index_codebase --directory /path/to/codebase --mode sequential
```

### Performance Expectations by Codebase Size

| Codebase Size | Recommended Mode | Expected Time (Burn as reference) |
|--------------|------------------|----------------------------------|
| Small (<100 files) | Sequential or Parallel | < 10 seconds |
| Medium (100-1000 files) | Parallel | 10-60 seconds |
| Large (1000-5000 files) | Parallel | 60-150 seconds |
| Very Large (5000+ files) | Pipeline | 60-100 seconds |

---

**Document Version**: 2.0
**Last Updated**: 2025-10-22 (Updated with dual-mode strategy)
**Next Review**: After Phase 0 completion
**Status**: ✅ Ready for Implementation
