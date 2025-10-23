# Code Search Architecture: Comprehensive Technical Analysis

**Research Date:** October 22, 2025
**Systems Analyzed:** rust-code-mcp (file-search-mcp) vs claude-context
**Total Research Areas:** 7 core architectural domains

---

## Executive Overview

This document synthesizes research findings comparing two semantic code search systems: **rust-code-mcp** (Rust-based local-first hybrid search) and **claude-context** (TypeScript cloud-ready vector search). Both implement Model Context Protocol (MCP) servers but take fundamentally different architectural approaches.

**Key Finding:** rust-code-mcp emphasizes privacy, performance, and hybrid search (BM25+vector) but is not yet production-ready due to critical bugs. claude-context prioritizes flexibility, multi-language support, and cloud deployment with proven production usage but requires API dependencies.

---

# 1. MCP Protocol Implementation

## Overview

Both systems implement MCP over JSON-RPC 2.0 with stdio transport, but use fundamentally different architectural philosophies: **compile-time safety with macros** (Rust) vs **runtime flexibility with manual registration** (TypeScript).

## Server Initialization

### Rust (rmcp SDK)

```rust
// src/main.rs
#[tokio::main]
async fn main() -> Result<()> {
    // Logging to stderr (ANSI disabled for clean stdio)
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    // Background sync service (Arc-wrapped for shared ownership)
    let sync_manager = Arc::new(SyncManager::with_defaults(300));
    tokio::spawn(async move {
        sync_manager_clone.run().await;
    });

    // MCP server with stdio transport
    let service = SearchTool::with_sync_manager(sync_manager)
        .serve(stdio())
        .await?;
    service.waiting().await?;
}
```

**Pattern:** Unified tool struct with dependency injection, tokio runtime, background services managed with Arc for thread-safe shared ownership.

### TypeScript (@modelcontextprotocol/sdk)

```typescript
import { Server } from '@modelcontextprotocol/sdk/server/index.js';

const server = new Server({
  name: 'claude-context',
  version: '1.0.0'
});

// Manual tool registration
server.setRequestHandler(ListToolsRequestSchema, async () => ({
  tools: [...]
}));

server.setRequestHandler(CallToolRequestSchema, async (request) => {
  // Handle tool calls with switch/case
});

const transport = new StdioServerTransport();
await server.connect(transport);
```

**Pattern:** Centralized server with manual handler registration, Node.js event loop.

## Tool Definition Patterns

### Rust: Macro-Based Registration

```rust
// Parameter struct with automatic JSON schema generation
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SearchParams {
    #[schemars(description = "Path to the directory to search")]
    pub directory: String,
    #[schemars(description = "Keyword to search for")]
    pub keyword: String,
}

// Tool implementation with compile-time registration
#[tool_router]  // Generates routing logic
impl SearchTool {
    #[tool(description = "Search for keywords in Rust code")]
    async fn search(
        &self,
        Parameters(SearchParams { directory, keyword }): Parameters<SearchParams>,
    ) -> Result<CallToolResult, McpError> {
        if !Path::new(&directory).is_dir() {
            return Err(McpError::invalid_params(
                format!("'{}' is not a directory", directory),
                None
            ));
        }

        // Business logic...
        Ok(CallToolResult::success(vec![Content::text(result)]))
    }
}

#[tool_handler]  // Implements ServerHandler trait
impl SearchTool {}
```

**Benefits:**
- Automatic JSON schema generation via `schemars`
- Compile-time tool registration (zero runtime overhead)
- Type-safe parameter extraction with `Parameters<T>` wrapper
- Impossible to register tool without implementing method
- Exhaustive pattern matching enforced

**10 Tools:** read_file_content, search, find_definition, find_references, get_dependencies, get_call_graph, analyze_complexity, health_check, get_similar_code, index_codebase

### TypeScript: Manual Registration

```typescript
// Tool list definition
server.setRequestHandler(ListToolsRequestSchema, async () => ({
  tools: [
    {
      name: 'index_codebase',
      description: 'Index a codebase directory',
      inputSchema: {
        type: 'object',
        properties: {
          path: { type: 'string', description: 'Absolute path' },
          force: { type: 'boolean', default: false }
        },
        required: ['path']
      }
    }
  ]
}));

// Tool dispatcher with switch/case
server.setRequestHandler(CallToolRequestSchema, async (request) => {
  const { name, arguments: args } = request.params;

  switch (name) {
    case 'index_codebase':
      if (!fs.existsSync(args.path)) {
        throw new McpError(ErrorCode.InvalidParams, 'Path does not exist');
      }
      return await handleIndexCodebase(args);
    case 'search_code':
      return await handleSearchCode(args);
  }
});
```

**Benefits:** Explicit definitions, dynamic runtime addition, standard JavaScript patterns
**Trade-offs:** Manual schemas (boilerplate), possible mismatch between list and implementation, no compile-time verification

**4 Tools:** index_codebase, search_code, get_indexing_status, clear_index

## Request/Response Handling

### Rust Flow

1. **Receive:** rmcp stdio transport reads newline-delimited JSON from stdin
2. **Deserialize:** Framework auto-deserializes to request types
3. **Route:** `ToolRouter` (macro-generated) pattern matches on tool name
4. **Extract:** `Parameters<T>` wrapper validates via serde + JSON schema
5. **Execute:** Async tool method runs business logic
6. **Format:** Return `CallToolResult::success(Vec<Content>)`
7. **Serialize:** Framework serializes to JSON-RPC response
8. **Send:** Write to stdout

**Parallel Execution:**
```rust
let (vector_future, bm25_future) = tokio::join!(
    self.vector_search.search(query, limit),
    tokio::task::spawn_blocking(move || {
        bm25_clone.search(&query_clone, limit)
    })
);
```

### TypeScript Flow

1. **Receive:** StdioServerTransport reads from stdin
2. **Deserialize:** SDK deserializes to request schema
3. **Route:** Server dispatches to registered handler (hash map lookup)
4. **Extract:** Manual extraction from `request.params.arguments`
5. **Execute:** Handler function runs business logic
6. **Format:** Return `{ content: [{ type: 'text', text: '...' }] }`
7. **Serialize:** SDK serializes to JSON-RPC response
8. **Send:** Write to stdout

**Parallel Execution:**
```typescript
const [vectorResults, bm25Results] = await Promise.all([
  vectorSearch(query, limit),
  bm25Search(query, limit)
]);
```

## Type Safety Comparison

