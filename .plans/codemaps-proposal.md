# Proposal: task-conditioned codemap tool for `file-search-mcp`

**Target:** single-crate workspace at `/home/molaco/Documents/rust-code-mcp-final` (crate `file-search-mcp`, modules under `src/`).
**Status:** v2.3. Revises an earlier draft after four rounds of review. v1→v2 fixed five algorithm-breaking issues; v2→v2.1 tightened four details; v2.2 audited every new piece against existing infrastructure to remove duplication; v2.3 cross-validates every cited API against the live workspace index and addresses async-hygiene + `#[non_exhaustive]` concerns surfaced by the Rust-idioms review. See §11 "Reuse map" for the explicit reuse contract and the appendix for revision history.

## 1. Goal

A single MCP tool, `build_codemap`, that turns a natural-language prompt into a focused subgraph of the indexed workspace: hierarchical outline + node/edge JSON + optional Mermaid. Shape is *seeds → expand → score → prune → project*, anchored to files that exist.

## 2. Where things live

Reusing the existing module layout — no new crates, no workspace migration.

| New code                              | Location                                                                                  | Purpose                                                                                          |
|---------------------------------------|-------------------------------------------------------------------------------------------|--------------------------------------------------------------------------------------------------|
| `Codemap*` response types             | `src/graph/codemap.rs` (new module, re-exported from `src/graph/mod.rs`)                  | `Codemap`, `CodemapNode`, `CodemapEdge`, `EdgeKind`, `CodemapStats`, `CodemapOptions` — all fields `pub`. `ModuleTreeNode` stays in `queries.rs`. |
| Span index + line↔byte cache          | `src/graph/snapshot.rs` (extend `OpenedSnapshot`)                                          | Lazy per-handle `HashMap<String /* file */, Vec<(start_byte, end_byte, NodeId)>>` plus per-file line→byte offset cache. |
| Raw-ID graph adapters                 | `src/graph/queries.rs` (append, `pub(crate)`)                                              | `callees_of(NodeId) -> Vec<NodeId>`, `referrers_of(NodeId) -> Vec<NodeId>`. Wrap existing private `usages_for_consumer_function` / `usages_for_target`. **No `EdgeKind` here** — classification lives in `codemap.rs` to keep the query layer feature-agnostic. |
| `ItemKind` predicates                 | `src/graph/model.rs` (append methods to the enum)                                          | `ItemKind::is_callable(self) -> bool` (Function \| Method \| AssocFunction), `ItemKind::is_type(self) -> bool` (Struct \| Enum \| Union \| Trait \| TypeAlias). Three-line predicates on the enum itself — no new `OpenedSnapshot` helpers. `crate_of`/`item_kind_of` are *not* added: `snap.node(rtxn, id)?.item_kind` and `Node.crate_id` are already accessible. |
| Span-resolution helper                | `src/graph/queries.rs` (append)                                                            | `enclosing_item_for_line_range(workspace_relative_file, line_start, line_end) -> Option<NodeId>`. Internally uses the span index + line→byte cache from `OpenedSnapshot`. Input is workspace-relative; normalization is the caller's job. |
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
#[non_exhaustive]   // future variants (Implements, Inherits, …) should not be semver-breaking
pub enum EdgeKind { Calls, Uses, Imports, Contains }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodemapStats {
    pub seed_count: usize,
    pub node_count: usize,
    pub edge_count: usize,
    pub embedded_nodes: usize,        // retained nodes that had a cached embedding
    pub embeddings_computed: usize,   // generated on-the-fly (ComputeMissing policy)
    pub total_ms: u64,
}
```

Counters for the embedding-cache cold start are exposed so consumers and later tuning passes can see them. (Earlier drafts proposed a `trait_dispatch_unresolved` counter; dropped because the underlying `Usage` table only contains *resolved* references — RA's `Definition::usages` filters unresolved sites before they reach the snapshot. The trait-dispatch blind spot is still documented in the tool description.)

## 4. Span index + line↔byte bridge

`Node.file` is `Option<String>` (workspace-relative) and `Node.span` is `Option<(u32, u32)>` (byte offsets) per `src/graph/model.rs`. But the search side (`ChunkContext` at `src/chunker/mod.rs:40`, `CodeChunk` at `src/chunker/mod.rs:62`) only carries `line_start`/`line_end`. So seeds need a line↔byte bridge.

**Why not reuse `src/semantic/position.rs`?** That module's `to_offset`/`line_col` go through RA's `LineIndex`, which needs an `AnalysisHost`. Building one at query time re-parses the workspace and runs RA — appropriate for the `find_definition`/`goto_definition` paths but heavy for a per-codemap line lookup. A plain `\n`-scan of the file is cheaper for the codemap's bounded use (≤20 files per query).

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

**Important caveat:** today the MCP server opens a fresh `OpenedSnapshot` (`src/graph/snapshot.rs:363`) per tool call (`src/tools/graph_tools.rs:2064`). So both caches are **per request**, not per process. The cost is bounded (one full scan of nodes for the span index, plus a few file reads), but a future optimization is a snapshot-handle cache keyed on `(canonical_dir, manifest_mtime)`. Not blocking for v1.

## 5. Algorithm

```text
inputs:
  snap:            &OpenedSnapshot
  prompt:          &str
  override_seeds:  Option<Vec<String>>                  // qualified names → lookup_by_qualified_name, RA fallback
  opts:            CodemapOptions {
      max_nodes: 80, depth: 3, top_k_seeds: 20,
      max_incoming_per_node: 8,
      embedding_policy: NoRerank | UseCachedOnly | ComputeMissing,
      include_snippets: false,
  }

