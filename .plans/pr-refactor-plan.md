# PR-Based Refactor Plan

Status: PR 17 complete; PR 18 is next. This is the executable sequence for the
module/file-boundary refactor in `.plans/refactor-plan.md`, corrected with the
Phase 0.6 boundary fixes.

Workflow basis: `THEORY_3.md`.

- Main workflow: Workflow A, structural refactor of an overgrown module graph.
- Compatibility rule: use Workflow C adapters/facades whenever existing paths
  must keep compiling during migration.
- Primitive operations used: `Move`, `Split`, `Rename`, `Lower`, optional
  `Lift`.

This plan is intentionally PR-sized. Each PR should be independently
reviewable, compile with `cargo check --all-targets`, and avoid formatting.

## Global Rules

Every PR starts with:

```sh
jj status
```

If `jj` is unavailable, use:

```sh
git status
```

Every code PR verifies with:

```sh
nix develop ../nix-devshells#cuda-code --command cargo check --all-targets
```

Targeted tests, when listed, use the same devshell:

```sh
nix develop ../nix-devshells#cuda-code --command cargo test <target> --lib
```

Never run `cargo fmt` or any formatter.

Do not widen visibility to `pub` or `pub(crate)` just to make a move compile.
Use the narrowest visibility that works. If a sibling module needs a moved
helper, prefer a narrow `pub(crate) use` from the parent facade over exposing an
entire implementation module.

Do not change MCP tool names, parameter struct names, or public Rust paths
unless the PR explicitly says it is a Phase 6 cleanup PR.

## PR 00: Baseline Record

Status: DONE.

Completed in this workspace:

- Required pre-PR command: `jj show --summary`
  - commit `8f7b65948e57168963ad1978f97f02d155738df2`
  - change `tvqkwplronnptoysqtzwrwoyqulvnzul`
- Baseline check: `nix develop ../nix-devshells#cuda-code --command cargo check --all-targets`
  - result: pass, warnings only
- Hypergraph baseline:
  - nodes: 2973
  - bindings: 5056
  - usages: 7935
- Workspace stats:
  - `pub`: 540
  - `pub(crate)`: 43
  - `pub_crate_share`: 0.07375643224699828
- Dead-public baseline:
  - `dead_pub_in_crate(krate="rust_code_mcp")`: 338 candidates
- Oversized-file line and complexity baseline:
  - `src/tools/graph_tools.rs`: 4488 lines, 92 fns, cyclomatic 816
  - `src/graph/queries.rs`: 4371 lines, 120 fns, cyclomatic 928
  - `src/graph/codemap.rs`: 2058 lines, 46 fns, cyclomatic 327
  - `src/embeddings/openrouter.rs`: 1618 lines, 70 fns, cyclomatic 138
  - `src/embeddings/backend.rs`: 895 lines, 55 fns, cyclomatic 95
  - `src/chunker/mod.rs`: 805 lines, 32 fns, cyclomatic 103
  - `src/indexing/embedding_batcher.rs`: 767 lines, 22 fns, cyclomatic 43
  - `src/tools/search_tool_router.rs`: 765 lines, 56 fns, cyclomatic 168
  - `src/indexing/unified.rs`: 742 lines, 24 fns, cyclomatic 77
  - `src/tools/search_tool.rs`: 629 lines, 1 fn, cyclomatic 90
  - `src/parser/mod.rs`: 621 lines, 21 fns, cyclomatic 45
  - `src/tools/query_tools.rs`: 561 lines, 20 fns, cyclomatic 107
  - `src/config/indexer.rs`: 534 lines, 20 fns, cyclomatic 55
  - `src/tools/index_tool.rs`: 526 lines, 16 fns, cyclomatic 86

Operation: baseline only, no structural change.

THEORY_3 mapping:

- Rule 1: start with version-control state.
- A1: freeze current behavior.
- A2: inventory the structural graph.

Scope:

- Record current check status.
- Record current workspace stats and line-count/complexity evidence.
- Record current facade/public-surface files that must stay compatible through
  the migration.

Commands:

```sh
jj status
nix develop ../nix-devshells#cuda-code --command cargo check --all-targets
```

Rust-code-mcp evidence to record:

- `workspace_stats(directory=...)`
- `build_hypergraph(directory=...)`
- `dead_pub_in_crate(directory=..., krate="rust_code_mcp")`
- `analyze_complexity` for the oversized files named in `.plans/refactor-plan.md`

Files expected:

- documentation/report only, or no file changes if the baseline is captured in
  the PR description.

Exit:

- Baseline is known.
- Working copy has no unrelated changes.
- No code moved yet.

## PR 01: Extract Graph Math Helper

Status: DONE.

Completed in this workspace:

- Required pre-PR command: `jj show --summary`
  - commit `5a3ff76fe37c5dffc4d2a8359055f0d40ebdd07b`
  - change `npyqvrmwurokmqpywwrtltrstuxkwqqk`
- Moved `cosine` from `src/tools/graph_tools.rs` to new
  `src/graph/math.rs`.
- Moved `cosine_basic_identities` into `src/graph/math.rs`.
- Added private `mod math;` plus narrow `pub(crate) use math::cosine;` in
  `src/graph/mod.rs`.
- Updated production callers:
  - `src/graph/codemap.rs` now uses `crate::graph::cosine`.
  - `src/tools/graph_tools.rs` now uses `crate::graph::cosine`.
- Verification: `nix develop ../nix-devshells#cuda-code --command cargo check --all-targets`
  - result: pass, warnings only

Operation: `Move`.

THEORY_3 mapping:

- A6: implement the smallest boundary repair.
- A7: verify the new boundary graph.

Boundary reason:

`cosine` is graph/search-analysis math used by both `graph::codemap` and the
tools semantic-overlap endpoint. It does not belong in the `tools` adapter
layer.

Steps:

1. Create `src/graph/math.rs`.
2. Move `cosine` from `src/tools/graph_tools.rs` to `src/graph/math.rs`.
3. Move `cosine_basic_identities` beside it.
4. Add a private module plus narrow re-export in `src/graph/mod.rs`:

   ```rust
   mod math;

   pub(crate) use math::cosine;
   ```

5. Update callers:
   - `src/graph/codemap.rs` uses `crate::graph::cosine`.
   - `src/tools/graph_tools.rs` uses `crate::graph::cosine`.

Verification:

```sh
nix develop ../nix-devshells#cuda-code --command cargo check --all-targets
```

Exit:

- `cosine` is no longer owned by `tools`.
- `tools` can call the graph-owned helper through a narrow crate-visible
  projection.
- No formatter was run.

## PR 02: Extract Graph Embedding Cache Helper

Status: DONE.

Completed in this workspace:

- Required pre-PR command: `jj show --summary`
  - commit `19933599d21e0f98077ad190a8a87d53e01b4570`
  - change `zpytqpympturyvuwusnttnyvnzvvpyvl`
- Created `src/graph/embedding_cache.rs`.
- Moved `ResolvedEmbedding` and `ensure_embeddings_for` out of
  `src/tools/graph_tools.rs`.
- Deleted `embedder_version`; call sites now use `EmbeddingBackend::identity()`
  directly.
- Added private `mod embedding_cache;` plus narrow
  `pub(crate) use embedding_cache::ensure_embeddings_for;` in
  `src/graph/mod.rs`.
- Kept `ResolvedEmbedding` graph-owned in `embedding_cache.rs`; callers use it
  through inference and do not need to name it from `graph::mod`.
- Updated production callers:
  - `src/graph/codemap.rs` now uses `crate::graph::ensure_embeddings_for`.
  - `src/tools/graph_tools.rs` now uses `crate::graph::ensure_embeddings_for`.
- Verification: `nix develop ../nix-devshells#cuda-code --command cargo check --all-targets`
  - result: pass, warnings only
- Boundary check: `rg -n "crate::tools|crate::mcp" src/graph`
  - result: no hits

Operation: `Move`.

THEORY_3 mapping:

- A6: move code to the boundary that owns the data.
- A7: verify `graph -> tools` is gone.

Boundary reason:

`ensure_embeddings_for` takes `OpenedSnapshot` and `NodeId`, so it is
graph-owned cache support. Keeping it in `tools` creates the forbidden
`graph -> tools` inversion.

Steps:

1. Create `src/graph/embedding_cache.rs`.
2. Move `ResolvedEmbedding` and `ensure_embeddings_for` from
   `src/tools/graph_tools.rs` to `src/graph/embedding_cache.rs`.
3. Delete `embedder_version`; inline `backend.identity()` at its call sites.
4. Add a private module plus narrow re-export in `src/graph/mod.rs`:

   ```rust
   mod embedding_cache;

   pub(crate) use embedding_cache::ensure_embeddings_for;
   ```

5. Update callers:
   - `src/graph/codemap.rs` uses `crate::graph::ensure_embeddings_for`.
   - `src/tools/graph_tools.rs` uses `crate::graph::ensure_embeddings_for`.
6. Keep `graph::embedding_cache` documented as a temporary graph-owned bridge.
   Phase 3 may absorb it into `graph::codemap::seeds` only if
   `semantic_overlaps` no longer needs it.

Verification:

```sh
nix develop ../nix-devshells#cuda-code --command cargo check --all-targets
rg -n "crate::tools|crate::mcp" src/graph
```

Exit:

- `rg -n "crate::tools|crate::mcp" src/graph` returns no real dependency
  hits.
- `graph::codemap -> search::SearchResult` remains a permitted temporary edge;
  it is fixed in PR 12, not here.

## PR 03: Split Tools Router And Params

Status: DONE.

Outcome:

- `src/tools/router.rs` (~720 LOC after the post-review doc trim) is the new
  home of `SearchToolRouter`. Declared as `mod router;` (not `pub`) — only the
  `search_tool_router` and `search_tool` facades inside `tools` reach it.
- `src/tools/search_tool_router.rs` is a 2-line facade
  (`pub use crate::tools::router::*;`), kept `pub mod` so the public path
  `rust_code_mcp::tools::search_tool_router::SearchToolRouter` still resolves.
- `src/tools/params/` is declared `mod params;` (not `pub`); its four family
  submodules (`audit.rs`, `graph.rs`, `indexing.rs`, `search.rs`) are also
  private — `params/mod.rs` flat-re-exports every struct via
  `pub use {family}::*;`. Callers continue to use
  `crate::tools::params::FooParams` without ever naming a family submodule.
