# claude-context vs rust-code-mcp: Side-by-Side Comparison

**Research Date:** 2025-10-19

---

## Quick Reference Table

| Feature | claude-context | rust-code-mcp (Current) | rust-code-mcp (After Fixes) |
|---------|----------------|------------------------|--------------------------|
| **Language** | TypeScript | Rust | Rust |
| **Status** | Production ✓ | Development | Target |
| **Token Efficiency** | 40% reduction ✓ | Unknown (broken) | 45-50% reduction (projected) |
| | | | |
| **CHANGE DETECTION** | | | |
| Method | Merkle tree | SHA-256 per-file | Merkle tree |
| Speed (unchanged) | Milliseconds ✓ | Seconds | Milliseconds ✓ |
| Speed (changed) | Seconds ✓ | Seconds | Seconds ✓ |
| Directory skipping | Yes ✓ | No | Yes ✓ |
| Complexity | O(1)/O(log n) | O(n) | O(1)/O(log n) ✓ |
| State persistence | ~/.context/merkle/ | sled cache only | Both ✓ |
| | | | |
| **SEARCH CAPABILITIES** | | | |
| Vector search | Yes (Milvus) ✓ | Yes (Qdrant) - BROKEN ✗ | Yes (Qdrant) ✓ |
| Lexical search (BM25) | No | Yes (Tantivy) ✓ | Yes (Tantivy) ✓ |
| Hybrid search | No | Yes - BROKEN ✗ | Yes (RRF) ✓ |
| | | | |
| **CODE CHUNKING** | | | |
| Primary method | AST (tree-sitter) ✓ | text-splitter | AST (RustParser) ✓ |
| Semantic units | Functions/classes ✓ | Tokens | Functions/structs ✓ |
| Fallback | RecursiveCharacterTextSplitter | None | text-splitter ✓ |
| Context enrichment | Yes ✓ | Partial | Yes ✓ |
| | | | |
| **EMBEDDINGS** | | | |
| Provider | OpenAI/Voyage/Ollama | fastembed | fastembed |
| Model | text-embedding-3-large (3072d) | all-MiniLM-L6-v2 (384d) | all-MiniLM-L6-v2 (384d) |
| Quality | High | Medium | Medium |
| Cost | $$$ (API calls) | Free ✓ | Free ✓ |
| Privacy | Sends to cloud | 100% local ✓ | 100% local ✓ |
| Offline capable | No | Yes ✓ | Yes ✓ |
| | | | |
| **STORAGE** | | | |
| Vector DB | Milvus/Zilliz Cloud | Qdrant (embedded/remote) | Qdrant ✓ |
| Lexical index | None | Tantivy ✓ | Tantivy ✓ |
| Metadata cache | Merkle snapshots | sled KV ✓ | sled + Merkle ✓ |
| Deployment | Cloud-first | Local-first ✓ | Local-first ✓ |
| | | | |
| **INDEXING** | | | |
| Incremental | Yes (file-level) ✓ | Yes (file-level) ✓ | Yes (file + dir level) ✓ |
| Auto-sync | Every 5 min | On search call | Background watch (optional) |
| Persistence | Yes ✓ | Partial | Yes ✓ |
| Populate vector DB? | Yes ✓ | NO ✗ | Yes ✓ |
| Populate lexical? | N/A | Yes ✓ | Yes ✓ |
| | | | |
| **MULTI-LANGUAGE** | | | |
| Supported | TypeScript, Python, Java, Go, Rust, etc. ✓ | Rust only | Rust (extensible) |
| tree-sitter parsers | Multiple ✓ | Rust only | Rust + others (planned) |
| | | | |
| **PERFORMANCE** | | | |
| Token reduction | 40% (proven) ✓ | N/A | 45-50% (projected) |
| Unchanged check | < 10ms ✓ | ~seconds | < 10ms ✓ |
| 1% change | ~1% index time | ~1% index time | < 1% index time ✓ |
| First index (100k LOC) | Unknown | ~2 min | < 2 min ✓ |
| | | | |
| **DEPLOYMENT** | | | |
| Dependencies | Node.js, npm, API keys | Rust, Qdrant | Rust, Qdrant |
| Cloud required | Yes (Zilliz Cloud) | No ✓ | No ✓ |
| Internet required | Yes (embeddings) | No ✓ | No ✓ |
| Air-gapped capable | No | Yes ✓ | Yes ✓ |
| Recurring cost | Subscription | $0 ✓ | $0 ✓ |

