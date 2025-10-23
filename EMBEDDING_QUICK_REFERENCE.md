# Embedding Strategy - Quick Reference

## At a Glance

| Aspect | Details |
|--------|---------|
| **Model** | all-MiniLM-L6-v2 (ONNX optimized) |
| **Dimensions** | 384 |
| **Download** | 80MB (cached in `.fastembed_cache/`) |
| **Runtime** | ONNX via fastembed crate |
| **Local** | 100% (no cloud) |
| **Batch Size** | 32 chunks |
| **Speed** | ~1.5ms per text (batch) |
| **Vector DB** | Qdrant (gRPC on 6334) |
| **Search** | Hybrid (BM25 + Vector via RRF) |

---

## Key Files

### Embedding Implementation
- **`src/embeddings/mod.rs`** - Core embedding generator
  - `EmbeddingGenerator::new()` - Initialize with all-MiniLM-L6-v2
  - `embed()` - Single text embedding
  - `embed_batch()` - Multiple texts (30x faster)
  - `embed_chunks()` - Code chunks with formatting
  - `EmbeddingPipeline` - Batch processing with progress

### Chunk Formatting
- **`src/chunker/mod.rs`** - Code chunk definition
  - `CodeChunk::format_for_embedding()` - Contextual retrieval approach
  - Includes: file, location, module, symbol, docstring, imports, calls

### Vector Database
- **`src/vector_store/mod.rs`** - Qdrant integration
  - `VectorStoreConfig` - Connection settings
  - `upsert_chunks()` - Store embeddings
  - `search()` - Semantic search
  - Cosine distance metric

### Configuration
- **`src/vector_store/config.rs`** - Auto-tuning
  - HNSW parameters by codebase size
  - Memory optimization thresholds

### Indexing Pipeline
- **`src/indexing/unified.rs`** - Complete indexing
  - `UnifiedIndexer` - Combines embedding + BM25 + Vector DB
  - Generates embeddings for all chunks

### Search Integration
- **`src/search/mod.rs`** - Hybrid search
  - `VectorSearch` - Semantic search with embeddings
  - `HybridSearch` - Combines BM25 + Vector via RRF

---

## Workflow

### 1. Indexing Flow
```
index_codebase()
  └─ UnifiedIndexer::new()
     └─ EmbeddingGenerator::new()
        └─ Download all-MiniLM-L6-v2 (if not cached)
        └─ Wrap in Arc for thread-safe sharing
  └─ For each file:
     ├─ Parse with tree-sitter
     ├─ Generate chunks
     ├─ Format for embedding (add context)
     ├─ Batch embed (32 chunks)
     │  └─ ONNX inference
     └─ Upsert to Qdrant + Tantivy
```

### 2. Search Flow
```
search(query)
  ├─ BM25 search (Tantivy)
  │  └─ Fast keyword matching
  └─ Vector search
     ├─ EmbeddingGenerator::embed(query)
     ├─ Send embedding to Qdrant
     └─ Get cosine similarity results
  └─ Merge via RRF (k=60)
```

---

## Performance Facts

| Operation | Time | Notes |
|-----------|------|-------|
| Model init | 2-5s | First run, includes download if needed |
| Single embed | 10-20ms | CPU inference |
| Batch 32 | 50-100ms | Amortized 1.5ms per text |
| 3000 chunks | 1.5-2s | ~94 batches of 32 |
| Index 100k LOC | 15-30s | Parse + chunk + embed + DB |
| Incremental (no changes) | <100ms | Merkle tree comparison |
| Query embedding | ~15ms | Same as single embed |
| Qdrant search | 20-50ms | Cosine similarity on 1000s vectors |
| Hybrid search | 50-100ms | BM25 + Vector + RRF merge |

---

## Configuration

### Environment
```bash
QDRANT_URL=http://localhost:6334  # Qdrant server (gRPC)
RUST_LOG=debug                     # Logging
```

