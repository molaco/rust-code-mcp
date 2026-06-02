# Proposal: `discover` — the targeting verb

A new top-level MCP verb for `rust-code-mcp-67` whose sole job is to return
**ranked candidate `Location`s with brief context** in response to a query (or
to a "land here" request with no query). Callers feed candidates back into
`observe` to inspect a chosen target.

This proposal merges the original sketch with code-grounded refinements from a
review pass. v1 is intentionally narrow and strictly read-only.

---

## 1. Summary

`discover` adds a fifth verb to the canonical agent surface:

```
observe   inspect one focused target with rich context
discover  find candidate targets given a query or scope         <- NEW
simulate  dry-run a write operation
act       apply a write operation
check     gate decision on commit-readiness
```

The split between `observe` and `discover` is structural, not aesthetic: their
**return shapes differ fundamentally**. `observe` returns one focused
`ContextView` (KBs per response). `discover` returns N candidate
`DiscoverCandidate`s (hundreds of bytes each). Forcing both through one verb
makes the schema a tagged union of incompatible payloads.

## 2. Motivation

`observe` requires an exact identity: `qualified_name`, `file`, or
`file + byte_start + byte_end`. An agent with no prior knowledge of the
workspace cannot use it. Today the only way to discover targets is to call
legacy aliases (`search`, `find_definition`, `crate_types`, `workspace_stats`,
`semantic_overlaps`, `similar_to_item`, `functions_with_filter`,
`items_with_attribute`, the audit family, `module_tree`, etc.). Roughly 17-20
of the legacy 45-tool surface is doing this work, and observe never absorbed
any of it.

`discover` collapses that work into one verb with a typed mode parameter while
preserving the cognitive split between finding targets and inspecting one.

## 3. Hard constraints

1. **Strictly read-only.** No mode may build, repair, or invalidate persistent
   indexes. Cold paths return diagnostics suggesting `index_codebase` /
   `build_hypergraph`. The current legacy `search` calls `clean_stale_index`
   and `ensure_indexed` (`query.rs:424-425`); `discover` must not.
2. **Returns targets, not views.** Candidates carry just enough context to
   pick. Inspection is `observe`'s job.
3. **No engine.** Every mode is a typed dispatch over an existing query
   primitive. Hybrid fusion (RRF) counts as algorithmic composition, not
   interpretation.
4. **Schema honesty.** Advertise no mode whose backing infrastructure does not
   exist yet (e.g. descriptions, DBSCAN). Errors on unimplemented variants
   would mislead agent training.
5. **Graceful when state is incomplete.** Agent may call `discover` before
   `build_hypergraph` or `index_codebase` has run. Return structured
   `RequiresHypergraph` / `RequiresChunkIndex` diagnostics, not crashes.

## 4. Tool signature

```rust
pub(crate) struct DiscoverParams {
    pub directory: String,                       // workspace root, always required
    pub request: DiscoverRequest,
    #[serde(flatten, default)]
    pub pagination: ListPaginationParams,        // reuses tools/graph/response.rs
}

#[serde(tag = "mode", content = "params", rename_all = "snake_case")]
pub(crate) enum DiscoverRequest {
    WorkspaceOverview(WorkspaceOverviewParams),
    Lookup(LookupParams),
    ByContent(ByContentParams),
    SimilarTo(SimilarToParams),
    Cluster(ClusterParams),
    BySignature(BySignatureParams),
    ByAttribute(ByAttributeParams),
    Structural(StructuralParams),
    StructuralSummary(StructuralSummaryParams),  // aggregates, not candidates
    ListContents(ListContentsParams),
}
```

Ten variants in v1. `ByDescription` is **not in the schema** until the
description layer exists; adding it later is a non-breaking enum extension.

Pagination defaults: `limit=50`, `offset=0`, `summary=false`. Not optional —
absence falls back to defaults, never unbounded.

## 5. Return shape

