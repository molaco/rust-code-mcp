# Claude-Context Research: Quick Reference Card

**TL;DR:** Production-proven Merkle tree + AST chunking approach achieves 40% token savings. rust-code-mcp needs 3 fixes to match/exceed.

---

## 🎯 Key Takeaways (30 seconds)

1. **Incremental Indexing:** ✅ YES - Merkle tree (3-phase: rapid/precise/incremental)
2. **Change Detection:** Merkle tree + SHA-256 (milliseconds for unchanged, seconds for changed)
3. **Caching:** Merkle snapshots (`~/.context/merkle/`) + Milvus vector DB
4. **Performance:** 40% token reduction, <10ms unchanged checks, production-proven
5. **vs rust-code-mcp:** Similar goals, different trade-offs (cloud vs local)

---

## 📊 One-Sentence Answers

### 1. Does it support incremental indexing?
**YES** - Three-phase Merkle tree approach: root hash check (milliseconds) → tree traversal (seconds) → reindex changed files only.

### 2. How does it detect file changes?
**Merkle tree with SHA-256 hashing** - Hierarchical fingerprinting (files → folders → root) enables O(1) unchanged checks and O(log n) change detection.

### 3. What caching does it use?
**Merkle snapshots + Milvus** - Local snapshots in `~/.context/merkle/` (persistent state) + Milvus/Zilliz Cloud (vector data).

### 4. What are the performance characteristics?
**40% token reduction** vs grep-only (proven in production), **millisecond** change detection for unchanged codebases, **seconds** for changed files.

### 5. How does it compare to rust-code-mcp?
**Similar architecture, different deployment** - claude-context is cloud-first (TypeScript, Milvus, OpenAI embeddings), rust-code-mcp is local-first (Rust, Qdrant, fastembed) with true hybrid search.

---

## ⚡ Critical Findings

### What Works (Proven at Scale)
- ✅ Merkle tree change detection (milliseconds)
- ✅ AST-based chunking (semantic units)
- ✅ File-level incremental updates (sufficient)
- ✅ State persistence (restart resilient)
- ✅ 40% token savings (production-validated)

### What rust-code-mcp Needs
- ❌ **CRITICAL:** Populate Qdrant (hybrid search broken)
- ❌ **HIGH:** Add Merkle tree (100-1000x speedup)
- ❌ **HIGH:** AST-first chunking (better quality)

### What rust-code-mcp Has (Already Working)
- ✅ Tantivy (BM25 search)
- ✅ RustParser (symbol extraction)
- ✅ fastembed (local embeddings)
- ✅ Qdrant infrastructure (just not populated)
- ✅ RRF algorithm (hybrid search ready)

---

## 🏆 Winner: Depends on Use Case

| Factor | claude-context | rust-code-mcp (after fixes) |
|--------|----------------|---------------------------|
| **Token efficiency** | 40% (proven) | 45-50% (projected) ✓ |
| **Change detection** | Milliseconds ✓ | Milliseconds ✓ |
| **Search type** | Vector-only | Hybrid (BM25 + Vector) ✓ |
| **Privacy** | Cloud APIs | 100% local ✓ |
| **Cost** | Subscription | $0 ✓ |
| **Multi-language** | Yes ✓ | Rust only (extensible) |
| **Embedding quality** | High (3072d) | Medium (384d) |
| **Offline** | No | Yes ✓ |

**Recommendation:**
- Enterprise with API budget + multi-language → **claude-context**
- Privacy/offline/cost-sensitive + Rust focus → **rust-code-mcp**

---

## 🔧 Implementation Roadmap (rust-code-mcp)

### Week 1: Fix Qdrant Population (CRITICAL)
```rust
// Add to search tool:
let chunks = chunker.chunk_file(&path)?;
let embeddings = embed_gen.embed_batch(&chunks)?;
vector_store.upsert_chunks(chunks.zip(embeddings)).await?; // ← MISSING
```
**Effort:** 2-3 days | **Impact:** Enables hybrid search