1. SEEDS
   if override_seeds.is_some():
       // primary path: direct qualified-name lookup (uses queries::lookup_by_qualified_name)
       // fallback: if a name doesn't resolve, try RA find_definition and then map
       //           the resulting (file, byte_span) through the span index
       seeds = override_seeds.iter()
           .filter_map(|qn| snap.lookup_by_qualified_name(qn).ok().flatten()
                              .map(|(nid, _)| nid)
                              .or_else(|| ra_find_definition_fallback(snap, qn)))
           .collect();
   else:
       // existing API: src/search/mod.rs:98-138
       hits = hybrid.search(prompt, opts.top_k_seeds * 3)?              // Vec<SearchResult> w/ chunk.context
       let ws_root = PathBuf::from(&snap.manifest.workspace_root)       // src/graph/storage.rs:460
                       .canonicalize()?;
       for hit in hits:
           // normalize: chunk.context.file_path is the indexer-time path (typically absolute);
           // Node.file is workspace-relative. Canonicalize, then strip ws_root.
           let Some(rel) = canonicalize_and_strip(&hit.chunk.context.file_path, &ws_root)
               else { continue };
           let (ls, le) = (hit.chunk.context.line_start as u32, hit.chunk.context.line_end as u32);
           if let Some(nid) = snap.enclosing_item_for_line_range(&rel, ls, le)? {
               // ItemKind methods (added in src/graph/model.rs) — no OpenedSnapshot helpers needed
               let kind = snap.node(&rtxn, nid)?.and_then(|n| n.item_kind);
               if matches!(kind, Some(k) if k.is_callable() || k.is_type()) {
                   if seeds.insert(nid) && seeds.len() == opts.top_k_seeds { break; }
               }
           }