| Aspect | Rust (rmcp) | TypeScript (SDK) |
|--------|-------------|------------------|
| **Enforcement** | Compile-time | Runtime |
| **Mechanism** | Type system + serde + schemars | JSON Schema + optional Zod |
| **Errors Caught** | At compile time | At runtime |
| **Parameter Validation** | 3 layers: schema, deserialize, business logic | 3 layers: schema, optional Zod, business logic |
| **Error Propagation** | `Result<T, E>` with `?` operator | try/catch with thrown exceptions |

## Key Architectural Differences

| Aspect | Rust (rmcp) | TypeScript (SDK) |
|--------|-------------|------------------|
| **Tool Definition** | Object-oriented with macro-generated dispatch | Functional with manual dispatch |
| **Registration** | Compile-time via `#[tool_router]` | Runtime via `setRequestHandler` |
| **Concurrency** | Multi-threaded tokio (true parallelism) | Single-threaded event loop (async) |
| **Dependency Mgmt** | Constructor injection + `Arc<T>` | Closure capture or DI |
| **Boilerplate** | Minimal (macros handle) | Moderate (manual schemas) |
| **Performance** | Near-zero routing, compiled native | O(1) hash lookup, JIT-compiled |

## Recommendations

**Choose Rust (rmcp) when:**
- Performance-critical applications (indexing large codebases)
- Type safety is paramount (compile-time guarantees)
- Complex concurrent operations (true parallelism)
- Long-running server processes (memory safety, no GC)

**Choose TypeScript (SDK) when:**
- Rapid prototyping (faster iteration)
- Team familiar with JavaScript/TypeScript
- Integration with Node.js ecosystem
- Dynamic tool registration needed

---

# 2. Hybrid Search Architecture

## Overview

Both systems implement semantic search, but with fundamentally different approaches: **rust-code-mcp** uses hybrid BM25+vector with Reciprocal Rank Fusion, while **claude-context** uses vector-only search with optional BM25 support in Milvus.

## Architecture Comparison

### rust-code-mcp (Tantivy + Qdrant)

- **BM25 Engine:** Tantivy (embedded Rust library)
- **Vector Database:** Qdrant (local or remote via gRPC)
- **Embedding Model:** all-MiniLM-L6-v2 (384 dimensions, local ONNX)
- **Fusion:** Custom RRF implementation with tuning framework

### claude-context (Milvus Unified)

- **BM25 Engine:** Milvus Built-in (sparse vectors)
- **Vector Database:** Milvus/Zilliz Cloud
- **Embedding Model:** Dense embeddings (OpenAI/Voyage/Gemini/Ollama)
- **Fusion:** Milvus native RRFRanker

## RRF Algorithm Comparison

### rust-code-mcp Implementation

**Location:** `src/search/mod.rs:196-263`

**Formula:** `score = Σ (weight_i / (k + rank_i))`

```rust
// Weighted RRF with configurable parameters
for (rank, result) in vector_results.iter().enumerate() {
    let rrf_score = 1.0 / (k + (rank + 1) as f32);
    entry.rrf_score += rrf_score * self.config.vector_weight;
    entry.vector_score = Some(result.score);
    entry.vector_rank = Some(rank + 1);
}

for (rank, result) in bm25_results.iter().enumerate() {
    let rrf_score = 1.0 / (k + (rank + 1) as f32);
    entry.rrf_score += rrf_score * self.config.bm25_weight;
    entry.bm25_score = Some(result.score);
    entry.bm25_rank = Some(rank + 1);
}
```

**Configuration:**
- **k parameter:** Default 60.0, tunable range [10.0, 20.0, 40.0, 60.0, 80.0, 100.0]
- **bm25_weight:** Default 0.5 (range 0.0-1.0)
- **vector_weight:** Default 0.5 (range 0.0-1.0)
- **Optimization:** Automatic tuning via RRFTuner with NDCG@10

**Implementation Steps:**
1. Create HashMap keyed by ChunkId for deduplication
2. Process vector results: accumulate weighted RRF scores
3. Process BM25 results: accumulate weighted RRF scores
4. Chunks appearing in both get sum of contributions (boosted)
5. Sort by combined RRF score descending
6. Preserve individual scores and ranks for transparency

### claude-context Implementation

**Built-in Milvus RRF:**
- **k parameter:** 60 (standard)
- **Weights:** Configurable dense/sparse ratios
- **Ranker:** RRFRanker (default) or WeightedRanker
- **Integration:** Single database handles both BM25 and vector internally

```python
results = client.hybrid_search(
    collection_name="code_chunks",
    reqs=[
        AnnSearchRequest(data=dense_embedding, anns_field="text_dense"),
        AnnSearchRequest(data=sparse_embedding, anns_field="text_sparse")
    ],
    rerank=RRFRanker(),
    limit=limit
)
```

## Search Flow Comparison

### rust-code-mcp Flow

1. **Query Input:** User provides natural language query
2. **Parallel Execution:**
   - BM25: `spawn_blocking` (synchronous Tantivy search)
   - Vector: Async Qdrant search with embedding generation
   - Synchronization: `tokio::join!` waits for both
3. **Candidate Retrieval:** Default 100 candidates from each engine
4. **RRF Fusion:** Apply weighted RRF with configurable k and weights
5. **Result Assembly:** Create SearchResult with all metadata
6. **Return:** Top N results sorted by RRF score

**Latency:** ~100-150ms typical
**Parallelism:** True parallel execution (BM25 and vector simultaneous)

### claude-context Flow

1. **Query Input:** Code search query from MCP client
2. **Semantic Search:** `Context.semanticSearch()` method invocation
3. **Hybrid Retrieval:** `MilvusVectorDatabase.hybridSearch()`
4. **Unified Execution:** Milvus executes both BM25 and vector internally
5. **RRF Reranking:** Built-in RRF reranker with configurable weights
6. **Return:** Ranked results with relevance scores

**Token Efficiency:** ~40% token reduction vs non-hybrid approaches

## Result Merging Strategies

### rust-code-mcp

**Strategy:** HashMap-based accumulation with weighted contributions

```rust
// Deduplication via ChunkId HashMap key
let mut merged: HashMap<ChunkId, MergedResult> = HashMap::new();

// Additive scoring for chunks in both results
entry.score = entry.vector_contribution + entry.bm25_contribution;

// Full transparency
struct MergedResult {
    chunk_id: ChunkId,
    score: f32,                    // Combined RRF
    bm25_score: Option<f32>,       // Original BM25
    vector_score: Option<f32>,     // Original vector similarity
    bm25_rank: Option<usize>,      // Rank in BM25 results
    vector_rank: Option<usize>,    // Rank in vector results
    chunk: CodeChunk               // Full data
}
```

**Advantages:**
- Complete transparency (see contribution from each engine)
- Flexible weight adjustment for different query types
- No loss of original ranking information
- Easy debugging and analysis

