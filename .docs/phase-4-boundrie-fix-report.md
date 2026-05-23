# Phase 4 Boundrie Fix Report

## Scope

Phase 4 moved indexing-owned project path and identity policy into
`rmc_indexing`, kept the server `ProjectPaths` API as a compatibility wrapper,
and reduced duplicate server helpers for data-root and embedding backend
resolution.

## Steps Completed

1. Ran `jj show --summary`.
2. Refreshed MCP evidence for `ProjectPaths` importers, project-path module
   dependencies, and duplicate helper clusters.
3. Recorded the Phase 4 responsibility split before editing.
4. Added `rmc_indexing::indexing::project_paths` with
   `IndexingProjectPaths`, `IndexedProfilePaths`, path constructors, indexed
   profile discovery, directory hash, collection prefix, and vector metadata
   identity reads.
5. Reexported the new indexing path API from `rmc_indexing::indexing`.
6. Migrated `rmc_server::mcp::project_paths::ProjectPaths` to delegate
   indexing path and identity derivation to the indexing facade while keeping
   the server-facing compatibility shape.
7. Removed the server crate's direct `sha2` dependency.
8. Confirmed `rmc_server::tools::project_paths` remains a compatibility
   reexport of the canonical MCP project-path module.
9. Removed the duplicate `indexing_support::data_dir` wrapper and used the
   canonical server data-root helper directly.
10. Added one MCP-facing backend resolver in `mcp::project_paths`, migrated
    query and graph similarity to it, and removed the thin endpoint resolver
    wrappers.
11. Kept the index endpoint's `resolve_backend` wrapper because it owns the
    legacy `model` parameter.
12. Rebuilt/reused the hypergraph and verified the helper clusters shrank.
13. Ran focused nix checks and focused resolver/project-path tests.
14. Recorded the Phase 4 ledger.

## Evidence

- Before the migration, `module_dependencies` showed
  `rmc_server::mcp::project_paths` depended directly on
  `rmc_indexing::indexing::identity`,
  `rmc_indexing::indexing::incremental::get_snapshot_path_for_identity`,
  `directories::ProjectDirs`, engine embedding profile/backend APIs, and
  `sha2`.
- Before the migration, `semantic_overlaps` found a `data_dir` duplicate
  cluster and a backend resolver cluster spanning project paths, query, graph
  similarity, and index.
- After the path move, `module_dependencies` for
  `rmc_server::mcp::project_paths` lists
  `rmc_indexing::indexing::project_paths` for indexing path policy, not
  `identity`, `incremental`, or `sha2`.
- `module_dependencies` for `rmc_indexing::indexing::project_paths` shows the
  indexing-owned module now owns dependencies on indexing identity, snapshot
  derivation, `EmbeddingBackend`, and `sha2`.
- `who_imports(target="sha2::Sha256")` shows no server importers after the
  direct server `sha2` dependency was removed.
- `who_imports(target="rmc_server::mcp::project_paths::ProjectPaths")` still
  returns production users in query, health, and index, plus tests and the
  compatibility `tools::project_paths` reexport.
- After `data_dir` consolidation,
  `functions_with_filter(krate="rmc_server", returns_type_pattern="PathBuf")`
  no longer returns `indexing_support::data_dir`.
- After backend resolver consolidation,
  `functions_with_filter(krate="rmc_server", returns_type_pattern="EmbeddingBackend")`
  returns only the shared `resolve_embedding_backend_for_mcp` helper and the
  index endpoint's legacy `resolve_backend` wrapper.
- Final `semantic_overlaps(crate_name="rmc_server", item_kind="Function")`
  reports no `data_dir` cluster and the backend resolver cluster reduced to
  the shared MCP helper plus the legacy index wrapper.

## Files Changed

- `crates/rmc-indexing/src/indexing/project_paths.rs`
- `crates/rmc-indexing/src/indexing/mod.rs`
- `crates/rmc-server/src/mcp/project_paths.rs`
- `crates/rmc-server/src/tools/endpoints/index.rs`
- `crates/rmc-server/src/tools/endpoints/indexing_support.rs`
- `crates/rmc-server/src/tools/endpoints/query.rs`
- `crates/rmc-server/src/tools/graph/similarity.rs`
- `crates/rmc-server/Cargo.toml`
- `Cargo.lock`
- `.docs/boundries-cleanup-progress.md`
- `.docs/phase-4-boundrie-fix-report.md`
- `.plans/boundries-plan.md`

## Verification

- MCP verification passed after rebuilding/reusing graph
  `2c6dfe88c8bad3b7db1838a94b00287b`: server project paths route indexing
  policy through `rmc_indexing::indexing::project_paths`; query and graph
  similarity route profile resolution through `rmc_server::mcp::project_paths`.
- Focused project-path tests passed:
  `nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server project_paths`.
- Focused resolver tests passed:
  `nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server resolve_backend`.
- Focused nix check passed:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-indexing -p rmc-server`.
- No formatting command was run.

## Commits

- `e14043cb`: `docs: record phase 4 step 1`
- `838381d3`: `docs: record phase 4 responsibility split`
- `8755d084`: `refactor: move indexing project paths`
- `7a54b668`: `docs: record phase 4 project path move`
- `31d872eb`: `docs: record phase 4 compatibility wrapper`
- `9c666fdd`: `refactor: consolidate server data dir helper`
- `bdc2d9f4`: `refactor: consolidate backend resolver helpers`
- `d216b1ba`: `docs: verify phase 4 helper consolidation`
- `b8b107e8`: `docs: record phase 4 check result`
- `1d050d0c`: `docs: record phase 4 ledger`

## Outcome

Phase 4 success criteria are met. Indexing/path identity policy now has an
indexing-owned facade, the server keeps MCP-facing data-root and compatibility
orchestration, the duplicate `data_dir` helper is removed, and backend
resolver duplication is reduced to a shared MCP helper plus the index
endpoint's deliberate legacy `model` wrapper.