- Family split: 9 search, 30+1 graph (the `ForbiddenDependencyRuleParam` alias
  travels with `ForbiddenDependencyCheckParams`), 7 audit, 1 indexing.
  `ListPaginationParams` lives in `params/mod.rs` as a shared cross-family
  util.
- `src/tools/search_tool.rs` is a facade that re-exports
  `crate::tools::params::*` and aliases `SearchToolRouter as SearchTool`
  **directly from `crate::tools::router`** (not through the
  `search_tool_router` facade) — a one-hop external surface, no
  facade-to-facade chain.
- `router.rs` docs were rewritten to drop stale claims (no more "Exposes 10
  tools" / "~200 LOC" / "90% complexity reduction"), point the external doctest
  at the canonical `rust_code_mcp::tools::search_tool::SearchTool` path, and
  describe the actual routing fan-out (query_tools, analysis_tools,
  graph_tools, indexing_tools, index_tool, health_tool, clear_cache_tool).
  The local `pub use crate::tools::params::{...}` was lowered to a plain
  `use` — nothing external consumes those via `router`'s path.
- Internal call sites updated: 41 occurrences in `router.rs` and 16 in
  `graph_tools.rs` migrated from `crate::tools::search_tool::*` to
  `crate::tools::params::*`. `grep -rn "crate::tools::search_tool::" src/`
  returns no matches.
- `nix develop ../nix-devshells#cuda-code --command cargo check --all-targets`
  green.

Operation: `Rename` + `Split` + Workflow C facade adapters.

THEORY_3 mapping:

- A5: shallow target layout.
- A6: split overloaded objects.
- C3: adapters witness old paths.

Boundary reason:

The tool router and parameter/schema structs are adapter-layer concerns. They
should be separate from endpoint implementation files.

Steps:

1. Move `src/tools/search_tool_router.rs` implementation to
   `src/tools/router.rs`.
2. Leave `src/tools/search_tool_router.rs` as:

   ```rust
   pub use crate::tools::router::*;
   ```

3. Create `src/tools/params/`.
4. Split `src/tools/search_tool.rs` parameter/schema structs by family:
   - `params/search.rs`
   - `params/graph.rs`
   - `params/audit.rs`
   - `params/indexing.rs`
   - `params/mod.rs`
5. Leave `src/tools/search_tool.rs` as:

   ```rust
   pub use crate::tools::params::*;
   ```

6. Update `src/tools/mod.rs`.
7. Update internal imports to prefer the new paths, while old public paths keep
   compiling.

Verification:

```sh
nix develop ../nix-devshells#cuda-code --command cargo check --all-targets
```

Exit:

- Router implementation lives in `tools/router.rs`.
- Param structs live under `tools/params/`.
- Old router and search-tool paths still compile through facades.

## PR 04: Split Tools Graph Core Endpoints

Status: DONE.

Outcome:

- New `src/tools/graph/` directory; declared `mod graph;` (private) in
  `src/tools/mod.rs`. Submodules use `pub(super)` visibility so only the
  `graph_tools` facade (sibling under `tools`) can reach them.
- `src/tools/graph/core.rs` (577 LOC) owns 15 core endpoints — imports/exports/
  reexports (`get_imports`, `module_dependencies`, `get_exports`,
  `get_reexports`, `get_declared_reexports`), usage (`who_imports`, `who_uses`,
  `who_uses_summary`), calls (`who_calls`, `calls_from`, `call_graph`,
  `callers_in_crate`, `recursive_callers_count`), modules (`module_tree`), and
  `workspace_stats` — plus 11 response structs/views (`BindingsListResponse`,
  `EnrichedBinding`, `UsagesListResponse`, `EnrichedUsage`,
  `UsageSummaryResponse`, `CallSitesResponse`, `CallSiteView`,
  `CallGraphResponse`, `ModuleDependenciesResponse`, `ModuleDependencyView`,
  `ModuleTreeResponse`) and 6 core-local helpers
  (`enrich_bindings`, `enrich_usages`, `call_site_views`, `namespace_label`,
  `module_dependency_view`, …).
- `src/tools/graph/response.rs` (285 LOC) owns 15 cross-family helpers
  (`open_workspace_snapshot`, `resolve_required_node`, `json_result`,
  `internal_error`, `list_page`, `page_list`, `ListPage`, `ListMeta`,
  `DEFAULT_LIST_LIMIT`, `clear_locations_for_summary`, `visibility_label`,
  `parse_item_kind_filter`, `parse_overlap_scope`, `line_range_overlaps`,
  `resolve_chunk_to_item`). PR 05/06 endpoints consume these through the
  facade — no relocation needed when the next families move out.
- `src/tools/graph_tools.rs` shrank from 4218 to 3421 LOC. New facade lines:
  ```rust
  pub use crate::tools::graph::core::*;
  pub(crate) use crate::tools::graph::response::*;
  ```
  External path `crate::tools::graph_tools::Name` still resolves for every
  moved-out symbol, so `router.rs` and tests compile unchanged.
- `#[cfg(test)] mod tests` block stays in `graph_tools.rs` — it consumes the
  endpoints via `use super::*;` through the facade re-exports.
- Helpers `node_to_item_ref`, `build_clusters`, `page_clusters_by_member_limit`
  were intentionally left in `graph_tools.rs` (PR 06 will relocate them with
  the similarity response structs they depend on).
- `nix develop ../nix-devshells#cuda-code --command cargo check --all-targets`
  green. `grep -rn "crate::tools" src/graph src/embeddings src/indexing
  src/search src/vector_store src/chunker src/parser` returns no hits.

Operation: `Split` + Workflow C facade adapter.

THEORY_3 mapping:

- A3: group dense endpoint families.
- A4: name the group.
- A6: split the overloaded object.

Boundary reason:

`graph_tools.rs` mixes graph endpoint families. Core graph-navigation endpoints
form one dense family.

Steps:

1. Create `src/tools/graph/`.
2. Create `src/tools/graph/mod.rs`.
3. Move core endpoints to `src/tools/graph/core.rs`:
   - imports/exports/reexports
   - `who_imports`
   - `who_uses`
   - `who_calls`
   - `calls_from`
   - `call_graph`
   - `module_tree`
4. Move shared response/enrichment helpers used by more than one graph endpoint
   family to `src/tools/graph/response.rs`.
5. Turn `src/tools/graph_tools.rs` into a facade only after all required
   symbols moved in this PR are re-exported from `tools::graph`.

Verification:

```sh
nix develop ../nix-devshells#cuda-code --command cargo check --all-targets
```

Additional checks:

- `analyze_complexity` on `src/tools/graph/core.rs`
- `get_imports(directory=..., module="rust_code_mcp::tools")`

Exit:

- Core graph tool endpoints have one named file.
- `src/tools/graph_tools.rs` still preserves old imports through re-exports.

## PR 05: Split Tools Graph Crate/Surface/Audit Endpoints

Status: DONE.

Outcome:

- Three new sibling files under `src/tools/graph/`:
  - `crates.rs` (146 LOC) — `crate_edges`, `crate_dependency_metric`,
    `forbidden_dependency_check` + response structs `CrateEdgesResponse`,
    `CrateDependencyMetricResponse`, `CrateMetricRendered`,
    `ForbiddenDependencyCheckResponse`.
  - `surface.rs` (968 LOC) — 12 endpoints (`dead_pub_in_crate`,
    `dead_pub_report`, `enum_variants`, `item_attributes`,
    `items_with_attribute`, `function_signature`, `functions_with_filter`,
    `pub_use_pub_type_audit`, `re_export_chain`, `missing_docs_audit`,
    `derive_audit`, `overlaps`) + 14 family-private response structs + 2
    family-private helpers (`enrich_dead_pub`, `enrich_crate_dead_pub`).
  - `audits.rs` (454 LOC) — `unsafe_audit`, `mut_static_audit`,
    `recursion_check`, `channel_capacity_audit`, `fn_body_audit` +
    `RecursionCycleRendered` struct.
- `src/tools/graph/mod.rs` now declares `pub(super) mod core; crates;
  surface; audits; response;`.
- `graph_tools.rs` shrank from 3421 → 1912 LOC (−1509). Facade re-export
  block now:
  ```rust
  pub use crate::tools::graph::core::*;
  pub use crate::tools::graph::crates::*;
  pub use crate::tools::graph::audits::*;
  pub use crate::tools::graph::surface::*;
  pub(crate) use crate::tools::graph::response::*;
  ```
  Every moved-out symbol still resolves at its old
  `crate::tools::graph_tools::Name` path.
- Family-private response structs use `pub(crate)` only as needed to satisfy
  the facade's `pub use ::*;` glob — no widening beyond that pattern.
- `nix develop ../nix-devshells#cuda-code --command cargo check --all-targets`
  green. `grep -rn "crate::tools"` over engine modules returns no hits.

Operation: `Split`.

THEORY_3 mapping:

- A3: dense groups.
- A6: split by concern.

Boundary reason:

Crate metrics, public-surface inspection, and audits are separate endpoint
families. Keeping them together keeps the adapter layer too broad for agents to
work locally.

Steps:

1. Move crate-level endpoints to `src/tools/graph/crates.rs`:
   - `crate_edges`
   - `crate_dependency_metric`
   - `forbidden_dependency_check`
2. Move public-surface endpoints to `src/tools/graph/surface.rs`:
   - `dead_pub*`
   - attributes/items attributes
   - missing docs
   - derive audit endpoint bridge
   - pub-use/pub-type audit
   - re-export chain
3. Move audit endpoint bridges to `src/tools/graph/audits.rs`:
   - unsafe
   - mut static
   - recursion
   - channel
   - fn-body audit
4. Keep shared JSON/response code in `src/tools/graph/response.rs`.

Verification:

```sh
nix develop ../nix-devshells#cuda-code --command cargo check --all-targets
```

Additional checks:

- `analyze_complexity` on the moved files
- `get_imports(directory=..., module="rust_code_mcp::tools::graph")`

Exit:

- Each graph endpoint family has a named file.
- No MCP tool name changes.

## PR 06: Split Tools Graph Similarity And Codemap Endpoints

Status: DONE.

Outcome:

- Two new sibling files under `src/tools/graph/`:
  - `similarity.rs` (583 LOC) — `similar_to_item`, `semantic_overlaps`
    endpoints + family-private helper `resolve_graph_tool_backend` + 5
    family-private response structs (`SimilarToItemResp`, `SeedItemRef`,
    `SimilarMatch`, `SemanticOverlapsResp`, `ScopeSummary`, `SimilarityPair`).
    Uses `crate::graph::{cosine, ensure_embeddings_for}` directly — PR 01/02's
    graph-side homes, no `crate::tools::graph_tools::*` paths involved.
  - `codemap.rs` (172 LOC) — `handle_build_codemap` endpoint only.
- `build_hypergraph` + `BuildHypergraphResponse` appended to `core.rs`
  (577 → 626 LOC); naturally fits with `workspace_stats` and the rest of the
  core graph endpoints. This completes the "pure facade" goal — every
  production endpoint now lives in a family file.
- Cross-family shared helpers moved into `response.rs` (285 → 467 LOC):
  `node_to_item_ref`, `build_clusters`, `page_clusters_by_member_limit`, and
  the cross-family structs `ItemRef` and `SimilarityCluster`. (Plan-spec said
  only `ItemRef`, but `SimilarityCluster` was the return type of
  `build_clusters` so it had to follow.)
- `src/tools/graph/mod.rs` (12 → 16 LOC) now declares all six endpoint
  families plus the shared `response`:
  ```rust
  pub(super) mod audits;
  pub(super) mod codemap;
  pub(super) mod core;
  pub(super) mod crates;
  pub(super) mod response;
  pub(super) mod similarity;
  pub(super) mod surface;
  ```
- `graph_tools.rs` shrank from 1912 → 933 LOC — 14 production lines (doc
  comment + 7 facade re-exports) plus the 919-line `#[cfg(test)] mod tests`
  block, which still resolves every endpoint through the facade glob
  re-exports. The `_path_marker` stub and stale `use std::path::Path;` were
  deleted (no remaining `Path` use after the moves). Final facade body:
  ```rust
  pub use crate::tools::graph::audits::*;
  pub use crate::tools::graph::core::*;
  pub use crate::tools::graph::crates::*;
  pub use crate::tools::graph::similarity::*;
  pub use crate::tools::graph::surface::*;
  pub(crate) use crate::tools::graph::codemap::*;
  // (response helpers are imported inside `mod tests` only — see below)
  ```
  `codemap` uses `pub(crate)` since `handle_build_codemap` is `pub(crate)` —
  rustc warned that a `pub use` glob can't widen visibility, and the
  endpoint's only caller (`router::build_codemap`) is in-crate. Access to
  `response.rs` helpers from the retained `#[cfg(test)] mod tests` block is
  scoped inside the test module (`use crate::tools::graph::response::*;`)
  rather than a module-level `pub(crate) use`, so the facade does not leak
  `open_workspace_snapshot`, `resolve_chunk_to_item`, `build_clusters`,
  pagination DTOs, etc. crate-wide through the old `graph_tools::*` path.
- `nix develop ../nix-devshells#cuda-code --command cargo check --all-targets`
  green. `grep -rn "crate::tools"` over engine modules returns no hits.
- Checkpoint 2 (PR 03-06) complete: the 4218-LOC `graph_tools.rs` mega-file
  is fully dissolved into 7 sibling family files in `src/tools/graph/`, with
  full external-path compatibility via the facade.

Operation: `Split`.

THEORY_3 mapping:

- A6: split the remaining endpoint families.
- C3: keep old facade paths.

Boundary reason:

Similarity and codemap endpoints are adapter bridges over graph/search/embedding
subsystems. They should be separate from core graph query endpoints.

Steps:

1. Move similarity endpoints to `src/tools/graph/similarity.rs`:
   - `similar_to_item`
   - `semantic_overlaps`
   - backend/profile resolution needed by these endpoints
2. `similarity.rs` may use:
   - `crate::graph::cosine`
   - `crate::graph::ensure_embeddings_for`
3. Move `build_codemap` endpoint bridge to `src/tools/graph/codemap.rs`.
4. Ensure `src/tools/graph/mod.rs` re-exports the endpoint functions.
5. Keep `src/tools/graph_tools.rs` as a pure facade:

   ```rust
   pub use crate::tools::graph::*;
   ```

Verification:

```sh
nix develop ../nix-devshells#cuda-code --command cargo check --all-targets
```

Exit:

- `graph_tools.rs` has no implementation logic.
- `tools/graph/similarity.rs` does not define graph-owned embedding-cache
  helpers; it only calls the graph-owned projections.

## PR 07: Move Standalone Tool Endpoints

Status: DONE.

Outcome:

- New `src/tools/endpoints/` directory; declared `mod endpoints;` (private)
  in `src/tools/mod.rs`. Six submodules under `pub(super)` visibility:
  ```rust
  pub(super) mod analysis;
  pub(super) mod cache;
  pub(super) mod health;
  pub(super) mod index;
  pub(super) mod indexing_support;
  pub(super) mod query;
  ```
- Implementation moved verbatim from six top-level files into the new
  subtree (2197 LOC total, exact preservation):
  - `analysis_tools.rs` (496) → `endpoints/analysis.rs`
  - `clear_cache_tool.rs` (333) → `endpoints/cache.rs`
  - `health_tool.rs` (171) → `endpoints/health.rs`
  - `index_tool.rs` (526) → `endpoints/index.rs`
  - `indexing_tools.rs` (110) → `endpoints/indexing_support.rs`
  - `query_tools.rs` (561) → `endpoints/query.rs`
- Each old file is now a 2-line facade:
  ```rust
  //! Compatibility facade — implementation lives in `crate::tools::endpoints::<new>`.
  pub use crate::tools::endpoints::<new>::*;
  ```
  External paths `rust_code_mcp::tools::query_tools::search`,
  `rust_code_mcp::tools::health_tool::HealthCheckParams`, etc. continue to
  resolve.
- Router (`src/tools/router.rs`) was repointed: 15 occurrences of
  `crate::tools::<old>::*` rewritten to `crate::tools::endpoints::<new>::*`
  (12 fn calls + 3 `Parameters<…Params>` type refs). Module-level doc
  comment refreshed to describe the new endpoint fan-out.
- Two PR 06 graph files (`src/tools/graph/codemap.rs:136`,
  `src/tools/graph/similarity.rs:99`) referenced
  `crate::tools::query_tools::create_hybrid_search` — a `pub(crate)` helper
  that doesn't traverse a `pub use ::*;` facade glob. Both call sites were
  updated directly to `crate::tools::endpoints::query::create_hybrid_search`.
  No visibility was widened.
- Each moved file's `#[cfg(test)] mod tests` block traveled with the
  implementation. Tests now run from `crate::tools::endpoints::<new>::tests::*`.
- `nix develop ../nix-devshells#cuda-code --command cargo check --all-targets`
  green. Both verification greps return empty:
  ```sh
  grep -rn "crate::tools::\(analysis_tools\|clear_cache_tool\|health_tool\|index_tool\|indexing_tools\|query_tools\)::" src/
  grep -rn "crate::tools" src/graph/ src/embeddings/ src/indexing/ src/search/ src/vector_store/ src/chunker/ src/parser/
  ```
- Checkpoint 2 (PR 03-07) complete: `tools/` is now a thin adapter layer
  with `params/`, `endpoints/`, `graph/` subtrees plus the `router.rs`
  entry point. Every old top-level file is a one-line facade.

Operation: `Move` + Workflow C facade adapters.

THEORY_3 mapping:

- A6: move files into the proposed directory.
- C3: old module paths re-export new implementation.

Boundary reason:

Standalone endpoint files are all adapter-layer endpoints and belong under
`tools/endpoints/`.

Steps:

1. Create `src/tools/endpoints/`.
2. Move:
   - `analysis_tools.rs` -> `endpoints/analysis.rs`
   - `clear_cache_tool.rs` -> `endpoints/cache.rs`
   - `health_tool.rs` -> `endpoints/health.rs`
   - `index_tool.rs` -> `endpoints/index.rs`
   - `indexing_tools.rs` -> `endpoints/indexing_support.rs`
   - `query_tools.rs` -> `endpoints/query.rs`
3. Leave each old file as a one-line `pub use` facade.
4. Update `src/tools/mod.rs` and router imports.

Verification:

```sh
nix develop ../nix-devshells#cuda-code --command cargo check --all-targets
```

Exit:

- Standalone endpoint implementations live under `tools/endpoints/`.
- In-repo tests/examples importing old paths still compile.

## PR 08: Create Graph Query Skeleton And Move Model Types

Status: DONE.

Outcome:

- New `src/graph/query/` directory; declared `mod query;` (private) in
  `src/graph/mod.rs`. The existing `pub mod queries;` stays alongside it as
  the migration facade.
- `src/graph/query/mod.rs` (17 LOC) declares 10 `pub(super)` family
  submodules: `audits`, `calls`, `crates`, `functions`, `imports`, `model`,
  `modules`, `overlaps`, `surface`, `usage`.
- `src/graph/query/model.rs` (420 LOC) — 32 result types moved verbatim
  from `queries.rs` lines 1-530: `DeadPubFinding`, `CrateDeadPub`,
  `ModuleDependency`, `ModuleDependencySymbol`, `CrateEdge`, `EdgeSymbol`,
  `ForbiddenDependencyRule`, `ForbiddenDependencyViolation`,
  `OverlapsReport`, `OverlapScope`, `TypeCollision`, `TypeLocation`,
  `ModuleShadow`, `WithinCrateDuplicate`, `CommonFnName`, `EnrichedCallSite`,
  `CallGraphNode`, `RecursiveCallersCount`, `UsageSummaryRow`,
  `ModuleTreeNode`, `WorkspaceStats`, `NodeKindCounts`, `VisibilityCounts`,
  `ItemWithAttribute`, `FunctionFilter`, `SelfKindFilter`,
  `FunctionWithSignature`, `PubTypeAliasMasqueradingAsReexport`,
  `ReExportLink`, `ReExportChain`, `CrateMetric`, `MutStaticFinding`.
- The other 9 family files (`imports.rs`, `usage.rs`, `calls.rs`,
  `crates.rs`, `surface.rs`, `audits.rs`, `functions.rs`, `modules.rs`,
  `overlaps.rs`) are 3-line placeholder stubs noting which later PR
  populates them.
- `src/graph/queries.rs` shrank from 4371 → 3966 LOC. The 32 type
  definitions were replaced by a single re-export at the top of the file:
  ```rust
  // Result types moved to `super::query::model` in PR 08; re-exported here so
  // `crate::graph::queries::FooResult` still resolves for external consumers.
  pub use super::query::model::*;
  ```
  Two unused imports (`rmcp::schemars`, `serde::{Deserialize, Serialize}`)
  were pruned — only the moved type derives referenced them.
- `classify_metadata`, the `impl OpenedSnapshot { ... }` block (~530
  through ~3065), the `ModuleDependencyAccumulator` /
  `ModuleDependencySymbolAccumulator` helper structs, and the entire
  `#[cfg(test)] mod tests` block (~3066 onward) all stay in `queries.rs` —
  PR 09/10/11 will move them family by family.
- External consumers unchanged: `crate::graph::queries::ItemWithAttribute`
  (`tools/graph/surface.rs:13`), `crate::graph::queries::ModuleTreeNode`
  (`graph/codemap.rs:18`),
  `crate::graph::queries::{FunctionFilter, SelfKindFilter}`
  (`graph/signatures.rs:144`), the `pub use queries::{...};` block in
  `graph/mod.rs`, and all `shared_snapshot` test imports continue resolving.
- `nix develop ../nix-devshells#cuda-code --command cargo check --all-targets`
  green. No new `crate::tools` references in engine modules.

Operation: `Split` + Workflow C facade adapter.

THEORY_3 mapping:

- A6: move shared contracts before behavior.
- C3: old module re-exports new type locations.

Boundary reason:

`graph::queries` has a large shared result-type surface. Moving the model first
stabilizes the contract before query functions move.

Steps:

1. Create `src/graph/query/`.
2. Create empty family modules:
   - `model.rs`
   - `imports.rs`
   - `usage.rs`
   - `calls.rs`
   - `crates.rs`
   - `surface.rs`
   - `audits.rs`
   - `functions.rs`
   - `modules.rs`
   - `overlaps.rs`
3. Move query result structs/enums to `query/model.rs`.
4. Keep `src/graph/queries.rs` re-exporting from `super::query`.
5. Update `src/graph/mod.rs` to expose the new query module while preserving
   existing public re-exports.

Verification:

```sh
nix develop ../nix-devshells#cuda-code --command cargo check --all-targets
```

Exit:

- Query result types compile from old and new paths.
- No query behavior moved yet.

## PR 09: Move Graph Query Import/Usage/Call Families

Status: DONE.

Outcome:

- 14 methods moved from the giant `impl OpenedSnapshot { ... }` block in
  `queries.rs` into three family files. Each family file got its own
  `impl OpenedSnapshot { ... }` block — Rust merges them at compile time, so
  callers (`snap.imports_of(m)`) need no path changes.

- **`src/graph/query/imports.rs`** (279 LOC): 5 endpoint methods
  (`imports_of`, `module_dependencies`, `exports_of`, `reexports_of`,
  `declared_reexports_of`), 2 family-private helper methods (`node_maps`,
  `module_ancestors`), 1 family-private free fn (`is_visible_from`), and
  the `ModuleDependencyAccumulator` + `ModuleDependencySymbolAccumulator`
  helper structs with their impls. All moved items are private (no
  visibility widening).

- **`src/graph/query/usage.rs`** (114 LOC): 4 methods (`who_imports`,
  `usages_of`, `usages_in`, `who_uses_summary`). No family-private
  helpers — every helper this family reaches into is shared with
  remaining-in-queries.rs methods, so they stayed in queries.rs and were
  widened to `pub(super)`.

- **`src/graph/query/calls.rs`** (334 LOC): 5 methods (`who_calls`,
  `calls_from`, `call_graph`, `callers_in_crate`,
  `recursive_callers_count`) + the family-private `call_graph_rec` helper
  (only used by `call_graph`).

- `queries.rs` shrank from 3966 → 3299 LOC (−667).

- 6 private helpers in `queries.rs` widened from `fn` to `pub(super) fn`
  (narrowest workable widening per Guardrail 2 — visible only to `graph::*`,
  not crate-wide): `bindings_for_from_module`, `bindings_for_target`,
  `usages_for_target`, `usages_for_consumer`, `usages_for_consumer_function`,
  and the free fn `dependency_node_for`. Each is reached by at least one
  moved method AND at least one method/test that stays in queries.rs, so it
  can't simply move with one family.

- No hardcoded qualified-name strings needed updating: the two
  `graph::queries::` literals in queries.rs reference items that did not
  move (`OpenedSnapshot::lookup_by_qualified_name` and the
  `ForbiddenDependencyRule` type).

- `#[cfg(test)] mod tests` block (1305 LOC) stayed in `queries.rs`
  unchanged — tests call methods via `snap.foo()` dispatch, which finds
  methods regardless of which file's `impl OpenedSnapshot` defines them.

- `nix develop ../nix-devshells#cuda-code --command cargo check --all-targets`
  green. Engine-modules `crate::tools` grep returns no hits. All existing
  external consumers (`graph::queries::ItemWithAttribute`,
  `graph::queries::ModuleTreeNode`, `graph::queries::{FunctionFilter,
  SelfKindFilter}`, `shared_snapshot` test fixture imports) continue to
  resolve through the unchanged facade re-exports and the test module.

Operation: `Split`.

THEORY_3 mapping:

- A3: dense query families.
- A6: split the mega-file by named surfaces.

Steps:

1. Move import/export/re-export queries to `graph/query/imports.rs`.
2. Move `who_imports`, `who_uses`, and usage summary queries to
   `graph/query/usage.rs`.
3. Move call graph queries to `graph/query/calls.rs`.
4. Keep `graph/queries.rs` as a facade.
5. Update imports without widening visibility beyond the narrowest working
   scope.

Verification:

```sh
nix develop ../nix-devshells#cuda-code --command cargo check --all-targets
nix develop ../nix-devshells#cuda-code --command cargo test graph:: --lib
```

Exit:

- Three query families live in named files.
- Existing public paths still compile.

## PR 10: Move Graph Query Crate/Surface/Audit Families

Status: DONE.

Outcome:

- 13 more methods moved out of `impl OpenedSnapshot { ... }` in `queries.rs`,
  using the same multi-impl-block pattern as PR 09.

- **`src/graph/query/crates.rs`** (456 LOC): 3 methods (`crate_edges`,
  `crate_dependency_metric`, `forbidden_dependency_check`) + 1 private impl
  helper (`crate_target_kind_by_name`) + 4 family-local free helpers
  (`glob_match`, `rule_allows_consumer_kind`, `matches_default_consumer_kind`,
  `normalize_consumer_kind`) + 3 helper unit tests co-located with them.

- **`src/graph/query/surface.rs`** (533 LOC): 7 methods (`enum_variants`,
  `item_attributes`, `items_with_attribute`, `pub_use_pub_type_audit`,
  `re_export_chain`, `dead_pub_in_crate`, `dead_pub_report`) + 5 family-local
  free helpers (`match_attribute`, `attr_matches_path_or_body`,
  `normalize_attr_pattern`, `attr_pattern_is_path_only`, `attr_path`) + 1
  co-located unit test.

- **`src/graph/query/audits.rs`** (146 LOC): 3 methods (`static_metadata`,
  `mut_static_audit`, `unsafe_audit`) + the `classify_metadata` free fn
  (moved from queries.rs top section) + the `MUT_STATIC_PATTERNS` const.

- `queries.rs` shrank from 3299 → 2238 LOC (−1061).

- Helper-test co-location decision: 4 free-helper unit tests
  (`forbidden_glob_match_smoke`, `forbidden_dependency_rule_defaults_to_lib_and_bin_consumers`,
  `forbidden_dependency_rule_explicit_consumer_kinds_override_default`,
  `match_attribute_accepts_bare_attribute_paths`) travelled with the helpers
  into their family files. Keeping them in `queries.rs::tests` would have
  required `pub(crate)` widening of the helpers — a violation of Guardrail 2.
  Family-private placement preserves the narrowest workable visibility AND
  matches the standard Rust convention of co-locating helper tests. The
  larger snapshot-backed `forbidden_dependency_*` integration tests stay in
  `queries.rs::tests` because they exercise full `OpenedSnapshot` methods.

- No new `pub(super)` widenings beyond the 6 from PR 09. The shared helpers
  reached by both moved and remaining methods (`format_binding_visibility`,
  `count_declared_visibility`, `overlap_scope_allows_crate`,
  `dependency_node_for`, etc.) are still reached only by methods that stay
  in queries.rs (`module_tree`, `workspace_stats`, `overlaps_with_scope`)
  plus iterator helpers already widened in PR 09.

- 1 re-export added to `queries.rs` after PR 08's model re-export:
  `pub use super::query::audits::classify_metadata;` — keeps
  `src/graph/statics.rs:73`'s `use crate::graph::queries::classify_metadata;`
  resolving.

- No hardcoded qualified-name strings needed updating: the literals at
  queries.rs:1054 (`OpenedSnapshot::lookup_by_qualified_name`) and
  queries.rs:2128 (`ForbiddenDependencyRule`) still reference items that
  resolve in queries.rs (a method that stays + a model type re-export).

- Two now-unused imports (`usage_category_label`, `StaticMetadata`) pruned
  from queries.rs to keep the file warning-clean after the moves.

- `nix develop ../nix-devshells#cuda-code --command cargo check --all-targets`
  green. Engine-modules `crate::tools` grep returns no hits.

Operation: `Split`.

THEORY_3 mapping:

- A6: split remaining dense query groups.

Steps:

1. Move crate graph queries to `graph/query/crates.rs`.
2. Move public-surface queries to `graph/query/surface.rs`:
   - dead public
   - item attributes
   - enum variants
   - pub-use/pub-type audit
   - re-export chain
3. Move audit queries to `graph/query/audits.rs`:
   - unsafe audit
   - mut static audit
   - static metadata
4. Keep `graph/queries.rs` as a facade.

Verification:

```sh
nix develop ../nix-devshells#cuda-code --command cargo check --all-targets
nix develop ../nix-devshells#cuda-code --command cargo test graph:: --lib
```

Exit:

- Crate, surface, and audit query families live in named files.

## PR 11: Move Remaining Graph Query Families And Test Support

Status: DONE.

Outcome:

- 6 more methods moved out of `impl OpenedSnapshot { ... }` in queries.rs
  into the last three family files. Same multi-impl-block pattern as
  PRs 09/10.

- **`src/graph/query/functions.rs`** (98 LOC): `function_signature`,
  `functions_with_filter` + family-private `filter_matches` helper.

- **`src/graph/query/modules.rs`** (331 LOC): `module_tree`,
  `workspace_stats` + 4 family-private helpers (`build_module_tree` method,
  `count_declared_visibility`, `visibility_count_notes`,
  `format_binding_visibility`) + 2 unit tests
  (`visibility_counts_separate_module_private_from_restricted`,
  `visibility_count_notes_flag_alias_fields`) co-located with the helpers
  they exercise. `workspace_stats` placement is a judgment call — the plan
  didn't explicitly assign it; co-locating with `module_tree` keeps
  workspace-level structural queries together.

- **`src/graph/query/overlaps.rs`** (281 LOC): `overlaps`,
  `overlaps_with_scope` + family-private `overlap_scope_allows_crate`
  helper + 1 unit test (`overlap_scope_filters_examples_and_vendor`).

- queries.rs shrank from 2238 → 1554 LOC (−684).

- **`queries.rs` is now a true facade** (post-review fix-up, see below).
  After PR 11 originally landed, reviewer flagged that queries.rs was still
  a 1554-LOC production module owning navigation primitives + shared
  helpers, contradicting the exit condition "contains only re-exports and
  test-only compatibility". Fix-up applied (squashed into PR 11):

  - **New `src/graph/query/navigation.rs`** (~254 LOC) — owns the lower-level
    `OpenedSnapshot` navigation primitives: `lookup_by_qualified_name`
    (+ 3 helpers: `lookup_by_qualified_name_inner`,
    `lookup_impl_module_item_alias`, plus the free fns
    `impl_module_item_alias_parts`, `is_impl_module_item_alias_candidate`),
    `node_by_id`, `find_root_module_of`, `callees_of`, `referrers_of`.
  - **New `src/graph/query/shared.rs`** (~168 LOC) — owns cross-family
    helpers: 5 LMDB iterator methods (`bindings_for_from_module`,
    `bindings_for_target`, `usages_for_target`, `usages_for_consumer`,
    `usages_for_consumer_function`), the `dependency_node_for` free fn,
    and the `MAX_REEXPORT_HOPS` const.
  - `query/mod.rs` updated to declare both new submodules (12
    `pub(super) mod ...;` lines, alphabetical).
  - **Family-file back-imports eliminated**: `imports.rs` and `surface.rs`
    were importing from `super::super::queries::*` (Medium #2). Now they
    use `super::shared::*`, breaking the circular implementation dependency.
  - **Visibility narrowed to `pub(in crate::graph)`** for the 6 items that
    need to be reachable from `queries.rs::tests` (in `graph::queries`):
    `impl_module_item_alias_parts`,
    `is_impl_module_item_alias_candidate`, `callees_of`, `referrers_of`,
    `dependency_node_for`, `MAX_REEXPORT_HOPS`. Subagent initially used
    `pub(crate)` but that's wider than needed — narrowed to `pub(in
    crate::graph)` per Guardrail 2 (narrowest workable widening).
    `MUT_STATIC_PATTERNS` in audits.rs was kept private (it was only ever
    used inside its own file).
  - **`queries.rs` reduced to ~30 production lines + ~1130 test lines**
    (from 1554 LOC pre-fixup). Production content is exclusively:
    the rewritten module-level doc comment + `pub use super::query::model::*;`
    + `pub use super::query::audits::classify_metadata;` + a few
    `#[cfg(test)] use` statements bringing names into scope for the
    unchanged test block.
  - **Unused `SelfKind` import** in `model.rs:13` removed (Low #1).
  - **Stale docs refreshed** (Low #2): module-level doc comment in
    `queries.rs:3` rewritten to describe the facade state; doc reference
    in `usages.rs:176` corrected to point at
    `graph::test_support::shared_snapshot` (the actual shared fixture
    name).

- **Test fixture relocation**:
  - Created `src/graph/test_support.rs` (42 LOC) with `pub(crate) fn shared_snapshot`.
  - Registered `#[cfg(test)] pub(crate) mod test_support;` in `src/graph/mod.rs`.
  - Updated 4 sibling test imports
    (`attributes.rs:279`, `signatures.rs:143`, `unsafe_audit.rs:244`,
    `statics.rs:72`) from `crate::graph::queries::tests::shared_snapshot` to
    `crate::graph::test_support::shared_snapshot`.
  - `queries.rs::tests` narrowed from `pub(crate) mod tests` to private
    `mod tests` (no longer re-exports the fixture path).
  - `usages.rs` private mirror: NOT replaced — it builds a synthetic
    minimal crate for pattern-coverage tests, materially different from the
    workspace-loading `shared_snapshot`. Renamed locally from
    `shared_snapshot` to `synthetic_snapshot` (10 sites) so the conceptual
    divergence is explicit; doc-comment cross-references
    `graph::test_support::shared_snapshot`.

- No new `pub(super)` widenings beyond the 6 from PR 09. All family-local
  helpers stayed private to their new files.

- Unused imports pruned from queries.rs after the moves
  (`BTreeSet`, `BTreeMap`, several label fns, `BindingVisibility`,
  `FunctionSignature`, `SelfKind`). `HashSet` and `Context` kept — still
  used by remaining lookup helpers.

- `nix develop ../nix-devshells#cuda-code --command cargo check --all-targets`
  green. `grep -rn "queries::tests::shared_snapshot" src/` returns empty.
  Engine-modules `crate::tools` grep returns empty.

- **Checkpoint 3 (PR 08-11) complete**: the 4371-LOC `graph::queries`
  mega-file is dissolved into 10 family files (`query/{model, imports,
  usage, calls, crates, surface, audits, functions, modules, overlaps}.rs`)
  + a `test_support` fixture sibling. queries.rs is now 1554 LOC of
  navigation primitives + a test module; PR 19 will fold or migrate the
  remaining content during facade cleanup.

Operation: `Split` + `Move`.

THEORY_3 mapping:

- A6: complete split.
- C3: facade keeps old path valid.

Steps:

1. Move function-signature queries to `graph/query/functions.rs`.
2. Move module-tree query to `graph/query/modules.rs`.
3. Move overlap report query to `graph/query/overlaps.rs`.
4. Move `queries.rs::tests::shared_snapshot` to
   `src/graph/test_support.rs` under `#[cfg(test)]`.
5. Update graph sibling test imports:
   - `attributes.rs`
   - `signatures.rs`
   - `unsafe_audit.rs`
   - `statics.rs`
6. Reconcile the private mirror in `usages.rs`.
7. Leave `src/graph/queries.rs` as a pure facade.

Verification:

```sh
nix develop ../nix-devshells#cuda-code --command cargo check --all-targets
nix develop ../nix-devshells#cuda-code --command cargo test graph:: --lib
```

Additional checks:

- `analyze_complexity` on `src/graph/query/*`

Exit:

- `graph/queries.rs` contains only re-exports and test-only compatibility if
  still needed.
- Query mega-file implementation is gone.

## PR 12: Split Codemap Model And Search Hit DTO

Status: DONE.

Outcome:

- `src/graph/codemap.rs` (2056 LOC) converted to a directory:
  `src/graph/codemap/{mod.rs, model.rs, seeds.rs}`.
- **`codemap/model.rs`** (107 LOC) — codemap data model types moved verbatim:
  `Codemap`, `CodemapNode`, `CodemapEdge`, `EdgeKind`, `CodemapStats`,
  `CodemapOptions` + its `impl Default`, `EmbeddingPolicy`. Re-exported from
  `mod.rs` via `pub use model::*;` so `crate::graph::codemap::Codemap` etc.
  still resolve unchanged.
- **`codemap/seeds.rs`** (145 LOC) — owns the new `SeedHit` DTO + seed
  resolution helpers. The DTO mirrors only the four `SearchResult` fields
  the algorithm reads (`file_path: PathBuf`, `line_start: u32`,
  `line_end: u32`, `score: f32`); pre-casts to `u32` to match
  `enclosing_item_for_line_range`'s signature. `seeds.rs` owns
  `resolve_override_seeds`, `resolve_search_seeds`, and
  `build_bm25_by_node` — search-hit normalization is now self-contained in
  `seeds.rs` and operates on `&[SeedHit]`, not `&[SearchResult]`.
- **`codemap/mod.rs`** (1857 LOC) keeps `build_codemap`, render fns,
  hierarchy fns, helpers, the `#[cfg(test)] mod tests` block, and
  `newest_source_mtime`. PR 13 will split these by concern. `build_codemap`'s
  signature changed: `hits: Option<&[crate::search::SearchResult]>` →
  `hits: Option<&[SeedHit]>`.
- **Boundary fix verified**: `grep -rn "crate::search::SearchResult"
  src/graph/codemap/` returns no code matches (only 3 doc-comment narrations
  of the fix). The `graph::codemap → search::SearchResult` inline-path
  dependency is gone.
- **Tools-side mapping** added at `src/tools/graph/codemap.rs`:
  ```rust
  fn search_results_to_seed_hits(
      results: &[crate::search::SearchResult],
  ) -> Vec<crate::graph::codemap::SeedHit> {
      results
          .iter()
          .map(|r| crate::graph::codemap::SeedHit {
              file_path: r.chunk.context.file_path.clone(),
              line_start: r.chunk.context.line_start as u32,
              line_end: r.chunk.context.line_end as u32,
              score: r.score,
          })
          .collect()
  }
  ```
  Call site rewritten to `let seed_hits = search_results_to_seed_hits(&hits);
  build_codemap(..., Some(&seed_hits), ...)`. The `SearchResult → SeedHit`
  mapping is now an explicit tools-layer adapter; the graph algorithm is
  search-independent.
- **`SeedHit` exposed as `pub`** because the tools layer constructs it.
  Internal helpers `resolve_override_seeds`, `resolve_search_seeds`,
  `build_bm25_by_node` are `pub(super)` — visible only within
  `graph::codemap::*` and `graph` (per Guardrail 2, narrowest workable
  cross-file widening).
- `ItemKind` import in `mod.rs` gated behind `#[cfg(test)]` since it's only
  used by the hand-built codemap test fixture after the moves (avoids
  dead-import warning).
- `nix develop ../nix-devshells#cuda-code --command cargo check --all-targets`
  green. Engine→tools grep returns no hits.

Operation: `Split` + adapter DTO.

THEORY_3 mapping:

- A6: split codemap by concern.
- C2/C3: create a new shape beside old search result input and adapt at the
  tools boundary.

Boundary reason:

`graph::codemap` should not directly require `search::SearchResult`. Seed-hit
normalization belongs at the codemap boundary, with the tools endpoint mapping
search results into a codemap-local DTO.

Steps:

1. Convert `src/graph/codemap.rs` into `src/graph/codemap/mod.rs`.
2. Create `src/graph/codemap/model.rs`.
3. Move codemap data model types to `model.rs`.
4. Create `src/graph/codemap/seeds.rs`.
5. Add a codemap-local seed-hit DTO in `seeds.rs`.
6. Move search-hit normalization into `seeds.rs`.
7. Update `tools/graph/codemap.rs` to map `crate::search::SearchResult` into
   the codemap-local DTO before calling graph code.
8. Keep `graph::codemap` public type paths stable through `pub use model::*`.

Verification:

```sh
nix develop ../nix-devshells#cuda-code --command cargo check --all-targets
rg -n "crate::search::SearchResult" src/graph/codemap src/graph/codemap.rs
```

Exit:

- No direct `crate::search::SearchResult` dependency remains inside codemap
  graph logic.
- Model types keep their external paths.

## PR 13: Split Codemap Render/Build/Hierarchy

Status: DONE.

Outcome:

- `src/graph/codemap/mod.rs` reduced from 1857 → 22 LOC — a true facade.
  Body:
  ```rust
  mod model;
  pub(super) mod seeds;
  pub(super) mod build;
  pub(super) mod hierarchy;
  pub(super) mod render;

  #[cfg(test)]
  mod test_support;

  pub use model::*;
  pub use seeds::SeedHit;
  pub(crate) use build::{build_codemap, newest_source_mtime};
  pub(crate) use render::{render_mermaid, render_outline};
  ```
  All five submodules are private (`mod` / `pub(super) mod`) — no
  `graph::codemap::model::*` etc. paths are exposed beyond what the explicit
  `pub use` re-exports surface. Post-review fix-up narrowed `pub mod model;`
  to `mod model;`; sibling files reach `model` items via
  `crate::graph::codemap::model::*` (still works from inside `codemap/`).
- **`src/graph/codemap/build.rs`** (950 LOC) — `build_codemap` (pub async fn),
  `newest_source_mtime` (pub(crate)), private algorithm helpers
  (`rank_referrer`, `prune_to_budget`, `line_of_byte`, `extract_snippet`,
  `node_qualified_name`), + 9 tests (build_codemap, prune, edge dedup,
  newest_source_mtime tempdir tests, graph_prox).
- **`src/graph/codemap/hierarchy.rs`** (109 LOC) — `project_hierarchy`,
  `filter_module_tree` (both private to `codemap/*`).
- **`src/graph/codemap/render.rs`** (385 LOC) — `render_mermaid`,
  `render_outline` (pub(crate)) + private helpers (`short_node_id`,
  `sanitize_mermaid_id`, `escape_label`) + 4 render tests.
- **`src/graph/codemap/seeds.rs`** grew from 145 → 397 LOC. Beyond PR 12's
  scope, the subagent absorbed `canonicalize_and_strip` and
  `enclosing_item_for_line_range` (formerly in mod.rs at `pub(crate)`) into
  seeds.rs and narrowed both to `pub(super)`. They were only used internally
  by seeds.rs anyway. The `canonicalize_and_strip_normalizes` tests moved
  with them.
- **`src/graph/codemap/test_support.rs`** (211 LOC, new) — shared
  `#[cfg(test)] pub(super) fn hand_built_codemap()` + `shared_fixture()` +
  `FixtureSnap` + private fixture helpers (`nid`, `make_node`). Mirrors the
  pattern PR 11 established for `graph/test_support.rs`.
- **`embedding_cache` decision**: stays graph-level. Confirmed
  `src/tools/graph/similarity.rs:324` still invokes
  `crate::graph::ensure_embeddings_for`, so absorbing the module into
  `codemap/seeds.rs` would force the similarity tool to reach into a
  codemap-internal helper. No code change.
- **Test redistribution**: the pre-PR-13 mega test module's `pure` and
  `fixture_dependent` submodules were flattened — each family file owns a
  single `mod tests` block containing its subject's tests. Fixtures shared
  across multiple test sites live in `test_support.rs`.
- Public path stability verified: `crate::graph::codemap::{Codemap,
  CodemapOptions, EmbeddingPolicy, build_codemap, render_mermaid,
  render_outline, newest_source_mtime, SeedHit}` all resolve unchanged.
  `src/tools/graph/codemap.rs:38,101` compiles without modification.
- `nix develop ../nix-devshells#cuda-code --command cargo check --all-targets`
  green. Engine→tools grep returns no hits.
  `crate::search::SearchResult` only appears in seeds.rs doc-comments now —
  the PR 12 boundary fix is preserved.
- **Checkpoint 4 (PR 12-13) complete**: 2056-LOC `codemap.rs` mega-file
  fully dissolved into `codemap/{mod, model, seeds, build, hierarchy, render,
  test_support}.rs`. Boundary fix landed (graph→search edge removed). mod.rs
  is a true 22-line facade.

Operation: `Split`.

THEORY_3 mapping:

- A6: split overloaded object into named internal concerns.
- A7: verify the new boundary graph.

Steps:

1. Move formatting/rendering to `src/graph/codemap/render.rs`.
2. Move BFS/subgraph construction to `src/graph/codemap/build.rs`.
3. Move filtered module hierarchy projection to
   `src/graph/codemap/hierarchy.rs`.
4. Decide the final home for `graph::embedding_cache`:
   - keep graph-level if `tools::graph::similarity` still shares it;
   - otherwise absorb it into `codemap/seeds.rs`.
5. Keep `src/graph/codemap/mod.rs` as the facade.

Verification:

```sh
nix develop ../nix-devshells#cuda-code --command cargo check --all-targets
nix develop ../nix-devshells#cuda-code --command cargo test codemap --lib
```

Additional checks:

- `analyze_complexity` on `src/graph/codemap/*`

Exit:

- Codemap is split by model, seeds, build, hierarchy, and render.

## PR 14: Split OpenRouter Config And DTOs

Status: DONE.

Outcome:

- `src/embeddings/openrouter.rs` (1618 LOC) converted to a directory:
  `src/embeddings/openrouter/{mod.rs, config.rs, request.rs, response.rs}`.

- **`config.rs`** (374 LOC) — all 17 runtime/env constants
  (`DEFAULT_BASE_URL`, `API_KEY_ENV`, `MAX_BATCH_INPUTS_ENV`,
  `MAX_BATCH_TOKENS_ENV`, etc.) + 4 public config types
  (`OpenRouterRuntimeConfig`, `OpenRouterEncodingFormat`,
  `OpenRouterProviderPreferences`, `OpenRouterProviderSort` + their impls)
  + public fn `openrouter_runtime_config()` + 10 private env-parsing
  helpers (`api_key_from_env`, `openrouter_runtime_config_from_env`,
  `resolve_openrouter_runtime_config`, `positive_usize_from_env`,
  `encoding_format_from_env`, `provider_preferences_from_env`,
  `provider_sort_from_env`, `optional_usize_from_env`,
  `optional_f64_from_env`, `resolve_api_key`).

- **`request.rs`** (15 LOC) — `EmbeddingRequest<'a>` wire DTO only.

- **`response.rs`** (170 LOC) — `EmbeddingResponse`, `EmbeddingResponseItem`,
  `EmbeddingResponseEmbedding` enum + impl, `parse_embeddings_response` fn,
  `decode_base64_f32_embedding`, `decode_base64_standard`.

- **`openrouter/mod.rs`** (1093 LOC) — still owns `OpenRouterEmbedder` +
  giant impl block, `OpenRouterRequestMetrics` + impl + logger,
  `OpenRouterBatchError`, `OpenRouterInput`, `OpenRouterInputBatch` + impl,
  batch planning helpers, retry helpers, `MAX_RETRIES` const, and the full
  test block. PR 15 will split these.

- `OpenRouterInput` deliberately stayed in `mod.rs` — it's batch-coupled
  (referenced by `OpenRouterInputBatch`, `plan_remote_input_batches`,
  `sort_openrouter_inputs`), and PR 15 will move it to `batch.rs` alongside
  the batching code. Putting it in `request.rs` would force unnecessary
  cross-module access.

- Re-exports in `openrouter/mod.rs` preserve all external paths via
  `pub use config::{openrouter_runtime_config, OpenRouterEncodingFormat,
  OpenRouterProviderPreferences, OpenRouterProviderSort,
  OpenRouterRuntimeConfig};`. The 2-line block in `src/embeddings/mod.rs:35-36`
  resolves unchanged.

- Visibility discipline (Guardrail 2 followed):
  - Public types/fn unchanged.
  - `pub(super)` granted only where strictly required for mod.rs's giant
    impl block and the test module to reach moved items: env-parsing helper
    fns the test block names by name, response DTO fields/variants
    `parse_embeddings_response` constructs, `EmbeddingRequest` fields the
    embedder reaches.
  - No `pub(crate)` widening anywhere.

- `nix develop ../nix-devshells#cuda-code --command cargo check --all-targets`
  green. No new warnings.

Operation: `Split`.

THEORY_3 mapping:

- A6: split a multi-concern implementation file.

Boundary reason:

Runtime config and wire DTOs are stable support concerns for the OpenRouter
client. Moving them first reduces later client churn.

Steps:

1. Convert `src/embeddings/openrouter.rs` into
   `src/embeddings/openrouter/mod.rs`.
2. Create:
   - `config.rs`
   - `request.rs`
   - `response.rs`
3. Move runtime config/env parsing to `config.rs`.
4. Move request DTOs to `request.rs`.
5. Move response DTOs and decoding helpers to `response.rs`.
6. Keep existing `embeddings/mod.rs` public re-exports unchanged.

Verification:

```sh
nix develop ../nix-devshells#cuda-code --command cargo check --all-targets
nix develop ../nix-devshells#cuda-code --command cargo test embeddings::openrouter --lib
```

Exit:

- Config and DTO concerns are no longer mixed into the client implementation.

## PR 15: Split OpenRouter Client Support

Status: DONE.

Outcome:

- `src/embeddings/openrouter/mod.rs` reduced from 1093 → 15 LOC — a true
  facade. Body:
  ```rust
  mod batch;
  mod client;
  mod config;
  mod metrics;
  mod request;
  mod response;
  mod retry;

  pub use config::{
      openrouter_runtime_config, OpenRouterEncodingFormat,
      OpenRouterProviderPreferences, OpenRouterProviderSort,
      OpenRouterRuntimeConfig,
  };
  pub(in crate::embeddings) use client::OpenRouterEmbedder;
  ```

- **`src/embeddings/openrouter/batch.rs`** (273 LOC) — batch planning:
  `OpenRouterBatchError`, `OpenRouterInput`, `OpenRouterInputBatch` + impl,
  `plan_remote_input_batches`, `sort_openrouter_inputs`,
  `plan_openrouter_batches`, `fallback_token_estimate`,
  `restore_original_embedding_order`, plus 7 batch-planning tests.

- **`src/embeddings/openrouter/retry.rs`** (38 LOC) — HTTP-retry classification:
  `MAX_RETRIES`, `body_snippet`, `is_payload_too_large`,
  `is_retryable_status`, `is_retryable_reqwest_error`, `sleep_for_retry`.

- **`src/embeddings/openrouter/metrics.rs`** (141 LOC) — kept separate from
  client.rs (plan §15 final tree lists it, and tracking spans the whole
  request flow): `OpenRouterRequestMetrics` + impl, `OpenRouterMetricsHandle`
  type alias, `log_openrouter_request_metrics`, plus 1 latency test.

- **`src/embeddings/openrouter/client.rs`** (450 LOC) — `OpenRouterEmbedder`
  struct + giant impl block: `new`, `dim`, `embed_documents`,
  `embed_queries`, `plan_remote_batches`, `estimate_token_lengths`,
  `embed_with_split`, `request_batch_with_split`, `request_batch`.

- **`pub(in crate::embeddings) use client::OpenRouterEmbedder;`** —
  visibility was `pub(super)` pre-split (meaning "visible from
  `embeddings`" when mod.rs was the home). After moving to `client.rs`,
  `pub(super)` would collapse to "visible only inside `openrouter`" which
  breaks `src/embeddings/mod.rs:68,85`. Used `pub(in crate::embeddings)` —
  strictly narrower than `pub(crate)`, preserves the original effective
  reachability.

- **Tests distributed by subject** — no `test_support.rs` needed. Each
  family file owns the tests that exercise it: 4 response tests in
  response.rs, 1 request test in request.rs, 7 config tests in config.rs,
  1 metrics test in metrics.rs, 7 batch tests in batch.rs.

- `OpenRouterInput` (deferred from PR 14) landed in `batch.rs` alongside
  the batching code — its natural home (batch-coupled, not a wire DTO).

- All `embeddings::openrouter::*` public paths preserved:
  `OpenRouterRuntimeConfig`, `OpenRouterEncodingFormat`,
  `OpenRouterProviderPreferences`, `OpenRouterProviderSort`,
  `openrouter_runtime_config()`, `OpenRouterEmbedder`. `src/embeddings/mod.rs`
  and `examples/index_codebase.rs` compile unchanged.

- `nix develop ../nix-devshells#cuda-code --command cargo check --all-targets`
  green. Engine→tools grep returns no hits.

- **Checkpoint 5 (PR 14-15) complete**: 1618-LOC `openrouter.rs` mega-file
  fully dissolved into `openrouter/{mod, config, request, response, batch,
  retry, metrics, client}.rs`. mod.rs is a 15-line facade. No file exceeds
  500 LOC.

Operation: `Split`.

THEORY_3 mapping:

- A6: split by dense implementation concerns.

Steps:

1. Create:
   - `batch.rs`
   - `retry.rs`
   - `metrics.rs`
   - `client.rs`
2. Move OpenRouter-specific input ordering and generic batch-planner adapter to
   `batch.rs`.
3. Move retryability and payload-too-large split logic to `retry.rs`.
4. Move request metrics to `metrics.rs`, unless it remains tiny enough to stay
   in `client.rs`.
5. Move `OpenRouterEmbedder` and HTTP orchestration to `client.rs`.
6. Keep `openrouter/mod.rs` as facade/re-export surface.

Verification:

```sh
nix develop ../nix-devshells#cuda-code --command cargo check --all-targets
nix develop ../nix-devshells#cuda-code --command cargo test embeddings::openrouter --lib
```

Additional checks:

- `analyze_complexity` on `src/embeddings/openrouter/*`

Exit:

- No OpenRouter file is a mega-file.
- External embedding paths remain unchanged.

## PR 16: Split Chunker And Parser Facades

Status: DONE.

Outcome:

**Chunker** (`src/chunker/mod.rs` 805 → 11 LOC, pure facade):
- `chunker/types.rs` (250 LOC) — `ChunkId`, `ChunkContext`, `CodeChunk` +
  `format_for_embedding`, `ChunkSplitConfig` + 2 tests.
- `chunker/chunker.rs` (297 LOC) — `Chunker` struct + main impl (`new`,
  `with_overlap`, `chunk_file`, `extract_symbol_code`,
  `extract_module_path`, `add_overlap`, `calculate_overlap`) + `Default`
  impl + 4 tests.
- `chunker/split.rs` (284 LOC) — second `impl Chunker` block with
  `split_oversized_chunks` + private `split_leaf_chunk`; free helpers
  (`make_part_chunk`, `token_count_or_estimate`, `is_container_kind`,
  `strictly_contains`, `nearest_skipped_parent`) + 3 tests.
- `chunker/mod.rs` body:
  ```rust
  mod chunker;
  mod split;
  mod types;

  pub use chunker::Chunker;
  pub use types::{ChunkContext, ChunkId, ChunkSplitConfig, CodeChunk};
  ```

**Parser** (`src/parser/mod.rs` 621 → 19 LOC, pure facade; existing
`call_graph.rs`/`imports.rs`/`type_references.rs` untouched per spec):
- `parser/types.rs` (103 LOC) — `Symbol`, `SymbolKind` + `as_str`,
  `Visibility`, `Range`, `ParseResult`.
- `parser/rust_parser.rs` (515 LOC) — `RustParser` struct + impl
  (`new`, `with_edition`, `parse_file`, `parse_source`,
  `parse_file_complete`, `parse_source_complete`) + 6 private helpers
  (`line_of_offset`, `extract_visibility`, `extract_docstring`,
  `node_to_range`, `extract_symbols_recursive`, `extract_item_symbols`) +
  8 parser tests.
- `parser/mod.rs` body:
  ```rust
  pub mod call_graph;
  pub mod imports;
  pub mod type_references;

  mod rust_parser;
  mod types;

  pub use imports::{extract_imports, extract_imports_from_ast, get_external_dependencies};
  pub use rust_parser::RustParser;
  pub use types::{ParseResult, Range, Symbol, SymbolKind, Visibility};

  pub(in crate::parser) use rust_parser::line_of_offset;
  ```
  `line_of_offset` is re-exported at `pub(in crate::parser)` so the
  untouched `type_references.rs:?` import `use super::line_of_offset` keeps
  resolving — narrowest workable widening per Guardrail 2.

**Visibility decisions**:
- `Chunker::add_overlap` widened from private to `pub(super)` (split.rs
  calls it).
- `Chunker::overlap_percentage` initially widened by subagent to
  `pub(super)`, then **narrowed back to private** (the only consumers are
  tests in the same `chunker.rs` file via `mod tests`, which already see
  private fields via child-of-parent rule).
- All other helpers remained at their original private visibility.
- No `pub(crate)` widenings.

**Public-path stability** verified: `crate::chunker::{Chunker, ChunkId,
CodeChunk, ChunkContext, ChunkSplitConfig}` and `crate::parser::{RustParser,
Symbol, SymbolKind, Visibility, Range, ParseResult, extract_imports,
extract_imports_from_ast, get_external_dependencies, call_graph, imports,
type_references}` all resolve unchanged.

`nix develop ../nix-devshells#cuda-code --command cargo check --all-targets`
green. Engine→tools grep returns no hits.

Operation: `Split`.

THEORY_3 mapping:

- A6: thin facade-heavy `mod.rs` files.

Steps:

1. Split `src/chunker/mod.rs` into:
   - `chunker/mod.rs`
   - `chunker/types.rs`
   - `chunker/chunker.rs`
   - `chunker/split.rs`
2. Split only the `src/parser/mod.rs` implementation into:
   - `parser/mod.rs`
   - `parser/types.rs`
   - `parser/rust_parser.rs`
3. Do not touch existing parser files:
   - `parser/call_graph.rs`
   - `parser/imports.rs`
   - `parser/type_references.rs`
4. Keep public paths stable through `mod.rs` re-exports.

Verification:

```sh
nix develop ../nix-devshells#cuda-code --command cargo check --all-targets
```

Additional checks:

- `get_imports` on touched modules
- `analyze_complexity` on new files

Exit:

- `chunker/mod.rs` and `parser/mod.rs` are facades, not implementation dumps.

## PR 17: Split Embeddings Backend Profile Model

Status: DONE.

Outcome:

- `src/embeddings/backend.rs` reduced from 895 → 535 LOC.
- **`src/embeddings/profile.rs`** (398 LOC, new) — profile data model +
  built-in registry: `EmbeddingProfile` struct + inherent impl (`name`,
  `parse`, `accepted_names`, `default_chunk_*_tokens`, `built_in_profiles`,
  `built_in_local_for_identity`, `built_in_api_for_identity`), `QueryPolicy`
  enum + impl, `LocalLoaderSpec` enum, `FastembedCpuModel` enum + impl,
  `Qwen3Variant` enum + impl, `BUILT_IN_PROFILES`
  (`LazyLock<Vec<EmbeddingProfile>>`), `PROFILE_ALIASES` const,
  `QWEN3_CODE_QUERY_PREFIX` and `BGE_SEARCH_QUERY_PREFIX` constants.
- **`src/embeddings/backend.rs`** stays focused on runtime wiring:
  `EmbeddingBackend` struct + impl (constructors, accessors, query
  formatting, identity encode/decode, legacy identity parsing),
  `EmbeddingRuntime` enum, `Default for EmbeddingBackend`.
- **`src/embeddings/mod.rs`** updated:
  ```rust
  pub use backend::{EmbeddingBackend, EmbeddingRuntime};

  mod profile;
  pub use profile::{EmbeddingProfile, FastembedCpuModel, LocalLoaderSpec, QueryPolicy, Qwen3Variant};
  ```
  `mod profile;` is private (matches chunker/parser pattern). External
  paths `crate::embeddings::EmbeddingProfile` etc. resolve unchanged via the
  `pub use` re-export.
- Sibling submodule imports rewritten (no visibility change, just path
  rewrite):
  - `src/embeddings/profile_registry.rs`: split `use super::backend::{...}`
    into `use super::backend::EmbeddingRuntime;` +
    `use super::profile::{EmbeddingProfile, QueryPolicy};`
  - `src/embeddings/fastembed_cpu.rs`: split into backend-side and
    profile-side imports.
- Two helpers widened from private to `pub(super)` because
  `backend.rs::from_v2_identity` calls them across the new module
  boundary: `EmbeddingProfile::built_in_local_for_identity` and
  `EmbeddingProfile::built_in_api_for_identity`. Narrowest workable
  widening per Guardrail 2.
- Tests redistributed: 12 `EmbeddingBackend`-focused tests stayed in
  `backend.rs` (default/dim/identity round-trips, legacy parsing, garbage
  rejection, profile-aware query formatting); 5+1 profile data-only tests
  moved to `profile.rs` (built-in data assertions, parse acceptance, alias
  acceptance, query-policy tag round-trip, OpenRouter input_type policy,
  plus a new `qwen3_variant_dims` slice).
- `src/embeddings/identity.rs` untouched per spec.
- `nix develop ../nix-devshells#cuda-code --command cargo check --all-targets`
  green. Engine→tools grep returns no hits.

Operation: `Split`.

THEORY_3 mapping:

- A6: separate profile data model from runtime backend wiring.

Steps:

1. Create `src/embeddings/profile.rs`.
2. Move profile data model and built-in registry types:
   - `EmbeddingProfile`
   - built-in registry
   - `QueryPolicy`
   - `LocalLoaderSpec`
   - `FastembedCpuModel`
   - `Qwen3Variant`
3. Keep `src/embeddings/backend.rs` focused on `EmbeddingBackend` and identity
   wiring.
4. Leave `identity.rs` unchanged.
5. Keep `embeddings/mod.rs` re-export behavior stable.

Verification:

```sh
nix develop ../nix-devshells#cuda-code --command cargo check --all-targets
```

Exit:

- Profile definitions are separated from backend runtime wiring.

## PR 18: Split Indexing Unified Helpers And Rename Error Collection

Operation: `Split` + `Rename`.

THEORY_3 mapping:

- A6: split pure helpers from orchestration.
- A4: rename object to match its surface.

Steps:

1. Create `src/indexing/unified_parallel.rs`.
2. Move pure parallel traversal helpers out of `unified.rs`.
3. Keep `UnifiedIndexer`, `IndexStats`, and `IndexFileResult` in
   `unified.rs`, unless a small `indexing/types.rs` is clearly cleaner.
4. Rename `src/indexing/errors.rs` to
   `src/indexing/error_collection.rs`.
5. Leave `src/indexing/error.rs` as the `IndexingError` enum.
6. Update `indexing/mod.rs` re-exports so public paths do not break.
7. Review `src/indexing/embedding_batcher.rs`; leave it intact if it is one
   coherent concern.

Verification:

```sh
nix develop ../nix-devshells#cuda-code --command cargo check --all-targets
```

Additional checks:

- `get_imports` on `rust_code_mcp::indexing`
- `dead_pub_in_crate(directory=..., krate="rust_code_mcp")`

Exit:

- `unified.rs` is slimmer.
- Error collection filename matches its concern.
- No fake split of `embedding_batcher.rs`.

## PR 19: Remove Migration Facades After Caller Migration

Operation: Workflow C caller migration + adapter removal.

THEORY_3 mapping:

- C4: migrate callers incrementally.
- C5: narrow old surface.
- C6: remove old implementation only when dead.

Steps:

1. Use `who_imports`, `who_uses`, `find_references`, and textual search to find
   in-repo imports of migration facades.
2. Migrate examples/tests/src callers from old paths to new paths in small
   batches.
3. Delete facade files only when no in-repo caller depends on them:
   - `src/tools/graph_tools.rs`
   - `src/tools/search_tool.rs`
   - `src/tools/search_tool_router.rs`
   - standalone old endpoint files under `src/tools/`
   - `src/graph/queries.rs`
4. Confirm `src/graph/codemap.rs` and `src/embeddings/openrouter.rs` no longer
   exist as flat files after their directory conversions.

Verification:

```sh
nix develop ../nix-devshells#cuda-code --command cargo check --all-targets
```

Additional checks:

- `who_imports` for each facade module before deletion
- `find_references` for old module names

Exit:

- Migration facades are gone only after callers are gone.
- Public compatibility paths that intentionally remain are re-exported from
  stable parent facades.

## PR 20: Lower Accidental Public Visibility

Operation: `Lower`.

THEORY_3 mapping:

- A9: guidelines after boundaries.
- C5: deprecate or narrow old surface.

Boundary reason:

Visibility cleanup before splits would be redone. After splits, public
projection can be narrowed accurately.

Steps:

1. Run:
   - `dead_pub_in_crate(directory=..., krate="rust_code_mcp")`
   - `dead_pub_report(directory=...)`
2. For each demotion candidate, check:
   - `who_imports`
   - `who_uses`
   - `find_references`
3. Demote internal-only items from `pub` to `pub(crate)` or narrower.
4. Keep intentional public facades:
   - `graph::OpenedSnapshot`
   - `graph::BuildOptions`
   - `indexing::{UnifiedIndexer, IncrementalIndexer}`
   - `embeddings::EmbeddingGenerator`
   - `search::HybridSearch`
   - `vector_store::VectorStore`
5. Rename name-only collisions only where it improves clarity without
   structural dedup:
   - audit option types
   - search/vector-store `SearchResult` names if still ambiguous

Verification:

```sh
nix develop ../nix-devshells#cuda-code --command cargo check --all-targets
```

Additional checks:

- `workspace_stats(directory=...)`
- `dead_pub_in_crate(directory=..., krate="rust_code_mcp")`

Exit:

- `pub_crate_share` meaningfully improves from the baseline.
- Dead-public findings shrink for non-facade modules.

## PR 21: Final Structural Verification Report

Operation: verification only.

THEORY_3 mapping:

- A7: verify the new boundary graph.

Scope:

- No planned code moves.
- Produce a final report or PR description with the final graph state.

Checks:

```sh
jj status
nix develop ../nix-devshells#cuda-code --command cargo check --all-targets
```

Rust-code-mcp checks:

- `build_hypergraph(directory=...)`
- `workspace_stats(directory=...)`
- `dead_pub_in_crate(directory=..., krate="rust_code_mcp")`
- `get_declared_reexports` on facade modules
- `get_imports` on touched top-level modules
- grep/text sweep for forbidden inline paths:

  ```sh
  rg -n "crate::tools|crate::mcp" src/graph src/indexing src/search src/embeddings src/vector_store src/chunker src/parser
  ```

Exit:

- No non-generated source file remains over the documented threshold unless
  explicitly justified as one coherent concern.
- `tools` depends inward only.
- `graph` and engine modules do not depend on `tools` or `mcp`.
- MCP tool names and parameter struct names are unchanged.
- Examples and tests compile through `cargo check --all-targets`.

## Optional PR 22: Lift Graph To A Crate

Operation: optional `Lift`.

THEORY_3 mapping:

- A8: decide whether modules should become crates.
- Workflow C: keep compatibility re-exports in the main crate.

Do not start unless PRs 01-21 are merged and one full verification pass is
unchanged.

Steps:

1. Re-check graph surfaces:
   - `get_declared_reexports`
   - `who_imports`
   - `who_uses_summary`
   - `crate_edges`
2. Create `crates/rmc-graph/`.
3. Move `src/graph/` internals to `crates/rmc-graph/src/`.
4. Add main-crate dependency on `rmc-graph`.
5. Keep compatibility re-exports in the main crate.
6. Verify no dependency back from `rmc-graph` to the main crate.

Verification:

```sh
nix develop ../nix-devshells#cuda-code --command cargo check --all-targets
```

Rust-code-mcp checks:

- `crate_edges(directory=...)`
- `forbidden_dependency_check` with the intended crate-boundary rules

Exit:

- Main crate depends on `rmc-graph`.
- `rmc-graph` depends on no main-crate modules.
- Compatibility paths in the main crate still compile.

## Final Target Layout After PR 21

```text
src/
  tools/
    mod.rs
    router.rs
    project_paths.rs
    params/
      mod.rs
      search.rs
      graph.rs
      audit.rs
      indexing.rs
    endpoints/
      mod.rs
      analysis.rs
      cache.rs
      health.rs
      index.rs
      indexing_support.rs
      query.rs
    graph/
      mod.rs
      core.rs
      crates.rs
      surface.rs
      audits.rs
      similarity.rs
      codemap.rs
      response.rs

  graph/
    mod.rs
    ids.rs
    model.rs
    storage.rs
    snapshot.rs
    loader.rs
    hir_trim.rs
    ast_resolve.rs
    extract.rs
    bindings.rs
    usages.rs
    impls.rs
    signatures.rs
    attributes.rs
    statics.rs
    docs_audit.rs
    derive_audit.rs
    unsafe_audit.rs
    fn_body_audit.rs
    channel_audit.rs
    recursion_check.rs
    audit_util.rs
    labels.rs
    math.rs
    embedding_cache.rs
    test_support.rs
    query/
      mod.rs
      model.rs
      imports.rs
      usage.rs
      calls.rs
      crates.rs
      surface.rs
      audits.rs
      functions.rs
      modules.rs
      overlaps.rs
    codemap/
      mod.rs
      model.rs
      seeds.rs
      build.rs
      hierarchy.rs
      render.rs

  embeddings/
    mod.rs
    backend.rs
    profile.rs
    profile_registry.rs
    batching.rs
    util.rs
    identity.rs
    qwen3.rs
    fastembed_cpu.rs
    token_lengths.rs
    error.rs
    openrouter/
      mod.rs
      config.rs
      client.rs
      request.rs
      response.rs
      batch.rs
      retry.rs
      metrics.rs

  chunker/
    mod.rs
    types.rs
    chunker.rs
    split.rs

  parser/
    mod.rs
    types.rs
    rust_parser.rs
    call_graph.rs
    imports.rs
    type_references.rs

  indexing/
    mod.rs
    unified.rs
    unified_parallel.rs
    indexer_core.rs
    embedding_batcher.rs
    file_processor.rs
    incremental.rs
    merkle.rs
    tantivy_adapter.rs
    consistency.rs
    identity.rs
    retry.rs
    error.rs
    error_collection.rs
```
