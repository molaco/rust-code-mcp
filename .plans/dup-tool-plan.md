# `semantic_overlaps` — duplicate-detection MCP tool plan

A new MCP tool that finds semantically similar items across a workspace via vector embeddings. Sits alongside the existing `similar_to_item` (v0.1, seed-driven) as the workspace-wide audit equivalent. Two phases: **v1.0 ships discovery + clustering**, **v1.1 layers in performance optimizations**.

The user's mental model: `similar_to_item` answers *"given X, what's like X?"*. `semantic_overlaps` answers *"what's duplicated that I don't know about?"*.

---

## Background

### What exists today

- **`similar_to_item` (v0.1)** — `/src/tools/graph_tools.rs::similar_to_item`. One seed in, top-K neighbors out. Single-target, ~100-300ms latency. Bridges hypergraph (Item → file/span) with the existing vector store (chunk embeddings).
- **`get_similar_code`** — older free-text vector search; doesn't know about Items. The vector store is built by `index_codebase` (LanceDB-backed via `fastembed-rs`).
- **`build_hypergraph`** — produces an LMDB snapshot of Items per workspace. Schema currently v10.

### What `semantic_overlaps` adds over `similar_to_item`

| Capability | `similar_to_item` v0.1 | `semantic_overlaps` v1.0 |
|---|---|---|
| Discovery without a seed | no | **yes** |
| Pairwise dedup of symmetric matches | n/a | yes |
| Cluster grouping (transitively similar items) | no | yes |
| Global ranking across all items | no | yes |
| Scope filters (crate, kind, cross-crate-only, skip-tests) | partial | full |
| Latency | <300ms | seconds-to-minutes |
| Use case | chat-time investigation | offline audit, refactor planning |

The two tools are complementary; both ship.

### Research summary

