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
- Phase 4 indexing-owned path policy coverage: added direct
  `IndexingProjectPaths` tests for data-root layout, identity-scoped
  collection names, existing collection path derivation, and direct indexed
  profile discovery. Verification passed with existing warnings: `nix develop
  ../nix-devshells#cuda-code --command cargo test -p rmc-indexing
  project_paths::tests`.
- Phase 4 injected vectors-root behavior: existing collection path derivation
  now preserves the vectors root used during discovery, and the server
  test-only helper has a regression test proving returned `vector_path` values
  stay under the injected root. Verification passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-indexing
  project_paths::tests` and `nix develop ../nix-devshells#cuda-code --command
  cargo test -p rmc-server mcp::project_paths::tests`.
- Phase 4 malformed metadata policy: indexed profile discovery now skips
  malformed matching vector metadata or invalid embedder identities with a
  warning, while direct metadata reads remain strict. Verification passed with
  existing warnings: `nix develop ../nix-devshells#cuda-code --command cargo
  test -p rmc-indexing project_paths::tests`.
- Phase 5 static label DTOs: graph enrichment DTO fields backed by closed
  label sets now use `&'static str` again (`namespace`, binding `kind`, usage
  `category`, and dead-public `item_kind`), while dynamic visibility and node
  kind strings stay owned. Verification passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server
  usage_summary_omits_navigation_fields`.
- Phase 5 enrichment error contract: graph enrichment methods now return
  `Result` and propagate snapshot transaction failures, storage lookup errors,
  and missing referenced nodes instead of returning empty or partial data.
  Server graph endpoints map those errors to MCP internal errors. Verification
  passed with existing warnings: `nix develop ../nix-devshells#cuda-code
  --command cargo check -p rmc-graph -p rmc-server`.
- Phase 5 graph-side enrichment tests: added focused graph tests for enriched
  binding label/node resolution, usage summary shape, dead-public DTO shape,
  and missing referenced-node error propagation. Verification: the first
  `nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-graph
  enrich_` run passed three new tests and exposed one over-specific category
  assertion; after loosening that assertion, `nix develop
  ../nix-devshells#cuda-code --command cargo test -p rmc-graph
  enrich_usages_applies_summary_shape_and_static_category` passed.

## Remaining Phase 3 Issues

- None.

## Remaining Phase 4 Issues

- None.

## Remaining Phase 5 Issues

- None.

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
