# Proposal: task-conditioned codemap tool for `file-search-mcp`

**Target:** single-crate workspace at `/home/molaco/Documents/rust-code-mcp-final` (crate `file-search-mcp`, modules under `src/`).
**Status:** v2. Revises an earlier draft after a code review found five issues that broke the algorithm as written. Each issue is addressed in §3–§5.

## 1. Goal

A single MCP tool, `build_codemap`, that turns a natural-language prompt into a focused subgraph of the indexed workspace: hierarchical outline + node/edge JSON + optional Mermaid. Shape is *seeds → expand → score → prune → project*, anchored to files that exist.

## 2. Where things live

Reusing the existing module layout — no new crates, no workspace migration.

| New code                              | Location                                                                                  | Purpose                                                                                          |
|---------------------------------------|-------------------------------------------------------------------------------------------|--------------------------------------------------------------------------------------------------|
| `Codemap*` response types             | `src/graph/codemap.rs` (new module, re-exported from `src/graph/mod.rs`)                  | `Codemap`, `CodemapNode`, `CodemapEdge`, `EdgeKind`, `CodemapStats`, `CodemapOptions` — all fields `pub`. `ModuleTreeNode` stays in `queries.rs`. |
| Span index + line↔byte cache          | `src/graph/snapshot.rs` (extend `OpenedSnapshot`)                                          | Lazy per-handle `HashMap<String /* file */, IntervalTree<u32 /* byte */, NodeId>>` plus per-file line→byte offset cache. |
| Raw-ID graph adapters                 | `src/graph/queries.rs` (append, `pub(crate)`)                                              | `callees_of(NodeId) -> Vec<(NodeId, EdgeKind)>`, `callers_of(NodeId) -> Vec<NodeId>`, `users_of_type(NodeId) -> Vec<NodeId>`. Wrap existing private `usages_for_consumer_function` / `usages_for_target`. |
| Item-kind helpers                     | `src/graph/queries.rs` (append)                                                            | `is_callable(NodeId)`, `is_type(NodeId)`, `crate_of(NodeId)`, `enclosing_item_for_line_range(file, line_start, line_end)`. |
| Algorithm                             | `src/graph/codemap.rs`                                                                     | `build_codemap`, scoring, projection, Mermaid/outline renderers.                                  |
| MCP tool                              | `src/tools/search_tool_router.rs` (append a `#[tool]` method) + `src/tools/graph_tools.rs` (extract helper) | Wires prompt → search → optional embed → `build_codemap` → response.                          |
| Optional persistence (v2+)            | `src/graph/storage.rs` — new `codemaps` sub-DB + `SCHEMA_VERSION` bump 11 → 12             | `key = blake3(prompt ‖ snapshot_id)[..16]`, value = `bincode(Codemap)`. **Deferred.**             |

## 3. Data types

In `src/graph/codemap.rs` (re-exported from `src/graph/mod.rs`). Every field public; `Codemap` embeds the existing `ModuleTreeNode` from `queries.rs`.

```rust
// src/graph/codemap.rs

use crate::graph::ids::NodeId;
use crate::graph::model::{ItemKind, NodeKind};
use crate::graph::queries::ModuleTreeNode;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Codemap {
    pub prompt: String,
    pub snapshot_id: String,
    pub generated_at_unix: u64,
    pub seeds: Vec<NodeId>,
    pub nodes: Vec<CodemapNode>,
    pub edges: Vec<CodemapEdge>,
    pub hierarchy: ModuleTreeNode,
    pub stats: CodemapStats,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodemapNode {
    pub id: NodeId,
    pub qualified_name: String,
    pub kind: NodeKind,
    pub item_kind: Option<ItemKind>,
    pub file: Option<String>,
    pub span: Option<(u32, u32)>,
    pub relevance: f32,
    pub is_seed: bool,
    pub snippet: Option<String>,
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
    pub embedded_nodes: usize,
    pub embeddings_computed: usize,
    pub trait_dispatch_unresolved: usize,
    pub total_ms: u64,
}
```

Counters for the *failure modes* — trait dispatch, cold embedding cache — are exposed so consumers and later tuning passes can see them.

## 4. Span index + line↔byte bridge

`Node.file` is `Option<String>` (workspace-relative) and `Node.span` is `Option<(u32, u32)>` (byte offsets) per `src/graph/model.rs:90-91`. But the search side (`CodeChunk` + `ChunkContext` at `src/chunker/mod.rs:38-58`) only carries `line_start`/`line_end`. So seeds need a line↔byte bridge.

Two indexes attached lazily to `OpenedSnapshot`:

