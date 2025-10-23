# Embedding Generation Strategy - Analysis Index

Generated: October 22, 2025

## Overview

This directory contains comprehensive documentation of the embedding generation strategies used in the **rust-code-mcp** codebase.

## Documents

### 1. EMBEDDING_ANALYSIS.md (23KB, 777 lines)
**Comprehensive Technical Deep-Dive**

The primary reference document containing:

- **Section 1:** FastEmbed Integration Architecture
  - Model selection rationale (all-MiniLM-L6-v2)
  - ONNX implementation
  - Caching strategy

- **Section 2:** Embedding Generator Implementation
  - Core architecture and design patterns
  - Initialization strategy
  - Methods: single, batch, chunk embedding
  - Pipeline for batch processing

- **Section 3:** Code Chunk Formatting
  - Contextual retrieval approach
  - Formatting strategy with context
  - Benefits (49% error reduction)

- **Section 4:** Vector Database Integration
  - Qdrant configuration
  - Collection creation
  - Upserting embeddings

- **Section 5:** Unified Indexing Pipeline
  - Complete indexing flow
  - Memory optimization
  - Integration with all components

- **Section 6:** Hybrid Search Integration
  - Vector search implementation
  - BM25 combination
  - RRF merging strategy

- **Section 7:** Incremental Indexing
  - Merkle tree change detection
  - Embedding generation in incremental updates

- **Section 8:** Caching and Optimization
  - Three-level caching strategy
  - Batch processing optimization
  - Memory efficiency

- **Section 9:** Configuration and Environment
  - Environment variables
  - Default directories
  - Setup instructions

- **Section 10:** Performance Characteristics
  - Embedding performance metrics
  - End-to-end indexing benchmarks
  - Search performance

- **Section 11:** Key Design Decisions
  - Why all-MiniLM-L6-v2
  - Why Arc<TextEmbedding>
  - Why batch pipeline
  - Why hybrid search
  - Why incremental indexing

- **Section 12-15:** Limitations, comparisons, examples, summary

### 2. EMBEDDING_QUICK_REFERENCE.md (7KB, 257 lines)
**Quick Lookup Guide**

Fast reference for common tasks:

- **At a Glance Table** - All key parameters
- **Key Files** - Where to find each component
- **Workflow** - Indexing and search flows
- **Performance Facts** - Quick benchmarks
- **Configuration** - Environment and defaults
- **HNSW Auto-tuning Table** - Parameter selection
- **Code Examples** - Common usage patterns
- **Architecture Diagram** - Visual overview
- **Contextual Retrieval Format** - Chunk formatting
- **Troubleshooting** - Common issues
- **When Embeddings Are Used** - Application points
- **Next Steps** - Future improvements

## Key Findings Summary

### Model: all-MiniLM-L6-v2 (ONNX)
- **Dimensions:** 384
- **Download:** 80MB (cached)
- **Speed:** 1.5ms per text (batch), 10-20ms single
- **Privacy:** 100% local execution
- **Quality:** Excellent for code search

### Implementation Highlights

1. **EmbeddingGenerator** (src/embeddings/mod.rs)
   - Arc-wrapped for thread safety
   - Lazy initialization
   - Single + batch + chunk methods

2. **Batch Pipeline** (EmbeddingPipeline)
   - Batch size: 32 (configurable)
   - 30-50x faster than sequential
   - Progress tracking included

3. **Contextual Formatting**
   - Includes file, location, module, symbol, docs
   - 49% error reduction vs bare code

4. **Vector Database** (Qdrant via src/vector_store/)
   - Cosine distance metric
   - Auto-tuned HNSW parameters
   - Automatic optimization by codebase size

5. **Unified Indexing** (src/indexing/unified.rs)
   - Combines all components
   - Embeddings generated in batches
   - Merkle tree change detection

6. **Hybrid Search** (src/search/mod.rs)
   - BM25 (Tantivy) + Vector (Qdrant)
   - RRF ranking (k=60)
   - Total latency: 50-100ms

### Performance

**Embedding Speed:**
- Single: 10-20ms
- Batch 32: 50-100ms (1.5ms amortized)
- 1000 chunks: 1.5-2s
- vs Sequential: 30-50x faster

**Indexing (100k LOC):**
- First run: 15-30s
- Incremental (no changes): <100ms
- Incremental (1 file changed): 2-5s

**Search:**
- Query embedding: 15ms
- Qdrant search: 20-50ms
- BM25 search: 10-30ms
- Hybrid search: 50-100ms

## How to Use These Documents

### For Quick Reference
Start with **EMBEDDING_QUICK_REFERENCE.md**
- Get model/dimension/speed overview
- Find specific code locations
- View performance benchmarks
- Copy code examples

### For Understanding Architecture
Read **EMBEDDING_ANALYSIS.md sections 1-6**
- Comprehensive component overview
- Design rationale
- Implementation details
- Integration points

### For Performance Analysis
Check **EMBEDDING_ANALYSIS.md sections 8, 10**
- Caching strategies
- Performance benchmarks
- Optimization techniques