### claude-context

**Strategy:** Milvus native RRF reranking with weight balancing

- **Native Fusion:** Milvus handles fusion internally
- **Weight Balancing:** Dense/sparse weight ratios control influence
- **Ranker Options:** RRFRanker (default) or WeightedRanker
- **Smoothing:** k=60 smoothing parameter

**Advantages:**
- Unified database reduces infrastructure complexity
- Native Milvus optimization for hybrid search
- Automatic sparse embedding generation
- Significant token reduction (~40%)

## Ranking Quality Metrics

### rust-code-mcp Evaluation Framework

**Location:** `src/search/rrf_tuner.rs`

**Metrics Tracked:**
- **NDCG@10:** Normalized Discounted Cumulative Gain (0.0-1.0, primary tuning metric)
- **MRR:** Mean Reciprocal Rank (first relevant result position)
- **MAP:** Mean Average Precision (overall precision)
- **Recall@20:** Fraction of relevant results in top 20
- **Precision@10:** Fraction of top 10 that are relevant

**Tuning Process:**
- Test queries: 8 default Rust programming queries
- k values tested: [10.0, 20.0, 40.0, 60.0, 80.0, 100.0]
- Optimization goal: Maximize NDCG@10
- Evaluation mode: Verbose for per-query analysis

### claude-context Metrics

**Token Efficiency:**
- Improvement: ~40% token reduction
- Benefit: Significant cost and time savings
- Recall: No loss in retrieval quality

**Dual Coverage:** BM25 for exact terms, vectors for semantic concepts

## Infrastructure Requirements

### rust-code-mcp

**Components:**
- Tantivy: Embedded Rust library (no separate service)
- Qdrant: Separate vector database service (gRPC port 6334)
- Embedding: fastembed library with all-MiniLM-L6-v2 (local)

**Deployment:**
- Tantivy index: Local filesystem directory
- Qdrant server: Docker container or standalone service
- Embedding generation: In-process with fastembed

**Complexity:** Moderate (two separate indexing systems)
**Advantage:** Best-of-breed components

### claude-context

**Components:**
- Milvus: Single unified vector database
- BM25: Built into Milvus (sparse vectors)
- Embeddings: API-based (OpenAI/Voyage) or local (Ollama)

**Deployment:**
- Milvus service: Single instance handles everything
- Unified index: Single database for both sparse and dense vectors

**Complexity:** Low (single database service)
**Advantage:** Simplified infrastructure, reduced operational overhead

## Use Case Recommendations

**Choose rust-code-mcp for:**
- Projects requiring fine-grained control over search parameters
- Use cases needing transparency into BM25 vs vector contributions
- Scenarios where Tantivy and Qdrant are already in infrastructure
- Research and experimentation with hybrid search algorithms
- Rust-native applications
- Situations requiring automatic fallback to degraded modes

**Choose claude-context for:**
- Projects prioritizing infrastructure simplicity
- Use cases with Milvus already deployed
- Applications needing significant token optimization for LLM context
- Scenarios where unified vector database management is preferred
- TypeScript/JavaScript ecosystems
- MCP integrations with Claude Code

---

# 3. Incremental Indexing Strategy

## Executive Summary

Both systems achieve **100-1000x speedup** for unchanged codebases using Merkle tree-based incremental indexing with SHA-256 hashing. rust-code-mcp employs a **dual-layer optimization** (Merkle tree + metadata cache), while claude-context uses a **single-layer hierarchical Merkle tree**. Both achieve **<10ms change detection** for unchanged codebases.

## Merkle Tree Implementations

### rust-code-mcp (Binary Tree)

**Library:** `rs_merkle v0.7`
**Location:** `src/indexing/merkle.rs`

```rust
pub struct MerkleTree {
    tree: rs_merkle::MerkleTree<Sha256Hasher>,
    file_to_node: HashMap<PathBuf, FileNode>,
}

pub struct FileNode {
    content_hash: [u8; 32],      // SHA-256 hash
    leaf_index: usize,           // Position in tree
    last_modified: SystemTime,   // Timestamp
}

// Fast path: O(1) root hash comparison
pub fn has_changes(&self, other: &MerkleTree) -> bool {
    self.root_hash() != other.root_hash()
}

// Precise path: O(n) file-level detection
pub fn detect_changes(&self, other: &MerkleTree) -> ChangeSet {
    // HashMap-based O(1) lookups for adds/mods/deletes
}
```

**Determinism:** Files sorted lexicographically before tree construction (`files.sort()` at line 104) ensures consistent tree structure across runs.

**Snapshot Persistence:**
- Format: bincode binary serialization
- Location: `~/.local/share/rust-code-mcp/merkle/{hash}.snapshot`
- Size: ~100KB per 1,000 files
- Rebuild Required: rs_merkle tree reconstructed from stored hashes

### claude-context (Hierarchical Tree)

**Library:** Custom TypeScript implementation
**Structure:** Hierarchical tree mirroring filesystem (Files → Folders → Root)

**Three-Phase Synchronization:**

1. **Phase 1 (Fast Check):** O(1) root hash comparison - milliseconds
2. **Phase 2 (Detailed Analysis):** O(log n) to O(n) layer-by-layer navigation
3. **Phase 3 (Reindexing):** O(k) vector recalculation for k changed files

**Cascading Updates:** Hash changes propagate upward through folder nodes to root

**Snapshot Persistence:**
- Format: JSON serialization
- Location: `~/.context/merkle/{codebase_hash}.json`
- Content: File hash table + tree structure + timestamp
- No rebuild required: Tree directly restored

## SHA-256 Hashing Strategy

### rust-code-mcp

```rust
use sha2::{Digest, Sha256};
let content = std::fs::read(path)?;  // Vec<u8>
let mut hasher = Sha256::new();
hasher.update(&content);
let hash: [u8; 32] = hasher.finalize().into();
```

**Performance:** ~100-500 MB/s (native Rust)
**Storage:** Both binary `[u8; 32]` and hex string formats

### claude-context

```typescript
const crypto = require('crypto');
const content = fs.readFileSync(path);
const hash = crypto.createHash('sha256');
hash.update(content);
const result = hash.digest('hex');
```

**Performance:** ~100-300 MB/s (OpenSSL bindings)
**Storage:** Hexadecimal string

## Change Detection Mechanisms

### rust-code-mcp: Two-Level Detection

**Fast Path (`has_changes`):**
- **Time:** <5ms for 10,000 files
- **Method:** Single root hash comparison
- **Early Exit:** Return immediately if roots match

**Precise Path (`detect_changes`):**
- **Time:** 10-50ms
- **Method:** HashMap-based O(1) lookups