```rust
// src/graph/snapshot.rs (extended)
use std::sync::OnceLock;

pub struct OpenedSnapshot {
    pub manifest: GraphManifest,
    pub snapshot_dir: PathBuf,
    pub env: Env<WithoutTls>,
    pub dbs: GraphDatabases,
    // new:
    span_index:        OnceLock<HashMap<String, Vec<(u32, u32, NodeId)>>>, // sorted by start
    line_to_byte:      Mutex<HashMap<String, Arc<Vec<u32>>>>,              // file → line_start byte offsets
}
```

Implementation notes:

- `span_index` is built once per `OpenedSnapshot` handle by scanning `dbs.nodes_by_id` once and grouping by file. At ~1,500 items the flat sorted `Vec` + binary search is faster than a tree.
- `line_to_byte` is built on demand: when an `enclosing_item_for_line_range(file, ls, le)` lookup arrives, read the file from disk *once* and remember its `\n`-offset prefix table. Bounded by number of files containing search hits (typically <20 per query).
- `enclosing_item_for_line_range` resolves `(file, line_start, line_end)` → `(byte_start, byte_end)` then queries `span_index` for the smallest enclosing item.

**Important caveat:** today the MCP server opens a fresh `OpenedSnapshot` per tool call (`src/tools/graph_tools.rs:2064`). So both caches are **per request**, not per process. The cost is bounded (one full scan of nodes for the span index, plus a few file reads), but a future optimization is a snapshot-handle cache keyed on `(canonical_dir, manifest_mtime)`. Not blocking for v1.

## 5. Algorithm

```text
inputs:
  snap:            &OpenedSnapshot
  prompt:          &str
  override_seeds:  Option<Vec<String>>                  // qualified names → find_definition
  opts:            CodemapOptions {
      max_nodes: 80, depth: 3, top_k_seeds: 20,
      max_incoming_per_node: 8,
      embedding_policy: NoRerank | UseCachedOnly | ComputeMissing,
      include_snippets: false,
  }

1. SEEDS
   if override_seeds.is_some():
       seeds = resolve_each_via_find_definition(override_seeds)
   else:
       hits = HybridSearch::query(prompt, k = opts.top_k_seeds * 3)  // BM25 + LanceDB + RRF
       for hit in hits:                                              // hit gives file + line_start..line_end
           if let Some(nid) = snap.enclosing_item_for_line_range(
                                  &hit.file_path, hit.line_start, hit.line_end):
               if snap.is_callable(nid) || snap.is_type(nid):
                   if seeds.insert(nid) && seeds.len() == opts.top_k_seeds:
                       break;

2. EXPAND  (bounded BFS, both directions, degree-capped, ID-level)
   retained = seeds.clone()
   frontier = seeds.clone()
   for _ in 0..opts.depth:
       next = HashSet::new()
       for n in &frontier:
           // outgoing — raw ID adapter classifies by target ItemKind
           for (target_id, kind) in snap.callees_of(n)?:
               record_edge(n, target_id, kind);                  // Calls if target is callable, else Uses
               if retained.insert(target_id) { next.insert(target_id); }

           // incoming — degree-capped by per-prompt score
           let mut callers = snap.callers_of(n)?;                // Vec<NodeId>
           callers.sort_by_key(|c| -score_caller_against_prompt(c));
           for caller in callers.into_iter().take(opts.max_incoming_per_node):
               record_edge(caller, n, EdgeKind::Calls);
               if retained.insert(caller) { next.insert(caller); }

           // type/data — only for type seeds, pulls consumer functions
           if snap.is_type(n):
               for consumer_fn in snap.users_of_type(n)?:        // dedup'd Vec<NodeId>
                   record_edge(consumer_fn, n, EdgeKind::Uses);
                   if retained.insert(consumer_fn) { next.insert(consumer_fn); }
       frontier = next

3. SCORE
   for node in &retained:
       let bm25_norm = normalized_search_score(node);            // 0..1 from hybrid hits, 0 if absent
       let graph_prox = 1.0 / (1.0 + min_call_distance(node, &seeds));
       let emb_sim = match opts.embedding_policy:
           NoRerank        => None,
           UseCachedOnly   => cached_cosine(node, prompt_emb),
           ComputeMissing  => Some(compute_and_cache_cosine(node, prompt_emb)),

       node.relevance = match emb_sim {
           Some(s) => 0.40*s + 0.35*bm25_norm + 0.25*graph_prox,
           None    =>          0.60*bm25_norm + 0.40*graph_prox,
       };

4. PRUNE
   keep all seeds.
   among non-seeds, keep top (opts.max_nodes - |seeds|) by relevance.
   drop edges whose endpoints aren't both retained.

5. PROJECT
   module_subtree = filter(snap.module_tree(workspace_root)) keeping branches containing retained items.

6. ASSEMBLE Codemap, fill diagnostics (incl. trait_dispatch_unresolved counter), return.
```