### For Implementation
Refer to **EMBEDDING_ANALYSIS.md sections 13-15**
- Code examples
- Comparisons with alternatives
- Summary tables

## Critical Code Locations

| Component | File | Key Methods |
|-----------|------|-------------|
| **Embedding Gen** | src/embeddings/mod.rs | `new()`, `embed()`, `embed_batch()`, `embed_chunks()` |
| **Pipeline** | src/embeddings/mod.rs | `EmbeddingPipeline::process_chunks()` |
| **Chunk Format** | src/chunker/mod.rs | `format_for_embedding()` |
| **Vector DB** | src/vector_store/mod.rs | `VectorStore::upsert_chunks()`, `search()` |
| **Config** | src/vector_store/config.rs | `QdrantOptimizedConfig::for_codebase_size()` |
| **Indexing** | src/indexing/unified.rs | `UnifiedIndexer::new_with_optimization()` |
| **Search** | src/search/mod.rs | `VectorSearch::search()`, `HybridSearch` |
| **Tool** | src/tools/index_tool.rs | `index_codebase()` |

## Technology Stack

- **Embedding Model:** all-MiniLM-L6-v2 (Hugging Face)
- **Runtime:** ONNX (via fastembed crate v4)
- **Vector DB:** Qdrant (gRPC on 6334)
- **Lexical Search:** Tantivy (BM25)
- **Code Parsing:** tree-sitter + Rust grammar
- **Chunking:** text-splitter + semantic
- **Change Detection:** Merkle trees + SHA-256

## Configuration Reference

**Environment Variables:**
```bash
QDRANT_URL=http://localhost:6334  # Qdrant server
RUST_LOG=debug                     # Logging level
```

**Default Caches:**
```
.fastembed_cache/
  └── models--Qdrant--all-MiniLM-L6-v2-onnx/
      └── blobs/  (ONNX model files)

~/.local/share/rust-code-mcp/
  ├── search/
  │   ├── index/{hash}/     (Tantivy BM25)
  │   └── cache/{hash}/     (Metadata)
  └── merkle/
      └── {hash}.snapshot   (Change detection)
```

## HNSW Auto-Tuning

Based on estimated lines of code:

| Codebase | M | EF-Const | EF | Threads |
|----------|---|----------|----|----|
| <100k | 16 | 100 | 128 | 8 |
| 100k-1M | 16 | 150 | 128 | 12 |
| >1M | 32 | 200 | 256 | 16 |

## Performance Characteristics

**Embedding Generation:**
- Model Load (first): 2-5s
- Single Text: 10-20ms
- Batch 32: 50-100ms
- Throughput: 320-640 texts/second

**Indexing (per 1000 chunks):**
- Parse: ~2-3s
- Chunk: ~0.5-1s
- Embed: ~1.5-2s (batches of 32)
- Database: ~1-2s

**Search (per query):**
- Embedding: ~15ms
- Qdrant Search: ~20-50ms
- BM25 Search: ~10-30ms
- RRF Merge: ~5ms
- Total: ~50-100ms

## Workflow Summary

### Indexing
1. Parse all files (tree-sitter)
2. Generate semantic chunks
3. Format with context
4. **Batch embed (32) via FastEmbed**
5. Upsert to Qdrant + Tantivy
6. Save Merkle snapshot

### Searching
1. **Embed query (FastEmbed)**
2. Search Qdrant (cosine distance)
3. Search Tantivy (BM25)
4. Merge via RRF ranking
5. Return results

## Limitations & Future Work

**Current Limitations:**
- Single model (all-MiniLM-L6-v2)
- Fixed 384 dimensions
- No vector quantization
- No LLM reranking

**Planned Improvements:**
- Multiple model support
- Vector quantization (int8)
- GPU acceleration
- LLM-based reranking
- Query embedding caching
- Streaming embeddings

## Key Design Decisions

| Decision | Rationale |
|----------|-----------|
| **all-MiniLM-L6-v2** | Local privacy, fast ONNX, small download |
| **Arc<TextEmbedding>** | Thread-safe sharing, concurrent requests |
| **Batch size 32** | ONNX optimal, 30-50x faster |
| **Hybrid Search** | Complementary BM25 + semantic |
| **Incremental Indexing** | 100-1000x speedup for unchanged |
| **Contextual Formatting** | 49% error reduction |
| **Cosine Distance** | Most suitable for semantic search |

## Related Documentation

- [ISSUES.md](ISSUES.md) - Known issues and fixes
- [DOCS/](DOCS/) - Additional documentation
- [tests/](tests/) - Integration tests
- [Cargo.toml](Cargo.toml) - Dependencies

## Next Steps

1. **For Integration:** Review EMBEDDING_QUICK_REFERENCE.md for configuration
2. **For Optimization:** Check EMBEDDING_ANALYSIS.md section 8 (Caching)
3. **For Extension:** See section 12 (Limitations)
4. **For Debugging:** Use section 14 (Comparisons) for expected behavior

## Contact & Attribution

Analysis Date: October 22, 2025
Codebase: rust-code-mcp (file-search-mcp)
Status: Production-grade embedding system
Architecture: Hybrid search with local ONNX embeddings

