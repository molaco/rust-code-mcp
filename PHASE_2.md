# Phase 2: Pipeline Mode Implementation Plan

**Project**: rust-code-mcp indexing pipeline optimization
**Phase**: 2 - Pipeline Mode (Streaming Stages)
**Estimated Effort**: 1-2 weeks
**Expected Improvement**: 6-10x over baseline (626s → 60-100s)
**Risk Level**: Medium-High
**Prerequisites**: Phase 0 and Phase 1 completed

---

## Overview

Phase 2 implements **streaming pipeline architecture** - a producer-consumer pattern where different stages of indexing run concurrently with work streaming between them. This provides better resource utilization than Phase 1's file-level parallelism.

**This phase is OPTIONAL** - only implement if parallel mode (Phase 1) doesn't provide sufficient performance for your use case.

### When to Use Pipeline Mode

✅ **Use Pipeline Mode When**:
- Indexing very large codebases (5,000+ files)
- System has 4GB+ RAM
- Maximum performance is critical
- Willing to accept higher complexity

❌ **Don't Use Pipeline Mode When**:
- Small/medium codebases (< 5,000 files) - parallel mode is sufficient
- Low-memory systems (< 4GB RAM)
- Debugging indexing issues - use sequential or parallel

### Architecture Comparison

**Parallel Mode (Phase 1)**:
```
File 1 ┐
File 2 ├─→ [Full Processing] ─→ Batch Index
File 3 ┘
(Process 8-12 complete files concurrently)
```

**Pipeline Mode (Phase 2)**:
```
Stage 1 (I/O):      File A, B → Reading files
                              ↓
Stage 2 (CPU):            Parse A, B, C, D → Parsing & chunking
                                          ↓
Stage 3 (GPU/CPU):                  Embed B, C → Generating embeddings
                                                ↓
Stage 4 (I/O):                            Index A → Indexing to stores

(Multiple stages running concurrently)
```

**Key Difference**: In pipeline mode, **stages overlap** - while Stage 1 reads files, Stage 2 parses, Stage 3 generates embeddings, and Stage 4 indexes. Better resource utilization.

---

## Task 2.1: Cross-File Embedding Batching

**Priority**: P2
**Effort**: 1-2 days
**Impact**: 10-20% faster embedding generation
**Risk**: Low-Medium

### Problem

Current implementation (even in parallel mode) batches embeddings per-file:
- File with 10 chunks → batch of 10
- File with 15 chunks → batch of 15

Embedding models perform much better with larger, consistent batches (64-128 chunks).

### Solution

Accumulate chunks across multiple files before generating embeddings.

### Location

`src/indexing/unified.rs` - enhance parallel mode or create batched variant

### Implementation

**Option A: Enhance Parallel Mode**

Modify `index_directory_parallel()` to batch embeddings across files:

```rust
pub async fn index_directory_parallel(&mut self, dir_path: &Path) -> Result<IndexStats> {
    // ... find files, set up workers ...

    // Instead of processing files individually, batch by chunks
    const EMBEDDING_BATCH_SIZE: usize = 128;
    let mut pending_chunks: Vec<(PathBuf, CodeChunk)> = Vec::new();
    let mut all_indexed_chunks = Vec::new();

    while let Some(result) = result_rx.recv().await {
        match result {
            Ok(IndexFileResult::Indexed { chunks, .. }) => {
                // Add to pending batch
                for chunk in chunks {
                    pending_chunks.push((file_path.clone(), chunk));

                    // Process batch when full
                    if pending_chunks.len() >= EMBEDDING_BATCH_SIZE {
                        let batch = pending_chunks.drain(..EMBEDDING_BATCH_SIZE).collect();
                        let embedded = self.embed_and_index_batch(batch).await?;
                        all_indexed_chunks.extend(embedded);
                    }
                }
            }
            // ... handle other cases ...
        }
    }

    // Process remaining chunks
    if !pending_chunks.is_empty() {
        let embedded = self.embed_and_index_batch(pending_chunks).await?;
        all_indexed_chunks.extend(embedded);
    }

    Ok(stats)
}

async fn embed_and_index_batch(
    &mut self,
    chunks: Vec<(PathBuf, CodeChunk)>
) -> Result<Vec<(CodeChunk, Embedding)>> {
    let chunk_texts: Vec<String> = chunks
        .iter()
        .map(|(_, c)| c.format_for_embedding())
        .collect();

    let embeddings = self.embedding_generator
        .embed_batch(chunk_texts)?;

    let result: Vec<_> = chunks.into_iter()
        .map(|(_, chunk)| chunk)
        .zip(embeddings.into_iter())
        .collect();

    Ok(result)
}
```