### Raw-ID graph adapters (the missing piece)

These wrap the existing private iterators (`src/graph/queries.rs:2257,2305`) and return NodeIds directly, classifying edges by target `ItemKind`:

```rust
// src/graph/queries.rs (appended)

impl OpenedSnapshot {
    /// Outgoing references from `caller_fn`, classified by target ItemKind.
    /// Wraps `usages_for_consumer_function`. Dedupes by target.
    pub(crate) fn callees_of(&self, caller_fn: NodeId)
        -> Result<Vec<(NodeId, EdgeKind)>>
    {
        let rtxn = self.env.read_txn()?;
        let mut out: HashMap<NodeId, EdgeKind> = HashMap::new();
        for entry in self.usages_for_consumer_function(&rtxn, caller_fn)? {
            let u = entry?;
            let kind = match self.node(&rtxn, u.target)?.and_then(|n| n.item_kind) {
                Some(ItemKind::Function | ItemKind::Method | ItemKind::AssocFunction)
                    => EdgeKind::Calls,
                _   => EdgeKind::Uses,
            };
            out.entry(u.target).or_insert(kind);
        }
        Ok(out.into_iter().collect())
    }

    /// Incoming-call sites for `target_fn`. Filters to usages whose
    /// `consumer_function.is_some()` (mirrors `who_calls`). Dedupes by caller.
    pub(crate) fn callers_of(&self, target_fn: NodeId) -> Result<Vec<NodeId>> {
        let rtxn = self.env.read_txn()?;
        let mut seen = HashSet::new();
        for entry in self.usages_for_target(&rtxn, target_fn)? {
            if let Some(caller) = entry?.consumer_function {
                seen.insert(caller);
            }
        }
        Ok(seen.into_iter().collect())
    }

    /// Distinct functions that reference `type_id` from inside their body.
    pub(crate) fn users_of_type(&self, type_id: NodeId) -> Result<Vec<NodeId>> {
        // identical shape to callers_of but kept separate for intent / future tuning.
        self.callers_of(type_id)
    }
}
```

`pub(crate)` keeps these internal; `codemap.rs` is a sibling under `src/graph/` so it can use them.

### Three concrete differences from the v1 draft

- **`max_incoming_per_node` replaces the same-crate filter.** Picks the most prompt-relevant callers regardless of crate; avoids amplifying noise from "everything calls `log::info`".
- **`embedding_policy` is explicit.** `NoRerank` (latency-first default), `UseCachedOnly` (use whatever `semantic_overlaps` warmed), `ComputeMissing` (best quality, slower). Scoring formula branches on whether embeddings exist rather than smuggling a zero into a fixed formula.
- **Seed override in v1.** Skips search; resolves names via existing `find_definition` path.

## 6. MCP tool surface

Add one method to the `#[tool_router]` impl in `src/tools/search_tool_router.rs`, alongside the existing 47 tools. The directory-resolution step reuses `open_workspace_snapshot` (`src/tools/graph_tools.rs:2064`).

```rust
#[tool(description = "
Build a task-conditioned subgraph (codemap) of the indexed workspace.

Returns nodes/edges/hierarchy focused on the prompt. Edges come from the
HIR-driven hypergraph: direct calls and non-import uses. NOTE: method
calls dispatched through trait objects or generic Fn bounds are NOT in
the underlying usage edges, so codemaps may miss callers that go through
trait dispatch — this is surfaced in stats.trait_dispatch_unresolved.

Tunable defaults: max_nodes=80, depth=3, embedding_policy='no_rerank'.
")]
async fn build_codemap(&self, params: Parameters<BuildCodemapParams>) -> ... {
    // 1. open_workspace_snapshot(directory) -> OpenedSnapshot
    // 2. if seed_qualified_names: resolve via find_definition path
    //    else: run HybridSearch::query(prompt, top_k_seeds * 3)
    // 3. (optional) compute prompt embedding via existing fastembed pipeline
    // 4. graph::codemap::build_codemap(&snap, prompt, seeds, prompt_emb, opts)
    // 5. format JSON / Mermaid / outline per `format` arg
}
```

`BuildCodemapParams`:

