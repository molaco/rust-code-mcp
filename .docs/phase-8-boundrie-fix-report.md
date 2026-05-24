# Phase 8 Boundrie Fix Report

## Scope

Phase 8 moved graph snapshot cleanup and workspace snapshot opening behind
graph-owned APIs. The server cache endpoint now remains MCP-facing: it parses
cache parameters, calls graph cleanup facades, and formats the existing text
response without constructing or interpreting `GraphPaths`.

The server graph response helper also uses a graph-owned
`open_current_for_workspace` facade so server graph response code no longer
constructs `GraphPaths` for current-snapshot opening.

## Steps Completed

1. Ran `jj show --summary`.
2. Added graph-owned snapshot cleanup DTOs and cleanup APIs.
3. Migrated server `clear_cache` hypergraph cleanup to call graph cleanup
   facades.
4. Moved the public cleanup facade to the graph snapshot layer.
5. Added `open_current_for_workspace` for server graph response snapshot
   opening.
6. Rebuilt the MCP hypergraph and verified server cache dependencies no
   longer reach graph storage layout.
7. Ran focused nix checks and recorded the Phase 8 ledger.

## Evidence

- MCP rebuilt graph `6a0f0a501756b0c9b36c694e073a60fc`, fingerprint
  `d291e5830be17d570abd3d5892e8c467a858c35d3bfcce3f5617e62be37f118d`.
- `module_dependencies(module="rmc_server::tools::endpoints::cache")`
  reports graph dependencies only on `rmc_graph::graph::snapshot` cleanup
  symbols: `GraphSnapshotCleanupOptions`, `GraphSnapshotCleanupReport`,
  `clear_workspace_snapshots`, and `clear_all_workspace_snapshots`.
- `get_imports(module="rmc_server::tools::endpoints::cache")` imports only
  the two graph snapshot cleanup DTOs from graph.
- `who_imports(target="rmc_graph::graph::GraphPaths")` returned 16 bindings,
  all in graph modules/tests, debug binaries, the compatibility reexport, or
  `probe_workspace`; no server module imports `GraphPaths`.
- `functions_with_filter(krate="rmc_graph", has_param_type="GraphPaths")`
  returned only graph snapshot functions: `open_current`, `open_specific`,
  and `publish_current`.
- Source search confirmed server cache code has no direct reference to
  `rmc_graph::graph::GraphPaths` or `rmc_graph::graph::storage`.

## Files Changed

- `crates/rmc-graph/src/graph/mod.rs`
- `crates/rmc-graph/src/graph/snapshot.rs`
- `crates/rmc-graph/src/graph/storage.rs`
- `crates/rmc-server/src/tools/endpoints/cache.rs`
- `crates/rmc-server/src/tools/graph/response.rs`
- `.docs/boundries-cleanup-progress.md`
- `.docs/phase-8-boundrie-fix-report.md`
- `.plans/boundries-plan.md`

## Verification

- Combined focused check passed:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-graph -p rmc-server`.
- Graph cleanup tests passed:
  `nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-graph clear_`
  (3 tests passed).
- Server cache tests passed:
  `nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server cache`
  (7 tests passed).
- MCP dependency verification passed after rebuilding the hypergraph.
- No formatting command was run.

## Commits

- `8306e692`: `docs: start phase 8 storage cleanup facade`
- `253b76c1`: `refactor: add graph storage cleanup facade`
- `ea86c85d`: `refactor: use graph storage cleanup facade in cache`
- `28d27cd2`: `refactor: hide graph paths behind snapshot facade`
- `36e10267`: `docs: record phase 8 check result`
- `aa0815de`: `docs: record phase 8 ledger`

## Outcome

Phase 8 success criteria are met. Graph storage layout decisions now stay in
graph snapshot/storage internals, and the server cache endpoint remains
responsible only for MCP parameter handling and response formatting.