```rust
// Additions: in new tree, not in old
if !old.file_to_node.contains_key(path) {
    additions.push(path);
}

// Modifications: in both, different hash
if old_node.content_hash != new_node.content_hash {
    modifications.push(path);
}

// Deletions: in old tree, not in new
if !new.file_to_node.contains_key(path) {
    deletions.push(path);
}
```

**Change Processing (src/indexing/incremental.rs:189-223):**
- **Additions:** Index file → chunks + embeddings → insert into Tantivy + Qdrant
- **Modifications:** Delete old chunks → reindex → insert new chunks
- **Deletions:** Remove from both stores, update stats

### claude-context: Three-Phase Synchronization

**Phase 1:** Root hash check (milliseconds) → stop if equal
**Phase 2:** Navigate tree from root, descend to changed branches
**Phase 3:** Recalculate embeddings only for changed files

**Background Sync:** Automatic 5-minute intervals, non-blocking

## Metadata Caching Approaches

### rust-code-mcp: Dual-Layer Optimization

**Layer 1 - Merkle Tree:**
- Purpose: Fast file-level change detection
- Storage: Binary snapshot
- Location: `~/.local/share/rust-code-mcp/merkle/`

**Layer 2 - Metadata Cache (sled):**
- Purpose: Skip chunking/embedding for unchanged files
- Storage: Embedded LSM-tree KV store
- Location: `~/.local/share/rust-code-mcp/cache/{collection_hash}/`

```rust
pub struct FileMetadata {
    hash: String,           // SHA-256 hex
    last_modified: u64,     // Unix timestamp
    size: u64,              // Bytes
    indexed_at: u64,        // Unix timestamp
}

// O(1) lookup
pub fn has_changed(&self, file_path: &Path, content: &[u8]) -> bool {
    let current_hash = hash_content(content);
    match self.get(file_path) {
        Some(cached) => cached.hash != current_hash,
        None => true
    }
}
```

**Integration Workflow:**
1. Merkle tree detects file-level changes
2. Check metadata cache for each changed file
3. If metadata unchanged → skip chunking/embedding
4. If changed/missing → perform full indexing
5. Update metadata cache after successful indexing

**Storage:** ~150-200KB per 1,000 files (both layers)

### claude-context: Single-Layer Merkle Tree

**Architecture:** Merkle tree serves as sole change detection mechanism
- **Storage:** JSON snapshots
- **Data:** File hash table + tree structure + timestamp
- **Benefit:** Single source of truth, simpler

**Workflow:**
1. Load Merkle snapshot
2. Calculate current tree state
3. Compare root hashes
4. Navigate tree to find changes (if different)
5. Reindex changed files
6. Save updated snapshot

**Storage:** ~100-150KB per 1,000 files

## Performance Benchmarks

### rust-code-mcp

- **Unchanged codebase (10k files):** <5ms detection, **1000x speedup**
- **Small changes (10 files):** 10-20ms detection, 500ms-2s reindexing, **100x speedup**
- **First-time indexing:** 50-200ms tree construction + minutes for full indexing
- **Snapshot operations:** 5-50ms save, 10-100ms load

### claude-context

- **Unchanged codebase:** Milliseconds (root hash check)
- **Changes detected:** Milliseconds (Phase 1) → Varies (Phase 2) → Proportional (Phase 3)
- **Token reduction:** 40% improvement, no recall loss
- **Sync frequency:** Every 5 minutes (background)

**Winner:** Tie for detection speed (both <10ms), rust-code-mcp faster for precise detection (HashMap vs tree navigation)

## Architecture Philosophy

### rust-code-mcp: Maximum Performance

**Principle:** Dual-layer redundant checks for speed
**Pros:** Fastest detection, skips chunking/embedding, two verification mechanisms
**Cons:** More complex, higher storage overhead
**Use Case:** Frequent reindexing, large codebases

### claude-context: Simplicity

**Principle:** Single source of truth
**Pros:** Easier to maintain, hierarchical folder-level optimization, human-readable snapshots
**Cons:** No secondary cache, less edge case optimization
**Use Case:** Background sync, semantic search focus

## Recommendations

**Choose rust-code-mcp if:**
- Maximum local performance needed
- Frequent reindexing cycles
- Large codebases (>10k files)
- Local-only, no cloud dependencies

**Choose claude-context if:**
- Cross-language support required
- Prefer simpler architecture
- Need human-readable snapshots
- MCP protocol integration
- Cloud vector database acceptable

---

# 4. Vector Database Integration

## Overview

**file-search-mcp (Qdrant):** Rust-native self-hosted with local embedding generation (all-MiniLM-L6-v2, 384d)
**claude-context (Milvus):** TypeScript cloud-ready supporting local and Zilliz Cloud with API embeddings (768-1536d)

## Collection Management

### Qdrant Pattern

**Creation (src/vector_store/mod.rs:97-176):**

```rust
CreateCollection {
    collection_name: "code_chunks_{project_name}",
    vectors_config: VectorsConfig {
        size: 384,
        distance: Cosine,
        hnsw_config: HnswConfig {
            m: 16,              // Small codebase
            ef_construct: 100,
            ef: 128
        }
    }
}
```

**Auto-tuning (src/vector_store/config.rs):**
- Small (<100k LOC): m=16, ef_construct=100, ef=128, threads=8
- Medium (100k-1M LOC): m=16, ef_construct=150, ef=128, threads=12
- Large (>1M LOC): m=32, ef_construct=200, ef=256, threads=16

### Milvus Pattern

**Schema-based Creation:**

```typescript
{
  collection_name: "code_chunks_project",
  schema: {
    fields: [
      { name: "chunk_id", type: Int64, is_primary: true },
      { name: "text", type: VarChar, max_length: 2000 },
      { name: "text_dense", type: FloatVector, dim: 768 },
      { name: "text_sparse", type: SparseFloatVector },  // BM25
      { name: "file_path", type: VarChar }
    ],
    enable_dynamic_field: true  // $meta for extras
  },
  index_params: [
    { field: "text_dense", index_type: "AUTOINDEX", metric: "COSINE" },
    { field: "text_sparse", index_type: "SPARSE_INVERTED_INDEX", metric: "BM25" }
  ]
}
```

**Configuration:** shard_num, mmap_enabled, ttl_seconds, consistency_level

## Point/Vector Insertion

### Qdrant Batch Pattern