```rust
pub struct DiscoverResponse {
    pub request_kind: &'static str,              // "workspace_overview", ...
    pub directory: String,
    pub scope: Option<DiscoverScope>,            // when the mode took a scope
    pub list_meta: ListMeta,                     // total / returned / offset / limit / summary
    pub candidates: Vec<DiscoverCandidate>,
    pub summary: Option<DiscoverSummary>,        // workspace_overview, cluster, structural_summary
    pub diagnostics: Vec<DiscoverDiagnostic>,    // RequiresHypergraph, MissingDescriptions, etc.
    pub fingerprint: Option<String>,             // snapshot fingerprint when applicable
    pub token_cost: usize,
}

pub struct DiscoverCandidate {
    pub target: NavigationTargetParams,          // ALWAYS valid for observe.goto
    pub qualified_name: Option<String>,          // None for chunk/file/span hits without graph node
    pub display_name: Option<String>,
    pub kind: Option<NodeKind>,
    pub item_kind: Option<ItemKind>,
    pub file: Option<String>,
    pub span: Option<Span>,
    pub visibility: Option<Visibility>,
    pub signature: Option<String>,               // for fns
    pub description: Option<String>,             // populated when description layer exists
    pub r#match: DiscoverMatch,
    pub preview: Option<String>,
}

pub struct DiscoverMatch {
    pub source: MatchSource,                     // see below
    pub score: Option<f32>,                      // None for exact/structural; ranked f32 for fuzzy/semantic/hybrid
    pub raw_score: Option<f32>,                  // pre-normalization (when meaningful)
    pub rank: usize,
    pub fused_from: Vec<MatchSource>,            // only for hybrid content search
}

#[serde(rename_all = "snake_case")]
pub enum MatchSource {
    Exact,
    FuzzyName,
    Bm25,
    VectorChunk,
    VectorItem,
    Attribute,
    Signature,
    Structural,
    DescriptionText,                             // present in enum, unused until D5
    DescriptionVector,                           // present in enum, unused until D5
    Hybrid,
}

pub struct DiscoverSummary {
    pub workspace_overview: Option<WorkspaceOverview>,
    pub clusters: Option<Vec<ClusterOverview>>,
    pub structural_aggregate: Option<StructuralAggregate>,
}

pub enum DiscoverDiagnostic {
    RequiresHypergraph { message: String },
    RequiresChunkIndex { message: String },
    MissingDescriptions { items_without_description: usize },
    StaleSnapshot { ... },
    PartialCoverage { mode: String, reason: String },
}
```

Critical refinements from the review:

- `target: NavigationTargetParams` is **always** valid — chunk and RA hits that
  have no graph node still give `(file, byte_start, byte_end)`, which observe
  resolves via its existing file-span path.
- `qualified_name: Option<String>` — None when the hit came from a chunk index
  miss or RA semantic search outside the graph.
- `score: Option<f32>` — None for exact and structural matches (where ranking
  is undefined), Some for fuzzy/semantic/hybrid (where ranking is meaningful).
  Avoids fake `1.0` for everything.
- `raw_score: Option<f32>` — preserves the source score before RRF
  normalization for hybrid mode.

## 6. Modes

### 6.1 `workspace_overview`

Landing page. No query.

```rust
pub struct WorkspaceOverviewParams {
    pub include_target_kinds: Option<Vec<String>>,   // lib/bin/example/test/...
    pub include_metrics: Option<bool>,               // efferent/afferent + instability
}
```

Returns: every local crate as a candidate, plus a `summary.workspace_overview`
with totals (`workspace_stats`) and per-crate inventory (`crate_dependency_metric`).

When the hypergraph snapshot is missing: returns empty `candidates` and a
`RequiresHypergraph` diagnostic. No crash.

Source primitives: `crate_dependency_metric`, `workspace_stats`,
`nodes_by_id` scan over `NodeKind::Crate`. No Cargo manifest reading.

### 6.2 `lookup`

Resolve a name to candidates.

```rust
pub struct LookupParams {
    pub name: String,
    pub fuzzy: bool,
    pub provider: LookupProvider,                    // Graph | Semantic | Both
    pub scope: Option<DiscoverScope>,
    pub kind: Option<NodeKind>,
}

#[serde(rename_all = "snake_case")]
pub enum LookupProvider {
    Graph,                                            // lookup_by_qualified_name
    Semantic,                                         // SemanticService symbol_search
    Both,                                             // union, deduped by file:span
}
```

