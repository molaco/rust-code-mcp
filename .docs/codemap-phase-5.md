# Codemap Phase 5 — `build_codemap` algorithm core

**Status:** complete.
**Scope:** §5 of `.plans/codemaps-proposal.md` (seeds → expand → score → prune → project → assemble). Renderers and MCP wiring deferred to Phase 6.

## Files changed

| File | LOC delta | Detail |
|---|---|---|
| `src/graph/codemap.rs` | +632 (455 → 1087) | `build_codemap` + helpers + 3 unit tests |
| `src/tools/graph_tools.rs` | +1 (visibility) | `fn cosine` → `pub(crate) fn cosine` so codemap reuses it |

## Entry point

```rust
pub(crate) async fn build_codemap(
    snap: &OpenedSnapshot,
    prompt: Option<&str>,
    override_seeds: Option<&[String]>,
    hits: Option<&[crate::search::SearchResult]>,
    opts: &CodemapOptions,
) -> anyhow::Result<Codemap>
```

The MCP tool layer (Phase 6) will run `HybridSearch::search` and pass `hits` in. Keeps `build_codemap` synchronous w.r.t. search and easier to unit-test.

## Algorithm summary

### Seeds
- `override_seeds` path: `lookup_by_qualified_name` per name; unresolved names become `Codemap.diagnostics` entries (`"unresolved seed: <name>"`). No RA fallback.
- Search path: hits → `canonicalize_and_strip` → `enclosing_item_for_line_range` → `ItemKind::is_callable() || is_type()` filter → first `top_k_seeds` accepted.

### Expand (bounded BFS, both directions, degree-capped)
- Outgoing: `callees_of(n)`; edge kind classified by target's `ItemKind::is_callable()` → `Calls` else `Uses`.
- Incoming: branch on `n`'s kind. Callable → `Calls`; type → `Uses`; else skip. Sorted by `rank_referrer` (BM25-hit-score primary, qualified-name tiebreak), top `max_incoming_per_node` retained.
- Edge weight summed on duplicates via `*edges.entry(...).or_insert(0) += 1`. Forced `#[derive(PartialEq, Eq, Hash)]` on `EdgeKind`.

### Score — three-phase async hygiene
- **Phase A (sync, RoTxn held):** compute `bm25_norm` (max-normalized) and `graph_prox = 1/(1+min_call_distance)` for each retained node. For non-`NoRerank`, look up `embeddings_by_target` per node, partition into `cached` (fresh per `EMBEDDER_VERSION`) and `missing`. **Drop RoTxn at end of scope.**
- **Phase B (async, no txn):** lazy-construct `EmbeddingGenerator` only when needed; call `embed_async(prompt.to_owned())`. For `ComputeMissing` call `ensure_embeddings_for(snap, &missing).await` and merge `ResolvedEmbedding.vector` into `cached`. Counts go to `embeddings_computed`.
- **Phase C (sync, no txn):** finalize per-node relevance. `Some(s) => 0.40*s + 0.35*bm25 + 0.25*prox`; `None => 0.60*bm25 + 0.40*prox`.

### Prune
Seeds unconditional. Non-seeds top-N by relevance up to `max_nodes - |seeds|`. Edges with a pruned endpoint dropped.

### Project — all-crates strategy
Collect distinct `Node.crate_id` of retained nodes, map each to `qualified_name`, call `snap.module_tree(...)`, filter each tree post-order. Single crate → return its tree as `hierarchy`. Multiple → wrap under a synthetic `ModuleTreeNode { qualified_name: "<workspace>", kind: "Workspace", … }`.

### Assemble
`snapshot_id ← snap.manifest.graph_id`. `seeds` deterministically sorted by qualified_name. `generated_at_unix` via `SystemTime::now`.

## Notes & decisions

- **Snippet extraction deferred.** `CodemapNode.snippet = None` for all v1 nodes. Permitted by the plan.
- **`min_call_distance` u32::MAX → graph_prox = 0** (not `1/(1+MAX)` which would underflow).
- **`bm25_by_node` sums** hit scores when multiple hits collapse to the same enclosing item (rather than `max`), so frequently-cited functions outrank one-off hits.
- **Determinism:** every `HashSet`/`HashMap` iteration that drives downstream state is sorted by `qualified_name` first.

## Tests

Three new `#[tokio::test]`s in `mod tests`:
1. `build_codemap_override_seeds_resolves_deterministically`
2. `build_codemap_depth_zero_returns_only_seeds`
3. `build_codemap_unresolved_seed_records_diagnostic`

All compile. They depend on the same `build_and_persist`-over-synthetic-Cargo.toml fixture as the Phase 2/3 tests; that fixture loads RA via cargo-metadata which fails on the synthetic manifest in the local environment. **This is a pre-existing environmental issue**, not a Phase 5 regression — the Phase 3 tests fail at the same fixture-load point.

## Build verification

`cargo check --lib` → 0.20s. 31 warnings (Phase 4 baseline was 23; +8 net):
- The +8 are all `dead_code` for items only consumed by `build_codemap` itself (`build_codemap`, `resolve_override_seeds`, `resolve_search_seeds`, `build_bm25_by_node`, `rank_referrer`, `node_qualified_name`, `project_hierarchy`, `filter_module_tree`).
- Once Phase 6 wires `build_codemap` into the MCP `#[tool]` method, this entire chain becomes live and the warning count should drop substantially.
