# Phase 11 Boundrie Fix Report

## Scope

Phase 11 tightened the `rmc-indexing` public surface after the search,
incremental indexing, and project-path facades were in place. Implementation
modules were made private where active consumers allowed it, external
test/example callers were migrated to facade exports, and support modules were
reviewed for intentional public API status.

`IncrementalIndexer` remains public as a compatibility export because tests,
benches, examples, a standalone indexing tool, and internal indexing facades
still consume it through `rmc_indexing::indexing`.

## Steps Completed

1. Ran `jj show --summary`.
2. Reviewed public indexing implementation modules with MCP/source evidence.
3. Migrated deep indexing-path consumers to facade reexports.
4. Made indexing implementation modules private while preserving supported
   facade exports.
5. Reviewed `metadata_cache`, `metrics`, `monitoring`, and `security` public
   API status.
6. Kept `IncrementalIndexer` public by explicit compatibility decision.
7. Ran focused nix checks.
8. Recorded the Phase 11 ledger.

## Evidence

- MCP showed `rmc_indexing::indexing` still exposed implementation modules
  before tightening, including `consistency`, `identity`, `indexer_core`,
  `merkle`, `retry`, `tantivy_adapter`, and `unified`.
- Source search found external deep-path consumers in rust-code-mcp tests and
  examples; those callers now use `rmc_indexing::indexing` facade exports.
- MCP after the implementation-module change no longer listed the tightened
  modules as declared exports, while supported facade exports remained visible:
  `UnifiedIndexer`, `IndexStats`, `IndexFileResult`, `IncrementalIndexer`,
  `get_snapshot_path`, `TantivyAdapter`, `FileSystemMerkle`, and `ChangeSet`.
- `metadata_cache`, `security`, and `monitoring::backup` are internal modules.
  Public support APIs now go through `metrics::MemoryMonitor` and
  `monitoring::{ComponentHealth, HealthMonitor, HealthStatus, Status}`.
- MCP `who_imports` confirmed `IncrementalIndexer` still has compatibility
  consumers outside production server code, so it was not made private.
- MCP `module_dependencies` confirmed server `index` and `sync` production
  paths use the Phase 3 `incremental_service` facade for incremental indexing.

## Files Changed

- `crates/rmc-indexing/src/indexing/embedding_batcher.rs`
- `crates/rmc-indexing/src/indexing/file_processor.rs`
- `crates/rmc-indexing/src/indexing/indexer_core.rs`
- `crates/rmc-indexing/src/indexing/mod.rs`
- `crates/rmc-indexing/src/indexing/tantivy_adapter.rs`
- `crates/rmc-indexing/src/lib.rs`
- `crates/rmc-indexing/src/metrics/mod.rs`
- `crates/rmc-indexing/src/monitoring/backup.rs`
- `crates/rmc-indexing/src/monitoring/mod.rs`
- `crates/rmc-indexing/src/security/mod.rs`
- `crates/rmc-server/src/tools/endpoints/health.rs`
- `crates/rmc-server/src/tools/endpoints/query.rs`
- `crates/rust-code-mcp/examples/benchmark_phases.rs`
- `crates/rust-code-mcp/tests/test_gpu_index_jsonrpc.rs`
- `crates/rust-code-mcp/tests/test_hybrid_search.rs`
- `crates/rust-code-mcp/tests/test_mcp_stdio_transport.rs`
- `crates/rust-code-mcp/tests/test_merkle_standalone.rs`
- `.docs/boundries-cleanup-progress.md`
- `.docs/phase-11-boundrie-fix-report.md`
- `.plans/boundries-plan.md`

## Verification

- Combined focused check passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-indexing -p rmc-server -p rust-code-mcp`.
- Touched rust-code-mcp integration test targets compiled with existing
  warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo test -p rust-code-mcp --test test_merkle_standalone --test test_hybrid_search --test test_mcp_stdio_transport --test test_gpu_index_jsonrpc --no-run`.
- Touched benchmark example check passed with its pre-existing unused-variable
  warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rust-code-mcp --example benchmark_phases`.
- No formatting command was run.

## Commits

- `765027f9`: `docs: start phase 11 indexing visibility`
- `571313d6`: `docs: record phase 11 indexing surface evidence`
- `81d2bd87`: `refactor: use indexing facade exports`
- `2b3f1090`: `refactor: tighten indexing implementation modules`
- `eb59e6f7`: `refactor: tighten indexing support modules`
- `a4149d64`: `docs: record incremental indexer compatibility`
- `cf3845a5`: `docs: record phase 11 check result`
- `7a3b9f40`: `docs: record phase 11 ledger`

## Outcome

Phase 11 success criteria are met. The indexing public API is facade-oriented,
implementation modules are no longer public because server code used to reach
them, support modules expose only the intended cross-crate entry points, and
`IncrementalIndexer` remains public only as an explicit compatibility export.