The `provider` enum addresses a real coverage gap: graph
`lookup_by_qualified_name` only sees hypergraph-extracted items; the semantic
service (RA) can resolve private items, macro-generated items, and items
outside the graph's filter. Forcing one provider loses coverage. `Both`
should be the agent's default; explicit `Graph` is the fast path.

- `fuzzy: false, provider: Graph` → exact `lookup_by_qualified_name`.
  Returns 0 or 1 candidate.
- `fuzzy: false, provider: Semantic` → `SemanticService::symbol_search_with_exact`
  with `exact=true`. Returns 0..N candidates.
- `fuzzy: true, provider: Graph` → in-memory scoring (trigram + edit distance)
  over node qualified names. **No persistent index in v1.** Bounded by snapshot
  walk; OK up to ~50k items. Persistent FST is D2 follow-up.
- `fuzzy: true, provider: Semantic` → RA fuzzy symbol search (already supported
  via `exact=false`).
- `provider: Both` → run both, dedupe by `(file, byte_start, byte_end)`,
  merge `MatchSource` provenance.

### 6.3 `by_content`

BM25 / vector / hybrid over code chunks.

```rust
pub struct ByContentParams {
    pub query: String,
    pub kind: ByContentKind,                          // Text | Semantic | Hybrid
    pub scope: Option<DiscoverScope>,
}

#[serde(rename_all = "snake_case")]
pub enum ByContentKind {
    Text,                                             // BM25 only
    Semantic,                                         // vector only
    Hybrid,                                           // RRF fusion (matches legacy `search`)
}
```

Requires a **new read-only search path**. The current `query.rs::search` mixes
in `ensure_indexed` and `clean_stale_index`. v1 work is to factor out a
pure-open path that:

1. Opens existing BM25 + vector indexes if present.
2. Returns `RequiresChunkIndex` diagnostic if either is missing.
3. Performs query embedding + retrieval.
4. Calls `resolve_chunk_to_item` (currently dead code in
   `tools/graph/response.rs:217`) to map chunks back to hypergraph Items.
5. Falls back to file/span candidates when no Item matches a chunk.

Score normalization: BM25 scores rescaled to 0..1 by max-in-set; vector
similarities already 0..1; hybrid uses RRF (`1 / (k + rank)`, k=60). `raw_score`
preserves the pre-fusion value.

### 6.4 `similar_to`

Vector neighbors of a known Item.

```rust
pub struct SimilarToParams {
    pub anchor: NavigationTargetParams,
    pub threshold: Option<f32>,
    pub max_results: Option<usize>,
}
```

Wraps `similar_to_item` (existing). Anchor must resolve to a hypergraph Item
NodeId; file-span anchors are resolved via `Location::from_file_span`. If the
anchor isn't an Item, returns a structured `PartialCoverage` diagnostic.

### 6.5 `cluster`

Workspace-wide clustering. **No algorithm parameter in v1.**

```rust
pub struct ClusterParams {
    pub substrate: ClusterSubstrate,                  // Items | Functions
    pub scope: Option<DiscoverScope>,
    pub threshold: Option<f32>,
    pub max_cluster_size: Option<usize>,
    pub output_mode: ClusterOutputMode,               // Clusters | Pairs
    pub cross_crate_only: Option<bool>,
}

#[serde(rename_all = "snake_case")]
pub enum ClusterSubstrate {
    Items,                                            // all hypergraph items
    Functions,                                        // ItemKind filter
}
```

Wraps the existing `semantic_overlaps` flow. `Modules` substrate is not
included until module-level embedding exists. Algorithm choice is omitted
entirely until a second algorithm is implemented; adding `algorithm: DBSCAN`
later is non-breaking.

Returns clusters via `summary.clusters`; members of each cluster appear as
candidates with `r#match.source = VectorItem` and `score` from the within-
cluster similarity.

### 6.6 `by_signature`

Function signature filter.

```rust
pub struct BySignatureParams {
    pub filter: FunctionFilter,                       // reuse the existing shape verbatim
    pub scope: DiscoverScope,                         // required: filter without scope is too broad
}
```

Thin wrapper over `functions_with_filter`. Reuses the existing `FunctionFilter`
serde shape — no re-invention. Candidates carry the full function signature
inline.

