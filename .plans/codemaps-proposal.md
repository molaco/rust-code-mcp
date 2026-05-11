# Proposal: task-conditioned codemap tool for `file-search-mcp`

**Target:** single-crate workspace at `/home/molaco/Documents/rust-code-mcp-final` (crate `file-search-mcp`, modules under `src/`).
**Status:** revision of an earlier multi-crate proposal, with fictional crate layout removed and embedding / trait-dispatch issues addressed.

## 1. Goal

A single MCP tool, `build_codemap`, that turns a natural-language prompt into a focused subgraph of the indexed workspace: hierarchical outline + node/edge JSON + optional Mermaid. The shape is *seeds → expand → score → prune → project*, and every implementation detail below is anchored to files that actually exist.

## 2. Where things live

Reusing the existing module layout — no new crates, no workspace migration.

| New code                    | Location                                              | Purpose                                                                                |
|-----------------------------|-------------------------------------------------------|----------------------------------------------------------------------------------------|
| `Codemap*` model types      | `src/graph/model.rs` (append)                         | `Codemap`, `CodemapNode`, `CodemapEdge`, `EdgeKind`, `CodemapStats`, `CodemapOptions`. |
| Span index                  | `src/graph/snapshot.rs` (extend `OpenedSnapshot`)     | Lazy per-snapshot `HashMap<FileKey, IntervalTree<NodeId>>`.                            |
| Algorithm                   | `src/graph/codemap.rs` (new module)                   | `build_codemap`, helpers, Mermaid/outline renderers.                                   |
| Item-kind helpers           | `src/graph/queries.rs` (append)                       | `is_callable(NodeId)`, `is_type(NodeId)`, `crate_of(NodeId)`, `enclosing_item(file, byte_range)`. |
| Optional persistence (v2)   | `src/graph/storage.rs` — new `codemaps` sub-DB + schema bump | `key = blake3(prompt ‖ snapshot_id)[..16]`, value = `bincode(Codemap)`.        |
| MCP tool                    | `src/tools/search_tool_router.rs` (append a `#[tool]` method) | Wires prompt → search → optional embed → `build_codemap` → response.            |

`Codemap` is the only public addition to the model module; everything else is internal to `src/graph/codemap.rs`.

## 3. Data types

```rust
// src/graph/model.rs (appended)

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Codemap {
    pub prompt: String,
    pub snapshot_id: String,          // existing snapshot identifier; not a new concept
    pub generated_at_unix: u64,
    pub seeds: Vec<NodeId>,
    pub nodes: Vec<CodemapNode>,
    pub edges: Vec<CodemapEdge>,
    pub hierarchy: ModuleTreeNode,    // already defined in src/graph/queries.rs:203
    pub stats: CodemapStats,
    pub diagnostics: Vec<String>,     // e.g. "12 trait-dispatch sites not resolved"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodemapNode {
    pub id: NodeId,
    pub qualified_name: String,
    pub kind: NodeKind,
    pub item_kind: Option<ItemKind>,  // ItemKind::Function | Method | Struct | …
    pub file: Option<String>,
    pub span: Option<(u32, u32)>,
    pub relevance: f32,
    pub is_seed: bool,
    pub snippet: Option<String>,      // optional, gated by opts.include_snippets
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodemapEdge { pub from: NodeId, pub to: NodeId, pub kind: EdgeKind, pub weight: u32 }

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum EdgeKind { Calls, Uses, Imports, Contains }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodemapStats {
    pub seed_count: usize,
    pub node_count: usize,
    pub edge_count: usize,
    pub embedded_nodes: usize,        // how many of the retained nodes had a cached embedding
    pub embeddings_computed: usize,   // how many we had to generate on-the-fly
    pub trait_dispatch_unresolved: usize,
    pub total_ms: u64,
}
```

Counters for the *failure modes* — trait dispatch, cold embedding cache — are exposed so consumers (and tuning work later) can see them rather than discovering them empirically.

## 4. Span index

`Node.file` is `Option<String>` (workspace-relative) and `Node.span` is `Option<(u32, u32)>` (byte offsets) per `src/graph/model.rs:90-91`. Building a per-file `IntervalTree<NodeId>` at first use is straightforward:

```rust
// src/graph/snapshot.rs
pub struct OpenedSnapshot {
    // …existing fields…
    span_index: OnceLock<HashMap<String, IntervalTree<u32, NodeId>>>,
}

impl OpenedSnapshot {
    pub fn span_index(&self) -> &HashMap<String, IntervalTree<u32, NodeId>> {
        self.span_index.get_or_init(|| build_span_index(&self.dbs.nodes_by_id))
    }
}
```