2. EXPAND  (bounded BFS, both directions, degree-capped, ID-level)
   //
   // Edge classification lives here (not in queries.rs). The classification
   // is a one-liner using ItemKind::is_callable() — no dedicated helper:
   //     let kind = if target_item_kind.map_or(false, |k| k.is_callable())
   //                { EdgeKind::Calls } else { EdgeKind::Uses };
   //
   retained = seeds.clone()
   frontier = seeds.clone()
   for _ in 0..opts.depth:
       next = HashSet::new()
       for n in &frontier:
           // outgoing — raw adapter returns NodeIds; classify by target ItemKind
           for target_id in snap.callees_of(n)?:
               let tk = snap.node(&rtxn, target_id)?.and_then(|nd| nd.item_kind);
               let kind = if tk.map_or(false, |k| k.is_callable()) { EdgeKind::Calls }
                          else                                     { EdgeKind::Uses };
               record_edge(n, target_id, kind);
               if retained.insert(target_id) { next.insert(target_id); }

           // incoming — branch on what `n` is, since the *same* underlying iterator
           // (referrers_of) means different things depending on n's kind.
           let nk = snap.node(&rtxn, n)?.and_then(|nd| nd.item_kind);
           let (record_kind, expand_incoming) = match nk {
               Some(k) if k.is_callable() => (Some(EdgeKind::Calls), true),
               Some(k) if k.is_type()     => (Some(EdgeKind::Uses),  true),
               _                          => (None,                  false),  // const/static/module: skip
           };
           if expand_incoming {
               let mut refs = snap.referrers_of(n)?;
               refs.sort_by_key(|c| -score_caller_against_prompt(c));
               for r in refs.into_iter().take(opts.max_incoming_per_node) {
                   record_edge(r, n, record_kind.unwrap());
                   if retained.insert(r) { next.insert(r); }
               }
           }
       frontier = next

3. SCORE
   //
   // Reuse:
   //   - cosine():                        src/tools/graph_tools.rs (free fn, signature: `fn cosine(a: &[f32], b: &[f32]) -> f32`)
   //   - EmbeddingGenerator::embed_async: src/embeddings/mod.rs    (signature: `async fn embed_async(&self, text: String) -> Result<Vec<f32>, EmbeddingError>` — takes OWNED String)
   //   - embed_batch_async:               src/embeddings/         (batch path used by semantic_overlaps; amortizes fastembed call)
   //   - embeddings_by_target read API:   dbs.embeddings_by_target.get(rtxn, NodeId.as_bytes())
   //   - compute-and-cache helper:        extract from semantic_overlaps body (src/tools/graph_tools.rs:717-1100)
   //                                       into `ensure_embeddings_for(snap, nids: &[NodeId]) -> Result<HashMap<NodeId, Vec<f32>>>`
   //                                       — batched, not single-NodeId; reuse from both call sites.
   //   - min_call_distance(node, &seeds): reverse BFS over referrers_of, modelled on
   //                                       recursive_callers_count (src/graph/queries.rs:852+).
   //   - bm25 score normalization:        SearchResult.score is post-RRF and not pre-normalized
   //                                       (src/search/mod.rs:47); codemap max-normalizes across this query's hit set.
   //
   // ASYNC HYGIENE (heed RoTxn is Send under WithoutTls, but holding it across an
   //                async fastembed call is poor practice and slow). The structure is:
   //
   //   PHASE A — pure sync (one read txn):
   //     for each retained node, collect bm25 score + graph_prox.
   //     if ComputeMissing / UseCachedOnly:
   //         look up cached EmbeddingRecord per node; collect (cached_nodes, missing_nodes).
   //     drop rtxn here.
   //
   //   PHASE B — async (no txn held):
   //     if ComputeMissing && !missing_nodes.is_empty():
   //         let new_vecs = ensure_embeddings_for(snap, &missing_nodes).await?;
   //         (this opens its own short rtxn+rwxn internally to read source + write cache)
   //
   //   PHASE C — pure sync (no txn needed; vectors are in memory):
   //     for each retained node, finalize emb_sim and combine into node.relevance.
   //
   for node in &retained {
       let bm25_norm  = normalized_search_score(node);            // 0..1 across this query's hits, 0 if absent
       let graph_prox = 1.0 / (1.0 + min_call_distance(node, &seeds) as f32);
       let emb_sim = match opts.embedding_policy {
           NoRerank        => None,
           UseCachedOnly   => cached_cosine(node, &prompt_emb),   // from PHASE A; None on miss
           ComputeMissing  => Some(cosine(emb_for(node), &prompt_emb)),  // from PHASE A∪B
       };
       node.relevance = match emb_sim {
           Some(s) => 0.40*s + 0.35*bm25_norm + 0.25*graph_prox,
           None    =>          0.60*bm25_norm + 0.40*graph_prox,
       };
   }