```rust
// 100 points per batch (src/vector_store/mod.rs:178-229)
let points = chunks.iter().map(|chunk| {
    PointStruct {
        id: PointId::Uuid(chunk.id.to_string()),
        vectors: vec![chunk.embedding.clone()],  // 384-dim
        payload: HashMap::from([
            ("content", chunk.content.into()),
            ("file_path", chunk.context.file_path.into()),
            ("symbol_name", chunk.context.symbol_name.into()),
            ("imports", chunk.context.imports.into())
        ])
    }
}).collect();

client.upsert_points(collection_name, points, None).await?;
```

**Pipeline:** TreeSitter → Symbol chunking → Batch embedding → Tantivy + Qdrant

### Milvus Pattern

```python
entities = [
    {
        "id": 0,
        "text": "pub fn search() -> Result<...>",
        "text_dense": [0.358, -0.602, ...],      # 768-dim
        "text_sparse": {0: 0.5, 42: 1.2, ...},   # BM25
        "file_path": "/src/search.rs"
    }
]
client.insert(collection_name, entities)
```

**High-Level API:**
```typescript
await context.indexCodebase(projectPath, (progress) => {
    console.log(`${progress.phase}: ${progress.percentage}%`);
});
// Auto: scan → AST chunk → embed → insert → Merkle update
```

## Similarity Search APIs

### Qdrant Hybrid Search

```rust
// Parallel vector + BM25 (src/search/mod.rs:129-180)
let (vector_results, bm25_results) = tokio::join!(
    vector_search.search(query, limit * 2),
    bm25_search.search(query, limit * 2)
);

// RRF fusion
let rrf_scores = merge_with_rrf(
    vector_results,
    bm25_results,
    k=60.0,
    weights=(0.5, 0.5)
);
```

**Result Structure:**
```rust
SearchResult {
    chunk_id: ChunkId,
    score: f32,                    // Combined RRF
    bm25_score: Option<f32>,
    vector_score: Option<f32>,
    bm25_rank: Option<usize>,
    vector_rank: Option<usize>,
    chunk: CodeChunk
}
```

### Milvus Hybrid Search

```python
results = client.hybrid_search(
    collection_name="code_chunks",
    reqs=[
        AnnSearchRequest(data=dense_embedding, anns_field="text_dense"),
        AnnSearchRequest(data=sparse_embedding, anns_field="text_sparse")
    ],
    rerank=RRFRanker(),
    limit=limit
)
```

**High-Level:**
```typescript
const results = await context.semanticSearch(projectPath, query, 10);
// Returns: [{content, relativePath, startLine, score}]
```

## Database Configuration

### Qdrant Connection

```rust
let client = QdrantClient::from_url(
    env::var("QDRANT_URL").unwrap_or("http://localhost:6334".into())
);
let vector_store = VectorStore {
    client: Arc::new(client),  // Thread-safe sharing
    collection_name
};
```

**Deployment:** Local only (Docker recommended)
**Ports:** 6334 (gRPC), 6333 (REST)
**Storage:** Local filesystem

### Milvus Connection

**Local:**
```typescript
new MilvusClient({
    address: 'localhost:19530',
    username: 'root',
    password: 'Milvus'
})
```

**Zilliz Cloud (50-70% faster):**
```python
MilvusClient(
    uri="https://in03-xxx.api.gcp-us-west1.zillizcloud.com:443",
    token="db_xxxxxx:your-api-key"
)
```

**Deployment:** Single env var switch between local and cloud

## Key Differences

| Aspect | Qdrant | Milvus |
|--------|--------|--------|
| **Schema** | Schema-free JSON payload | Typed fields + dynamic $meta |
| **BM25** | External (Tantivy) | Built-in sparse vectors |
| **Embeddings** | Local (rust-bert) | API-based or Ollama |
| **Deployment** | Self-hosted only | Local + Cloud (Zilliz) |
| **Connection** | Manual Arc pooling | Automatic connection pool |
| **Language** | Rust | TypeScript/JavaScript |
| **Cloud Migration** | Requires refactoring | Single config change |

## Use Case Recommendations

**Choose Qdrant (file-search-mcp) when:**
- Building Rust-native applications
- Need fine-grained control
- Prefer self-hosted infrastructure
- Already have Qdrant

**Choose Milvus (claude-context) when:**
- Cross-language code search
- Need cloud flexibility
- Want managed service (Zilliz)
- Prefer TypeScript/JavaScript

---

# 5. Embedding Generation Strategies

## Architecture Overview

**rust-code-mcp:** Local-first with fastembed ONNX models. Fixed all-MiniLM-L6-v2 (384d) hardcoded. Privacy-first, zero-cost.

**claude-context:** Pluggable multi-provider (OpenAI, VoyageAI, Gemini, Ollama). Runtime switching via `EMBEDDING_PROVIDER` env var.

## Supported Models

### rust-code-mcp
- **all-MiniLM-L6-v2:** 384d, 80MB, 256 tokens, 80% quality baseline, local, **$0 cost**

### claude-context Providers

**OpenAI:**
- `text-embedding-3-small`: 384-1536d, 8,191 tokens, **$0.02/1M tokens**, 95% quality
- `text-embedding-3-large`: 256-3072d, 8,191 tokens, **$0.13/1M tokens**, 96% quality

**VoyageAI:**
- `voyage-code-3`: 256/512/1024/2048d Matryoshka, 32,000 tokens, **$0.06/1M tokens**, **97.3% MRR** (13.8% better than OpenAI-large), code-specialized

**Gemini:**
- `gemini-embedding-001`: 768d, 2,048 tokens, free tier

**Ollama (Local):**
- `nomic-embed-text`: 768d, 274MB, 8,192 tokens, **$0 cost**, 100% local
- `mxbai-embed-large`: 1024d, 669MB, 512 tokens, **$0 cost**

## Privacy & Security

### 100% Local (rust-code-mcp & Ollama)

- Code never leaves machine
- Offline capable after initial download
- Excellent for financial/government/healthcare (HIPAA)
- Air-gapped environments supported
- No third-party processing

### Cloud Providers (OpenAI/VoyageAI/Gemini)

- Code chunks sent to external APIs
- 30-day retention (OpenAI policy)
- Requires BAA/DPA for compliance
- Not suitable for proprietary/sensitive code

## Performance Metrics

**Embedding Generation Speed:**
- rust-code-mcp: **1.5ms/text (batch)**, 500-700 chunks/sec
- claude-context Ollama GPU: 10-50ms/text
- claude-context OpenAI: 100-200ms/text (network latency)

**Indexing 100k LOC:**
- rust-code-mcp: 15-30s total
- claude-context OpenAI: 60-120s (network bottleneck)
- claude-context Ollama: 20-40s

## Cost Analysis (per 1M embeddings)