Add `intervaltree = "0.2"` (or hand-roll: at ~1,500 items the trees are small and a `Vec<(span, NodeId)>` + binary search works fine). Lazy init keeps the cost off `OpenedSnapshot::open` — built once per process per snapshot.

`enclosing_item(file, byte_range) → Option<NodeId>` becomes a one-line tree query plus a tiebreak for nested items (prefer the smallest enclosing item).

## 5. Algorithm

```text
inputs:
  snap:         &OpenedSnapshot
  prompt:       &str
  override_seeds: Option<Vec<String>>   // qualified names, resolved via find_definition
  opts:         CodemapOptions {
      max_nodes: 80, depth: 3, top_k_seeds: 20,
      max_incoming_per_node: 8,         // replaces same-crate filter
      embedding_policy: NoRerank | UseCachedOnly | ComputeMissing,
      include_snippets: false,
  }

1. SEEDS
   if override_seeds.is_some():
       seeds = resolve_each_via_find_definition(override_seeds)
   else:
       hits = HybridSearch::query(prompt, k = opts.top_k_seeds * 3)
       for hit in hits:
           if let Some(nid) = span_index.enclosing_item(&hit.file, hit.byte_range):
               if snap.is_callable(nid) || snap.is_type(nid):
                   seeds.insert(nid);
                   if seeds.len() == opts.top_k_seeds { break; }

2. EXPAND  (bounded BFS, both directions, degree-capped)
   retained = seeds.clone()
   frontier = seeds.clone()
   for _ in 0..opts.depth:
       next = HashSet::new()
       for n in &frontier:
           // outgoing — primary signal
           for callee in snap.calls_from(n):
               record_edge(n, callee, Calls);
               if retained.insert(callee) { next.insert(callee); }

           // incoming — degree-capped by relevance
           let mut callers: Vec<_> = snap.who_calls(n).collect();
           callers.sort_by_key(|c| -score_caller_against_prompt(c));
           for caller in callers.into_iter().take(opts.max_incoming_per_node):
               record_edge(caller, n, Calls);
               if retained.insert(caller) { next.insert(caller); }

           // type/data — for struct/enum/trait seeds
           if snap.is_type(n):
               for usage in snap.who_uses(n):
                   if let Some(consumer_fn) = usage.consumer_function:
                       record_edge(consumer_fn, n, Uses);
                       if retained.insert(consumer_fn) { next.insert(consumer_fn); }
       frontier = next

3. SCORE
   for node in &retained:
       let bm25_norm = normalized_search_score(node);   // 0..1, from hybrid hits
       let graph_prox = 1.0 / (1.0 + min_call_distance(node, &seeds));
       let emb_sim = match opts.embedding_policy:
           NoRerank             => 0.0,
           UseCachedOnly        => cached_cosine(node, prompt_emb).unwrap_or(0.0),
           ComputeMissing       => compute_and_cache_cosine(node, prompt_emb),

       node.relevance = if emb_sim > 0.0 { 0.40*emb_sim + 0.35*bm25_norm + 0.25*graph_prox }
                       else              {                  0.60*bm25_norm + 0.40*graph_prox };

4. PRUNE
   keep all seeds.
   among non-seeds, keep top (opts.max_nodes - |seeds|) by relevance.
   drop edges whose endpoints aren't both retained.

5. PROJECT
   module_subtree = filter(snap.module_tree(workspace_root)) keeping branches that contain retained items.

6. ASSEMBLE Codemap, fill diagnostics, return.
```

Three concrete differences from the earlier proposal:

- **`max_incoming_per_node` replaces the same-crate filter.** Picks the most prompt-relevant callers regardless of crate; avoids amplifying noise from "everything calls `log::info`".
- **`embedding_policy` is explicit.** `NoRerank` for latency-sensitive defaults; `UseCachedOnly` for opportunistic reranking using whatever `semantic_overlaps` has already warmed; `ComputeMissing` for the "I want best quality, will wait" mode. Scoring weights branch on whether embeddings are available rather than secretly returning 0 and pretending the formula is consistent.
- **Seed override is in v1.** Skips search entirely when the agent already knows the entry point; resolves names via the existing `find_definition` path.

## 6. MCP tool surface

Add one method to the `#[tool_router]` impl in `src/tools/search_tool_router.rs`, alongside the existing ~47 tools:

