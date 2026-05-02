# Comparison: rust-code-mcp vs claude-context (Zilliz)

**Date:** 2025-10-19
**Purpose:** Detailed comparison with production-proven implementation
**Reference:** https://github.com/zilliztech/claude-context

---

## Executive Summary

**claude-context** is a production-ready TypeScript implementation by Zilliz that has **validated our Strategy 4 (Merkle Tree)** approach. Their implementation proves that **Merkle tree-based incremental synchronization** is the optimal solution for large-scale codebase indexing, achieving:

- ✅ **40% token reduction** vs grep-only approaches
- ✅ **Millisecond-level change detection** via Merkle root comparison
- ✅ **Incremental updates** - Only reindex changed files
- ✅ **Multi-language support** via tree-sitter
- ✅ **Production-deployed** across multiple organizations

**Key Insight:** We should **implement Strategy 4 (Merkle Tree) FIRST**, not as an optional optimization.

---

## Architecture Comparison

### claude-context (Production Proven)

```
┌─────────────────────────────────────────────────────┐
│  Phase 1: Rapid Detection (milliseconds)           │
│  ─────────────────────────────────                 │
│  Calculate Merkle root hash                        │
│  Compare with cached snapshot                       │
│  If same → Skip update entirely                    │
└────────────────────┬────────────────────────────────┘
                     ↓ (if root changed)
┌─────────────────────────────────────────────────────┐
│  Phase 2: Precise Comparison                        │
│  ───────────────────────────                       │
│  Traverse Merkle tree                              │
│  Identify changed leaf nodes (files)                │
│  Build list of files to reindex                    │
└────────────────────┬────────────────────────────────┘
                     ↓
┌─────────────────────────────────────────────────────┐
│  Phase 3: Incremental Update                        │
│  ───────────────────────────                       │
│  For each changed file:                            │
│    ├─ Parse with tree-sitter (AST)                 │
│    ├─ Split into semantic chunks (functions/classes)│
│    ├─ Generate embeddings (OpenAI/Voyage/Ollama)   │
│    └─ Update Milvus/Zilliz vector DB               │
│                                                     │
│  Save new Merkle snapshot to ~/.context/merkle/    │
└─────────────────────────────────────────────────────┘
```

### rust-code-mcp (Current)

```
Current (Broken):
┌─────────────────────────────────────────────────────┐
│  User calls search() tool                          │
│  ↓                                                 │
│  Index directory:                                  │
│    ├─ For each file:                              │
│    │   ├─ Calculate SHA-256 hash                  │
│    │   ├─ Compare with cached hash                │
│    │   ├─ If changed: Add to Tantivy ✓           │
│    │   └─ Update cache                            │
│    └─ NO Qdrant indexing ✗                        │
│  ↓                                                 │
│  Search Tantivy only (BM25)                        │
│  Hybrid search unusable ✗                         │
└─────────────────────────────────────────────────────┘

Proposed (Fixed):
Same as claude-context but with:
- Tantivy (BM25) instead of just Milvus
- Qdrant for vectors
- fastembed (local) instead of API embeddings
```

---

## Feature Matrix Comparison

| Feature | claude-context | rust-code-mcp (Current) | rust-code-mcp (Proposed) |
|---------|----------------|------------------------|--------------------------|
| **Change Detection** | Merkle tree ✓ | SHA-256 per file ✓ | Merkle tree ✓ |
| **Detection Speed** | Milliseconds ✓ | Seconds (hash all files) | Milliseconds ✓ |
| **Hierarchical Skip** | Yes (dirs) ✓ | No | Yes (dirs) ✓ |
| **Vector Index** | Milvus/Zilliz ✓ | Qdrant (not populated) ✗ | Qdrant ✓ |
| **Lexical Index** | No | Tantivy ✓ | Tantivy ✓ |
| **Hybrid Search** | Vector only | RRF ready (no data) | BM25 + Vector ✓ |
| **Code Parsing** | tree-sitter ✓ | tree-sitter ✓ | tree-sitter ✓ |
| **Chunking** | AST-based ✓ | text-splitter ✓ | AST + text-splitter ✓ |
| **Embeddings** | OpenAI/Voyage/Ollama | fastembed (local) ✓ | fastembed ✓ |
| **State Storage** | ~/.context/merkle/ ✓ | ~/.local/share/.../cache ✓ | Same + merkle/ ✓ |
| **Multi-language** | tree-sitter parsers ✓ | Rust only | Rust (extensible) |
| **Incremental** | File-level ✓ | File-level ✓ | File + Dir level ✓ |
| **Background Watch** | No | No | Optional (notify) ✓ |
| **Token Savings** | 40% proven ✓ | N/A | TBD |
| **Production Use** | Yes ✓ | No | Target |

