# Phase 2 Boundrie Fix Report

## Scope

Phase 2 added an indexing-owned BM25 search facade and migrated server
query/codemap production paths away from direct `TantivyAdapter` construction.
`TantivyAdapter` remains public as a compatibility export. A post-review
correction made the facade read-only so search and health probes no longer
create missing Tantivy indexes as a side effect.

## Steps Completed

1. Ran `jj show --summary`.
2. Refreshed MCP evidence for server query and codemap dependencies on
   `rmc_indexing::indexing::tantivy_adapter`.
3. Added `rmc_indexing::indexing::search::open_bm25_search`.
4. Reexported `open_bm25_search` from `rmc_indexing::indexing`.
5. Migrated `rmc_server::tools::endpoints::query::try_open_bm25` to the new
   facade.
6. Migrated `rmc_server::tools::graph::codemap` to the same facade.
7. Kept `TantivyAdapter` public for compatibility.
8. Rebuilt the hypergraph and verified server production modules no longer
   depend on `rmc_indexing::indexing::tantivy_adapter`.
9. Ran the focused nix check command and retry; both were blocked by an
   external CUDA/GCC compiler failure in `candle-kernels`.
10. Recorded the Phase 2 ledger.
11. Applied the post-review read-only BM25 opener correction.
12. Migrated the health probe from `Bm25Search::new` to the indexing facade.
13. Added four focused tests for missing paths, existing empty directories, and
    valid Tantivy indexes.

## Evidence

- Before the migration, `module_dependencies` showed
  `rmc_server::tools::endpoints::query` and
  `rmc_server::tools::graph::codemap` depended on
  `rmc_indexing::indexing::tantivy_adapter` through inline references.
- `who_imports(target="rmc_indexing::indexing::tantivy_adapter::TantivyAdapter")`
  returned four bindings, all in indexing modules/tests or the compatibility
  reexport.
- After the migration, `build_hypergraph(force_rebuild=true)` produced graph
  `06c80cff231427cb53c75e7c071397fd`.
- Refreshed `module_dependencies` and `get_imports` for server `query` and
  `codemap` showed both now depend on `rmc_indexing::indexing::search` for
  `open_bm25_search`, and neither reports
  `rmc_indexing::indexing::tantivy_adapter`.
- Post-review source reads showed `TantivyAdapter::new` and
  `Bm25Search::new` both open or create a missing Tantivy index. The facade now
  checks for `meta.json`, opens with `tantivy::Index::open_in_dir`, and returns
  an error without touching missing or partial index directories.

## Files Changed

- `crates/rmc-indexing/src/indexing/search.rs`
- `crates/rmc-indexing/src/indexing/mod.rs`
- `crates/rmc-server/src/tools/endpoints/query.rs`
- `crates/rmc-server/src/tools/endpoints/health.rs`
- `crates/rmc-server/src/tools/graph/codemap.rs`
- `.docs/boundries-cleanup-progress.md`
- `.docs/phase-2-boundrie-fix-report.md`
- `.plans/boundries-plan.md`

## Verification

- MCP verification passed after rebuilding the hypergraph: server `query` and
  `codemap` depend on `rmc_indexing::indexing::search`, not
  `rmc_indexing::indexing::tantivy_adapter`.
- Focused nix check attempted:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-indexing -p rmc-server`.
  Result: failed before checking touched crates because `candle-kernels v0.10.2`
  hit a CUDA/GCC `cc1plus` internal compiler error while compiling
  `src/moe/moe_wmma_gguf.cu`; Cargo then did not exit promptly and was
  terminated.
- Focused nix check retry attempted:
  `nix develop ../nix-devshells#cuda-code --command env CARGO_BUILD_JOBS=1 cargo check -p rmc-indexing -p rmc-server --jobs 1`.
  Result: same `candle-kernels` CUDA/GCC internal compiler error.
- Post-review focused nix check passed with CUDA thread caps:
  `nix develop ../nix-devshells#cuda-code --command env CUDAFORGE_THREADS=1 RAYON_NUM_THREADS=1 CARGO_BUILD_JOBS=1 cargo check -p rmc-indexing -p rmc-server --jobs 1`.
- Post-review focused tests passed with CUDA thread caps:
  `nix develop ../nix-devshells#cuda-code --command env CUDAFORGE_THREADS=1 RAYON_NUM_THREADS=1 CARGO_BUILD_JOBS=1 cargo test -p rmc-indexing open_bm25_search --jobs 1`.
  Result: `4 passed; 0 failed`.
- No formatting command was run.

## Commits

- `93e2b5b7`: `docs: record phase 2 step 1`
- `18e8e7c8`: `refactor: add indexing search facade`
- `29b87d19`: `docs: record phase 2 adapter ownership`
- `dee9f48e`: `refactor: use indexing search facade in query`
- `6d6f4a21`: `refactor: use indexing search facade in codemap`
- `1cb8e884`: `docs: record phase 2 compatibility export`
- `f30e7981`: `docs: verify phase 2 dependencies`
- `c56b74ee`: `docs: record phase 2 check result`
- `c2ae6cf0`: `docs: record phase 2 ledger`
- `2ae2e365`: `fix: open bm25 search read-only`

## Outcome

Phase 2 success criteria are met by MCP evidence: server production query and
codemap paths no longer open `TantivyAdapter` directly, indexing owns concrete
Tantivy search opening through `open_bm25_search`, and the compatibility export
remains available. The post-review capped nix check and the focused
`open_bm25_search` tests passed.