| Source | Finding |
|---|---|
| codesim ([arxiv 2401.09885](https://arxiv.org/html/2401.09885v1)) | Threshold-based similarity is the dominant approach. **0.80** is a common starting point for general embedders. |
| GraphCodeBERT papers ([arxiv 2408.08903](https://arxiv.org/html/2408.08903v1)) | Threshold depends on the model: Word2Vec 0.85, Doc2Vec 0.75, Code2Vec 0.91, **CodeBERT 0.95**, CodeT5 0.87. Implication: hardcode a sane default but expose the knob. |
| HDBSCAN docs | Single-linkage is fast and natural for "transitively similar" duplicates but suffers from chaining (one outlier bridges distant clusters). HDBSCAN handles varying density and is more robust. **For v1.0, use single-linkage**; HDBSCAN is a v1.2+ option. |
| `similarity-rs` | Token-based clone detection exists for Rust; syntactic-only, complementary to our embedding approach. |
| LanceDB docs | Batch query API exists. <1ms per query in best case, multi-threaded. **Free perf win for v1.1**. |
| LLM-clone study ([arxiv 2511.01176](https://arxiv.org/html/2511.01176v1)) | Token-based and tree-based methods detect Type 1-3 clones well; Type 4 (semantically equivalent, syntactically distinct) requires embeddings or LLMs. |

---

## v1.0 — Discovery + clustering (~300 LOC, no schema bump)

### Tool surface

New file: `src/tools/search_tool.rs::SemanticOverlapsParams`.

```rust
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct SemanticOverlapsParams {
    /// Workspace root (directory containing Cargo.toml)
    pub directory: String,

    /// Optional crate qualified name to scope the scan. Defaults to all
    /// local crates.
    #[serde(default)]
    pub crate_name: Option<String>,

    /// Optional item-kind filter ("Function" | "Struct" | "Enum" | "Trait"
    /// | "Method"). Default: all kinds.
    #[serde(default)]
    pub item_kind: Option<String>,

    /// Minimum cosine similarity. Default 0.80 (general embedder
    /// convention). Raise to 0.85+ for stricter "definitely duplicate"
    /// signal.
    #[serde(default)]
    pub threshold: Option<f32>,

    /// Cap on returned pairs OR cluster member count. Default 50.
    #[serde(default)]
    pub max_pairs: Option<usize>,

    /// Output mode: "pairs" (raw similarity edges) or "clusters" (grouped
    /// transitively-similar items). Default "clusters".
    #[serde(default)]
    pub output_mode: Option<String>,

    /// Drop matches whose qualified name contains `::tests::`. Default
    /// true — test fixtures dominate noise.
    #[serde(default)]
    pub skip_test_chunks: Option<bool>,

    /// Drop pairs whose two items share a crate. Default false.
    #[serde(default)]
    pub cross_crate_only: Option<bool>,
}
```

Response:
```rust
#[derive(Serialize)]
struct SemanticOverlapsResp {
    scope: ScopeSummary,
    threshold: f32,
    pair_count: usize,                 // total pairs above threshold (pre-cap)
    output_mode: String,
    pairs: Option<Vec<SimilarityPair>>,    // when output_mode == "pairs"
    clusters: Option<Vec<SimilarityCluster>>, // when output_mode == "clusters"
}

#[derive(Serialize)]
struct SimilarityPair { a: ItemRef, b: ItemRef, similarity: f32 }

#[derive(Serialize)]
struct SimilarityCluster {
    members: Vec<ItemRef>,
    avg_similarity: f32,
    min_similarity: f32,
    size: usize,
    truncated: bool,                   // true if member count was capped
}

#[derive(Serialize)]
struct ItemRef {
    qualified_name: String,
    item_kind: Option<String>,
    file: String,
    span: (u32, u32),
}
```

### Algorithm

1. **Enumerate seed items.** Iterate `nodes_by_id`, filter to `Item` kind matching `crate_name` (resolved to NodeId) and optional `item_kind`. Skip items missing `(file, span)` (synthetic / macro-generated).

2. **For each seed, fetch source bytes.** Read `[file, span]` slice. Cache file contents per file path (avoid re-reading the same file N times). Skip items with empty / whitespace-only source.

3. **Run vector search per seed.** Reuse `similar_to_item`'s pipeline: `vector_only_search(seed_source, K)` where K = 20.

4. **Build similarity graph.** For each result chunk:
   - Resolve back to a hypergraph Item via `(file, line_range)` overlap with `nodes_by_id`. Skip if no Item covers the range.
   - Skip self-match (already-fixed v0.1 logic via `Path::ends_with`).
   - Skip if `cross_crate_only=true` and `node_a.crate_id == node_b.crate_id`.
   - Skip if `skip_test_chunks=true` and either node's qualified_name contains `::tests::`.
   - Skip if score < threshold.
   - Insert directed edge `(a, b, score)` into the graph.

5. **Symmetric dedup.** A→B and B→A become one undirected edge. Take the **average** of the two scores (more conservative than max).

6. **Pairs output.** Sort pairs by score descending, take top `max_pairs`, return as `[{a, b, similarity}]`.

7. **Clusters output (single-linkage).** Treat the similarity graph as undirected. Run union-find: for each edge, union the two endpoints. Output: `[{members, avg_similarity, min_similarity, size}]`. Sort clusters by size descending, then min_similarity descending. Cap each cluster's members at `max_pairs` (set `truncated: true` when cap kicks in). Drop trivial size-1 clusters.

### Wrapper-side concerns

- Wrap the whole scan in `tokio::task::spawn_blocking` (mirrors `unsafe_audit` and `build_hypergraph` pattern). The runtime worker stays free.
- All-or-nothing per call; no streaming partial results.

### Cost estimates (without v1.1)

- file_search_mcp: ~1200 items × 150ms vector search = **~3 minutes**.
- coding-agent: ~2200 items × 150ms = **~5.5 minutes**.
- With `crate_name` scope (typical use): 100-300 items × 150ms = **15-45s**. Tractable for an interactive call.

### Tests

Pure unit tests (no DB, no network):
- Union-find clusterer against a hand-constructed similarity graph.
- Chunk-to-Item mapper given known `(file, line_range, span)` triples.

End-to-end test deferred — depends on indexed state, hard to make deterministic.

---

## v1.1 — Performance optimizations (~200 LOC, **schema bump v10 → v11**)

Layer on top of v1.0; only ship after v1.0 is verified working. Order of expected wins:

### v1.1a. Embedding cache (biggest win)

**Problem**: each scan re-reads file source, generates embeddings via `EmbeddingGenerator`, and queries. Embedding generation per-item is the hot spot — vector search itself is fast.

**Solution**: persist `NodeId → (content_hash, Vec<f32>, embedder_version)` in a new sub-DB `embeddings_by_target`. On scan:

1. For each seed Item, hash the source bytes (SHA-256 truncated to 16 bytes).
2. Lookup in the cache: if (NodeId, content_hash, embedder_version) match, reuse.
3. Else: generate embedding, store it.
4. Run nearest-neighbor against cached vectors directly (skip the LanceDB roundtrip when feasible — match in-memory vectors).

**Schema changes**:
```rust
// In storage.rs
pub embeddings_by_target: Database<Bytes, SerdeBincode<EmbeddingRecord>>,

// In model.rs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingRecord {
    pub content_hash: [u8; 16],
    pub vector: Vec<f32>,
    pub embedder_version: String,   // e.g. "fastembed:all-MiniLM-L6-v2:v1"
    pub generated_at_unix: u64,
}
```

- DEFAULT_MAX_DBS currently 16, we'd be at 13 → 14 sub-DBs. Headroom fine.
- Schema bump v10 → v11; old snapshots auto-rebuild via `graph_id_for` hashing `SCHEMA_VERSION`.

**Cost reduction**: First scan still pays the cost. Subsequent scans on unchanged code are essentially free per item — just cache lookups. Modified items re-embed.

### v1.1b. Batch vector queries

**Problem**: 1200 sequential vector searches × 150ms = 3 minutes.

**Solution**:
1. Stage seed embeddings into a `Vec<Vec<f32>>`.
2. Issue a single batch query via LanceDB's batch API.
3. Process the result tensor.

**Expected speedup**: 5-10× on the vector-search portion alone. Combined with v1.1a's cache: 10-50× total scan-time reduction.

### v1.1c. Skip identical-source items

If two items have byte-identical source (rare but happens for trivial getters / generated code), skip embedding generation entirely — content_hash equality is sufficient signal. Group them as their own cluster directly.

### v1.1d. Pre-clustering with k-means (optional, only for huge workspaces)

For 5000+ items: cluster embeddings into ~32 buckets via k-means (~1s for 5k items), run pairwise nearest-neighbor only within each bucket. Cuts O(N²) → O(N × k). Probably not needed for typical Rust workspaces.

---

## Implementation order

1. **Refactor: extract chunk → Item resolver** as a free fn `resolve_chunk_to_item(snap, chunk_file, chunk_lines) -> Option<NodeId>`. Used by both `similar_to_item` (already inline) and `semantic_overlaps`. ~30 LOC.
2. **v1.0 scaffolding**: param struct, MCP route, response shape, `tokio::spawn_blocking` wrapper. ~80 LOC.
3. **v1.0 core algorithm**: seed enumeration → per-seed search → graph build → dedup → output. ~150 LOC.
4. **v1.0 clustering**: union-find on the similarity graph. ~50 LOC.
5. **v1.0 unit tests**: clusterer + chunk-to-item resolver. ~50 LOC.
6. **Update TOOLS.md** with the new tool entry.
7. **Ship v1.0**, manually audit findings on coding-agent and a smaller workspace. Tune the default threshold based on what surfaces.
8. **v1.1a (embedding cache)**: schema bump v10→v11, new sub-DB, cache-lookup logic. ~100 LOC. Don't ship until v1.0 is validated — premature caching adds complexity without clear win.
9. **v1.1b (batch queries)**: refactor scan to use batch API. ~50 LOC.
10. **v1.1c (skip identical)**: ~10 LOC. Trivial.
11. **Skip v1.1d unless real workspaces hit the perf ceiling.**

---

## Risks

| Risk | Mitigation |
|---|---|
| Threshold tuning is workspace-dependent | Default 0.80; doc the per-model values from research; expose param |
| Single-linkage chaining (one outlier bridges distant clusters) | Document; future option to swap in HDBSCAN-style cut |
| Chunk → Item mapping is fuzzy when chunks span multiple items or vice-versa | Skip ambiguous mappings; report which seeds got dropped |
| Test fixtures dominate output noise | `skip_test_chunks: true` default |
| Embedding cache invalidation on embedder version change | Include embedder version + dim in `EmbeddingRecord`; clear on mismatch |
| Output volume on workspace-wide scans | `max_pairs` cap + clusters output mode (more compact than pairs) |
| Wall-clock latency unsuitable for chat | Document as offline-audit tool; combine with v1.1 cache for repeated runs |

---

## Decision points (resolve before implementing)

1. **Default threshold**: 0.80 (research consensus for general embedders) or 0.85 (more conservative)? **Recommend 0.80** with a doc note that 0.85+ is safer for "definitely a duplicate" signal.
2. **Cluster vs pairs default**: clusters is more actionable. **Recommend default to clusters**; pairs is for callers who want raw signal.
3. **Cross-crate-only default**: noise reduction is huge with this on, but you lose intra-crate findings (test fixture duplication, etc.). **Recommend default false**; document the toggle.
4. **Test mod filter default**: definitely **default true**. Test fixtures are the dominant noise source.

---

## What NOT to build (yet)

- **Cross-workspace duplicate detection** — needs index federation, out of scope.
- **HDBSCAN clustering** — single-linkage is enough for v1.0; revisit if outliers cluster badly.
- **Code-similarity-aware embedders** (CodeBERT/GraphCodeBERT) — these need Python; the existing fastembed pipeline is good enough for surface-level pattern detection. v1.0 surfaces candidates for human review, not autonomous refactor decisions.
- **Pre-PR git-aware audit** (only audit items modified since base ref) — useful but pulls in `git2` or shells to git, separate concern.

---

## Effort summary

| Phase | LOC | Schema bump? | Cost reduction |
|---|---|---|---|
| v1.0 | ~300 | no | n/a (baseline) |
| v1.1a — embedding cache | ~100 | yes (v10 → v11) | ~5-20× on warm scans |
| v1.1b — batch queries | ~50 | no | ~5-10× |
| v1.1c — skip identical | ~10 | no | trivial |
| v1.1d — k-means pre-cluster | ~80 | no | only for >5k items |
| **Total** | **~540** | **1 bump** | **10-50× total when both v1.1a + v1.1b shipped** |

---

## Sources

- [Source Code Clone Detection Using Unsupervised Similarity Measures (arxiv 2024)](https://arxiv.org/html/2401.09885v1)
- [Improving Source Code Similarity Detection Through GraphCodeBERT (arxiv 2024)](https://arxiv.org/html/2408.08903v1)
- [Augmenting the Interpretability of GraphCodeBERT for Code Similarity Tasks](https://arxiv.org/html/2410.05275v1)
- [HDBSCAN: How HDBSCAN Works](https://hdbscan.readthedocs.io/en/latest/how_hdbscan_works.html)
- [HDBSCAN: Comparing Python Clustering Algorithms](https://hdbscan.readthedocs.io/en/latest/comparing_clustering_algorithms.html)
- [similarity-rs: token-based Rust clone detection](https://lib.rs/crates/similarity-rs)
- [LanceDB Vector Search docs](https://docs.lancedb.com/search/vector-search)
- [An Empirical Study of LLM-Based Code Clone Detection (arxiv 2025)](https://arxiv.org/html/2511.01176v1)
- [CC2Vec: Combining Typed Tokens with Contrastive Learning (FSE 2024)](https://wu-yueming.github.io/Files/FSE2024_CC2Vec.pdf)