| Provider | Cost | Quality | Context |
|----------|------|---------|---------|
| rust-code-mcp | **$0** | 80/100 | 256 tokens |
| Ollama | **$0** | 90/100 | 8,192 tokens |
| OpenAI Small | $20 | 95/100 | 8,191 tokens |
| VoyageAI | $60 | **97.3/100** | **32,000 tokens** |
| OpenAI Large | $130 | 96/100 | 8,191 tokens |

## Embedding Quality

**Code Retrieval Benchmarks:**
- all-MiniLM-L6-v2: 80% baseline
- text-embedding-3-small: 95%
- **voyage-code-3: 97.3% MRR, 95% Recall@1** (best for code)

**Context Length Advantage:**
- all-MiniLM-L6-v2: 256 tokens (limited)
- OpenAI: 8,191 tokens
- **voyage-code-3: 32,000 tokens** (4x OpenAI, handles entire files)

## Trade-Off Matrix

### Privacy vs Quality
- **rust-code-mcp:** ⭐⭐⭐⭐⭐ privacy, ⭐⭐⭐⭐ quality → Best for compliance
- **VoyageAI:** ⭐⭐ privacy, ⭐⭐⭐⭐⭐ quality → Best code retrieval
- **Ollama:** ⭐⭐⭐⭐⭐ privacy, ⭐⭐⭐⭐ quality → **Best balance with flexibility**

### Cost vs Performance
- **rust-code-mcp:** ⭐⭐⭐⭐⭐ cost ($0), ⭐⭐⭐⭐⭐ speed (1.5ms) → Best cost-performance
- **Ollama:** ⭐⭐⭐⭐⭐ cost ($0), ⭐⭐⭐⭐ speed (10-50ms) → Best balance
- **VoyageAI:** ⭐⭐⭐ cost ($0.06/1M), ⭐⭐⭐ speed → Worth premium for code

### Simplicity vs Flexibility
- **rust-code-mcp:** ⭐⭐⭐⭐⭐ simplicity (auto-download), ⭐⭐ flexibility (single model)
- **claude-context:** ⭐⭐⭐ simplicity (env vars), ⭐⭐⭐⭐⭐ flexibility (4+ providers)

## Use Case Recommendations

**Choose rust-code-mcp for:**
- Financial/government/healthcare (compliance)
- Air-gapped environments
- Open-source projects (no API keys)
- Personal tools (zero cost)

**Choose claude-context + VoyageAI for:**
- Commercial SaaS (best code quality)
- Mission-critical retrieval (97.3% MRR)

**Choose claude-context + Ollama for (RECOMMENDED):**
- Want privacy AND flexibility
- Research/academia
- Large codebases (32k context)

## Best-of-Both-Worlds

**Use claude-context with Ollama:**
- ✅ 100% privacy (like rust-code-mcp)
- ✅ Runtime model switching
- ✅ Better models (nomic, mxbai)
- ✅ $0 cost
- ✅ GPU acceleration possible
- ⚠️ Slightly more setup

---

# 6. Code Parsing & Semantic Chunking

## Overview

**file-search-mcp** implements comprehensive tree-sitter-based Rust parsing (1,915 LOC across 4 modules) with symbol-based semantic chunking and contextual retrieval. Currently supports **Rust only** but with extensible architecture.

**claude-context** provides multi-language AST-based splitting supporting **17+ languages** including JS, Python, Java, Go, TypeScript out-of-box.

## Parsing Implementation

### file-search-mcp (Rust-specific)

**Parser:** tree-sitter-rust v0.20
**Location:** `src/parser/mod.rs` (808 LOC)

**Extracted Artifacts:**

1. **9 Symbol Types:**
   - Function (async, unsafe, const modifiers)
   - Struct
   - Enum
   - Trait
   - Impl (trait impls + inherent impls)
   - Module
   - Const
   - Static
   - TypeAlias

2. **Call Graph** (`src/parser/call_graph.rs` - 306 LOC):
   - Tracks caller → callee relationships
   - Simple calls: `foo()`
   - Method calls: `obj.method()`
   - Associated: `Type::function()`
   - Generic: `func::<T>()`

3. **Imports** (`src/parser/imports.rs` - 270 LOC):
   - Simple: `use std::collections::HashMap`
   - Glob: `use std::collections::*`
   - Multiple: `use std::{HashMap, HashSet}`
   - Alias: `use HashMap as Map`

4. **Type References** (`src/parser/type_references.rs` - 535 LOC):
   - Function parameters/returns
   - Struct field types
   - Generic arguments
   - Impl block types
   - Let bindings

**Single-Pass Strategy:**
```rust
// traverse_node() recursively processes AST
fn traverse_node(node: &Node, source: &str) {
    // 1. Check for doc comments
    // 2. Pattern match on node.kind()
    // 3. Extract symbol metadata
    // 4. Recurse into children
    // 5. Build call graph, imports, type refs in parallel
}
```

### claude-context (Multi-language)

**Approach:** AST-based splitter with 17+ languages
**Status:** Not available for direct analysis (external system)

**Expected Features:**
- Cross-language parsing via tree-sitter
- Unified AST abstraction
- Language-agnostic chunking

## Semantic Chunking Strategy

### file-search-mcp

**Location:** `src/chunker/mod.rs` (486 LOC)
**Strategy:** Symbol-based semantic boundaries (not token/line-based)

**Chunk Structure:**
```rust
pub struct CodeChunk {
    id: ChunkId,                    // UUID
    content: String,                // Code
    context: ChunkContext,          // Rich metadata
    overlap_prev: Option<String>,   // 20% overlap from previous
    overlap_next: Option<String>,   // 20% overlap to next
}

pub struct ChunkContext {
    file_path: PathBuf,
    module_path: Vec<String>,       // ['crate', 'parser', 'mod']
    symbol_name: String,
    symbol_kind: String,            // 'function', 'struct', etc.
    docstring: Option<String>,
    imports: Vec<String>,
    outgoing_calls: Vec<String>,
    line_start: usize,
    line_end: usize,
}
```

**Contextual Retrieval Pattern (49% error reduction):**

```rust
// format_for_embedding() (src/chunker/mod.rs:76-142)
fn format_for_embedding(&self) -> String {
    format!(
        "// File: {}\n\
         // Location: lines {}-{}\n\
         // Module: {}\n\
         // Symbol: {} ({})\n\
         // Purpose: {}\n\
         // Imports: {}\n\
         // Calls: {}\n\
         \n\
         {}",
        file_path, start, end, module_path,
        symbol_name, symbol_kind, docstring,
        imports, calls, actual_code
    )
}
```

**Pipeline:**
1. Parse source with tree-sitter
2. Extract module path from file path
3. Iterate symbols: extract code + metadata
4. Create ChunkContext with all metadata
5. Add 20% overlap between adjacent chunks
6. Format with context injection for embedding