4. PRUNE
   keep all seeds.
   among non-seeds, keep top (opts.max_nodes - |seeds|) by relevance.
   drop edges whose endpoints aren't both retained.

5. PROJECT
   for each crate represented by retained nodes:
       tree = snap.module_tree(crate_qualified_name, None)
       module_subtree = filter(tree) keeping branches containing retained items.

6. ASSEMBLE Codemap, fill diagnostics (incl. trait_dispatch_unresolved counter), return.
```

### Raw-ID graph adapters (the missing piece)

These wrap the existing private iterators (`src/graph/queries.rs:2257,2305`) and return distinct NodeIds. **The query layer stays feature-agnostic**: it does not know about `EdgeKind`. Edge classification (Calls vs Uses) happens in `codemap.rs` by reading `Node.item_kind` of the endpoint.

```rust
// src/graph/queries.rs (appended)

impl OpenedSnapshot {
    /// Distinct outgoing references from `caller_fn`'s body. Includes calls,
    /// type references, const reads — anything `usages_for_consumer_function`
    /// produces. Caller classifies by reading targets' item_kind.
    pub(crate) fn callees_of(&self, caller_fn: NodeId) -> Result<Vec<NodeId>> {
        let rtxn = self.env.read_txn()?;
        let mut seen = HashSet::new();
        for entry in self.usages_for_consumer_function(&rtxn, caller_fn)? {
            seen.insert(entry?.target);
        }
        Ok(seen.into_iter().collect())
    }