---

## Implementation Details Comparison

### 1. Merkle Tree Structure

**claude-context:**
```typescript
// State persists in ~/.context/merkle/{codebase-id}.snapshot
{
  rootHash: string,           // Merkle root for instant comparison
  fileHashes: {
    "path/to/file.ts": {
      hash: string,           // File content hash
      leafIndex: number,      // Position in Merkle tree
    }
  },
  merkleTree: SerializedTree, // Full tree for traversal
  lastSync: timestamp,
}
```

**rust-code-mcp (current):**
```rust
// Uses sled KV store: ~/.local/share/rust-code-mcp/search/cache
// Key: file_path (String)
// Value: FileMetadata {
//   hash: String,           // SHA-256 of content
//   last_modified: u64,     // Unix timestamp
//   size: u64,              // File size
//   indexed_at: u64,        // When indexed
// }
// No Merkle tree - must hash all files to detect changes
```

**rust-code-mcp (proposed):**
```rust
// Add merkle/ subdirectory
// ~/.local/share/rust-code-mcp/search/merkle/{project-hash}.snapshot
pub struct MerkleSnapshot {
    root_hash: [u8; 32],                    // Merkle root
    file_hashes: HashMap<PathBuf, FileNode>,
    tree: MerkleTree<Sha256Hasher>,
    last_sync: SystemTime,
}

pub struct FileNode {
    content_hash: [u8; 32],
    leaf_index: usize,
    last_modified: SystemTime,
}
```

### 2. Code Chunking Strategy

**claude-context:**
- **Primary:** AST-based via tree-sitter
  - JavaScript: Split by function definitions
  - Python: Split by class and function
  - Java: Split by method
  - Go: Split by function
- **Fallback:** RecursiveCharacterTextSplitter
  - 1000 chars per chunk
  - 200 char overlap (20%)

**rust-code-mcp (current):**
- Uses `text-splitter` crate with `CodeSplitter`
- Tree-sitter aware but not AST-guided
- 512 token chunks
- 20% overlap

**Recommendation:** Adopt claude-context's AST-first approach:
```rust
// Enhanced chunking
pub enum ChunkingStrategy {
    AstBased {
        parser: RustParser,
        unit: SyntaxUnit, // Function, Struct, Impl, Module
    },
    TextBased {
        max_tokens: usize,
        overlap: f64,
    },
}

// Primary: AST-based
let chunks = match strategy {
    ChunkingStrategy::AstBased { parser, unit } => {
        // Parse file
        let symbols = parser.parse_file(path)?;

        // Each symbol = 1 chunk
        symbols.into_iter()
            .map(|symbol| CodeChunk {
                content: symbol.text(),
                context: ChunkContext {
                    symbol_name: symbol.name,
                    symbol_kind: symbol.kind,
                    // ...
                },
            })
            .collect()
    }
    // Fallback: text-splitter
    ChunkingStrategy::TextBased { max_tokens, overlap } => {
        splitter.chunks(&source, max_tokens)
    }
};
```

### 3. Embedding Generation

**claude-context:**
- **OpenAI:** text-embedding-3-large (3072d)
- **Voyage AI:** voyage-code-3 (specialized for code)
- **Ollama:** Local models (privacy-first)
- **Cost:** API calls ($$$)

**rust-code-mcp:**
- **fastembed:** all-MiniLM-L6-v2 (384d)
- **Cost:** Free (local ONNX)
- **Privacy:** 100% local

**Trade-off Analysis:**
| Aspect | claude-context | rust-code-mcp |
|--------|----------------|---------------|
| Quality | Higher (3072d) | Lower (384d) |
| Speed | API latency | Local (faster) |
| Cost | Pay per use | One-time download |
| Privacy | Sends code to API | 100% local |
| Offline | No | Yes |