---

## Feature Breakdown

### 1. Change Detection

**claude-context:**
```
Phase 1: Calculate Merkle root → Compare with cache
  ↓ (if unchanged)
  Return in <10ms ✓

  ↓ (if changed)
Phase 2: Traverse tree → Find changed files
  ↓
Phase 3: Reindex only changed files
```

**rust-code-mcp (current):**
```
For each file:
  ├─ Read content
  ├─ Calculate SHA-256
  ├─ Compare with cached hash
  └─ If changed: Reindex

Problem: Must hash ALL files every time (slow)
```

**rust-code-mcp (after fix):**
```
Same as claude-context (Merkle tree)
+ Additional Tantivy indexing
```

**Winner:** claude-context ✓ (rust-code-mcp will match after fix)

---

### 2. Search Capabilities

**claude-context:**
- Vector search only (Milvus)
- Semantic similarity
- No exact match ranking

**rust-code-mcp (current):**
- Lexical search works (Tantivy/BM25) ✓
- Vector search broken (Qdrant empty) ✗
- Hybrid search non-functional ✗

**rust-code-mcp (after fix):**
- Lexical search (Tantivy/BM25) ✓
- Vector search (Qdrant) ✓
- Hybrid search (RRF fusion) ✓
- **Best of both worlds**

**Winner:** rust-code-mcp (after fix) ✓✓

---

### 3. Code Chunking

**claude-context:**
- Primary: AST-based (tree-sitter)
  - Functions, classes, methods
  - Semantic completeness
- Fallback: RecursiveCharacterTextSplitter (1000 chars, 200 overlap)

**rust-code-mcp (current):**
- Only: text-splitter (token-based, 512 tokens)
- Tree-sitter aware but not AST-guided
- **Missing:** Symbol-based chunking

**rust-code-mcp (after fix):**
- Primary: AST-based (RustParser)
  - Functions, structs, impls, traits
  - Same as claude-context approach
- Fallback: text-splitter

**Winner:** claude-context ✓ (rust-code-mcp will match after fix)

---

### 4. Embeddings

**claude-context:**
- OpenAI text-embedding-3-large (3072d) - High quality
- Cost: $0.00013/1K tokens (adds up fast)
- Privacy: Code sent to OpenAI servers
- Offline: No

**rust-code-mcp:**
- fastembed all-MiniLM-L6-v2 (384d) - Medium quality
- Cost: $0 (free)
- Privacy: 100% local, zero API calls
- Offline: Yes

**Trade-off:**
| Aspect | claude-context | rust-code-mcp |
|--------|----------------|---------------|
| Quality | Higher (3072d) | Lower (384d) |
| Cost | $$$ | Free |
| Privacy | Cloud | Local |
| Offline | No | Yes |

**Winner:** Depends on use case
- Enterprise with API budget → claude-context
- Privacy/offline/cost-sensitive → rust-code-mcp ✓

---

### 5. Storage & Deployment

**claude-context:**
- Milvus/Zilliz Cloud (managed service)
- Requires internet + subscription
- Elastic scaling
- Cloud-native

**rust-code-mcp:**
- Qdrant (embedded or self-hosted)
- No internet required
- Local control
- Self-hosted

**Winner:** rust-code-mcp ✓ (local-first, zero dependencies)

---

## Use Case Recommendations