    /// Distinct functions whose body contains a reference to `target`. Mirrors
    /// the `consumer_function.is_some()` filter used by `who_calls`. Semantics
    /// of each edge depend on `target`'s item_kind (callable → caller, type →
    /// consumer) — that's the caller's concern.
    pub(crate) fn referrers_of(&self, target: NodeId) -> Result<Vec<NodeId>> {
        let rtxn = self.env.read_txn()?;
        let mut seen = HashSet::new();
        for entry in self.usages_for_target(&rtxn, target)? {
            if let Some(referrer) = entry?.consumer_function {
                seen.insert(referrer);
            }
        }
        Ok(seen.into_iter().collect())
    }
}
```

`pub(crate)` keeps these internal; `codemap.rs` is a sibling under `src/graph/` so it can use them without widening the public API.

### Three concrete differences from the v1 draft

- **`max_incoming_per_node` replaces the same-crate filter.** Picks the most prompt-relevant callers regardless of crate; avoids amplifying noise from "everything calls `log::info`".
- **`embedding_policy` is explicit.** `NoRerank` (latency-first default), `UseCachedOnly` (use whatever `semantic_overlaps` warmed), `ComputeMissing` (best quality, slower). Scoring formula branches on whether embeddings exist rather than smuggling a zero into a fixed formula.
- **Seed override in v1.** Skips search; resolves names via `lookup_by_qualified_name` first, with RA `find_definition` as a fallback for unresolved names.

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
    // 1. open_workspace_snapshot(directory) -> OpenedSnapshot       (src/tools/graph_tools.rs:2064)
    // 2. if seed_qualified_names: snap.lookup_by_qualified_name per name
    //                              (src/graph/queries.rs:439, already handles re-export hops);
    //                              for unresolved names, fall back to RA goto_definition
    //                              via src/semantic/position.rs:79+ and map (file, span) back
    //                              through enclosing_item_for_line_range.
    //    else:                     hybrid.search(prompt, top_k_seeds * 3)   (src/search/mod.rs:98)
    //                              resolve hits: canonicalize file_path, strip snap.manifest.workspace_root,
    //                              call enclosing_item_for_line_range with line range.
    // 3. (if embedding_policy != NoRerank) EmbeddingGenerator::embed_async(prompt.to_owned())
    //                              (src/embeddings/mod.rs — takes OWNED String, not &str)
    // 4. graph::codemap::build_codemap(&snap, prompt, seeds, prompt_emb, opts)
    // 5. format JSON / Mermaid / outline per `format` arg
    // Errors: map via internal_error("label") (src/tools/graph_tools.rs:2210)
    //         and McpError::invalid_params for validation.
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

| Piece                                                                                              | LOC       |
|----------------------------------------------------------------------------------------------------|-----------|
| Response types in `src/graph/codemap.rs`                                                           | 70        |
| Span index + line→byte cache on `OpenedSnapshot`                                                   | 140       |
| Raw-ID adapters (`callees_of`, `referrers_of`)                                                     | 80        |
| `ItemKind::is_callable` / `is_type` predicates (`src/graph/model.rs`) + `enclosing_item_for_line_range` + `min_call_distance` (mirrors `recursive_callers_count` BFS template) | 100 |
| Extract `ensure_embeddings_for` (batched) out of `semantic_overlaps` body + call from both sites   | 120       |
| Tiny `canonicalize_and_strip(path, ws_root)` helper (PathBuf-based; *not* the VFS-based `resolve_workspace_relative`) | 20 |
| `src/graph/codemap.rs` core (BFS + scoring + prune + projection)                                   | 350       |
| Renderers (Mermaid + outline)                                                                      | 120       |
| MCP tool wiring + param struct + error mapping (uses existing `open_workspace_snapshot`, `internal_error`) | 130 |
| Unit tests (seed resolution incl. line→byte bridge, BFS termination, edge classification, prune)   | 400       |
| One end-to-end test against the local snapshot (golden node-set qualified names)                   | 80        |
| **Total new LOC**                                                                                  | **~1,610** |
| **Of which reused (zero new LOC):** `cosine`, `EmbeddingGenerator::embed_async`, `HybridSearch::search`, `lookup_by_qualified_name`, `module_tree`, `open_workspace_snapshot`, `internal_error`, `read_file_content`, private `usages_for_*` iterators, `embeddings_by_target` read/write API | — |

Realistic time: **3 working days** for v1. The reuse audit shifted LOC slightly (added `ensure_node_embedding` extraction; removed redundant `is_callable`/`is_type`/`crate_of`/`item_kind_of` helpers; trimmed `classify_outgoing`).

## 10. Known limitations (call out in the tool description)

1. **Trait dispatch / `dyn Trait` / generic `F: Fn(..)` are invisible** *and unmeasurable*. RA's `Definition::usages` filters unresolved references before they reach the `Usage` table, so the codemap cannot count them as a diagnostic — only document the blind spot in the tool description.
2. **Macro-expanded items may lack spans.** Span lookup falls back to module-level; items still appear but aren't reachable from search-hit line ranges.
3. **First call against a fresh snapshot is slow** when `embedding_policy = compute_missing`. The cache fills as a side effect via the shared `ensure_embeddings_for` helper, which uses `embed_batch_async` to amortize one fastembed call across all missing items.
4. **Span index and line→byte cache are per-request in v1** because `OpenedSnapshot` is reopened per tool call. A snapshot-handle cache is a separate change; not blocking.
5. **`max_nodes` is a hard budget.** Seeds always survive; non-seeds may be pruned aggressively. `stats.node_count` exposes whether the budget bit.
6. **Algorithm core is parameterized by `&OpenedSnapshot`, not a trait.** Means unit tests need a real heed env (the existing test pattern in `src/graph/queries.rs` does this via `tempdir`). A future `SnapshotView` trait abstracting `callees_of`/`referrers_of`/`node`/`module_tree` would enable in-memory fakes per architecture guide §10, but is out of scope for v1.

## 11. Infrastructure reuse map

Every external dependency the codemap module takes on, with the existing call site and whether v1 must touch it.

| Capability                          | Reuses                                                                                    | Touch?     |
|-------------------------------------|-------------------------------------------------------------------------------------------|------------|
| Open snapshot from a directory      | `open_workspace_snapshot` (`src/tools/graph_tools.rs:2064`)                               | No         |
| Workspace root from snapshot        | `snap.manifest.workspace_root: String` (`src/graph/storage.rs:460`) → parse to `PathBuf`  | No         |
| Hybrid retrieval (BM25 + LanceDB + RRF) | `HybridSearch::search(query, limit)` / `search_with_k` (`src/search/mod.rs:98-138`)   | No         |
| Search-hit shape                    | `SearchResult { chunk: CodeChunk, score, bm25_score, vector_score, ranks }`               | No (read it directly) |
| Qualified-name → NodeId             | `OpenedSnapshot::lookup_by_qualified_name` (`src/graph/queries.rs:439`) — handles re-export hops | No   |
| RA goto-definition fallback         | `src/semantic/position.rs:79+` `goto_definition` — only on `lookup_by_qualified_name` miss | No (call as-is) |
| Module hierarchy                    | `OpenedSnapshot::module_tree(crate_name: &str, depth: Option<usize>)` (`src/graph/queries.rs:1994`) | No |
| Outgoing references from a fn body  | New `pub(crate) callees_of` wrapping private `usages_for_consumer_function` (`src/graph/queries.rs:2305`) | Yes (~40 LOC) |
| Incoming references to an item      | New `pub(crate) referrers_of` wrapping private `usages_for_target` (`src/graph/queries.rs:2257`) | Yes (~40 LOC) |
| BFS distance to a seed set          | Mirror the frontier+visited loop from `recursive_callers_count` (`src/graph/queries.rs:852+`) | Yes (~40 LOC) |
| Item-kind predicates                | New `ItemKind::is_callable()` / `is_type()` methods on the enum (`src/graph/model.rs`) — 2 three-line impls | Yes (~10 LOC) |
| Item-kind getter                    | Read `snap.node(rtxn, id)?.item_kind` directly — no wrapper                               | No         |
| Cosine similarity                   | `cosine(a: &[f32], b: &[f32]) -> f32` (`src/tools/graph_tools.rs:2361`)                   | No         |
| Per-item embedding cache            | `dbs.embeddings_by_target` heed sub-DB (`src/graph/storage.rs:115`); read with `get(rtxn, NodeId.as_bytes())` | No |
| Ad-hoc text embedding (prompt)      | `EmbeddingGenerator::embed_async(text: String)` (`src/embeddings/mod.rs`) — takes owned `String`; caller `prompt.to_owned()` | No |
| Compute-and-cache for many NodeIds  | Extract `ensure_embeddings_for(snap, nids: &[NodeId]) -> HashMap<NodeId, Vec<f32>>` out of `semantic_overlaps` body (`src/tools/graph_tools.rs:717-1100`) using its existing `embed_batch_async` path. Batched, not single-NodeId. Reused from both `semantic_overlaps` and codemap. | Yes (~120 LOC factoring) |
| Source-text reading (for snippets)  | `read_file_content` (`src/tools/query_tools.rs:20`) + byte-slice the span                 | No         |
| Path normalization at query time    | Tiny new helper `canonicalize_and_strip(&Path, &Path) -> Option<String>` — `resolve_workspace_relative` (`src/graph/usages.rs:167`) is build-time only because it requires `&Vfs` | Yes (~20 LOC) |
| Line→byte conversion at query time  | Tiny new `\n`-scan per file; `src/semantic/position.rs:34-52` LineIndex path would need a heavy `AnalysisHost` at query time | Yes (~60 LOC) |
| MCP tool wiring                     | `#[tool_router]` / `#[tool]` macros in `src/tools/search_tool_router.rs`                  | Yes (one method) |
| MCP error mapping                   | `internal_error(label)` closure (`src/tools/graph_tools.rs:2210`), `McpError::invalid_params(msg, None)` for validation | No |