**Option B: Separate Batched Mode**

Create a dedicated `index_directory_batched()` that processes in file batches:

```rust
pub async fn index_directory_batched(&mut self, dir_path: &Path) -> Result<IndexStats> {
    const FILE_BATCH_SIZE: usize = 20;  // Process 20 files at a time
    const EMBEDDING_BATCH_SIZE: usize = 128;

    let rust_files: Vec<PathBuf> = /* ... find files ... */;

    for file_batch in rust_files.chunks(FILE_BATCH_SIZE) {
        let mut batch_chunks = Vec::new();

        // Parse and chunk all files in batch
        for file in file_batch {
            let chunks = self.parse_and_chunk_file(file).await?;
            batch_chunks.extend(chunks);
        }

        // Generate embeddings for entire batch (128+ chunks)
        let embeddings = self.embedding_generator
            .embed_batch(batch_chunks.iter().map(|c| c.format_for_embedding()))?;

        // Index batch
        self.batch_index_chunks(
            batch_chunks.into_iter().zip(embeddings.into_iter()).collect()
        ).await?;
    }

    Ok(stats)
}
```

### Testing

```bash
# Test batched embedding
cargo test embeddings::tests::test_large_batch

# Benchmark batch sizes
cargo bench --bench embedding_batch_sizes
```

### Configuration

Update `src/embeddings/mod.rs`:

```rust
// Increase default batch size
const DEFAULT_BATCH_SIZE: usize = 128;  // Was 32
```

---

## Task 2.2: Implement Pipeline Mode (Streaming Stages)

**Priority**: P2
**Effort**: 1 week
**Impact**: 2-3x additional speedup over parallel mode
**Risk**: High (complex architecture)

### Problem

Even with parallel mode, resources aren't optimally utilized:
- I/O and CPU work don't overlap
- Embedding model sits idle while reading files
- Parser sits idle during embedding generation

### Solution

Streaming pipeline with 4 stages, each running concurrently:

```
Stage 1 (I/O)     → Stage 2 (CPU)      → Stage 3 (GPU/CPU)  → Stage 4 (I/O)
File Reading         Parse + Chunk        Embed Chunks          Index to Stores
2 workers           4 workers            2 workers             1 worker
```

### Implementation

**Step 1**: Design channel structure

```rust
// In src/indexing/unified.rs

struct PipelineChannels {
    // Stage 1 → Stage 2: File paths
    files_tx: mpsc::Sender<PathBuf>,
    files_rx: mpsc::Receiver<PathBuf>,

    // Stage 2 → Stage 3: Parsed chunks (no embeddings yet)
    chunks_tx: mpsc::Sender<Vec<CodeChunk>>,
    chunks_rx: mpsc::Receiver<Vec<CodeChunk>>,

    // Stage 3 → Stage 4: Chunks with embeddings
    embedded_tx: mpsc::Sender<Vec<(CodeChunk, Embedding)>>,
    embedded_rx: mpsc::Receiver<Vec<(CodeChunk, Embedding)>>,
}

impl PipelineChannels {
    fn new() -> Self {
        let (files_tx, files_rx) = mpsc::channel(100);
        let (chunks_tx, chunks_rx) = mpsc::channel(500);
        let (embedded_tx, embedded_rx) = mpsc::channel(500);

        Self {
            files_tx,
            files_rx,
            chunks_tx,
            chunks_rx,
            embedded_tx,
            embedded_rx,
        }
    }
}
```

