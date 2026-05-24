# Phase 12 Boundrie Fix Report

## Scope

Phase 12 tightened the `rmc-graph` public surface after server graph tools had
been moved to graph-owned facades. The work reduced avoidable implementation
module exposure under `rmc_graph::graph`, preserved stable compatibility
modules, and kept debug examples compiling through supported public paths.

`codemap`, `ids`, `model`, and `snapshot` remain public graph modules by
explicit compatibility decision. `codemap` is still used directly by server
codemap tools; the others are stable graph data/open-snapshot surfaces while
root reexports remain the preferred path.

## Steps Completed

1. Ran `jj show --summary`.
2. Treated `rmc_graph::graph` as a compatibility facade.
3. Identified stable graph public groups that must remain visible.
4. Added graph-owned missing-docs and derive-audit wrappers, migrated
   server/debug callers to facade exports, and made implementation modules
   private where production callers no longer needed them.
5. Verified debug examples and graph tests through supported paths.
6. Documented remaining compatibility exports.
7. Ran focused nix checks.
8. Recorded the Phase 12 ledger.

## Evidence

- MCP `get_exports(module="rmc_graph::graph", consumer="rmc_server")` showed
  96 server-visible graph bindings before tightening.
- MCP `get_reexports(module="rmc_graph::graph")` showed 74 explicit facade
  reexports, confirming the root graph module remains the compatibility
  facade.
- After the implementation-module change, MCP `get_exports` showed 88
  server-visible graph bindings, down from 96.
- MCP `who_imports` for `rmc_graph::graph::docs_audit`,
  `rmc_graph::graph::derive_audit`, and `rmc_graph::graph::loader` showed only
  graph-internal module/test importers after server and debug consumers moved.
- MCP `module_dependencies(module="rmc_server::tools::graph::codemap")`
  confirmed server codemap tools still depend on graph codemap exports, so the
  `codemap` module remains intentionally public.

## Files Changed

- `crates/rmc-graph/src/graph/mod.rs`
- `crates/rmc-graph/src/graph/query/audits.rs`
- `crates/rmc-graph/src/graph/query/model.rs`
- `crates/rmc-server/src/tools/graph/core.rs`
- `crates/rmc-server/src/tools/graph/similarity.rs`
- `crates/rmc-server/src/tools/graph/surface.rs`
- `crates/rust-code-mcp/examples/debug_itemscope.rs`
- `crates/rust-code-mcp/examples/spike_usages.rs`
- `crates/rust-code-mcp/examples/timing_extract.rs`
- `.docs/boundries-cleanup-progress.md`
- `.docs/phase-12-boundrie-fix-report.md`
- `.plans/boundries-plan.md`

## Verification

- Combined focused check passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-graph -p rmc-server -p rust-code-mcp`.
- Graph no-run tests compiled with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-graph --no-run`.
- Server no-run tests compiled with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server --no-run`.
- Touched debug examples compiled with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rust-code-mcp --example debug_itemscope --example spike_usages --example timing_extract`.
- No formatting command was run.

## Commits

- `36d3eaaa`: `docs: start phase 12 graph visibility`
- `656cbbaf`: `docs: record phase 12 graph facade evidence`
- `b8fd6f94`: `docs: record phase 12 stable graph exports`
- `4f4860cf`: `refactor: tighten graph implementation modules`
- `8ed5a684`: `docs: verify graph debug consumers`
- `767a9b52`: `docs: record graph compatibility exports`
- `8f70f1d4`: `docs: record phase 12 check result`
- `70d39335`: `docs: record phase 12 ledger`

## Outcome

Phase 12 success criteria are met. `rmc_graph::graph` no longer exposes the
avoidable implementation modules to production consumers, server graph tools
use graph-owned facades for the migrated audit paths, and the remaining broad
graph exports are documented compatibility surfaces.
