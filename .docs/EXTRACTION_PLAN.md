# Bloop Pattern Extraction Plan

**Purpose**: Document which patterns to extract from bloop for integration into rust-code-mcp

**Status**: Phase 0 - Planning
**Created**: 2025-10-17

---

## Executive Summary

Based on analysis of bloop's codebase, we'll extract specific patterns rather than fork the entire project. This approach keeps rust-code-mcp lean and focused on MCP integration while leveraging proven patterns for:
- Tantivy schema design with code-specific fields
- Local embedding generation (ONNX/fastembed)
- Qdrant vector database integration
- Tree-sitter code parsing and symbol extraction
- File watching and incremental indexing

---

## Extraction Strategy

### ✅ What to Extract (Patterns & Approaches)
- **Architectural patterns**: How components fit together
- **Schema designs**: Field definitions and indexing strategies
- **Algorithm implementations**: RRF fusion, chunking logic
- **Integration patterns**: How to use Qdrant, tree-sitter, etc.

### ❌ What NOT to Extract
- Desktop app code (webserver/, agent/)
- Git repository management (we'll start with local directories)
- Complex orchestration (background sync queues)
- Multi-repo handling (single repo focus for MVP)
- LLM/AI agent code (out of scope)

---

## Phase-by-Phase Extraction Plan

## Phase 1: Persistent Index + Incremental Updates (Weeks 2-4)

### Files to Reference from bloop:
1. **`indexes/schema.rs`** (306 lines)
   - Study `File` schema structure (lines 18-68)
   - Extract field definitions we need:
     - `unique_hash` (line 25) - File identification
     - `content` + `raw_content` (lines 42, 56) - Dual text/bytes storage
     - `symbols` + `symbol_locations` (lines 47-48) - For Phase 2
     - `lang` (line 51) - Language detection
     - `last_commit_unix_seconds` (line 53) - For Phase 1 metadata
   - **Action**: Adapt schema for single-repo use case

2. **`cache.rs`** (22KB)
   - Study FileCache structure for metadata storage
   - Likely uses SQL/SQLite for file metadata
   - **Action**: Implement simpler version with sled or rocksdb

3. **`background/sync.rs`** (571 lines)
   - Study incremental indexing logic (lines 279-363)
   - Look at how files are tracked and updated
   - **Action**: Simplify for local directory watching (no git)

### New Files to Create:
```
src/
├── schema.rs           # Tantivy schema (adapt from bloop)
├── metadata_cache.rs   # File metadata tracking (simplified from cache.rs)
└── watcher.rs          # File watching (notify crate, inspired by background/)
```

### Dependencies to Add:
```toml
notify = "6"           # File watching
sled = "0.34"          # Metadata cache (or rocksdb)
sha2 = "0.10"          # File hashing
```

---

## Phase 2: Tree-sitter + Symbol Extraction (Weeks 5-6)

### Files to Reference from bloop:
1. **`intelligence/code_navigation.rs`** (540 lines)
   - Study scope graph structures (lines 52-57)
   - Token definitions (lines 460-465)
   - Symbol extraction patterns
   - **Action**: Simplify for symbol indexing only (no navigation yet)

2. **`intelligence/language/`** directory
   - Look at language-specific parsers
   - Focus on Rust parser implementation
   - **Action**: Start with Rust only, add more languages later

3. **`symbol.rs`** (small file, ~900 bytes)
   - Symbol representation
   - **Action**: Adapt for Tantivy storage

### New Files to Create:
```
src/
├── parser.rs           # Tree-sitter integration
├── symbols/
│   ├── mod.rs
│   ├── rust.rs         # Rust-specific symbol extraction
│   └── types.rs        # Symbol types (struct, fn, trait, etc.)
```

### Dependencies to Add:
```toml
tree-sitter = "0.20"
tree-sitter-rust = "0.20"
```

---

## Phase 3: Semantic Chunking (Week 7)

### Files to Reference from bloop:
1. **`semantic/chunk.rs`** (25KB)
   - Study chunking strategies
   - Token counting and limits
   - Code-aware splitting
   - **Action**: Adapt chunking logic for Rust code

### New Files to Create:
```
src/
└── chunker.rs          # Code-aware text chunking
```

### Dependencies to Add:
```toml
text-splitter = "0.16"  # Semantic text splitting
```

---

## Phase 4: Local Embeddings (Week 8)

### Files to Reference from bloop:
1. **`semantic/embedder.rs`** (330 lines)
   - Study `Embedder` trait (lines 56-61)
   - `LocalEmbedder` CPU implementation (lines 80-178)
   - Batch embedding logic (lines 167-176)
   - **Action**: Use fastembed-rs instead of direct ONNX

2. **`semantic/schema.rs`**
   - Embedding dimensions
   - Model configuration
   - **Action**: Choose appropriate model from fastembed

### New Files to Create:
```
src/
└── embeddings/
    ├── mod.rs
    └── local.rs        # fastembed integration
```

### Dependencies to Add:
```toml
fastembed = "3"         # Easier than direct ONNX
# OR for more control:
# ort = "2.0"          # ONNX runtime
# tokenizers = "0.19"
```

**Note**: Bloop uses custom ONNX models, we'll use fastembed's pre-trained models for simplicity.

---

## Phase 5: Qdrant Integration (Week 9)

### Files to Reference from bloop:
1. **`semantic/embedder.rs`**
   - Study `EmbedQueue` structure (lines 15-47)
   - Batch processing patterns
   - `EmbedChunk` with Qdrant payload (lines 49-54)

2. **`semantic/schema.rs`**
   - Qdrant collection schema
   - Point payload structure

3. **`semantic/execute.rs`** (small, ~2KB)
   - Qdrant operations
   - Search patterns

### New Files to Create:
```
src/
└── vector_store/
    ├── mod.rs
    ├── client.rs       # Qdrant client wrapper (embedded or remote)
    └── schema.rs       # Collection schema
```

### Dependencies to Add:
```toml
qdrant-client = "1.11"
```

### Qdrant Deployment Options:

#### Option 1: Embedded Qdrant (Recommended for MVP)
```toml
# In-process Qdrant - no separate server needed
qdrant = "0.4"  # Embedded library
```
**Pros:**
- No Docker required
- Single binary deployment
- Simpler development workflow
- Lower memory overhead for small datasets

**Cons:**
- Less scalable than server mode
- No remote access
- Tied to application lifecycle

#### Option 2: Nix with Qdrant
```bash
# Run Qdrant from nixpkgs
nix run nixpkgs#qdrant

# Or add to your existing Nix shell
buildInputs = with pkgs; [
  qdrant  # Qdrant server binary
];
```
**Pros:**
- Declarative setup
- Easy to reproduce
- No Docker needed

**Cons:**
- Requires Nix
- Separate process (not embedded)

#### Option 3: Remote Qdrant (Production)
```toml
# Connect to existing Qdrant instance
qdrant-client = "1.11"
# Configure via environment variables or config file
```

### Implementation Strategy:

**Phase 5 (Week 9) - MVP:**
- Use **embedded Qdrant** for simplicity
- Configuration via environment variable:
  ```bash
  QDRANT_MODE=embedded  # or "remote"
  QDRANT_URL=http://localhost:6333  # if remote
  ```

**Phase 8 (Week 14-16) - Production:**
- Add Nix flake for production deployment
- Support both embedded and remote modes
- Document deployment options

---

## Phase 6: Hybrid Search + RRF (Week 10-11)

### Files to Reference from bloop:
1. **`indexes/reader.rs`** (11KB)
   - Study search execution
   - Result scoring patterns

2. **`query/`** directory
   - Query parsing and execution
   - Hybrid search coordination

### Patterns to Extract:
- **Reciprocal Rank Fusion (RRF)** algorithm
  ```rust
  // Pseudo-code from bloop pattern:
  fn rrf_score(rank: usize, k: f32) -> f32 {
      1.0 / (k + rank as f32)
  }

  // Combine scores from Tantivy + Qdrant
  fn fuse_results(tantivy: Vec<Hit>, qdrant: Vec<Hit>) -> Vec<Hit> {
      let mut combined = HashMap::new();
      for (rank, hit) in tantivy.iter().enumerate() {
          combined.insert(hit.id, rrf_score(rank, 60.0));
      }
      for (rank, hit) in qdrant.iter().enumerate() {
          *combined.entry(hit.id).or_insert(0.0) += rrf_score(rank, 60.0);
      }
      // Sort by combined score...
  }
  ```

### New Files to Create:
```
src/
└── search/
    ├── mod.rs
    ├── hybrid.rs       # Hybrid search coordinator
    └── fusion.rs       # RRF implementation
```

---

## Key Bloop Files Reference Map

### High Priority (Study deeply, extract patterns)
| File | Size | Purpose | Extraction Phase |
|------|------|---------|------------------|
| `indexes/schema.rs` | 306 lines | Tantivy schema design | Phase 1 |
| `semantic/embedder.rs` | 330 lines | Embedding generation | Phase 4 |
| `semantic/chunk.rs` | 25KB | Code chunking | Phase 3 |
| `intelligence/code_navigation.rs` | 540 lines | Symbol extraction | Phase 2 |
| `indexes/reader.rs` | 11KB | Search patterns | Phase 6 |

### Medium Priority (Reference as needed)
| File | Size | Purpose | When Needed |
|------|------|---------|-------------|
| `background/sync.rs` | 571 lines | Incremental sync | Phase 1 |
| `cache.rs` | 22KB | Metadata caching | Phase 1 |
| `semantic/schema.rs` | ~3KB | Qdrant schema | Phase 5 |
| `intelligence/language.rs` | ~4KB | Language support | Phase 2 |

### Low Priority (Skip or defer)
| File | Size | Purpose | Why Skip |
|------|------|---------|----------|
| `webserver/` | Multiple files | Web UI | Out of scope |
| `agent/` | 18KB | AI agent | Out of scope |
| `remotes.rs` | 10KB | Git remote mgmt | Start with local |
| `commits.rs` | 15KB | Commit analysis | Future feature |

---

## Integration Points in rust-code-mcp

### Current Code → Bloop Patterns

| Current Location | Bloop Pattern | Integration Strategy |
|------------------|---------------|----------------------|
| `search_tool.rs:109-120` (schema) | `indexes/schema.rs:70-135` | Replace with richer schema |
| `search_tool.rs:123` (in-memory) | `indexes/writer.rs` | Add persistent storage |
| `search_tool.rs:205-267` (dir walk) | `background/sync.rs:304-363` | Add incremental tracking |
| `search_tool.rs:307-321` (search) | `indexes/reader.rs` + RRF | Add hybrid search |

---

## Simplifications vs Bloop

### What We're Simplifying:

1. **No Git Integration** (Phase 0-8)
   - Bloop: Full git clone/pull/sync
   - Us: Start with local directories
   - Rationale: Simpler MVP, add git later

2. **Single Repository** (Phase 0-8)
   - Bloop: Multi-repo management
   - Us: Single project at a time
   - Rationale: MCP servers are typically project-scoped

3. **No Web UI**
   - Bloop: Full Tauri desktop app
   - Us: MCP tools only
   - Rationale: Claude/Cursor provides the UI

4. **Rust-only** (Initially)
   - Bloop: Multi-language support
   - Us: Rust first, expand later
   - Rationale: Focused MVP

5. **Simpler Metadata Cache**
   - Bloop: Complex SQL schema
   - Us: Key-value store (sled/rocksdb)
   - Rationale: Fewer moving parts

---

## Dependencies Comparison

### Bloop's Stack (What they use):
```toml
tantivy = "0.22"          # ✅ Same (full-text search)
qdrant-client = "1.9"     # ✅ Same (vector DB)
tree-sitter = "0.20"      # ✅ Same (parsing)
ort = "1.16"              # ⚠️ Different (we'll use fastembed)
tokio = "1"               # ✅ Same (async)
rayon = "1"               # ✅ Same (parallel)
tracing = "0.1"           # ✅ Same (logging)
```

### Our Additions:
```toml
rmcp = { git = "..." }    # MCP protocol (new)
fastembed = "3"           # Simpler embeddings (vs direct ONNX)
sled = "0.34"             # Simpler cache (vs SQL)
notify = "6"              # File watching (bloop has custom)
qdrant = "0.4"            # Embedded Qdrant (no Docker needed)
```

---

## Testing Strategy

### Phase 1-2 Testing (Indexing)
- Use `/home/molaco/Documents/rust-code-mcp` as test corpus
- Expected: ~368 LOC, should index in <1s
- Test incremental updates by modifying files

### Phase 3-4 Testing (Embeddings)
- Use small Rust functions as test cases
- Verify embedding dimensions (384 or 768)
- Test batch processing

### Phase 5-6 Testing (Hybrid Search)
- Test queries:
  - "search for files" → Should favor BM25
  - "find code that handles errors" → Should favor semantic
  - "Tantivy index creation" → Hybrid should excel
- Compare precision@10 vs BM25-only

---

## Success Criteria

### Phase 1 Complete When:
- [ ] Index persists across restarts
- [ ] File changes trigger selective reindexing
- [ ] Metadata cache tracks file hashes

### Phase 2 Complete When:
- [ ] Rust symbols extracted (structs, fns, traits)
- [ ] Symbols searchable in Tantivy
- [ ] Symbol locations accurate

### Phase 6 Complete When:
- [ ] Hybrid search returns relevant results
- [ ] RRF fusion improves precision vs BM25-only
- [ ] Query latency <500ms for 100k LOC

---

## Risk Mitigation

### Risk: Bloop patterns too complex
**Mitigation**: Extract incrementally, simplify as we go

### Risk: Qdrant deployment complexity
**Status**: Docker not available on system
**Mitigation**:
- ✅ **Use embedded Qdrant** for MVP (in-process, no Docker needed)
- Make Qdrant optional (degrade to BM25-only mode)
- Provide Nix flake for production deployment
- Support both embedded and remote modes via config

### Risk: Embeddings too slow
**Mitigation**:
- Use smaller models (384-dim vs 768-dim)
- Batch processing
- Cache embeddings aggressively

---

## Next Steps (Week 2 - Phase 1)

1. **Update `Cargo.toml`** with Phase 1 dependencies
2. **Create `src/schema.rs`** based on bloop's schema
3. **Make Tantivy index persistent** (change line in search_tool.rs:123)
4. **Implement `src/metadata_cache.rs`** for file tracking
5. **Add `src/watcher.rs`** for file change detection

**Goal**: Have persistent, incremental indexing working by end of Week 4.

---

**Last Updated**: 2025-10-17
**Next Review**: End of Phase 1 (Week 4)