The two genuinely new pieces of plumbing (span index + line→byte cache, raw-ID graph adapters) are the only places where the codemap adds something that doesn't already exist in some form. Every other capability calls existing code unchanged.

**Score-normalization note.** `SearchResult.score` is post-RRF but not pre-normalized into [0,1] (`src/search/mod.rs:50-51`). The codemap normalizes per-query by dividing by the max score in the hit set. This is a one-liner local to `codemap.rs`, not new infrastructure.

## 12. V1 implementation decisions

- **(a) Embedding policy default:** `no_rerank`. The tool description will call out `cached_only` and `compute_missing` as quality/latency tradeoffs.
- **(b) Format default:** `json`. Mermaid and outline remain opt-in because they are derivable from JSON and increase token cost.
- **(c) Persistence:** no `codemaps_by_key` sub-DB in v1. Only node embeddings are cached as side effects in `embeddings_by_target`.
- **(d) Trait-dispatch resolution:** out of scope for codemap. That belongs in a separate `who_uses`/usage-extraction workstream.
- **(e) Snapshot-handle reuse:** out of scope for v1. Revisit only if span-index construction shows up in profiles.

## Appendix: changes from prior drafts

### v1 → v2 (algorithm-breaking fixes)

- **Seed extraction**: v1 assumed search hits carried byte ranges. They don't (`src/chunker/mod.rs:38`). Now bridges `line_start`/`line_end` → bytes via a per-file line offset cache before querying the span index.
- **BFS over IDs**: v1 said `snap.calls_from(n) -> NodeId`. The real `calls_from`/`who_calls` return `EnrichedCallSite` with qualified names (`src/graph/queries.rs:633,675`). Added raw-ID `pub(crate)` adapters over the existing private `usages_for_*` iterators.
- **Edge classification**: v1 labelled every outgoing edge `Calls`. `calls_from` actually returns *all* non-import refs from a fn body (`src/graph/queries.rs:671`). Now classified by target `ItemKind`: callable → `Calls`, else `Uses`.
- **Span-index lifetime**: v1 said "built once per process per snapshot". Tools open a fresh `OpenedSnapshot` per request (`src/tools/graph_tools.rs:2064`). Restated as per-handle; snapshot-reuse listed as deferred optimization.
- **Type placement**: v1 placed `Codemap` in `model.rs`, but it references `ModuleTreeNode` from `queries.rs`. Moved to a new `src/graph/codemap.rs`; all fields explicitly `pub`.

