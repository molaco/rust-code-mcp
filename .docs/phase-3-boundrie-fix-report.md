# Phase 3 Boundrie Fix Report

## Scope

Phase 3 added an indexing-owned incremental indexing service facade and
migrated server index/sync production paths away from direct
`IncrementalIndexer` construction. `IncrementalIndexer` remains public as a
compatibility API.

## Steps Completed

1. Ran `jj show --summary`.
2. Refreshed MCP evidence for direct `IncrementalIndexer` imports and module
   dependencies in server index/sync production modules.
3. Added `rmc_indexing::indexing::incremental_service`.
4. Reexported `IncrementalIndexRequest`, `IncrementalIndexOutcome`, and
   `index_project_incrementally` from `rmc_indexing::indexing`.
5. Confirmed the facade accepts directory/backend/options while indexing owns
   `IncrementalIndexer` construction and change detection.
6. Migrated `rmc_server::tools::endpoints::index::index_codebase` to the
   facade while keeping the existing vector-store version-mismatch MCP error.
7. Migrated `rmc_server::mcp::sync` to the facade while preserving stored
   embedder identity handling for legacy indexes.
8. Kept `IncrementalIndexer` and its reexport public for compatibility.
9. Rebuilt the hypergraph and verified production server modules no longer
   depend on `rmc_indexing::indexing::incremental`.
10. Ran the focused nix check.
11. Recorded the Phase 3 ledger.

## Evidence

- Before the migration, `module_dependencies` showed
  `rmc_server::tools::endpoints::index` depended on
  `rmc_indexing::indexing::incremental` through `IncrementalIndexer`,
  `IncrementalIndexer::with_backend`, `IncrementalIndexer::clear_all_data`, and
  `IncrementalIndexer::index_with_change_detection`.
- Before the migration, `module_dependencies` showed
  `rmc_server::mcp::sync` depended on `rmc_indexing::indexing::incremental`
  through `IncrementalIndexer`, `IncrementalIndexer::with_backend`, and
  `IncrementalIndexer::index_with_change_detection`.
- Initial `who_imports(target="rmc_indexing::indexing::incremental::IncrementalIndexer")`
  returned 14 bindings, including the two production server modules.
- After the migration, `build_hypergraph(force_rebuild=true)` produced graph
  `b2f982db0f3dcfb48cf162255b8d6696`.
- Refreshed `module_dependencies` for server `index` and `sync` showed both
  now depend on `rmc_indexing::indexing::incremental_service`, not
  `rmc_indexing::indexing::incremental`.
- Refreshed `who_imports` returned 11 `IncrementalIndexer` bindings. Remaining
  direct importers are compatibility consumers, tests, benches, tools, the
  public reexport, and the indexing-owned service.

## Files Changed

- `crates/rmc-indexing/src/indexing/incremental_service.rs`
- `crates/rmc-indexing/src/indexing/mod.rs`
- `crates/rmc-server/src/tools/endpoints/index.rs`
- `crates/rmc-server/src/mcp/sync.rs`
- `.docs/boundries-cleanup-progress.md`
- `.docs/phase-3-boundrie-fix-report.md`
- `.plans/boundries-plan.md`

## Verification

- MCP verification passed after rebuilding the hypergraph: server production
  `index` and `sync` modules depend on `incremental_service`, not
  `incremental`.
- Focused nix check passed:
  `nix develop ../nix-devshells#cuda-code --command env CUDAFORGE_THREADS=1 RAYON_NUM_THREADS=1 CARGO_BUILD_JOBS=1 cargo check -p rmc-indexing -p rmc-server --jobs 1`.
- No formatting command was run.

## Commits

- `dadd4305`: `docs: record phase 3 step 1`
- `cfbfd981`: `refactor: add incremental indexing service facade`
- `7d0b595c`: `docs: record phase 3 facade shape`
- `faaa16a6`: `refactor: use incremental service in index endpoint`
- `5c88f5e7`: `refactor: use incremental service in sync manager`
- `78f38279`: `docs: record phase 3 compatibility export`
- `60fb890b`: `docs: verify phase 3 dependencies`
- `0e28bf4e`: `docs: record phase 3 check result`
- `53d5393b`: `docs: record phase 3 ledger`

## Outcome

Phase 3 success criteria are met: server index/sync production code no longer
constructs `IncrementalIndexer` directly, indexing owns incremental indexer
construction and Merkle/change detection behind `index_project_incrementally`,
and `IncrementalIndexer` remains public for compatibility.