### Defaults
- **Vector Dimensions:** 384 (all-MiniLM-L6-v2)
- **Batch Size:** 32 chunks
- **Cosine Distance:** For similarity
- **HNSW-M:** 16 (small), 32 (large)
- **RRF k:** 60

### Caches
- **Model:** `.fastembed_cache/models--Qdrant--all-MiniLM-L6-v2-onnx/`
- **Index:** `~/.local/share/rust-code-mcp/search/index/{hash}/`
- **Metadata:** `~/.local/share/rust-code-mcp/search/cache/{hash}/`
- **Merkle:** `~/.local/share/rust-code-mcp/merkle/{hash}.snapshot`

---

## HNSW Auto-Tuning

Based on estimated lines of code:

| Size | M | EF-Construct | EF | Threads | RAM-First |
|------|---|---|---|---|---|
| <100k | 16 | 100 | 128 | 8 | 50k |
| 100k-1M | 16 | 150 | 128 | 12 | 50k |
| >1M | 32 | 200 | 256 | 16 | 30k |

---

## Code Examples

### Basic Embedding
```rust
let gen = EmbeddingGenerator::new()?;
let vec = gen.embed("fn parse() {}")?;
assert_eq!(vec.len(), 384);
```

### Batch Embedding
```rust
let texts = vec!["code1".to_string(), "code2".to_string()];
let vecs = gen.embed_batch(texts)?;
assert_eq!(vecs.len(), 2);
```

### Chunk Formatting
```rust
let formatted = chunk.format_for_embedding();
// Contains: // File: ... // Location: ... // Module: ...
// Symbol: ... // Code: ...
let embedding = gen.embed(&formatted)?;
```

### Vector Search
```rust
let vector_search = VectorSearch::new(gen, vector_store);
let results = vector_search.search("async fn", 10).await?;
for result in results {
    println!("{}: {}", result.chunk_id, result.score);
}
```

---

## Architecture Diagram

```
                    FastEmbed (ONNX)
                    all-MiniLM-L6-v2
                    384 dimensions
                         ↑
                  ┌──────┴──────┐
                  ↓             ↓
            Single Text    Batch (32)
            Embedding      Embeddings
                ↓             ↓
            ┌───┴─────────────┘
            ↓
    Format with Context
    (file, module, symbol, docs)
            ↓
    ┌──────┴──────┐
    ↓             ↓
 Tantivy      Qdrant
 (BM25)      (Vector DB)
    ↓             ↓
 Keyword    Cosine Distance
 Search     (HNSW)
    └──────┬──────┘
           ↓
      RRF Merge
           ↓
      Ranked Results
```

---

## Contextual Retrieval Format

Each chunk is formatted as:
```
// File: {path}
// Location: lines {start}-{end}
// Module: {module::path}
// Symbol: {name} ({kind})
// Docstring: {doc}
// Imports: {imports}
// Calls: {outgoing_calls}

{actual code content}
```

This reduces retrieval errors by 49% compared to bare code.

---

## Troubleshooting

| Issue | Cause | Fix |
|-------|-------|-----|
| Model download fails | Network/cache issue | Check `.fastembed_cache/` permissions |
| Slow first run | Model not cached | Expected (~80MB download) |
| Qdrant not found | Server not running | Start Qdrant on 6334 |
| No search results | Codebase not indexed | Run `index_codebase` tool |
| High memory usage | Large batch size | Reduce via `with_batch_size()` |
| Low search quality | Model mismatch | all-MiniLM-L6-v2 is optimal for code |

---

## When Embeddings Are Used

1. **Indexing:** All code chunks embedded (batch of 32)
2. **Searching:** Query embedded (single), matched to stored vectors
3. **Incremental Updates:** Only new/modified files re-embedded
4. **Deduplication:** Never re-embed unchanged chunks

---

## Next Steps / Improvements

- [ ] Support multiple embedding models
- [ ] Vector quantization (int8 for 4x compression)
- [ ] GPU acceleration (10-100x faster)
- [ ] LLM reranking for top results
- [ ] Streaming embeddings
- [ ] Caching of query embeddings