**Recommendation:** Keep fastembed (local-first), but add optional API providers:
```rust
pub enum EmbeddingProvider {
    Local { model: FastembedModel },
    OpenAI { api_key: String, model: String },
    Voyage { api_key: String },
}
```

### 4. Vector Database

**claude-context:**
- **Milvus/Zilliz Cloud** (managed service)
- High performance, elastic scaling
- Multi-replica availability
- Cost: Cloud subscription

**rust-code-mcp:**
- **Qdrant** (self-hosted recommended)
- Local or cloud deployment
- Open source
- Cost: Free (self-hosted)

**Comparison:**
| Feature | Milvus/Zilliz | Qdrant |
|---------|---------------|--------|
| Deployment | Cloud-first | Local-first |
| Setup | Managed | Docker/binary |
| Cost | Subscription | Free (self-hosted) |
| Performance | Enterprise | Excellent |
| Privacy | Cloud | Full control |

**Recommendation:** Keep Qdrant (aligns with local-first philosophy).

---

## Performance Benchmarks

### claude-context (Published Metrics)

**Token Savings:**
- **40% reduction** vs grep-only approaches
- Maintained equivalent recall accuracy
- Tested on real-world codebases

**Change Detection Speed:**
- **Milliseconds** for unchanged codebases (Merkle root check)
- **Seconds** for precise comparison on changes
- **Minutes saved** vs full reindexing

**Use Case Performance:**
| Task | Grep (Claude Code) | claude-context | Improvement |
|------|-------------------|----------------|-------------|
| Find implementation | 5 min (multi-round) | Instant | 300x faster |
| Refactoring | High token cost | 40% less tokens | 1.67x efficient |
| Bug investigation | Multiple searches | Single query | 3-5x faster |

### rust-code-mcp (Projected)

Based on our architecture and claude-context's proven results:

**With Merkle Tree (Strategy 4):**
- ✅ Millisecond change detection (same as claude-context)
- ✅ File-level incremental updates
- ✅ Directory-level skipping
- ✅ Expected: 60-80% skip rate on typical git workflows

**Advantages over claude-context:**
- ✅ **Dual indexing** (BM25 + Vector) vs vector-only
- ✅ **Local embeddings** (fastembed) vs API calls
- ✅ **Self-hosted** (Qdrant) vs cloud dependency

**Target Metrics:**
| Codebase | First Index | Incremental (1% change) | Unchanged Check |
|----------|-------------|------------------------|-----------------|
| 10k LOC | < 30s | < 1s | < 10ms |
| 100k LOC | < 2min | < 3s | < 20ms |
| 500k LOC | < 5min | < 8s | < 50ms |
| 1M LOC | < 10min | < 15s | < 100ms |

---

## Key Learnings from claude-context

### 1. Merkle Tree is Essential, Not Optional

**Their Implementation:**
- 3-phase approach (rapid → precise → incremental)
- State persists across restarts
- Hierarchical skip (entire directories)

**Our Mistake:** Treating Merkle tree as "Phase 3 optimization"

**Correction:** Implement Merkle tree in Phase 1 (core infrastructure)

### 2. AST-Based Chunking is Superior

**Their Approach:**
- Primary: tree-sitter AST parsing
- Chunks = semantic units (functions, classes)
- Fallback: character-based

**Our Current:** text-splitter only (token-based)

**Improvement:** Adopt AST-first strategy

### 3. Hybrid Search Requires Both Indexes

**Their Setup:** Vector search only (Milvus)

**Our Advantage:** Tantivy (BM25) + Qdrant (Vector) = True hybrid

**Validation:** Their 40% token savings with vector-only suggests even better results with hybrid

### 4. State Persistence is Critical

**Their Implementation:**
- Persistent Merkle snapshots
- Survives restarts
- Project-specific storage

**Our Current:** Partial (metadata cache only)

**Must Add:** Merkle snapshot persistence

---

## Updated Strategy Recommendation

### Original Recommendation
**Strategy 1 (Unified) + Strategy 3 (Background)**

### NEW Recommendation (Based on claude-context)
**Strategy 4 (Merkle Tree) + Strategy 1 (Unified) + Strategy 3 (Background)**