### v2 → v2.1 (detail tightening)

- **Incoming-edge classification**: v2 unconditionally labelled incoming references as `EdgeKind::Calls`, which mislabels type consumers. Now the incoming-expansion branch reads `n`'s kind and picks `Calls` (if `n` is callable) or `Uses` (if `n` is a type). The dedicated `users_of_type` adapter is dropped; both branches share `referrers_of`.
- **File-path normalization**: `CodeChunk.file_path` holds the disk path passed to `Chunker::chunk_file` (`src/chunker/mod.rs:169-200`) — typically absolute. `Node.file` is workspace-relative. Seed step now canonicalizes and strips the snapshot workspace root before calling `enclosing_item_for_line_range`.
- **Query/feature layer separation**: v2 had `callees_of` returning `(NodeId, EdgeKind)`, coupling the generic query layer back to the codemap feature. Now `callees_of` and `referrers_of` return plain `Vec<NodeId>`; `codemap.rs` classifies by reading endpoint `Node.item_kind` via the existing `OpenedSnapshot::node` lookup.
- **Seed-override resolution**: v2 said "resolve via `find_definition`". The right primitive for qualified-name → NodeId is `lookup_by_qualified_name` (`src/graph/queries.rs:439`), which already handles re-export hops. Use it first; fall back to RA `find_definition` + span-index mapping only for names that don't resolve directly.

### v2.1 → v2.2 (infrastructure reuse audit)

