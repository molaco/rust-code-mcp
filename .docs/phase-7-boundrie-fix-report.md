# Phase 7 Boundrie Fix Report

## Scope

Phase 7 moved workspace-wide semantic-overlap orchestration behind a
graph-owned similarity facade. The server now adapts MCP parameters, resolves
the embedding backend, calls `rmc_graph::graph::run_semantic_overlaps`, maps
typed graph errors, and serializes graph-owned DTOs.

`similar_to_item` remains server-owned because it still depends on server
project-path policy, server hybrid-search construction, and vector-only
search.

## Steps Completed

1. Ran `jj show --summary`.
2. Added `crates/rmc-graph/src/graph/query/similarity.rs`.
3. Added graph-owned semantic-overlap options, errors, output DTOs, pair DTOs,
   item refs, and cluster DTOs.
4. Moved graph item enumeration, embedding-cache refresh, identical-source
   scoring, cosine pair scoring, and cluster construction into graph.
5. Migrated server `semantic_overlaps` to call the graph facade.
6. Removed public graph reexports of `ensure_embeddings_for` and `cosine`.
7. Kept `similar_to_item` server-owned.
8. Rebuilt the MCP hypergraph and verified server production dependencies no
   longer reach graph `embedding_cache` or `math` for semantic-overlap
   behavior.
9. Ran focused nix checks and recorded the Phase 7 ledger.

## Evidence

- MCP rebuilt graph `56dbddbd49bf25977fef1d75a269d455`, fingerprint
  `53b0c34cc7a90b62bade00ab81ce4ae4baf13a37429fee9d4dd4c740b5364aae`.
- `module_dependencies(module="rmc_server::tools::graph::similarity")`
  reports graph dependencies on `rmc_graph::graph::query::similarity` facade
  exports: `GraphSimilarityError`, `SemanticOverlapOptions`, and
  `run_semantic_overlaps`.
- The same dependency check reports no server dependency on graph
  `embedding_cache` or `math`.
- `who_imports(target="rmc_graph::graph::embedding_cache::ensure_embeddings_for")`
  reports only graph query/test importers.
- `who_imports(target="rmc_graph::graph::math::cosine")` reports only graph
  math/query/test importers.
- MCP `semantic_overlaps(crate_name="rmc_graph", item_kind="Function",
  summary=true, max_pairs=40)` returned 178 seeds, 18 total pairs, and 15
  total clusters.
- Source search confirms `similar_to_item` remains in server routing,
  parameters, and implementation code, not as a graph facade.

## Files Changed

- `crates/rmc-graph/src/graph/codemap/build.rs`
- `crates/rmc-graph/src/graph/mod.rs`
- `crates/rmc-graph/src/graph/query/mod.rs`
- `crates/rmc-graph/src/graph/query/model.rs`
- `crates/rmc-graph/src/graph/query/similarity.rs`
- `crates/rmc-server/src/tools/graph/response.rs`
- `crates/rmc-server/src/tools/graph/similarity.rs`
- `crates/rmc-server/src/tools/graph/tests.rs`
- `.docs/boundries-cleanup-progress.md`
- `.docs/phase-7-boundrie-fix-report.md`
- `.plans/boundries-plan.md`

## Verification

- Graph-only check passed:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-graph`.
- Combined focused check passed:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-graph -p rmc-server`.
- Graph similarity tests passed:
  `nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-graph similarity_`
  (6 tests passed).
- MCP dependency verification passed after rebuilding the hypergraph.
- No formatting command was run.

## Commits

- `7a3b26a8`: `docs: start phase 7 similarity facade`
- `94f20c92`: `refactor: add graph similarity facade`
- `2091f947`: `docs: verify phase 7 graph similarity internals`
- `e3ba55e4`: `refactor: use graph similarity facade in server`
- `d4d74fd2`: `docs: keep similar item search server owned`
- `1c9f904e`: `docs: verify phase 7 similarity dependencies`
- `e97f982b`: `docs: record phase 7 check result`
- `13d1382e`: `docs: record phase 7 ledger`

## Outcome

Phase 7 success criteria are met. Server `semantic_overlaps` now asks graph for
semantic-overlap results, graph owns embedding-cache and scoring mechanics,
low-level helper exposure was reduced, and `similar_to_item` stayed
server-owned to avoid moving server/indexing path policy into graph.