### 6.7 `by_attribute`

Attribute-pattern filter.

```rust
pub struct ByAttributeParams {
    pub pattern: String,
    pub scope: DiscoverScope,                         // required
    pub include_doc_comments: Option<bool>,           // matches docstring bodies too
}
```

Wraps `items_with_attribute`. Current implementation accepts `crate_name` +
`attribute_pattern`; the wrapper exposes `scope.crate_name` and
`attribute_pattern`. Each candidate's matched attribute string lands in
`preview`.

### 6.8 `structural`

Items whose structural property maps to a concrete location.

```rust
pub struct StructuralParams {
    pub role: StructuralRole,
    pub scope: Option<DiscoverScope>,
    pub threshold: Option<f32>,                       // for HighComplexity
}

#[serde(rename_all = "snake_case")]
pub enum StructuralRole {
    DeadPub,
    Unsafe,
    MutStatic,
    MissingDocs,
    MissingRequiredDerive,
    Recursion,
    UnboundedChannel,
    FunctionBodyPattern,                              // fn_body_audit
    PubUsePubType,
    HighComplexity,
    Cycle,                                            // SCC member items
}
```

Each role maps to one of the existing audit queries. Candidates carry no score
(structural matches are boolean — `score = None`).

`HighComplexity` requires `threshold`. Defaults should match the gates
thresholds (cyclomatic 20, etc.) so `discover` and `check` agree on what's
"complex."

### 6.9 `structural_summary`

Aggregate facts that don't map to single-item candidates.

```rust
pub struct StructuralSummaryParams {
    pub role: StructuralAggregateRole,
    pub scope: Option<DiscoverScope>,
}

#[serde(rename_all = "snake_case")]
pub enum StructuralAggregateRole {
    CrateMetrics,                                     // crate_dependency_metric
    CrateEdges,                                       // crate_edges
    NameCollisions,                                   // overlaps
    DeadPubReport,                                    // workspace-wide rollup
}
```

Returns no candidates; populates `summary.structural_aggregate` only. Each
role corresponds to a legacy tool whose output is a table/report, not a list
of items. Forcing these into the candidate model would either flatten lossily
or balloon the schema.

### 6.10 `list_contents`

Directory listing at a scale.

```rust
pub struct ListContentsParams {
    pub scope: NavigationTargetParams,                // workspace, crate, or module
    pub item_kinds: Option<Vec<ItemKind>>,
    pub pub_only: Option<bool>,
    pub include_modules: Option<bool>,
}
```

Calls a new `children_of(parent: NodeId)` helper that wraps the existing
`children_by_parent` LMDB DB into a sorted candidate list. `crate_types`
covers crate-scope type listings; module-scope requires the new helper.

No score. Sorted by qualified name.

## 7. Implementation map

**New code:**

| File | Purpose |
|---|---|
| `crates/rmc-server/src/tools/params/discover.rs` | `DiscoverParams`, `DiscoverRequest`, all mode-specific param structs, `DiscoverScope` |
| `crates/rmc-server/src/tools/dispatch/discover.rs` | Tagged dispatcher (mirrors `dispatch/observe.rs`) |
| `crates/rmc-server/src/tools/endpoints/discover.rs` | Ten mode handlers, each a thin wrapper |
| `crates/rmc-server/src/tools/endpoints/discover_render.rs` | `DiscoverCandidate`, `DiscoverMatch`, `DiscoverSummary` serializers |
| `crates/rmc-graph/src/graph/query/children.rs` | `children_of(parent)` query helper for `list_contents` |
| **Promote out of dead code:** `tools/graph/response.rs::resolve_chunk_to_item` (currently `#[allow(dead_code)]` at line 217) — wire it into `by_content`, add tests |
| **Read-only search path:** factor a non-mutating `open_existing_hybrid_search` out of `query.rs::create_hybrid_search` and `ensure_indexed`. Used by `by_content` |
| Fuzzy-name in-memory scorer (D2): trigram overlap + edit distance over `nodes_by_id` qualified names |

**Wiring:**