**Step 2**: Implement stage workers

```rust
impl UnifiedIndexer {
    pub async fn index_directory_pipeline(&mut self, dir_path: &Path) -> Result<IndexStats> {
        tracing::info!("Using PIPELINE mode: streaming stages");

        // Find all files
        let rust_files: Vec<PathBuf> = /* ... */;

        // Create channels
        let channels = PipelineChannels::new();

        // Spawn stage workers
        let stage1_handle = self.spawn_stage1_file_reading(channels.files_rx, channels.chunks_tx);
        let stage2_handle = self.spawn_stage2_parsing(channels.chunks_rx, channels.embedded_tx);
        let stage3_handle = self.spawn_stage3_embedding(channels.embedded_rx);

        // Feed files to pipeline
        for file in rust_files {
            channels.files_tx.send(file).await?;
        }
        drop(channels.files_tx);  // Signal no more files

        // Wait for pipeline to complete
        let stats = stage3_handle.await??;

        Ok(stats)
    }

    // Stage 1: File Reading (I/O bound)
    fn spawn_stage1_file_reading(
        &self,
        mut files_rx: mpsc::Receiver<PathBuf>,
        chunks_tx: mpsc::Sender<Vec<CodeChunk>>,
    ) -> JoinHandle<Result<()>> {
        let parser = self.parser.clone();
        let chunker = self.chunker.clone();
        let metadata_cache = self.metadata_cache.clone();
        let file_filter = self.file_filter.clone();

        tokio::spawn(async move {
            // Spawn 2 workers
            let mut workers = JoinSet::new();

            for _ in 0..2 {
                let files_rx = files_rx.clone();
                let chunks_tx = chunks_tx.clone();
                let parser = parser.clone();
                let chunker = chunker.clone();

                workers.spawn(async move {
                    while let Some(file_path) = files_rx.recv().await {
                        // Read file
                        let content = tokio::fs::read_to_string(&file_path).await?;

                        // Check cache
                        // ... metadata checking ...

                        // Send to Stage 2 (parsing)
                        chunks_tx.send((file_path, content)).await?;
                    }
                    Ok(())
                });
            }

            // Wait for all workers
            while let Some(result) = workers.join_next().await {
                result??;
            }

            Ok(())
        })
    }

    // Stage 2: Parsing & Chunking (CPU bound)
    fn spawn_stage2_parsing(
        &self,
        mut files_rx: mpsc::Receiver<(PathBuf, String)>,
        chunks_tx: mpsc::Sender<Vec<CodeChunk>>,
    ) -> JoinHandle<Result<()>> {
        let parser = self.parser.clone();
        let chunker = self.chunker.clone();

        tokio::spawn(async move {
            // Spawn 4 workers (CPU-bound)
            let mut workers = JoinSet::new();

            for _ in 0..4 {
                let files_rx = files_rx.clone();
                let chunks_tx = chunks_tx.clone();
                let parser = parser.clone();
                let chunker = chunker.clone();

                workers.spawn(async move {
                    while let Some((file_path, content)) = files_rx.recv().await {
                        // Parse
                        let parse_result = parser.parse_source_complete(&content)?;

                        // Chunk
                        let chunks = chunker.chunk_file(&file_path, &content, &parse_result)?;

                        // Send to Stage 3 (embedding)
                        if !chunks.is_empty() {
                            chunks_tx.send(chunks).await?;
                        }
                    }
                    Ok(())
                });
            }

            while let Some(result) = workers.join_next().await {
                result??;
            }

            Ok(())
        })
    }

    // Stage 3: Embedding Generation (GPU/CPU bound)
    fn spawn_stage3_embedding(
        &self,
        mut chunks_rx: mpsc::Receiver<Vec<CodeChunk>>,
    ) -> JoinHandle<Result<IndexStats>> {
        let embedding_generator = self.embedding_generator.clone();
        let tantivy_writer = Arc::new(Mutex::new(self.tantivy_writer.clone()));
        let vector_store = self.vector_store.clone();

        tokio::spawn(async move {
            let mut stats = IndexStats::default();

            // Spawn 2 embedding workers (share model via Arc<Mutex<>>)
            let mut workers = JoinSet::new();

            for _ in 0..2 {
                let chunks_rx = chunks_rx.clone();
                let embedding_generator = embedding_generator.clone();

                workers.spawn(async move {
                    let mut local_chunks = Vec::new();

                    while let Some(chunks) = chunks_rx.recv().await {
                        local_chunks.extend(chunks);

                        // Batch embeddings across files (128 chunks)
                        if local_chunks.len() >= 128 {
                            let batch = local_chunks.drain(..128).collect::<Vec<_>>();
                            let texts: Vec<String> = batch.iter()
                                .map(|c| c.format_for_embedding())
                                .collect();

                            let embeddings = embedding_generator.embed_batch(texts)?;

                            // Send to Stage 4 (indexing)
                            let indexed: Vec<_> = batch.into_iter()
                                .zip(embeddings.into_iter())
                                .collect();

                            // Index immediately
                            // ... index to Tantivy and Qdrant ...
                        }
                    }

                    // Process remaining
                    if !local_chunks.is_empty() {
                        // ... embed and index ...
                    }

                    Ok(())
                });
            }

            while let Some(result) = workers.join_next().await {
                result??;
            }

            Ok(stats)
        })
    }
}
```