### Week 2-3: Add Merkle Tree (HIGH)
```rust
// Add rs_merkle dependency
let merkle = MerkleIndexer::build_tree(&dir)?;
if let Some(cached) = load_snapshot()? {
    if merkle.root_hash() == cached.root_hash() {
        return Ok(()); // ← <10ms, 1000x faster
    }
    let changed = merkle.detect_changes(&cached);
}
```
**Effort:** 1-2 weeks | **Impact:** 100-1000x speedup

### Week 4: AST-First Chunking (HIGH)
```rust
// Use existing RustParser for chunking
let symbols = parser.parse_file(&path)?;
let chunks = symbols.into_iter()
    .map(|sym| CodeChunk::from_symbol(sym))
    .collect();
```
**Effort:** 3-5 days | **Impact:** Better semantic quality

### Week 5: Background Watching (Optional)
```rust
// Add notify crate for auto-sync
let watcher = notify::watcher()?;
watcher.watch(&dir, RecursiveMode::Recursive)?;
```
**Effort:** 1 week | **Impact:** Real-time updates

---

## 📈 Expected Outcomes

### After All Fixes
| Metric | Target | vs claude-context |
|--------|--------|-------------------|
| Token efficiency | 45-50% | Better (hybrid vs vector-only) |
| Unchanged check | < 10ms | Same (Merkle tree) |
| Privacy | 100% local | Better (no cloud APIs) |
| Cost | $0 | Better (no subscription) |
| Search quality | BM25 + Vector | Better (hybrid) |

### Timeline
- **3-4 weeks** total
- All components already present
- Just need integration + Merkle tree

---

## 📚 Three-Phase Merkle Tree (Explained)

### Phase 1: Lightning-Fast Detection
```
Current Merkle root: 0x8a3f...
Cached Merkle root:  0x8a3f...
→ MATCH! Skip all processing (<10ms) ✓
```

### Phase 2: Precise Comparison
```
Current Merkle root: 0x8a3f...
Cached Merkle root:  0x7b2e...
→ DIFFER! Traverse tree:
  - src/ hash changed
    - lib.rs unchanged ✓
    - parser/ hash changed
      - mod.rs CHANGED ← Found it!
```

### Phase 3: Incremental Updates
```
Changed files: [parser/mod.rs]
→ Reindex only this file
→ Save new Merkle snapshot
```

---

## 🎓 Lessons Learned

### From claude-context (Production-Proven)
1. Merkle tree is **essential**, not optional
2. AST chunking **superior** to token-based
3. File-level granularity **sufficient**
4. State persistence **critical**
5. 40% efficiency gains **realistic**

### Mistakes to Avoid (rust-code-mcp)
1. ❌ Treating Merkle tree as Phase 3 optimization
2. ❌ Not populating vector store during indexing
3. ❌ Using text-splitter when AST parser available
4. ❌ Relying on mtime instead of content hashing
5. ❌ Not persisting change detection state

---

## 🔗 Full Documentation

- **Complete report:** `claude_context_research_report.yaml` (27 KB)
- **Summary:** `claude_context_research_summary.md` (16 KB)
- **Comparison:** `claude_context_vs_rust_code_mcp_comparison.md` (9.5 KB)
- **Index:** `README.md`

---

## 📞 Quick Commands

### Find Merkle tree details
```bash
grep -A 20 "merkle_tree_structure:" claude_context_research_report.yaml
```

### View comparison table
```bash
head -100 claude_context_vs_rust_code_mcp_comparison.md
```

### Get implementation roadmap
```bash
grep -A 30 "implementation_roadmap:" claude_context_research_report.yaml
```

---

## 🎯 Bottom Line

**claude-context proves:**
- Merkle tree + AST chunking works at scale
- 40% token savings achievable
- File-level incremental updates sufficient

**rust-code-mcp can exceed this by:**
- Adding Merkle tree (match speed)
- Fixing Qdrant (enable hybrid search)
- Using AST chunking (match quality)

**Result:** 45-50% token savings + 100% local privacy + $0 cost

---

**Generated:** 2025-10-19
**Research Confidence:** High (production-validated)
**Implementation Timeline:** 3-4 weeks to parity and beyond
