# Scalable Rust Codebase Search & Analysis

## Problem Statement

Existing code intelligence tools (rust-analyzer, cursor-rust-tools) fail on large Rust codebases:
- High memory usage (multiple GBs)
- Slow or unresponsive on monorepos
- Can crash/timeout on extremely large codebases (>200k LOC)
- Requires entire project context in memory

This project builds a scalable, in-process MCP server for analyzing large Rust codebases using syntax-based parsing, vector embeddings, and hybrid search.

## Architecture Overview

```
File System Watcher → Merkle Tree (Change Detection)
    ↓
Tree-sitter Parser → AST + Metrics + Call Graph
    ↓
Smart Chunking (with context + overlap)
    ↓
Embeddings (384d, quantized) + Metadata
    ↓
Storage: Qdrant (vectors) + RocksDB (metadata) + Memory-mapped indices
    ↓
Hybrid Search: Vector + BM25 + Re-ranking
    ↓
MCP Protocol Layer → Tools + Resources
    ↓
CLI / REST API
```

## Key Design Decisions

### Why Not rust-analyzer?
- Doesn't scale to large codebases
- High memory footprint
- Can crash on complex projects
- Requires complete semantic analysis

### Why Tree-sitter?
- Constant memory per file
- Parallel processing without shared state
- Works on broken/incomplete code
- Never "breaks" - graceful degradation

### Why Vector Search?
- Works without perfect type resolution
- Semantic similarity at scale
- Approximate but always returns results

### Hybrid Approach (Best of Both Worlds)
1. **Fast layer**: Tree-sitter (structure)
2. **Good layer**: Vector search (semantics)
3. **Best-effort**: Lightweight semantic analysis
4. **Deep layer**: rust-analyzer on-demand (small scopes only)

## Implementation Plan

### Phase 1: Foundation (Week 1-2)
1. File system watcher (`notify-rs`)
2. Merkle tree + RocksDB for change detection
3. Project configuration (`.codebase-mcp.toml`)
4. Exclusion patterns, memory limits

### Phase 2: Parsing (Week 3-4)
5. Tree-sitter integration
6. `rust-code-analysis` for metrics
7. AST traversal + symbol extraction
8. Simple call graph (string-based, no type resolution)

### Phase 3: Smart Chunking (Week 5)
9. Chunk by functions/structs/modules
10. Add context (file path + module + docstring)
11. 20% overlap between chunks
12. Rich metadata (imports, calls, complexity)

### Phase 4: Storage (Week 6-7)
13. Qdrant setup (optimized: 384d, quantized, memory-mapped)
14. RocksDB for metadata (or SQLite)
15. Memory-mapped indices (`memmap2` + `rkyv`)
16. Batch processing pipeline (parallel workers)

### Phase 5: Embeddings (Week 8)
17. Load `all-MiniLM-L6-v2` (384d) with quantization
18. Embed chunks with context
19. Stream embeddings to Qdrant
20. Background indexing with progress

### Phase 6: Query & Retrieval (Week 9-10)
21. Vector search (Qdrant)
22. BM25 search (`tantivy`)
23. Reciprocal Rank Fusion (RRF)
24. Cross-encoder re-ranking (top 20)
25. Progressive disclosure (summaries first)

### Phase 7: Incremental Updates (Week 11)
26. Dependency tracking between files
27. Invalidate dependent chunks only
28. Background reindexing on file change

### Phase 8: MCP Protocol (Week 12-13)
29. MCP server (rust MCP SDK, stdio transport)
30. Tools: `search_code`, `find_definition`, `get_dependencies`
31. Resources: `rust:///file/ast`, `rust:///symbol/docs`
32. Progressive context loading

### Phase 9: Interface (Week 14)
33. CLI tool
34. Query parser
35. Optional: REST API (`axum`)

### Phase 10: Optimization (Week 15-16)
36. Benchmark on 1M+ LOC codebase
37. Profile memory usage
38. Optimize bottlenecks
39. Add metrics (latency, accuracy, memory)
40. Documentation

**Total: 16 weeks (4 months) to production MVP**

## Technology Stack

### Core Dependencies
```toml
[dependencies]
# Parsing & Analysis
tree-sitter = "0.20"
tree-sitter-rust = "0.20"
rust-code-analysis = "0.0.25"  # Mozilla's metrics library

# Storage & Indexing
qdrant-client = "1.x"          # Vector database
rocksdb = "0.21"               # Metadata storage
memmap2 = "0.9"                # Memory-mapped files
rkyv = "0.7"                   # Zero-copy serialization

# Search & Retrieval
tantivy = "0.22"               # BM25 full-text search
fastembed = "3"                # Fast embeddings (or candle-core)

# Change Detection
notify = "6.0"                 # File system watcher
rs_merkle = "1.4"              # Merkle tree for hashing

# Parallelism
rayon = "1.8"                  # Parallel processing
tokio = "1.35"                 # Async runtime

# MCP
# Use official rust MCP SDK

# Optional
candle-core = "0.4"            # ML inference for re-ranking
hf-hub = "0.3"                 # HuggingFace model loading
axum = "0.7"                   # REST API (optional)
```

## Key Research Findings

### From State-of-the-Art Analysis

**1. Hybrid Search (GitHub/Sourcegraph)**
- Vector alone: 60% accuracy
- BM25 alone: 55% accuracy
- Hybrid + re-ranking: 80-85% accuracy
- **Use Reciprocal Rank Fusion (RRF)**

**2. Contextual Retrieval (Anthropic 2024)**
- Traditional RAG: 49% errors
- With context: 35% errors (-29%)
- With re-ranking: 5.7% errors (-49% from hybrid)
- **Always embed chunks with file/module context**