**Step 3**: Add to mode dispatcher

```rust
impl UnifiedIndexer {
    pub async fn index_directory_with_mode(
        &mut self,
        dir_path: &Path,
        mode: IndexingMode
    ) -> Result<IndexStats> {
        match mode {
            IndexingMode::Sequential => {
                self.index_directory_sequential(dir_path).await
            }
            IndexingMode::Parallel => {
                self.index_directory_parallel(dir_path).await
            }
            IndexingMode::Pipeline => {
                self.index_directory_pipeline(dir_path).await
            }
        }
    }
}
```

### Testing

```bash
# Test pipeline mode
cargo run --release -- index /path/to/burn --mode pipeline --force-reindex

# Compare all modes
./scripts/benchmark_all_modes.sh
```

### Monitoring

Add metrics to track stage performance:

```rust
struct PipelineMetrics {
    stage1_files_processed: AtomicUsize,
    stage2_chunks_generated: AtomicUsize,
    stage3_embeddings_generated: AtomicUsize,
    stage4_chunks_indexed: AtomicUsize,
}
```

---

## Phase 2 Testing Strategy

### Unit Tests

```bash
# Test pipeline components
cargo test indexing::unified::tests::test_pipeline_mode
cargo test indexing::unified::tests::test_stage_workers
```

### Integration Tests

```bash
# Full pipeline test
cargo test --test test_pipeline_integration -- --ignored --nocapture
```

### Benchmarking

**Compare all three modes**:

```bash
# Sequential (baseline)
time cargo run --release -- index /path/to/burn --mode sequential --force-reindex

# Parallel (Phase 1)
time cargo run --release -- index /path/to/burn --mode parallel --force-reindex

# Pipeline (Phase 2)
time cargo run --release -- index /path/to/burn --mode pipeline --force-reindex
```

**Expected results** (Burn, 1,569 files):
- Sequential: ~500s (with Phase 0)
- Parallel: ~100-150s (4-5x faster)
- Pipeline: ~60-100s (6-10x faster, 1.5-2x faster than parallel)

### Performance Validation