### Phase 1: Merkle Tree Infrastructure (Week 1-2) 🔥 PRIORITY

1. **Add Merkle tree implementation**
   ```rust
   // Cargo.toml
   rs_merkle = "1.4"

   // src/indexing/merkle.rs
   pub struct MerkleIndexer {
       snapshot_path: PathBuf,
       tree: MerkleTree<Sha256Hasher>,
       file_map: HashMap<PathBuf, FileNode>,
   }

   impl MerkleIndexer {
       pub fn build_tree(root: &Path) -> Result<Self>;
       pub fn detect_changes(&self, old: &Self) -> Vec<PathBuf>;
       pub fn save_snapshot(&self) -> Result<()>;
       pub fn load_snapshot(path: &Path) -> Result<Option<Self>>;
   }
   ```

2. **Three-phase change detection**
   ```rust
   // Phase 1: Rapid (milliseconds)
   let current = MerkleIndexer::build_tree(&project_root)?;
   let cached = MerkleIndexer::load_snapshot(&snapshot_path)?;

   if let Some(cached) = cached {
       if current.root_hash() == cached.root_hash() {
           return Ok(IndexStats::unchanged()); // Done in <10ms
       }

       // Phase 2: Precise (seconds)
       let changed_files = current.detect_changes(&cached);

       // Phase 3: Incremental (variable)
       for file in changed_files {
           unified_indexer.index_file(&file).await?;
       }
   } else {
       // First index - process all files
       unified_indexer.index_directory(&project_root).await?;
   }

   // Save new snapshot
   current.save_snapshot()?;
   ```

3. **Directory-level optimization**
   ```rust
   // If directory node unchanged, skip all children
   pub fn skip_unchanged_subtrees(&self, old: &Self) -> SkipMap {
       let mut skip = HashMap::new();

       for (dir, hash) in &self.directory_hashes {
           if old.directory_hashes.get(dir) == Some(hash) {
               skip.insert(dir.clone(), SkipReason::UnchangedSubtree);
           }
       }

       skip
   }
   ```

### Phase 2: AST-First Chunking (Week 3)

1. **Symbol-based chunking**
   ```rust
   pub fn chunk_by_symbols(file: &Path, parser: &mut RustParser) -> Result<Vec<CodeChunk>> {
       let symbols = parser.parse_file(file)?;

       symbols.into_iter()
           .map(|symbol| CodeChunk {
               id: ChunkId::from_symbol(&symbol),
               content: symbol.text.clone(),
               context: ChunkContext {
                   file_path: file.to_path_buf(),
                   symbol_name: symbol.name,
                   symbol_kind: symbol.kind,
                   line_start: symbol.range.start_line,
                   line_end: symbol.range.end_line,
                   docstring: symbol.docstring,
               },
           })
           .collect()
   }
   ```

2. **Fallback to text-splitter**
   ```rust
   // For files that fail parsing or non-Rust files
   pub fn chunk_by_text(file: &Path, max_tokens: usize) -> Result<Vec<CodeChunk>> {
       let content = fs::read_to_string(file)?;
       let splitter = CodeSplitter::new(/* ... */);
       splitter.chunks(&content, max_tokens)
   }
   ```

### Phase 3: Unified Indexing (Week 4)

Same as original Strategy 1, but integrated with Merkle tree:

```rust
pub async fn index_with_merkle(
    &mut self,
    project_root: &Path,
) -> Result<IndexStats> {
    // 1. Build Merkle tree
    let current_tree = MerkleIndexer::build_tree(project_root)?;

    // 2. Load cached snapshot
    let cached_tree = self.load_merkle_snapshot()?;

    // 3. Detect changes
    let changed_files = if let Some(cached) = cached_tree {
        if current_tree.root_hash() == cached.root_hash() {
            return Ok(IndexStats::unchanged());
        }
        current_tree.detect_changes(&cached)
    } else {
        // First run - all files
        collect_all_rust_files(project_root)?
    };

    // 4. Index changed files (unified pipeline)
    for file in &changed_files {
        self.index_file(file).await?;
    }

    // 5. Save Merkle snapshot
    current_tree.save_snapshot(&self.merkle_snapshot_path())?;

    Ok(IndexStats {
        total_files: self.count_files(project_root)?,
        indexed_files: changed_files.len(),
        skipped_files: total_files - changed_files.len(),
    })
}
```

