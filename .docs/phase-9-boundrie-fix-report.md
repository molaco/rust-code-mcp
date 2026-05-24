# Phase 9 Boundrie Fix Report

## Scope

Phase 9 tightened server-internal boundaries after the graph and indexing
facades were in place. The router stayed a thin MCP adapter, stale server
helper duplication was removed, parameter structs remained crate-internal, and
the public server facade was re-verified.

`semantic` remains public in this phase because graph static-audit tests still
assert the symbol path `rmc_server::semantic::SEMANTIC`.

## Steps Completed

1. Ran `jj show --summary`.
2. Reviewed `tools::router` and kept business logic out of router methods.
3. Removed the unused `tools::endpoints::indexing_support` helper module.
4. Verified `tools::params` remains an internal `pub(crate)` parameter facade.
5. Verified `semantic` symbol-level expectations and left `semantic` public.
6. Migrated the remaining `tools::project_paths` compatibility caller to
   `mcp::project_paths` and removed the compatibility reexport module.
7. Rebuilt the MCP hypergraph and verified the intended server public exports.
8. Ran focused nix checks and recorded the Phase 9 ledger.

## Evidence

- MCP rebuilt graph `e669ac6eeba2bb252aa05150b435baa2`, fingerprint
  `cff3f2f33f298d34766a50b6578f4212466eadbfdf76f6399bf5b36567eddb29`.
- `get_exports(module="rmc_server::tools", consumer="rmc_server")` returned
  only the intended tools facade exports: `SearchToolRouter`, `SearchTool`,
  `index_codebase`, and `IndexCodebaseParams`.
- `get_exports(module="rmc_server::mcp", consumer="rmc_server")` returned
  `SyncManager` plus the public `sync` and `project_paths` modules.
- Root `rmc_server` exports remain `tools`, `mcp`, and `semantic`.
- `get_declared_reexports(module="rmc_server::tools")` returned only the four
  intended tools reexports.
- `get_declared_reexports(module="rmc_server::mcp")` returned the
  `SyncManager` glob reexport.
- Source search found no remaining `indexing_support` references.
- Source search found no remaining `rmc_server::tools::project_paths`,
  `crate::tools::project_paths`, or `tools::project_paths` callers.

## Files Changed

- `crates/rmc-server/src/tools/endpoints/indexing_support.rs`
- `crates/rmc-server/src/tools/endpoints/mod.rs`
- `crates/rmc-server/src/tools/mod.rs`
- `crates/rmc-server/src/tools/project_paths.rs`
- `crates/rmc-server/src/tools/router.rs`
- `crates/rust-code-mcp/tests/test_mcp_stdio_transport.rs`
- `.docs/boundries-cleanup-progress.md`
- `.docs/phase-9-boundrie-fix-report.md`
- `.plans/boundries-plan.md`

## Verification

- Server/bin focused check passed:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-server -p rust-code-mcp`.
- Server test target compilation passed:
  `nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server --no-run`.
- Stdio regression test target compilation passed:
  `nix develop ../nix-devshells#cuda-code --command cargo test -p rust-code-mcp --test test_mcp_stdio_transport --no-run`.
- Graph semantic expectation check passed:
  `nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-graph semantic`
  (1 test passed).
- MCP public-surface verification passed after rebuilding the hypergraph.
- No formatting command was run.

## Commits

- `d3c0c78f`: `docs: start phase 9 server cleanup`
- `a59551c9`: `docs: record phase 9 router boundary`
- `27faf679`: `refactor: remove unused server indexing helpers`
- `0c84f62c`: `docs: record phase 9 params boundary`
- `9a6b22db`: `docs: record semantic visibility decision`
- `fccbc47a`: `refactor: remove project paths compatibility reexport`
- `28e8e683`: `docs: verify phase 9 server exports`
- `200aaa7d`: `docs: record phase 9 check result`
- `2febe4d1`: `docs: record phase 9 ledger`

## Outcome

Phase 9 success criteria are met. The router remains a delegation layer, stale
server compatibility/helper surfaces were removed, `tools::params` stays
internal, the public server facade is explicit, and `semantic` stays public
only because current graph tests still rely on that qualified symbol path.