| File | Change |
|---|---|
| `crates/rmc-server/src/tools/router.rs` | Add `async fn discover(...)` method following the existing `observe` / `simulate` / `act` / `check` pattern |
| `crates/rmc-server/src/tools/inventory.rs` | Add `ToolCategory::Discover`, add canonical `discover_agent` entry, reclassify the ~17-20 legacy aliases that become discover modes |
| `crates/rmc-server/src/tools/params/verbs.rs` | Re-export `DiscoverParams` alongside `ObserveParams`, `ActParams`, `CheckParams` |

**Existing infrastructure reused (no changes):**

- `HybridSearch`, `Bm25Search`, `VectorStore` (`rmc-engine`)
- `OpenedSnapshot` query methods: `lookup_by_qualified_name`,
  `functions_with_filter`, `items_with_attribute`, `crate_types`,
  `crate_dependency_metric`, `crate_edges`, `overlaps`, all audits
- `semantic_overlaps`, `similar_to_item` (`rmc-graph::similarity`)
- `SemanticService` (`rmc-server::semantic`) for `provider: Semantic` lookup
- `ListPaginationParams`, `list_page`, `page_list`, `ListMeta`
  (`tools/graph/response.rs`)
- `NavigationTargetParams`, `Span` (`tools/params/navigate.rs`)

## 8. Inventory migration

After `discover` ships, these `ToolExposure::Compatibility` entries collapse
into discover modes (taken from `inventory.rs`):

| Legacy tool | Discover mode |
|---|---|
| `search` | `by_content { kind: Hybrid }` |
| `get_similar_code` | `by_content { kind: Semantic }` |
| `find_definition` | `lookup { provider: Semantic, fuzzy: false }` or `lookup { provider: Both }` |
| `similar_to_item` | `similar_to` |
| `semantic_overlaps` | `cluster` |
| `functions_with_filter` | `by_signature` |
| `items_with_attribute` | `by_attribute` |
| `crate_types` | `list_contents { scope: crate, item_kinds: [...] }` |
| `module_tree` | `list_contents { include_modules: true }` (recursive variant) |
| `workspace_stats` | `workspace_overview` (counts in `summary`) |
| `dead_pub_in_crate` | `structural { role: DeadPub, scope.crate }` |
| `dead_pub_report` | `structural_summary { role: DeadPubReport }` |
| `unsafe_audit` | `structural { role: Unsafe }` |
| `mut_static_audit` | `structural { role: MutStatic }` |
| `missing_docs_audit` | `structural { role: MissingDocs }` |
| `derive_audit` | `structural { role: MissingRequiredDerive }` |
| `recursion_check` | `structural { role: Recursion }` |
| `channel_capacity_audit` | `structural { role: UnboundedChannel }` |
| `fn_body_audit` | `structural { role: FunctionBodyPattern }` |
| `pub_use_pub_type_audit` | `structural { role: PubUsePubType }` |
| `overlaps` | `structural_summary { role: NameCollisions }` |
| `crate_dependency_metric` | `structural_summary { role: CrateMetrics }` |
| `crate_edges` | `structural_summary { role: CrateEdges }` |
| `forbidden_dependency_check` | **stays in `check`** (gates territory) |
| `find_references` | **stays in `observe`** (anchored, navigation-flavored) |

Around 19 legacy aliases collapse into one verb with ten modes. The four-verb
claim becomes five-verb; each verb has a coherent single job.

## 9. What stays in `observe`

Anchored navigation — operations that take a target and explore *around* it.
These don't fit the "list of candidates" model:

- `goto`, `show_body`, `show_callers`, `follow_trail`, `zoom` (current observe surface)
- `who_uses`, `who_imports`, `who_uses_summary` — "from this thing, find related"
- `who_calls`, `calls_from`, `call_graph`, `callers_in_crate`,
  `recursive_callers_count` — call-graph navigation
- `enum_variants`, `item_attributes`, `function_signature` — node metadata
- `get_imports`, `get_exports`, `get_reexports`, `get_declared_reexports`,
  `module_dependencies` — module navigation
- `re_export_chain` — chain walks
- `find_references` — anchored on a target, even though it returns a list
- `analyze_complexity` — single-target metric

## 10. Open decisions

