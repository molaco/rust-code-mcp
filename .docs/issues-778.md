# Issues 778: Remaining Phase 3-6 Review Items

Source: review list for Phases 3-6 after the boundary cleanup work.

Status baseline:

- Fixed: Phase 5 server test compile regression from stale `EnrichedUsage`
  import/construction.
- Fix commit: `8e50ffd1` (`test: fix graph enriched usage test`).
- Verification: `nix develop ../nix-devshells#cuda-code --command cargo test
  -p rmc-server --no-run` passed with existing warnings.

## Resolved In This Remediation Series

- Phase 3 indexing facade tests: added focused tests for
  `index_project_incrementally` force reindex behavior, backend construction
  inputs, and facade-level error propagation. Verification passed with
  existing warnings: `nix develop ../nix-devshells#cuda-code --command cargo
  test -p rmc-indexing incremental_service::tests`.
- Phase 3 elapsed timing semantics: `IncrementalIndexOutcome.elapsed` now
  measures the full facade call, including force-reindex snapshot deletion and
  `clear_all_data`; the field has an explicit doc comment and a focused test.
  Verification passed with existing warnings: `nix develop
  ../nix-devshells#cuda-code --command cargo test -p rmc-indexing
  incremental_service::tests`.
- Phase 3 version-mismatch error mapping: added a server regression test that
  an `anyhow`-wrapped `VectorStoreError::VersionMismatch` still maps to the
  actionable MCP `clear_cache` guidance with stored/configured embedder IDs.
  Verification passed with existing warnings: `nix develop
  ../nix-devshells#cuda-code --command cargo test -p rmc-server
  version_mismatch_error_keeps_clear_cache_guidance`.

## Remaining Phase 3 Issues

- None.

## Remaining Phase 4 Issues

- Medium: Direct tests for `IndexingProjectPaths` are thin. Most coverage
  still flows through the server wrapper instead of testing the indexing-owned
  path policy directly.
- Low: The test-only server helper mixes an injected vectors root with
  production `data_dir()` for returned paths. Current tests only check
  discovery, but future path assertions could be surprised by this.
- Low / Medium: One malformed vector metadata file can abort all indexed
  profile discovery. Decide whether strict failure is intended, then document
  or test that policy.

## Remaining Phase 5 Issues

- Medium: Moved enrichment DTOs changed several fields from `&'static str` to
  `String`. JSON behavior is preserved, but the static-label guarantee is gone.
- Medium: Enrichment helpers still swallow snapshot transaction or lookup
  failures by returning empty or partial data. This was copied behavior, but it
  is now part of the graph facade API contract.
- Medium: No focused graph-side tests cover the new enrichment facade and DTO
  shape.

## Remaining Phase 6 Issues

- Medium: Audit error mapping is string-based. `graph_audit_error` classifies
  failures by substring matching, which is brittle compared with typed errors.
- Medium: Some audit handlers still do synchronous graph work inside async
  handlers. `mut_static_audit` and `recursion_check` call directly, while
  heavier handlers use `spawn_blocking`.
- Medium: No focused tests cover graph audit facade DTO mapping or server audit
  error mapping.

## Current-Suite Issues Not Attributable To Phases 3-6

- High: `rmc-graph` has a stale loader test assumption. The targeted test
  `graph::loader::tests::load_crate_target_kinds_finds_workspace_targets`
  expects `src/lib.rs` at the workspace root, but this repo root is a virtual
  workspace and has no `src/lib.rs`.
- Medium: `rmc-indexing` tests are not reliably regular in this environment.
  A targeted `test_calculate_safe_batch_size` hung past 60 seconds, likely
  because simple unit tests construct `IndexerCore::new`, which initializes the
  default embedding generator.