- **Wrong search API name.** v2.1 said `HybridSearch::query`; actual API is `HybridSearch::search(query, limit)` / `search_with_k` (`src/search/mod.rs:98-138`). Fixed in §5 and §6.
- **Cosine reuse.** Existing `cosine(a, b)` lives at `src/tools/graph_tools.rs:2361`. v2.1 implied a fresh implementation; v2.2 names it and calls it directly.
- **Prompt-embedding reuse.** `EmbeddingGenerator::embed_async(text)` (`src/embeddings/mod.rs:126`) is the existing entry point. v2.2 names it explicitly in §5 SCORE and the MCP tool body.
- **Compute-and-cache extraction.** The lazy-populate logic in `semantic_overlaps` (`src/tools/graph_tools.rs:717-1100`) duplicates exactly what `ComputeMissing` needs. v2.2 factors it into a reusable `ensure_node_embedding(snap, rtxn, rwxn, nid)` helper called from both sites.
- **BFS reuse.** `min_call_distance(node, &seeds)` now explicitly mirrors the frontier+visited template of `recursive_callers_count` (`src/graph/queries.rs:847-943`) instead of being described as fresh code.
- **`ItemKind` predicates moved.** `is_callable`/`is_type` are now methods on the `ItemKind` enum in `src/graph/model.rs`, not new helpers on `OpenedSnapshot`. `crate_of` and `item_kind_of` helpers are dropped entirely — callers use `snap.node(rtxn, id)?.item_kind` and `Node.crate_id` directly.
- **`trait_dispatch_unresolved` counter dropped.** RA's `Definition::usages` filters unresolved references before they reach the `Usage` table; we cannot count what isn't in the snapshot. The blind spot remains documented in the tool description but is no longer claimed to be quantifiable.
- **Path normalization clarified.** `resolve_workspace_relative` (`src/graph/usages.rs:167`) takes `(&Vfs, FileId, &Path)` — VFS is build-time only. The codemap layer needs a tiny `canonicalize_and_strip(path, ws_root)` over `PathBuf` because no VFS exists at query time. Flagged so it doesn't look like a 6th copy of an existing function.
- **Line→byte rationale.** `src/semantic/position.rs` already converts line/col via RA's `LineIndex`, but that needs an `AnalysisHost`. v2.2 documents *why* the codemap keeps its own `\n`-scan: query-time AnalysisHost construction is overkill for ≤20 files per call.
- **Workspace root accessor.** v2 / v2.1 said `snap.workspace_root()`. There is no such method. v2.2 uses `snap.manifest.workspace_root: String` (parsed to `PathBuf`).
- **Reuse map added (§11).** Every external dependency the codemap takes on is now listed with file:line and whether v1 touches it.

### v2.2 → v2.3 (cross-validation against live index + idioms review)

Every cited API and type was checked via `function_signature` / `find_definition` against the indexed workspace (1,526 items). All citations resolved. Adjustments:

- **`EmbeddingGenerator::embed_async` takes owned `String`, not `&str`.** Verified signature: `async fn embed_async(&self, text: String) -> Result<Vec<f32>, EmbeddingError>`. The MCP-tool layer must call `.to_owned()` on the prompt. Reflected in §6 and the reuse map.
- **`ensure_node_embedding` → `ensure_embeddings_for(snap, &[NodeId])` (batched).** The existing `semantic_overlaps` body uses `embed_batch_async` to amortize the fastembed call across many items. Per-NodeId async calls in a loop is the slow path. The extracted helper now takes a slice and returns a `HashMap<NodeId, Vec<f32>>`. Reuse-map row and LOC table updated.
- **Async hygiene in SCORE.** Heed `RoTxn` is `Send` under `WithoutTls`, but holding it across `embed_batch_async` is poor practice. §5 SCORE now splits into three phases: pure-sync data collection (txn held), async batched embedding (no txn), pure-sync finalization. Mirrors the existing `semantic_overlaps` structure.
- **`#[non_exhaustive]` on `EdgeKind`.** Per rusty checklist §4 — `EdgeKind` is serialized into the MCP JSON; future variants (`Implements`, `Inherits`, …) should not be semver-breaking. Added.
- **Line-number drift fixed.** `OpenedSnapshot` is at `src/graph/snapshot.rs:363` (was 362); `ChunkContext` at `src/chunker/mod.rs:40` (was "38-58"); `recursive_callers_count` fn declaration at `src/graph/queries.rs:852` (range still ~852–943).
- **`SnapshotView` trait flagged for the future.** §10 acknowledges that the v1 algorithm core takes `&OpenedSnapshot` directly, which keeps unit tests bound to a real heed env (the existing test pattern). A trait abstraction is the right sans-I/O refactor but is out of v1 scope.
- **`read_file_content` is not duplicated.** The second occurrence (`src/tools/search_tool_router.rs:116`) is the `#[tool]` wrapper that delegates to `src/tools/query_tools.rs:20`. The proposal already cites the implementation site, not the wrapper.