1. **`list_contents` recursion.** Module listing — recursive or single-level by
   default? Recommend single-level, with `recursive: bool` parameter for the
   `module_tree`-like case.
2. **`workspace_overview` includes `cargo metadata`?** No. Reads only the
   hypergraph snapshot. A future mode could add manifest parsing if needed.
3. **`cluster.substrate = Modules`** — not in v1 (no module embeddings).
   Adding it later requires either a new embedding pipeline or naive
   import-graph clustering.
4. **Fuzzy scoring tunables** — trigram overlap weight vs edit distance.
   Recommend hardcoded for v1; revisit if recall complaints surface.
5. **`HighComplexity` threshold defaults** — must agree with `check`'s gate
   thresholds. Recommend reading from the same config file.
6. **Snapshot fingerprint in response** — should `discover` always include the
   fingerprint, only on warm paths, or only when explicitly requested?
   Recommend always; lets observe verify it hasn't drifted.
7. **`provider: Both` deduplication key** — `(file, byte_start, byte_end)` is
   the minimal stable key but breaks across reformatting. Acceptable trade-off
   for v1; revisit when this bites.

## 11. What this does *not* do

- **Does not generate descriptions.** That's the description sub-agent's
  responsibility (separate Phase). When that lands, add `ByDescription` mode.
- **Does not run incremental indexing.** Strictly read-only. Returns
  `RequiresChunkIndex` / `RequiresHypergraph` diagnostics on cold paths.
- **Does not modify state.** All operations are pure reads against snapshots.
- **Does not interpret natural language.** Every mode is a typed dispatch.
- **Does not subsume `observe`.** Different return shape, different cognitive
  role.
- **Does not advertise capabilities that don't exist.** `ByDescription`,
  DBSCAN/HDBSCAN, module clustering, and persistent fuzzy-name indexes are
  out until their backing infrastructure ships.

## 12. Phasing

Tightened from the original proposal based on what the existing codebase
actually supports today:

**D1 — Glue over existing read-only primitives. ~1 week.**

- `workspace_overview`
- `lookup` with `provider: Graph, fuzzy: false`
- `list_contents`
- `by_signature`
- `by_attribute`
- `structural` for all single-item roles
- `structural_summary` for all aggregate roles

These wrap existing query methods; no new infra. Closes ~70% of the bootstrap
gap.

**D2 — Fuzzy lookup + semantic provider. ~3-4 days.**

- `lookup` with `fuzzy: true, provider: Graph` (in-memory trigram + edit
  distance scoring over hypergraph node names)
- `lookup` with `provider: Semantic` (wrap `SemanticService::symbol_search_with_exact`)
- `lookup` with `provider: Both` (union + dedup)
- Optional: new `children_of` helper for `list_contents` module scope

**D3 — Read-only `by_content`. ~1 week.**

- Factor a non-mutating `open_existing_hybrid_search` out of
  `query.rs::create_hybrid_search` and `ensure_indexed`.
- Promote `resolve_chunk_to_item` from dead code, add tests.
- Wire `by_content` over the read-only path.
- Returns `RequiresChunkIndex` diagnostic on cold paths.

**D4 — `similar_to` + `cluster`. ~3-4 days.**

- `similar_to` wraps `similar_to_item`.
- `cluster` wraps `semantic_overlaps` with the existing union-find algorithm
  only.
- Both depend on D3's read-only infrastructure being in place.

**D5 — `by_description`. Blocked on description layer.**

- Add `ByDescription(ByDescriptionParams)` variant to the enum.
- New BM25 index over description text.
- New vector index over description embeddings.
- Lands when descriptions populate.

Total: ~2.5-3 weeks for D1-D4; D5 is decoupled.

---

## 13. Bottom line

Keep `discover` as a separate fifth verb. Make v1 small and strict: glue over
what already works, refuse to advertise what doesn't. The verb's job is
producing valid `NavigationTargetParams` that the agent can feed to `observe`;
that's the only contract that matters. Everything else — modes, scopes,
scoring, summaries — exists to support that contract.

Adding `discover` resolves the discovery-vs-navigation tension that today
forces agents through the legacy tool surface to bootstrap. After D1 alone,
the agent has a workable path from zero knowledge to a useful target list
without touching any compatibility-tier tool.