### Phase 4: Background Watching (Week 5)

Same as original Strategy 3, integrated with Merkle:

```rust
impl BackgroundIndexer {
    pub async fn on_file_change(&mut self, path: PathBuf) {
        // Update Merkle tree incrementally
        if let Err(e) = self.merkle_tree.update_file(&path) {
            tracing::error!("Merkle update failed: {}", e);
        }

        // Index to both stores
        if let Err(e) = self.unified_indexer.index_file(&path).await {
            tracing::error!("Indexing failed: {}", e);
        }

        // Save updated snapshot
        if let Err(e) = self.merkle_tree.save_snapshot() {
            tracing::error!("Snapshot save failed: {}", e);
        }
    }
}
```

---

## Architectural Advantages Over claude-context

While claude-context is excellent, our Rust implementation has unique advantages:

### 1. True Hybrid Search

**claude-context:** Vector-only (Milvus)

**rust-code-mcp:** Tantivy (BM25) + Qdrant (Vector)

**Benefit:** Better results for:
- Exact identifier matches (BM25 excels)
- Semantic similarity (Vector excels)
- Combined relevance (RRF fusion)

### 2. Local-First Privacy

**claude-context:**
- Embeddings: OpenAI/Voyage APIs (sends code to cloud)
- Storage: Zilliz Cloud (managed service)

**rust-code-mcp:**
- Embeddings: fastembed (100% local, no API calls)
- Storage: Qdrant (self-hosted, full control)

**Benefit:** Suitable for proprietary/sensitive codebases

### 3. Zero Cloud Dependencies

**claude-context:** Requires internet + API keys + cloud subscription

**rust-code-mcp:** Works completely offline

**Benefit:** Air-gapped environments, no recurring costs

### 4. Performance Control

**claude-context:** Limited by API rate limits, cloud latency

**rust-code-mcp:** Limited only by local hardware

**Benefit:** Predictable performance, no quota exhaustion

---

## Implementation Checklist (Updated)

### Must Have (MVP - Weeks 1-4)
- [ ] **Merkle tree implementation** (Strategy 4)
  - [ ] Build tree from directory
  - [ ] Persist snapshots (~/.local/share/.../merkle/)
  - [ ] Detect changed files
  - [ ] Directory-level skipping
- [ ] **Unified indexing pipeline** (Strategy 1)
  - [ ] Parse → Chunk → Embed → Index
  - [ ] Populate both Tantivy + Qdrant
  - [ ] Incremental updates
- [ ] **AST-first chunking**
  - [ ] Symbol-based splits (functions, structs)
  - [ ] Fallback to text-splitter
- [ ] **End-to-end testing**
  - [ ] Test on rust-analyzer (~200k LOC)
  - [ ] Measure metrics vs targets

### Should Have (v0.2 - Week 5)
- [ ] **Background file watching** (Strategy 3)
  - [ ] notify integration
  - [ ] Debouncing (100ms)
  - [ ] Worker pool
  - [ ] Incremental Merkle updates
- [ ] **Multi-language support**
  - [ ] Pluggable tree-sitter parsers
  - [ ] Python, TypeScript, Go, etc.

### Nice to Have (v0.3 - Future)
- [ ] **Optional cloud embeddings**
  - [ ] OpenAI API support
  - [ ] Voyage AI support
- [ ] **Advanced optimizations**
  - [ ] Write buffers (Strategy 5)
  - [ ] GPU acceleration
- [ ] **Web UI dashboard**
  - [ ] Index health monitoring
  - [ ] Search result visualization

---

## Risk Assessment

### Low Risk (Proven by claude-context)
✅ Merkle tree for change detection
✅ AST-based chunking with tree-sitter
✅ Incremental file-level updates
✅ State persistence across restarts
✅ 40%+ token/query efficiency gains

### Medium Risk (Our Additions)
⚠️ Tantivy integration (mature library, but not tested with Merkle)
⚠️ Local embeddings (lower quality than OpenAI, but proven by many projects)
⚠️ Background watching (notify crate is mature)

