# Phase 6 Boundrie Fix Report

## Scope

Phase 6 moved graph audit orchestration behind graph-owned facade functions.
The server audit endpoints now parse MCP parameters, call graph audit entry
points, paginate/wrap responses, and keep `spawn_blocking` around the
RA-load-backed calls.

## Steps Completed

1. Ran `jj show --summary`.
2. Refreshed MCP evidence for server graph audit imports, module
   dependencies, `loader::load` importers, and graph exports.
3. Added graph-owned audit facade functions:
   `run_unsafe_audit`, `run_mut_static_audit`, `run_recursion_check`,
   `run_channel_capacity_audit`, and `run_fn_body_audit`.
4. Added graph-owned audit option/result DTOs for recursion, channel capacity,
   function-body, unsafe, and mutable-static audit output.
5. Migrated server audit endpoints to call the graph facade.
6. Kept server ownership of MCP response envelopes, pagination, summary
   stripping, parameter defaults, error mapping, and async blocking
   orchestration.
7. Rebuilt the MCP hypergraph and verified the server no longer depends on
   audit internals in production.
8. Ran focused nix checks and recorded the Phase 6 ledger.

## Evidence

- Before the migration,
  `module_dependencies(module="rmc_server::tools::graph::audits")` showed
  direct server dependencies on `rmc_graph::graph::loader::load`,
  `channel_audit`, `fn_body_audit`, `recursion_check`, and snapshot audit
  methods.
- `who_imports(target="rmc_graph::graph::loader::load")` did not show the
  server because the server used fully qualified inline paths. The
  `module_dependencies` check was therefore the authoritative evidence.
- After the migration, MCP built graph
  `350719e344857be9514c69be176c11a7`, fingerprint
  `59335f0aaf01780beb5032be2ff2022bbe20c2903f067ec4c6c8cd60e802adaf`.
- After the migration,
  `module_dependencies(module="rmc_server::tools::graph::audits")` reports
  server dependencies on `rmc_graph::graph::query::audits` facade
  functions/options and `rmc_graph::graph::query::model` DTOs.
- The same MCP dependency result no longer reports production server
  dependencies on `loader`, `channel_audit`, `fn_body_audit`,
  `recursion_check`, `unsafe_audit`, or snapshot audit methods.
- Source search in `rmc_server::tools::graph::audits` found no remaining
  direct references to graph `loader`, individual audit modules, `NodeId`,
  `NodeKind`, snapshot lookup, or `to_hex`.
- `get_exports(module="rmc_graph::graph", consumer="rmc_server")` reports 83
  visible graph exports, including the new audit facade functions/options and
  DTOs.

## Files Changed

- `crates/rmc-graph/src/graph/query/audits.rs`
- `crates/rmc-graph/src/graph/query/model.rs`
- `crates/rmc-graph/src/graph/mod.rs`
- `crates/rmc-server/src/tools/graph/audits.rs`
- `.docs/boundries-cleanup-progress.md`
- `.docs/phase-6-boundrie-fix-report.md`
- `.plans/boundries-plan.md`

## Verification

- Graph-only check passed:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-graph`.
- Server check passed:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-server`.
- Combined focused check passed:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-graph -p rmc-server`.
- MCP dependency verification passed after rebuilding the hypergraph.
- No formatting command was run.

## Commits

- `f6989e95`: `docs: start phase 6 audit facade`
- `c045a04f`: `refactor: add graph audit facade`
- `dcc6665e`: `refactor: use graph audit facade in server`
- `e37adafd`: `docs: verify phase 6 server audit split`
- `1c6d886b`: `docs: verify phase 6 audit dependencies`
- `550a943e`: `docs: record phase 6 check result`
- `7b74638e`: `docs: record phase 6 ledger`

## Outcome

Phase 6 success criteria are met. Graph now owns audit loading, snapshot
access, crate-filter resolution, audit dispatch, and graph DTO construction.
The server no longer manually orchestrates graph audit internals, and no
graph-to-server dependency was introduced.