**3. GitHub Copilot 2024 Redesign**
- Reduced context retrieval: 30-40s → 5s
- Custom embedding model: 37.6% better, 2x faster, 8x less memory
- **Use smaller models (384d) with quantization**

**4. BigDataflow (Academic)**
- Distributed dataflow at scale (millions of LOC)
- Map-reduce style processing
- **Build DFG in database, not memory**

**5. Sourcegraph/Zoekt**
- Trigram indexing without semantic analysis
- Sub-second searches at scale
- **Syntax-based search scales better than semantic**

## Scalability Strategy

### Memory Management
```rust
// Never load entire codebase
// Query index → Load only needed files

struct ScalableIndexer {
    batch_size: usize,      // Process N files at a time
    workers: usize,         // Parallel workers
    memory_limit: usize,    // Hard memory cap (4GB default)
}
```

### Incremental Processing
```rust
// Salsa-inspired dependency tracking
struct IncrementalIndex {
    file_hashes: HashMap<PathBuf, Hash>,
    dependencies: HashMap<PathBuf, Vec<PathBuf>>,  // File A imports B

    fn reindex(&mut self, changed: Vec<PathBuf>) {
        // 1. Reprocess changed files
        // 2. Find dependents
        // 3. Reprocess dependents only
        // Don't reprocess entire codebase
    }
}
```

### Smart Chunking
```rust
struct CodeChunk {
    content: String,
    context: ChunkContext,      // File + module + purpose
    overlap: Option<String>,    // 20% overlap with prev/next
}

struct ChunkContext {
    file_path: PathBuf,
    module_path: Vec<String>,
    symbol_name: String,
    docstring: Option<String>,
    imports: Vec<String>,
    outgoing_calls: Vec<String>,  // Simple DFG alternative
    complexity: f64,
    maintainability: f64,
}
```

### Qdrant Configuration for Scale
```rust
CollectionConfig {
    vectors: VectorsConfig {
        params: VectorParams {
            size: 384,  // Smaller = faster
            distance: Distance::Cosine,
        },
    },
    optimizers_config: OptimizersConfig {
        indexing_threshold: 10000,
        memmap_threshold: 50000,  // Memory-map after 50k vectors
    },
    hnsw_config: HnswConfig {
        m: 16,              // Lower = less memory
        ef_construct: 100,  // Lower = faster build
        payload_m: 8,
    },
}
```

## MCP Tools & Resources

### Tools (Actions)
- `search_code(query: str, limit: int)` - Hybrid semantic + keyword search
- `find_definition(symbol: str)` - Locate symbol definitions
- `find_references(symbol: str)` - Find all usages
- `get_dependencies(file: str)` - File dependency graph
- `get_call_graph(function: str)` - Function call relationships
- `analyze_complexity(file: str)` - Code metrics
- `get_similar_code(snippet: str)` - Find similar code patterns

### Resources (Data)
- `rust:///{crate}/symbols` - Symbol listings
- `rust:///{file}/ast` - Parsed AST
- `rust:///{file}/metrics` - Code quality metrics
- `rust:///{symbol}/docs` - Extracted documentation
- `rust:///{symbol}/references` - All references to symbol

## Performance Targets

### Benchmarks
- **Index time**: <5 min for 1M LOC
- **Memory usage**: <4GB for 1M LOC
- **Query latency**: <200ms p95
- **Retrieval accuracy**: >80%
- **Incremental update**: <1s for single file change

### Scaling Characteristics
| Codebase Size | rust-analyzer | This Tool |
|---------------|---------------|-----------|
| <50k LOC      | ✅ Fast       | ⚠️ Overkill |
| 50-200k LOC   | ⚠️ Slow      | ✅ Fast     |
| 200k-1M LOC   | ❌ Breaks    | ✅ Works    |
| >1M LOC       | ❌ Fails     | ✅ Designed for this |

## Similar Tools & Validation

Our approach is validated by industry leaders:

- **Sourcegraph/Zoekt**: Trigram indexing without semantics, scales to millions of repos
- **GitHub Code Search (Blackbird)**: Syntax + embeddings, no language server dependency
- **Google Code Search**: Pure trigram, handles 2B+ LOC
- **Mozilla rust-code-analysis**: Tree-sitter based, scales to massive codebases

## Trade-offs

### Advantages
✅ Won't break on large codebases
✅ Predictable performance
✅ Horizontal scalability
✅ Graceful degradation
✅ Works on incomplete/broken code
✅ Low memory footprint

### Disadvantages
⚠️ Less accurate than rust-analyzer (no perfect type resolution)
⚠️ Can't trace through complex trait implementations
⚠️ Approximate dataflow (not perfect)

**Philosophy**: 80% accuracy at scale > 100% accuracy that crashes

## Future Enhancements (Post-MVP)

1. **GNN-based re-ranking**: Graph neural networks on code property graphs
2. **On-demand semantic analysis**: Integrate `ra_ap_ide` for small scopes
3. **Distributed indexing**: Horizontal scaling across machines
4. **Custom embedding models**: Fine-tune on Rust-specific corpus
5. **Abstract interpretation**: Dataflow analysis for security/correctness
6. **Cross-crate analysis**: Multi-crate dependency intelligence

## References

- State-of-the-art research: `STATE_OF_THE_ART_CODEBASE_ANALYSIS.md`
- rust-analyzer documentation: https://rust-analyzer.github.io/
- Tree-sitter: https://tree-sitter.github.io/
- SCIP Protocol: https://scip.dev/
- Qdrant: https://qdrant.tech/
- Anthropic Contextual Retrieval (2024)
- GitHub Copilot Architecture (2024)
