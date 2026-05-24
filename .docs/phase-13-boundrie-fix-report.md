# Phase 13 Boundrie Fix Report

## Scope

Phase 13 performed the final architecture verification for the boundaries
cleanup. It refreshed the hypergraph, re-ran crate-level dependency checks,
reviewed public surfaces, checked server module dependencies with both import
and inline-reference coverage, reviewed semantic overlap clusters, and ran the
focused build/test suite through the project nix dev shell.

One implementation exposure was tightened during the verification:
`rmc_indexing::indexing::error_collection` is now `pub(crate)` because source
search and MCP evidence showed only internal crate usage.

## Steps Completed

1. Ran `jj show --summary`.
2. Rebuilt the hypergraph and refreshed workspace/crate dependency metrics.
3. Refreshed public-surface checks for `rmc_engine`, `rmc_graph::graph`,
   `rmc_indexing::indexing`, and `rmc_server`.
4. Tightened the leftover public indexing `error_collection` module.
5. Refreshed server deep-dependency checks with both `get_imports` and
   `module_dependencies`.
6. Refreshed semantic-overlap checks across the core crates.
7. Ran the focused test/check suite through the `cuda-code` nix dev shell.
8. Recorded the final ledger and report.

## Before And After

- Phase 0 baseline: 45 crates, 296 modules, 2448 items, 49 cross-crate edges,
  `pub_crate_share=0.46781789638932497`, and zero forbidden dependency
  violations.
- Phase 13 final check: 45 crates, 303 modules, 2567 items, 48 cross-crate
  edges, `pub_crate_share=0.4449760765550239`, and zero forbidden dependency
  violations.
- Core crate instability changed from `rmc_server=0.4`, `rmc_indexing=0.125`,
  `rmc_graph=0.08333333333333333`, and `rmc_engine=0.06666666666666667` to
  `rmc_server=0.3333333333333333`, `rmc_indexing=0.125`,
  `rmc_graph=0.08333333333333333`, and `rmc_engine=0.06666666666666667`.
- Final public-surface counts were `rmc_engine=6`, `rmc_graph::graph=88`,
  `rmc_indexing::indexing=21`, and `rmc_server=3`.

## Evidence

- `build_hypergraph(force_rebuild=true)` produced graph
  `b9e01b5aeda04ae51a1c584f0512f8dc`, then the public-surface fix rebuild
  produced graph `504065740f18b789a68fae1df31f284e`.
- The five-rule forbidden dependency check returned `violation_count=0`.
- Server production module dependency checks showed no dependency on
  `docs_audit`, `derive_audit`, `loader`, `storage`, `tantivy_adapter`, or
  `error_collection`.
- Semantic overlap checks found only intentional wrappers, small paired
  helpers, and follow-up refactor candidates outside the boundary cleanup
  scope.

## Files Changed

- `crates/rmc-indexing/src/indexing/mod.rs`
- `.docs/boundries-cleanup-progress.md`
- `.docs/phase-13-boundrie-fix-report.md`
- `.plans/boundries-plan.md`

## Verification

- Combined focused check passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-indexing -p rmc-graph -p rmc-server -p rust-code-mcp`.
- Indexing no-run tests compiled with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-indexing --no-run`.
- Graph no-run tests compiled with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-graph --no-run`.
- Server no-run tests compiled with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server --no-run`.
- Rust-code-mcp examples compiled with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rust-code-mcp --example debug_itemscope --example spike_usages --example timing_extract --example benchmark_phases`.
- Selected rust-code-mcp integration tests compiled with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo test -p rust-code-mcp --test test_merkle_standalone --test test_hybrid_search --test test_mcp_stdio_transport --test test_gpu_index_jsonrpc --no-run`.
- Existing warnings included the neighboring `nix-devshells` dirty Git tree,
  existing `dead_code`/`unreachable_pub` warnings, existing unused variables in
  `benchmark_phases`, and an existing `JsonRpcResponse` dead-code warning in
  `test_gpu_index_jsonrpc`.
- No formatting command was run.

## Commits

- `503eab95`: `docs: start phase 13 final verification`
- `90c2fba3`: `docs: record phase 13 crate checks`
- `5c4333fe`: `refactor: hide indexing error collection module`
- `7e218340`: `docs: record phase 13 dependency checks`
- `6b686707`: `docs: record phase 13 semantic checks`
- `5245aa3d`: `docs: record phase 13 check result`

## Outcome

Phase 13 success criteria are met. The final forbidden dependency check has no
violations, server production modules no longer reach the targeted graph and
indexing implementation modules, remaining public surfaces are documented as
compatibility boundaries, and the focused check/test suite compiles.