```rust
struct BuildCodemapParams {
    directory: String,
    task_prompt: Option<String>,             // required unless seed_qualified_names given
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

Pure projection of `Codemap`. ~120 LOC, no layout intelligence — Mermaid handles layout, LLM clients handle display.

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

## 8. Persistence (deferred)

`src/graph/storage.rs:123` has `SCHEMA_VERSION = 11`. Adding a `codemaps_by_key` sub-DB requires a bump to 12 plus the existing migration path. **Defer** — caching only pays off on verbatim prompt reuse, which is rare. Cheap win in v1: nodes that go through `ComputeMissing` write into the existing `embeddings_by_target` sub-DB and so persist for future `semantic_overlaps` and codemap calls. No schema change.

## 9. LOC and time estimate

| Piece                                                                                             | LOC       |
|---------------------------------------------------------------------------------------------------|-----------|
| Response types in `src/graph/codemap.rs`                                                          | 80        |
| Span index + line→byte cache on `OpenedSnapshot`                                                  | 140       |
| Raw-ID adapters (`callees_of`, `callers_of`, `users_of_type`)                                     | 80        |
| Item-kind / crate / `enclosing_item_for_line_range` / `min_call_distance` helpers                 | 100       |
| `src/graph/codemap.rs` core (BFS + scoring + prune + projection)                                  | 400       |
| Renderers (Mermaid + outline)                                                                     | 120       |
| MCP tool wiring + param struct + error mapping                                                    | 150       |
| Unit tests (seed resolution incl. line→byte bridge, BFS termination, edge classification, prune)  | 400       |
| One end-to-end test against the local snapshot (golden node-set qualified names)                  | 80        |
| **Total**                                                                                         | **~1,550** |

Realistic time: **3 working days** for v1. Drift from the v1 draft comes from line↔byte bridging, raw-ID adapters, and edge classification — all forced by the actual data shapes.

## 10. Known limitations (call out in the tool description)

1. **Trait dispatch / `dyn Trait` / generic `F: Fn(..)` are invisible.** Usage edges resolve syntactic call sites, not virtual dispatch. `stats.trait_dispatch_unresolved` counts unresolved sites in the seed neighborhood so consumers can judge trust.
2. **Macro-expanded items may lack spans.** Span lookup falls back to module-level; items still appear but aren't reachable from search-hit line ranges.
3. **First call against a fresh snapshot is slow** when `embedding_policy = compute_missing`. The cache fills as a side effect.
4. **Span index and line→byte cache are per-request in v1** because `OpenedSnapshot` is reopened per tool call. A snapshot-handle cache is a separate change; not blocking.
5. **`max_nodes` is a hard budget.** Seeds always survive; non-seeds may be pruned aggressively. `stats.node_count` exposes whether the budget bit.

## 11. Decisions needed before implementation

- **(a) Embedding policy default.** Recommendation: `no_rerank` (latency-first), with the tool description noting `cached_only` and `compute_missing` improve quality.
- **(b) Format default.** Recommendation: `json` only. Mermaid and outline are derivable client-side; defaulting to `all` triples token cost.
- **(c) v1 = no persistence, embeddings cached as side-effects.** Defer `codemaps_by_key` sub-DB unless a repeat-prompt workflow shows up.
- **(d) Trait-dispatch resolution is out of scope for codemap.** Separate, larger workstream against `who_uses` itself.
- **(e) Snapshot-handle reuse.** Out of scope for v1; revisit if span-index build time shows up in profiles.

## Appendix: changes from v1 draft

- **Seed extraction**: v1 assumed search hits carried byte ranges. They don't (`src/chunker/mod.rs:38`). Now bridges `line_start`/`line_end` → bytes via a per-file line offset cache before querying the span index.
- **BFS over IDs**: v1 said `snap.calls_from(n) -> NodeId`. The real `calls_from`/`who_calls` return `EnrichedCallSite` with qualified names (`src/graph/queries.rs:633,675`). Added raw-ID `pub(crate)` adapters over the existing private `usages_for_*` iterators.
- **Edge classification**: v1 labelled every outgoing edge `Calls`. `calls_from` actually returns *all* non-import refs from a fn body (`src/graph/queries.rs:671`). Now classified by target `ItemKind`: callable → `Calls`, else `Uses`.
- **Span-index lifetime**: v1 said "built once per process per snapshot". Tools open a fresh `OpenedSnapshot` per request (`src/tools/graph_tools.rs:2064`). Restated as per-handle; snapshot-reuse listed as deferred optimization.
- **Type placement**: v1 placed `Codemap` in `model.rs`, but it references `ModuleTreeNode` from `queries.rs`. Moved to a new `src/graph/codemap.rs`; all fields explicitly `pub`.
