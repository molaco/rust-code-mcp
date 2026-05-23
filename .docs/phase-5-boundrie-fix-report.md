# Phase 5 Boundrie Fix Report

## Scope

Phase 5 added a graph-owned query/DTO enrichment facade for repeated graph
response translation paths and migrated server graph tools to use it. The
server still owns MCP result wrapping, pagination, parameter parsing, and
workspace snapshot opening.

## Steps Completed

1. Ran `jj show --summary`.
2. Refreshed MCP evidence for graph response/core/surface imports,
   dependencies, direct `OpenedSnapshot` parameters, and graph exports.
3. Identified the repeated server-local enrichment helpers as the first
   facade target.
4. Added `rmc_graph::graph::query::enrichment` with `OpenedSnapshot` helpers
   for bindings, usages, dead-public findings, and per-crate dead-public
   reports.
5. Added graph-owned `EnrichedBinding`, `EnrichedUsage`,
   `EnrichedDeadPub`, and `EnrichedCrateDeadPub` DTOs.
6. Reexported the new DTOs through `rmc_graph::graph`.
7. Verified the new DTOs preserve existing MCP JSON field names and serde
   skip/rename behavior.
8. Migrated server `graph::core` and `graph::surface` call sites to the graph
   facade.
9. Removed server-local enrichment helpers and the now-unused server
   `response::visibility_label` helper.
10. Rebuilt the MCP hypergraph and verified direct server `OpenedSnapshot`
    helper parameters dropped.
11. Verified existing graph exports remain visible to `rmc_server`.
12. Ran focused nix checks and recorded the Phase 5 ledger.

## Evidence

- Before the migration,
  `functions_with_filter(krate="rmc_server", has_param_type="OpenedSnapshot")`
  reported seven server graph helpers:
  `core::enrich_bindings`, `core::enrich_usages`,
  `response::resolve_chunk_to_item`, `response::resolve_required_node`,
  `response::visibility_label`, `surface::enrich_crate_dead_pub`, and
  `surface::enrich_dead_pub`.
- Before the migration, `module_dependencies` showed server
  `graph::response`, `graph::core`, and `graph::surface` depending on raw
  graph internals such as `snapshot`, `storage`, `model`, `ids`, `labels`,
  and `query::model`.
- After the migration, MCP rebuilt graph
  `085eaff90b1189f8e7a4dc3374610742`, fingerprint
  `349e4a62bdb66681623fdc7432c538e80f98e667ffd92cac4a9400383a022759`.
- After the migration,
  `functions_with_filter(krate="rmc_server", has_param_type="OpenedSnapshot")`
  reports two remaining server graph helpers:
  `response::resolve_chunk_to_item` and `response::resolve_required_node`.
- `module_dependencies` now shows `core` no longer depends on
  `rmc_graph::graph::labels`.
- `module_dependencies` now shows `surface` has
  `rmc_graph::graph::snapshot` import count `0`; remaining snapshot usage is
  from the opened snapshot value and non-enrichment endpoint calls.
- `response` still depends on graph `snapshot` and `storage` because
  `open_workspace_snapshot` intentionally remains server-owned in this phase.
- `get_exports(module="rmc_graph::graph", consumer="rmc_server",
  summary=true, limit=120)` reports 68 visible exports. Existing exports such
  as `OpenedSnapshot`, `GraphPaths`, `GraphEnvOptions`, `Node`, `NodeKind`,
  `Binding`, `Usage`, `DeadPubFinding`, and `CrateDeadPub` remain visible.
  The new enrichment DTOs are also visible.

## Files Changed

- `crates/rmc-graph/src/graph/query/enrichment.rs`
- `crates/rmc-graph/src/graph/query/model.rs`
- `crates/rmc-graph/src/graph/query/mod.rs`
- `crates/rmc-graph/src/graph/mod.rs`
- `crates/rmc-server/src/tools/graph/core.rs`
- `crates/rmc-server/src/tools/graph/surface.rs`
- `crates/rmc-server/src/tools/graph/response.rs`
- `.docs/boundries-cleanup-progress.md`
- `.docs/phase-5-boundrie-fix-report.md`
- `.plans/boundries-plan.md`

## Verification

- Graph-only check passed:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-graph`.
- Server check passed:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-server`.
- Combined focused check passed:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-graph -p rmc-server`.
- MCP verification passed for reduced direct server `OpenedSnapshot`
  parameters, reduced raw enrichment dependencies, and graph export
  compatibility.
- No formatting command was run.

## Commits

- `ecd3f445`: `docs: record phase 5 step 1`
- `5c12e38e`: `docs: record phase 5 response boundary evidence`
- `558106bc`: `refactor: add graph enrichment facade`
- `03e73ec4`: `docs: record phase 5 dto shape check`
- `d35b211d`: `refactor: use graph enrichment facade in server`
- `0420a460`: `docs: verify phase 5 snapshot boundary`
- `51ea5085`: `docs: verify phase 5 graph exports`
- `af625dd1`: `docs: record phase 5 check result`
- `a9a303a0`: `docs: record phase 5 ledger`

## Outcome

Phase 5 success criteria are met for the repeated response enrichment paths.
Server graph response code now uses graph-owned query/DTO enrichment APIs for
bindings, usages, and dead-public reports while preserving MCP JSON shapes.
Direct server `OpenedSnapshot` helper parameters dropped from seven to two,
and existing graph exports remain available for compatibility.