### Choose claude-context if:
- You need multi-language support NOW (TypeScript, Python, Java, etc.)
- You want highest embedding quality (3072d)
- You're okay with cloud dependencies
- You have API budget
- You want managed infrastructure (Zilliz Cloud)

### Choose rust-code-mcp if:
- You need 100% local/private (no code sent externally)
- You want zero ongoing costs
- You need offline capability
- You want true hybrid search (BM25 + Vector)
- You prefer self-hosted infrastructure
- You work with Rust codebases primarily

---

## Critical Differences Summary

### claude-context Advantages
1. ✓ Production-proven (40% token savings)
2. ✓ Merkle tree change detection (millisecond checks)
3. ✓ Multi-language support
4. ✓ AST-based chunking
5. ✓ Managed infrastructure

### rust-code-mcp Advantages (after fixes)
1. ✓ True hybrid search (BM25 + Vector vs vector-only)
2. ✓ 100% local/private (no API calls)
3. ✓ Zero ongoing costs (vs subscription)
4. ✓ Offline capable
5. ✓ Self-hosted (full control)
6. ✓ Rust performance and safety

### rust-code-mcp Current Issues
1. ✗ Qdrant never populated (critical bug)
2. ✗ No Merkle tree (slower change detection)
3. ✗ Not using AST for chunking (lower quality)

---

## Implementation Priority for rust-code-mcp

### Critical (Week 1)
**Fix Qdrant population:**
- Parse → Chunk → Embed → Index to Qdrant
- Enables hybrid search (core feature)
- **Estimated effort:** 2-3 days

### High Priority (Week 2-3)
**Implement Merkle tree:**
- Add rs_merkle dependency
- Three-phase detection
- Persist snapshots
- **Estimated effort:** 1-2 weeks
- **Speedup:** 100-1000x for unchanged codebases

### High Priority (Week 4)
**AST-first chunking:**
- Use existing RustParser for symbol chunks
- Fallback to text-splitter
- **Estimated effort:** 3-5 days
- **Improvement:** Better semantic quality

### Nice to Have (Week 5+)
**Background file watching:**
- notify crate integration
- Auto-sync like claude-context
- **Estimated effort:** 1 week

---

## Performance Projections

### Token Efficiency
- **claude-context:** 40% reduction (vector-only)
- **rust-code-mcp (projected):** 45-50% reduction (hybrid)
  - BM25 catches exact matches (high precision)
  - Vector catches semantic matches (high recall)
  - RRF fusion combines strengths

### Indexing Speed
- **claude-context:** Milliseconds for unchanged (Merkle root check)
- **rust-code-mcp (current):** Seconds (hash all files)
- **rust-code-mcp (after fix):** Milliseconds (Merkle root check)

### Memory Usage
- **claude-context:** Unknown (cloud-managed)
- **rust-code-mcp:**
  - Merkle tree: ~1-2 KB per file
  - Metadata cache: ~200 bytes per file
  - Total overhead (1M LOC): ~50-100 MB

---

## Conclusion

**Current State:**
- claude-context: Production-ready, vector-only, cloud-dependent
- rust-code-mcp: Development, hybrid-capable but broken, local-first

**After Fixes:**
- rust-code-mcp should **exceed** claude-context in:
  - Token efficiency (45-50% vs 40%)
  - Search quality (hybrid vs vector-only)
  - Privacy (100% local vs cloud APIs)
  - Cost (free vs subscription)

**Timeline:**
- 2-3 days: Fix Qdrant (enable hybrid)
- 1-2 weeks: Add Merkle tree (match change detection speed)
- 3-5 days: AST chunking (match chunking quality)
- **Total:** 3-4 weeks to parity and beyond

**Recommendation:**
Implement all three fixes. rust-code-mcp has the foundation to surpass claude-context while maintaining local-first principles.

---

**Generated:** 2025-10-19
**Source:** Comprehensive research of zilliztech/claude-context