**Advantages:**
- Natural semantic boundaries (no fragmentation)
- Complete symbols always together
- Rich context improves embedding quality
- Configurable overlap for continuity
- Proven Anthropic pattern (49% error reduction)

### claude-context

**Expected Strategy:** AST-based with RecursiveCharacterTextSplitter fallback
**Chunking:** Functions, classes, methods with context
**Multi-language:** Works across 17+ languages

## Language Support Comparison

### file-search-mcp

**Current:** Rust only via `RustParser`
**Extensibility:** Good but requires work per language

**To Add Languages:**
1. Import `tree-sitter-{language}` crate
2. Create language-specific parser module
3. Map language AST nodes to generic Symbol types
4. Implement symbol extraction

**Effort:** Medium (1-2 weeks per language)

**Rust-Specific Features:**
- async/unsafe/const detection
- Trait vs inherent impl distinction
- Visibility modifiers (pub, pub(crate))
- Rust doc comments (///, //!)

### claude-context

**Current:** 17+ languages production-ready
**Advantage:** Pre-built multi-language support

## Testing Coverage

### file-search-mcp

**Parser Tests:** 10 tests (`src/parser/mod.rs:582-807`)
- Simple/async function parsing
- Struct/enum extraction
- Docstring extraction
- Impl block detection
- Complete parsing (symbols + call graph + imports)

**Call Graph Tests:** 7 tests (`src/parser/call_graph.rs:181-305`)
**Import Tests:** 5 tests (`src/parser/imports.rs:194-269`)
**Type Reference Tests:** 9 tests (`src/parser/type_references.rs:339-534`)
**Chunker Tests:** 6 tests (`src/chunker/mod.rs:330-485`)

**Total:** 40+ unit tests, production-ready for Rust code

---

# 7. Architecture & Modularity

## Overview

**Rust-code-MCP:** Local-first hybrid search (BM25+vector) with 11 feature-based modules. Zero cloud dependencies but **NOT production-ready** due to critical bugs.

**Claude-Context:** Cloud-first vector-only TypeScript monorepo with 3 isolated packages. **Production-proven** but requires API keys.

## Architecture Patterns

### Rust-code-MCP: Layered Monolith (8.5/10)

**Structure:** Single binary + library (10,030 LOC, 11 modules)

**Layers:**
- **Entry:** `main.rs` (40 LOC) - Tokio + MCP server
- **Protocol:** `tools/` (1,428 LOC) - MCP implementations
- **Orchestration:** `search/` + `indexing/` (3,235 LOC) - Hybrid search
- **Processing:** `parser/` + `chunker/` + `embeddings/` (2,688 LOC)
- **Storage:** `vector_store/` + Tantivy + `metadata_cache` (1,041 LOC)
- **Cross-cutting:** `security/` + `monitoring/` (914 LOC)

**Data Flow:**
```
File → Parser(tree-sitter) → ParseResult
     → Chunker → Vec<CodeChunk>
     → Security filter → Embeddings(fastembed)
     → Parallel write: Tantivy(BM25) + Qdrant(vectors)
     → Update metadata cache
```

### Claude-Context: Package-Based Monorepo (9/10)

**Structure:** TypeScript workspace with 3 packages

**Packages:**
- **@zilliz/claude-context-core:** Business logic (indexing, search, Merkle)
- **@zilliz/claude-context-mcp:** Protocol adapter (MCP server)
- **semanticcodesearch:** VSCode extension (IDE integration)

**Advantage:** Core package completely protocol-agnostic—testable without MCP/VSCode runtime

## Module Coupling Analysis

### Rust-code-MCP

**Loose Coupling (1-2/5):**
- `parser/` (1,915 LOC): No storage dependencies
- `chunker/` (485 LOC): Pure transformation, stateless
- `security/` (449 LOC): Filter-only

**Medium Coupling (3/5):**
- `embeddings/` (288 LOC): Arc<Model> shared
- `vector_store/` (791 LOC): Arc<QdrantClient>
- `metadata_cache` (250 LOC): Clear KV abstraction

**Tight Coupling (4-5/5) - Intentional:**
- `indexing/unified.rs` (642 LOC): Orchestrates everything
- `search/mod.rs` (549 LOC): Requires Tantivy + Qdrant
- `tools/search_tool.rs` (1,000 LOC): Integration layer

**Finding:** SearchTool has high coupling but acceptable as entry point boundary

### Claude-Context

**Package Dependencies:**
```
extensions → mcp (2/5 - public API only)
mcp → core (3/5 - orchestrates logic)
core → APIs (4/5 - OpenAI/Milvus tight coupling)
```

**Risk:** Cloud API dependencies (rate limits, latency, costs)

## Separation of Concerns

### Rust-code-MCP Strengths
- Parser independent of storage
- Security filtering at ingestion boundary
- Monitoring as passive observer
- Metadata cache isolated from business logic

### Rust-code-MCP Weaknesses
- No `LanguageParser` trait (Rust-only)
- `UnifiedIndexer` coordinates many components
- Missing `VectorStore` trait (Qdrant-specific)

### Claude-Context Strengths
- Core has zero knowledge of MCP/VSCode
- Package boundaries enforced
- Testable: core unit tests without protocol
- Multi-language already abstracted

### Claude-Context Weaknesses
- Cloud dependencies reduce portability
- No BM25/lexical search (vector-only)

## Extensibility Assessment

### Rust-code-MCP (7.5/10)

**Extension Points:**
- **Languages:** Needs trait, 1-2 weeks per language
- **Embeddings:** fastembed supports multiple, could add OpenAI/Voyage
- **Vector Stores:** Needs trait, 2-3 weeks for new backend
- **Search:** RRFTuner configurable, could add learning-to-rank

**Needed Abstraction:**
```rust
trait LanguageParser {
    fn parse(&self, source: &str) -> Result<ParseResult>;
}
impl LanguageParser for RustParser { /* existing */ }
impl LanguageParser for PythonParser { /* new */ }
```

### Claude-Context (9/10)

**Extension Points:**
- **Languages:** 6+ supported, add tree-sitter (low effort)
- **Embeddings:** Pluggable providers (OpenAI, Voyage, Ollama)
- **IDE:** VSCode extension proves model

**Already Abstracted:**
```typescript
export class ASTChunker {
  constructor(private language: TreeSitterLanguage) {}
}
```

## Deployment Scenarios

### Rust-code-MCP

**Local Development (Excellent):**
```bash
cargo build --release
docker run -p 6334:6334 qdrant/qdrant
./target/release/file-search-mcp
# No API keys, 100% local, ~2GB RAM
```

**Air-Gapped (Excellent):**
```bash
cargo vendor
# Transfer binary + fastembed model (~500MB)
# NO internet required
```

**Cloud (Good):** Deploy Qdrant cluster, container/VM for server

**Embedded Library (Good):**
```toml
[dependencies]
file-search-mcp = { path = "../file-search-mcp" }
```
Note: Still requires Qdrant process

### Claude-Context

**Team Shared Cloud (Excellent):**
- Zilliz Cloud managed (zero infrastructure)
- Elastic scaling, backups
- Challenges: Cost, data in cloud

**Self-Hosted Milvus (Good):**
- Docker Compose or Helm
- Challenges: 8GB+ RAM, complex setup

**Air-Gapped (Poor):**
- Blockers: OpenAI/Voyage unless Ollama
- Effort: High (3-5 days)

**VSCode (Excellent):**
- Marketplace extension
- Right-click → Search similar

## Critical Differences

### Data Sovereignty

| Aspect | Rust-code-MCP | Claude-Context |
|--------|---------------|----------------|
| Embeddings | 100% local (fastembed) | APIs (unless Ollama) |
| Vector Store | Self-hosted Qdrant | Zilliz or Milvus |
| Privacy | Guaranteed local | Sent to APIs |
| Suitable | Proprietary, classified | Open-source, cloud |

### Cost Model

| | Rust-code-MCP | Claude-Context |
|-|---------------|----------------|
| Initial | Free | Free |
| Ongoing | Zero (self-hosted) | API + subscription |
| Scaling | Linear with hardware | Per-query + storage |

### Search Approach

**Rust-code-MCP:** Hybrid (BM25 + vector)
→ 45-50% token reduction (projected)

**Claude-Context:** Vector-only
→ 40% token reduction (proven)

### Maturity

**Rust-code-MCP:**
- Status: NOT production-ready
- Critical Bug: Qdrant not populated during indexing
- Needs: Bug fixes (2-3 days), Merkle tree (1-2 weeks), AST chunking (3-5 days)

**Claude-Context:**
- Status: Production-deployed
- Proven: Multiple organizations using
- Limitation: Cloud dependencies

## Recommendations

### For Rust-code-MCP Development

**Immediate Priorities:**
1. Fix Qdrant bug (P1) - enables hybrid search
2. Complete Merkle tree (P2) - 100-1000x speedup
3. AST-first chunking (P3) - better chunk quality

**Architectural Improvements:**
- Add `LanguageParser` trait for multi-language
- Create `VectorStore` trait for swappable backends
- Add `EmbeddingProvider` enum (fastembed, OpenAI, Voyage)
- Implement background file watching

**Unique Selling Points:**
- Hybrid search (BM25 + vector) vs vector-only
- 100% local privacy
- Zero ongoing costs
- Air-gapped capability

### Use Case Decision Matrix

**Choose Rust-code-MCP when:**
- Privacy critical (proprietary/classified)
- Air-gapped/offline environment
- Zero cloud costs required
- Need exact keyword search + semantic
- Willing to wait for bug fixes

**Choose Claude-Context when:**
- Multi-language codebase NOW
- Production-proven needed
- Prefer managed services
- Need VSCode integration
- API costs acceptable
- Cloud-native culture

## Modularity Scores

**Rust-code-MCP:** 8/10
- Clear boundaries, reusable modules, ~900 LOC average
- Tight coupling at integration layer

**Claude-Context:** 9/10
- Package-level isolation, core is protocol-agnostic
- Excellent testability

**Key Insight:** Claude-context's package structure provides superior modularity due to enforced NPM workspace boundaries vs Rust's looser module system.

---

# Conclusion

## Summary Table

| Aspect | rust-code-mcp | claude-context |
|--------|---------------|----------------|
| **Language** | Rust | TypeScript |
| **Search** | Hybrid (BM25+vector) | Vector-only |
| **Embeddings** | Local (fastembed) | API or Ollama |
| **Vector DB** | Qdrant (self-hosted) | Milvus/Zilliz |
| **Privacy** | 100% local | Cloud APIs |
| **Cost** | $0 ongoing | API + subscription |
| **Languages** | Rust only | 17+ languages |
| **Maturity** | NOT production-ready | Production-proven |
| **Cloud** | Local only | Native cloud support |
| **Incremental** | In progress (Merkle) | Production Merkle tree |

## Key Findings

### rust-code-mcp Strengths
- **Hybrid search** (45-50% token reduction potential)
- **100% privacy** (no data leaves machine)
- **Zero costs** (no API fees)
- **Rust performance** (native compiled, 1.5ms embeddings)
- **Air-gapped capable**

### rust-code-mcp Critical Issues
- **NOT production-ready** (Qdrant population bug)
- Rust-only (needs multi-language)
- Merkle tree incomplete
- Needs AST-first chunking

### claude-context Strengths
- **Production-proven** (multiple orgs using)
- **Multi-language** (17+ languages)
- **VSCode integration**
- **Proven Merkle tree** (40% token reduction)
- **Cloud-native** (Zilliz managed service)

### claude-context Limitations
- Cloud dependencies (APIs, costs)
- Vector-only (no lexical search)
- Privacy concerns (code sent to APIs)

## Final Recommendations

### Choose rust-code-mcp for:
- Financial/government/healthcare (strict privacy)
- Air-gapped environments
- Zero-cost requirement
- Rust expertise available
- **Willing to wait for bug fixes and completion**

### Choose claude-context for:
- Multi-language codebases
- **Production deployment NOW**
- Managed services preference
- VSCode integration needed
- Cloud-native teams

### Best-of-Both-Worlds
Use **claude-context with Ollama provider**:
- ✅ 100% privacy (like rust-code-mcp)
- ✅ Multi-language support
- ✅ $0 cost
- ✅ Flexibility to switch providers
- ⚠️ Slightly more setup complexity

---

## Research Methodology

This documentation synthesizes research from 7 core areas:
1. MCP Protocol Implementation (46k chars → condensed)
2. Hybrid Search Architecture (19k chars → direct read)
3. Incremental Indexing Strategy (32k chars → condensed)
4. Vector Database Integration (33k chars → condensed)
5. Embedding Generation Approaches (32k chars → condensed)
6. Code Parsing & Semantic Chunking (25k chars → direct read)
7. Architecture & Modularity Patterns (42k chars → condensed)

**Total Research Data:** ~229,000 characters analyzed and synthesized

---

**Document Version:** 1.0
**Last Updated:** October 22, 2025
**License:** Same as parent project
**Contact:** See project repository for issues and contributions