```rust
#[tool(description = "
Build a task-conditioned subgraph (codemap) of the indexed workspace.

Returns nodes/edges/hierarchy focused on the prompt. Edges come from the
HIR-driven hypergraph: direct calls and non-import uses. NOTE: method
calls dispatched through trait objects or generic Fn bounds are NOT in
the underlying usage edges, so codemaps may miss callers that go through
trait dispatch — surface this when relevant via the diagnostics field.

Tunable defaults: max_nodes=80, depth=3, embedding_policy='no_rerank'.
")]
async fn build_codemap(&self, params: Parameters<BuildCodemapParams>) -> ... {
    // 1. resolve directory → OpenedSnapshot (existing pattern)
    // 2. if seed_qualified_names: resolve via find_definition; else: HybridSearch
    // 3. (optionally) compute prompt embedding
    // 4. graph::codemap::build_codemap(...)
    // 5. format JSON / Mermaid / outline per `format` arg
}
```

`BuildCodemapParams`:

```rust
struct BuildCodemapParams {
    directory: String,
    task_prompt: Option<String>,             // required unless seed_qualified_names is given
    seed_qualified_names: Option<Vec<String>>,
    max_nodes: Option<usize>,                // default 80, cap 500
    depth: Option<u8>,                       // default 3, cap 5
    max_incoming_per_node: Option<usize>,    // default 8
    embedding_policy: Option<String>,        // "no_rerank" (default) | "cached_only" | "compute_missing"
    format: Option<String>,                  // "json" (default) | "mermaid" | "outline" | "all"
    include_snippets: Option<bool>,          // default false
}
```

## 7. Mermaid + outline rendering

Pure projection of `Codemap`. Keep it small (~120 LOC), no graph layout intelligence — Mermaid handles layout, LLM clients handle display.

```text
flowchart LR
  subgraph m_auth ["mod auth"]
    n1["fn login"]:::seed
    n2["fn verify_token"]
  end
  n1 -->|calls| n2
  n2 -.->|uses| n3["struct User"]
  classDef seed fill:#fde68a,stroke:#92400e
```

Outline format: indented module → item tree from the filtered `ModuleTreeNode`, with seed marker and `file:line` on each leaf.

## 8. Persistence (v2 — defer)

`src/graph/storage.rs` already has `SCHEMA_VERSION = 11`. Adding a `codemaps_by_key` sub-DB requires a `SCHEMA_VERSION = 12` bump and the existing migration path. **Recommend deferring** — caching only pays off if the prompt is repeated verbatim, which is rare. Cheaper win: cache the *embedding* of any node that goes through `ComputeMissing` so it persists for future codemap and `semantic_overlaps` calls. That reuses `embeddings_by_target` without a schema bump.

## 9. Honest LOC and time estimate

| Piece                                                                         | LOC       |
|-------------------------------------------------------------------------------|-----------|
| Model types in `src/graph/model.rs`                                           | 80        |
| Span index on `OpenedSnapshot`                                                | 80        |
| Item-kind / `crate_of` / `enclosing_item` / `min_call_distance` helpers       | 100       |
| `src/graph/codemap.rs` core (BFS + scoring + prune + projection)              | 400       |
| Renderers (Mermaid + outline)                                                 | 120       |
| MCP tool wiring + param struct + error mapping                                | 130       |
| Unit tests (seed resolution, BFS termination, score determinism, projection)  | 350       |
| One end-to-end test (golden snapshot of node-set qualified names)             | 80        |
| **Total**                                                                     | **~1,340** |

Realistic time: **3 working days** for v1. Where the slip comes from vs. earlier estimates: helpers don't exist yet, embedding-policy branching, snapshot tests for a non-deterministic-feeling algorithm.

## 10. Known limitations (call out in the tool description)

1. **Trait dispatch / `dyn Trait` / generic `F: Fn(..)` are invisible.** Underlying Usage edges resolve syntactic call sites, not virtual dispatch. The tool's `diagnostics` should count unresolved sites in the seed neighborhood and report them, so consumers know when to trust the map.
2. **Macro-expanded items may lack spans.** Span lookup falls back to module-level; affected items still appear but won't be reachable from search-hit byte ranges.
3. **First call against a fresh snapshot is the slow one** when `embedding_policy = compute_missing`. The cache fills as a side effect.
4. **`max_nodes` is a hard budget.** Seeds always survive; non-seeds may be pruned aggressively. Stats expose `node_count` so the consumer can ask for more.

## 11. Decisions needed before implementation

- **(a) Embedding policy default.** Recommendation: `no_rerank` (latency-first), with the tool description noting that `cached_only` and `compute_missing` improve quality.
- **(b) Format default.** Recommendation: `json` only. Mermaid and outline are derivable client-side from the JSON if needed; defaulting to `all` triples token cost.
- **(c) v1 = no persistence, only cache embeddings as side-effects.** Defer the `codemaps_by_key` sub-DB unless there is a specific repeat-prompt workflow.
- **(d) Trait-dispatch resolution is out of scope for codemap.** It's a separate, larger workstream against `who_uses` itself.