### High Risk (Deferred)
🔴 Multi-language support (requires extensive testing)
🔴 Distributed indexing (complex, not needed yet)
🔴 Custom embedding fine-tuning (research-level)

---

## Migration from Current Implementation

### Step 1: Add Merkle Tree (No Breaking Changes)

```rust
// src/indexing/merkle.rs - NEW FILE
pub struct MerkleIndexer { /* ... */ }

// src/tools/search_tool.rs - MODIFY
async fn search(&self, params: SearchParams) -> Result<CallToolResult> {
    // NEW: Merkle-based change detection
    let merkle = MerkleIndexer::build_tree(&directory)?;
    let changed = detect_changes(&merkle)?;

    // EXISTING: Index to Tantivy (but only changed files)
    for file in changed {
        index_to_tantivy(&file)?;

        // NEW: Also index to Qdrant
        index_to_qdrant(&file).await?;
    }

    // EXISTING: Search
    hybrid_search.search(&keyword, 10).await
}
```

### Step 2: Populate Qdrant (Breaking Fix)

This fixes the critical bug where Qdrant is never populated:

```rust
// BEFORE (Broken):
index_writer.add_document(doc!(/* only Tantivy */));

// AFTER (Fixed):
// Parse and chunk
let symbols = parser.parse_file(&file)?;
let chunks = chunk_by_symbols(&file, symbols)?;

// Generate embeddings
let embeddings = embedding_gen.embed_batch(&chunks)?;

// Index to both stores
bm25.index_chunks(&chunks)?;
vector_store.upsert_chunks(chunks.zip(embeddings)).await?;
```

### Step 3: AST Chunking (Enhancement)

Replace text-splitter with symbol-based chunking as primary method.

### Step 4: Background Watching (Optional)

Enable via CLI flag: `--watch`

---

## Performance Projections

Based on claude-context's proven results and our enhancements:

### Token Efficiency
- **claude-context:** 40% reduction (vector-only)
- **rust-code-mcp:** 45-50% reduction (hybrid BM25 + vector)
  - BM25 catches exact matches (high precision)
  - Vector catches semantic matches (high recall)
  - RRF fusion ranks by combined relevance

### Indexing Speed
- **Unchanged check:** < 10ms (Merkle root comparison)
- **1% change:** < 1% of full index time
- **10% change:** < 10% of full index time
- **First index (100k LOC):** ~2 min (including embeddings)

### Memory Usage
- **Merkle tree:** ~1-2 KB per file
- **Metadata cache:** ~200 bytes per file
- **Total overhead (1M LOC):** ~50-100 MB

---

## Conclusion

### What claude-context Validates

1. ✅ **Merkle tree is essential** - Not optional, critical for scale
2. ✅ **AST-based chunking works** - Semantic units outperform token splits
3. ✅ **Incremental updates are feasible** - File-level granularity sufficient
4. ✅ **40% efficiency gains realistic** - Proven in production
5. ✅ **State persistence required** - Snapshots enable restart resilience

### Where We Improve on claude-context

1. ✅ **True hybrid search** - BM25 + Vector vs vector-only
2. ✅ **100% local/private** - No cloud APIs or subscriptions
3. ✅ **Zero-cost embeddings** - fastembed vs OpenAI/Voyage fees
4. ✅ **Self-hosted** - Full control vs managed service dependency

### Revised Recommendation

**Implement in this order:**

1. **Week 1-2:** Merkle tree infrastructure (Strategy 4)
2. **Week 3:** AST-first chunking
3. **Week 4:** Unified pipeline to both indexes (Strategy 1)
4. **Week 5:** Background watching (Strategy 3)

**Critical fix:** Populate Qdrant during indexing (currently missing!)

**Expected outcome:**
- Millisecond change detection (like claude-context)
- 45-50% token efficiency (better than claude-context)
- 100% local privacy (unlike claude-context)
- True hybrid search (better than vector-only)

---

**Document Version:** 1.0
**Last Updated:** 2025-10-19
**Research Sources:**
- https://github.com/zilliztech/claude-context
- https://zilliz.com/blog/why-im-against-claude-codes-grep-only-retrieval-it-just-burns-too-many-tokens
- https://zc277584121.github.io/ai-coding/2025/08/15/build-code-retrieval-for-cc.html