| Codebase | Files | Sequential | Parallel | Pipeline | Best Mode |
|----------|-------|-----------|----------|----------|-----------|
| Small | <100 | 10s | 5s | 5s | Parallel |
| Medium | 100-1000 | 120s | 30s | 25s | Parallel |
| Large | 1000-5000 | 500s | 120s | 80s | Pipeline |
| Very Large | 5000+ | 2000s | 500s | 300s | Pipeline |

---

## Success Criteria

### Phase 2 Complete When:

- ✅ Pipeline mode implemented and working
- ✅ Cross-file embedding batching working
- ✅ Burn indexing (pipeline): ~60-100s (6-10x faster than baseline)
- ✅ Pipeline mode faster than parallel on large codebases (5000+ files)
- ✅ Memory usage < 2GB
- ✅ All stages running concurrently (verify with profiling)
- ✅ All tests passing
- ✅ All three modes (sequential/parallel/pipeline) functional
- ✅ Mode selection via MCP tool working correctly

### Quality Gates

1. ✅ All unit tests pass
2. ✅ All integration tests pass
3. ✅ Pipeline mode 1.5-2x faster than parallel on very large codebases
4. ✅ Memory usage within limits
5. ✅ No deadlocks or channel issues
6. ✅ Graceful error handling
7. ✅ Documentation complete

---

## Known Issues & Solutions

### Issue 1: Channel Backpressure

**Problem**: Slow stage causes channels to fill up.

**Solution**: Use bounded channels with appropriate sizes:
```rust
let (tx, rx) = mpsc::channel(100);  // Limit buffering
```

### Issue 2: Shared Embedding Model

**Problem**: Embedding model may not be thread-safe.

**Solution**: Use Arc<Mutex<>> or dedicated embedding worker:
```rust
let embedding_generator = Arc::new(Mutex::new(self.embedding_generator.clone()));
```

### Issue 3: Error Propagation

**Problem**: Error in one stage needs to stop entire pipeline.

**Solution**: Use shared cancellation token:
```rust
use tokio_util::sync::CancellationToken;

let cancel_token = CancellationToken::new();
// Pass to all workers, cancel on error
```

---

## Rollback Plan

1. **Disable pipeline mode**:
   ```rust
   IndexingMode::Pipeline => {
       tracing::warn!("Pipeline mode disabled, using parallel");
       self.index_directory_parallel(dir_path).await
   }
   ```

2. **Fall back to parallel as default** if issues occur

3. **Feature flag** for pipeline mode:
   ```toml
   [features]
   pipeline-mode = []
   ```

---

## Performance Tuning

### Worker Count Tuning

```rust
// Adjust worker counts based on workload
struct PipelineConfig {
    stage1_workers: usize,  // I/O: 2-4
    stage2_workers: usize,  // CPU: 4-8
    stage3_workers: usize,  // GPU/CPU: 1-2
}

impl PipelineConfig {
    fn for_codebase_size(file_count: usize) -> Self {
        if file_count < 1000 {
            Self { stage1_workers: 2, stage2_workers: 4, stage3_workers: 1 }
        } else if file_count < 5000 {
            Self { stage1_workers: 2, stage2_workers: 6, stage3_workers: 2 }
        } else {
            Self { stage1_workers: 4, stage2_workers: 8, stage3_workers: 2 }
        }
    }
}
```

### Channel Size Tuning

```rust
// Tune based on memory constraints
let channel_size = if available_memory_gb < 4 {
    50  // Small buffers
} else {
    200  // Larger buffers for better throughput
};
```

---

## Next Steps

After Phase 2 completion:

1. Benchmark all three modes on various codebases
2. Document when to use each mode
3. Collect user feedback
4. Proceed to **PHASE_3.md** for optional fine-tuning

---

## References

- Main plan: `SPEED.md`
- Previous phases: `PHASE_0.md`, `PHASE_1.md`
- Next phase: `PHASE_3.md`
- Analysis: `LIST.md` - Strategy #12 (Streaming pipeline)

---

**Document Version**: 1.0
**Last Updated**: 2025-10-22
**Status**: ✅ Ready for Implementation (Optional)
